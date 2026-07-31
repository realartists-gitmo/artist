use std::path::{Path, PathBuf};

pub(crate) use crate::profiles::BUILTIN_NAMES;

pub(crate) fn config_root() -> Option<PathBuf> {
    std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|p| p.join("artist")))
}

/// Creates the editable defaults, but never changes an existing file.
///
/// Each built-in profile is written as markdown with frontmatter, so editing a
/// scaffolded file is the same operation as writing a new profile from scratch.
/// The scaffolded copy must reproduce the built-in's tool policy — a
/// `planner.md` written without its `tools.allow` block would silently gain
/// write access the moment it replaced the built-in.
pub(crate) fn scaffold(root: &Path) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let profile_dir = root.join("profiles");
    if let Err(error) = std::fs::create_dir_all(&profile_dir) {
        diagnostics.push(format!(
            "{}: cannot create profile directory: {error}",
            profile_dir.display()
        ));
        return diagnostics;
    }
    for name in BUILTIN_NAMES {
        create(
            &profile_dir.join(format!("{name}.md")),
            &profile_file(name, root),
            &mut diagnostics,
        );
    }
    diagnostics
}

fn profile_file(name: &str, root: &Path) -> String {
    let tools = match name {
        "explorer" | "planner" | "reviewer" => "tools:\n  allow: [read, find, grep, skill]\n",
        _ => "",
    };
    let body = match name {
        "default" => inherited_main_prompt(root),
        _ => profile_prompt(name).to_owned(),
    };
    format!(
        "---\ndescription: {}\n{tools}---\n\n{body}",
        profile_description(name),
    )
}

/// Before profiles, the session prompt lived in `prompts/main.md`. Seed the
/// default profile from a customized copy so upgrading does not silently
/// discard the user's edits.
fn inherited_main_prompt(root: &Path) -> String {
    std::fs::read_to_string(root.join("prompts/main.md"))
        .ok()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| include_str!("system_prompt.md").to_owned())
}

fn create(path: &Path, contents: &str, diagnostics: &mut Vec<String>) {
    if path.exists() {
        return;
    }
    if let Err(error) = std::fs::write(path, contents) {
        diagnostics.push(format!("{}: cannot create default: {error}", path.display()));
    }
}

pub(crate) fn profile_description(name: &str) -> &'static str {
    match name {
        "default" => "General-purpose agent carrying the full session prompt",
        "worker" => "Implementation-focused agent for bounded changes and verification",
        "explorer" => "Read-heavy agent for tracing code and gathering evidence",
        "planner" => {
            "Planning agent that turns requirements and code evidence into an executable plan"
        }
        _ => "Review agent focused on correctness, regressions, security, and missing tests",
    }
}

pub(crate) fn profile_prompt(name: &str) -> &'static str {
    match name {
        // The session root is instantiated from `default`, so this profile
        // carries the full system prompt rather than a delegation blurb.
        "default" => include_str!("system_prompt.md"),
        "worker" => {
            "Implement the requested change, verify it, and report modified files and residual risks.\n"
        }
        "explorer" => {
            "Inspect without editing. Return concise findings with file and symbol references.\n"
        }
        "planner" => {
            "Analyze requirements and the current code before planning. Return an ordered, implementation-ready plan with exact files, dependencies, verification steps, and risks. Do not edit files.\n"
        }
        _ => {
            "Review like a code owner. Lead with concrete findings ordered by severity, cite files and symbols, explain impact and reproduction, and avoid style-only feedback. Do not edit files.\n"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scaffold_does_not_overwrite() {
        let d = tempfile::tempdir().unwrap();
        scaffold(d.path());
        let worker = d.path().join("profiles/worker.md");
        std::fs::write(&worker, "mine-profile").unwrap();
        scaffold(d.path());
        assert_eq!(std::fs::read_to_string(worker).unwrap(), "mine-profile");
        for name in BUILTIN_NAMES {
            assert!(d.path().join(format!("profiles/{name}.md")).is_file());
        }
    }

    /// Upgrading from the pre-profile layout must not discard a customized
    /// system prompt: the default profile is seeded from `prompts/main.md`.
    #[test]
    fn a_customized_main_prompt_seeds_the_default_profile() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("prompts")).unwrap();
        std::fs::write(d.path().join("prompts/main.md"), "my careful prompt").unwrap();

        scaffold(d.path());

        let default = std::fs::read_to_string(d.path().join("profiles/default.md")).unwrap();
        assert!(default.contains("my careful prompt"), "{default}");
        let profiles = crate::profiles::Profiles::discover_from(d.path(), Some(d.path()));
        assert_eq!(
            profiles.get("default").unwrap().instructions,
            "my careful prompt"
        );
    }

    #[test]
    fn a_fresh_install_seeds_the_default_profile_from_the_builtin_prompt() {
        let d = tempfile::tempdir().unwrap();
        scaffold(d.path());
        let profiles = crate::profiles::Profiles::discover_from(d.path(), Some(d.path()));
        assert_eq!(
            profiles.get("default").unwrap().instructions,
            include_str!("system_prompt.md").trim()
        );
    }

    /// The scaffolded copy replaces the built-in, so it has to carry the same
    /// tool policy or a read-only profile silently gains write access.
    #[test]
    fn scaffolded_profiles_reproduce_builtin_policy() {
        let d = tempfile::tempdir().unwrap();
        scaffold(d.path());
        let profiles = crate::profiles::Profiles::discover_from(d.path(), Some(d.path()));
        for name in ["explorer", "planner", "reviewer"] {
            let profile = profiles.get(name).unwrap();
            assert!(profile.permits("read"), "{name} should keep read");
            assert!(!profile.permits("write"), "{name} must not gain write");
            assert!(!profile.permits("bash"), "{name} must not gain bash");
        }
        assert!(profiles.get("worker").unwrap().permits("bash"));
        assert!(profiles.diagnostics().is_empty(), "{:?}", profiles.diagnostics());
    }

}
