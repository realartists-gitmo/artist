//! Which commands are tree jobs, and how they are keyed.
//!
//! A tree job is an expensive, mutually-exclusive command whose result is a
//! function of the worktree rather than of the caller, and which keeps
//! incremental state a concurrent edit can poison. Cargo is the sharpest
//! instance and the only one whose contention we actually observe, so it is the
//! only descriptor written out in full; the rest of the table is additions, not
//! redesigns.
//!
//! **Unknown commands are not guessed at.** Absent a descriptor a command is
//! its own domain and its own key, which reproduces today's behaviour exactly —
//! a wrong guess here would either share a result between commands that are not
//! interchangeable or serialize things that never contended.

use std::path::Path;

use crate::coalesce::TreeJob;

/// Classify a shell command.
///
/// `None` means "not a tree job": run it as before, no coalescing, no domain.
pub fn classify(command: &str, project: &Path) -> Option<TreeJob> {
    let words = shell_words(command)?;
    let (program, args) = words.split_first()?;
    let program = program.rsplit('/').next().unwrap_or(program);
    let project = project.display();

    match program {
        // One `target/` lock covers every cargo subcommand, so they all share a
        // domain. They do not share a key: a build result is not a test result,
        // and handing one to a caller who asked for the other is the silent
        // failure this distinction exists to prevent.
        "cargo" => {
            let subcommand = args.iter().find(|arg| !arg.starts_with('-'))?;
            Some(TreeJob {
                domain: format!("cargo:{project}"),
                key: format!("cargo:{project}:{}", normalize(args)),
                // A killed cargo loses work but not correctness: artifacts are
                // written through a temporary and renamed, so a partial build
                // is a cache miss next time rather than a corrupt tree.
                preemptible: matches!(
                    subcommand.as_str(),
                    "build" | "test" | "check" | "clippy" | "doc" | "bench" | "nextest"
                ),
            })
        }

        // Package installers mutate a shared tree in place rather than a cache.
        // A killed install leaves the tree half-written in a way nothing
        // downstream detects, so these are never preempted — a matching
        // request queues behind the one already running.
        "npm" | "pnpm" | "yarn" | "bun" if is_install(args) => Some(TreeJob {
            domain: format!("node_modules:{project}"),
            key: format!("{program}:{project}:{}", normalize(args)),
            preemptible: false,
        }),
        "pip" | "pip3" | "uv" | "poetry" if is_install(args) => Some(TreeJob {
            domain: format!("pyenv:{project}"),
            key: format!("{program}:{project}:{}", normalize(args)),
            preemptible: false,
        }),

        // Their own daemons and lock files, and the heaviest things a laptop
        // is likely to run.
        "gradle" | "gradlew" | "mvn" | "bazel" | "buck2" => Some(TreeJob {
            domain: format!("{program}:{project}"),
            key: format!("{program}:{project}:{}", normalize(args)),
            preemptible: true,
        }),

        // `.tsbuildinfo` has cargo's exact poisoning failure.
        "tsc" => Some(TreeJob {
            domain: format!("tsc:{project}"),
            key: format!("tsc:{project}:{}", normalize(args)),
            preemptible: true,
        }),

        // Content-addressed caches, so poisoning is not a concern — but the
        // CPU cost and the shareable-result argument both still hold.
        "go" if matches!(args.first().map(String::as_str), Some("build" | "test")) => {
            Some(TreeJob {
                domain: format!("go:{project}"),
                key: format!("go:{project}:{}", normalize(args)),
                preemptible: true,
            })
        }

        _ => None,
    }
}

fn is_install(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "install" | "ci" | "add" | "sync"))
}

/// Reduce an argument list to a comparable key.
///
/// Deliberately conservative: exact match after trimming, with no attempt to
/// understand that `-p a -p b` and `-p b -p a` are the same run. Over-keying
/// costs a queue; under-keying hands someone a result for a command they did
/// not run, so the safe direction is to share less.
fn normalize(args: &[String]) -> String {
    args.join(" ")
}

/// Split a command the way a shell would, well enough to identify a program.
///
/// Returns `None` for anything with shell control flow — pipes, redirects,
/// chained commands, subshells. Those are not a single tree job, and treating
/// `cargo test && rm -rf x` as one would be badly wrong.
fn shell_words(command: &str) -> Option<Vec<String>> {
    if command.contains(['|', '&', ';', '>', '<', '`', '\n'])
        || command.contains("$(")
        || command.contains("||")
    {
        return None;
    }
    let words: Vec<String> = command
        .split_whitespace()
        // An inline `FOO=bar cargo test` prefix would otherwise be read as the
        // program name.
        .skip_while(|word| word.contains('=') && !word.starts_with('-'))
        .map(str::to_owned)
        .collect();
    (!words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> &'static Path {
        Path::new("/p")
    }

    /// The distinction the whole design rests on: one lock, two results.
    #[test]
    fn cargo_subcommands_share_a_domain_but_not_a_key() {
        let build = classify("cargo build", project()).unwrap();
        let test = classify("cargo test", project()).unwrap();
        assert_eq!(build.domain, test.domain, "one target/ lock");
        assert_ne!(build.key, test.key, "a build result is not a test result");
    }

    #[test]
    fn identical_invocations_share_a_key() {
        assert_eq!(
            classify("cargo test -p artist-agent", project()).unwrap().key,
            classify("cargo test -p artist-agent", project()).unwrap().key
        );
        assert_ne!(
            classify("cargo test -p artist-agent", project()).unwrap().key,
            classify("cargo test -p artist-cli", project()).unwrap().key
        );
    }

    /// Two worktrees never contend, so they must not serialize.
    #[test]
    fn different_projects_are_different_domains() {
        assert_ne!(
            classify("cargo test", Path::new("/a")).unwrap().domain,
            classify("cargo test", Path::new("/b")).unwrap().domain
        );
    }

    /// A half-written `node_modules/` is undetectable downstream, so installs
    /// queue rather than being killed.
    #[test]
    fn installers_are_not_preemptible() {
        for command in ["npm install", "pnpm install", "uv sync", "poetry add x"] {
            let job = classify(command, project())
                .unwrap_or_else(|| panic!("{command} should be a tree job"));
            assert!(!job.preemptible, "{command}");
        }
        assert!(classify("cargo test", project()).unwrap().preemptible);
    }

    /// `cargo publish` mutates the world rather than a cache; killing it is not
    /// a cache miss.
    #[test]
    fn cargo_commands_that_are_not_cache_builds_are_not_preemptible() {
        assert!(!classify("cargo publish", project()).unwrap().preemptible);
        assert!(!classify("cargo install ripgrep", project()).unwrap().preemptible);
    }

    /// An unknown command reproduces today's behaviour rather than being
    /// guessed at.
    #[test]
    fn an_unknown_command_is_not_a_tree_job() {
        assert!(classify("ls -la", project()).is_none());
        assert!(classify("rg pattern", project()).is_none());
        assert!(classify("git status", project()).is_none());
    }

    /// A compound command is not one job, and must never be treated as one —
    /// coalescing `cargo test && deploy` would skip the deploy.
    #[test]
    fn a_compound_command_is_never_a_tree_job() {
        for command in [
            "cargo test && cargo build",
            "cargo test | tee log",
            "cargo test; rm -rf /",
            "cargo test > out.txt",
            "echo $(cargo test)",
        ] {
            assert!(classify(command, project()).is_none(), "{command}");
        }
    }

    /// An environment prefix must not be mistaken for the program.
    #[test]
    fn an_env_prefix_does_not_hide_the_program() {
        let job = classify("RUST_LOG=debug cargo test", project()).unwrap();
        assert!(job.key.contains("cargo"));
        assert!(job.preemptible);
    }

    /// A path-qualified program is the same program.
    #[test]
    fn a_path_qualified_program_is_recognised() {
        assert!(classify("/usr/bin/cargo test", project()).is_some());
    }

    #[test]
    fn a_bare_program_with_no_subcommand_is_not_a_job() {
        assert!(classify("cargo", project()).is_none());
        assert!(classify("cargo --version", project()).is_none());
    }
}
