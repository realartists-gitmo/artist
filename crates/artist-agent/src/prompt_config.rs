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
    if let Err(error) = std::fs::create_dir_all(root.join("prompts")) {
        diagnostics.push(format!(
            "{}: cannot create prompt directory: {error}",
            root.join("prompts").display()
        ));
        return diagnostics;
    }
    create(
        &root.join("prompts/main.md"),
        include_str!("system_prompt.md"),
        &mut diagnostics,
    );
    for name in BUILTIN_NAMES {
        create(
            &profile_dir.join(format!("{name}.md")),
            &profile_file(name),
            &mut diagnostics,
        );
    }
    diagnostics
}

/// The shared prompt every profile is composed on top of.
///
/// A profile body says what makes that profile different; this says what is
/// true of every agent in the harness. Prepending it means a focused profile
/// is a few lines rather than a copy of the whole prompt.
pub(crate) fn base_prompt() -> (String, Vec<String>) {
    load_base(config_root().as_deref())
}

fn load_base(root: Option<&Path>) -> (String, Vec<String>) {
    let fallback = include_str!("system_prompt.md").trim_end().to_owned();
    let Some(root) = root else {
        return (fallback, Vec::new());
    };
    let mut diagnostics = scaffold(root);
    let path = root.join("prompts/main.md");
    match std::fs::read_to_string(&path) {
        Ok(value) if !value.trim().is_empty() => (value.trim_end().to_owned(), diagnostics),
        Ok(_) => {
            diagnostics.push(format!(
                "{}: shared prompt is empty; using built-in",
                path.display()
            ));
            (fallback, diagnostics)
        }
        Err(error) => {
            diagnostics.push(format!(
                "{}: cannot read shared prompt ({error}); using built-in",
                path.display()
            ));
            (fallback, diagnostics)
        }
    }
}

fn profile_file(name: &str) -> String {
    let tools = match name {
        "explorer" | "planner" | "reviewer" => "tools:\n  allow: [read, find, grep, skill]\n",
        _ => "",
    };
    format!(
        "---\ndescription: {}\n{tools}---\n\n{}",
        profile_description(name),
        profile_prompt(name)
    )
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
        "default" => "General-purpose agent with no additional specialization",
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
        // Adds nothing beyond the shared prompt every profile already gets.
        "default" => "",
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

    /// A customized shared prompt is what every profile composes on, so editing
    /// it reaches the focused profiles too rather than only the default.
    #[test]
    fn a_customized_shared_prompt_is_used_verbatim() {
        let d = tempfile::tempdir().unwrap();
        scaffold(d.path());
        std::fs::write(d.path().join("prompts/main.md"), "my careful prompt").unwrap();
        let (base, diagnostics) = load_base(Some(d.path()));
        assert_eq!(base, "my careful prompt");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn an_unreadable_shared_prompt_falls_back_to_the_builtin() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("prompts/main.md")).unwrap();
        let (base, diagnostics) = load_base(Some(d.path()));
        assert_eq!(base, include_str!("system_prompt.md").trim_end());
        assert!(diagnostics.iter().any(|d| d.contains("using built-in")));
    }

    /// `default` contributes nothing of its own — the shared prompt is the
    /// whole of it, so composing must not duplicate anything.
    #[test]
    fn the_default_profile_adds_nothing_to_the_shared_prompt() {
        let d = tempfile::tempdir().unwrap();
        scaffold(d.path());
        let profiles = crate::profiles::Profiles::discover_from(d.path(), Some(d.path()));
        assert!(profiles.get("default").unwrap().instructions.is_empty());
        assert!(
            !profiles.get("planner").unwrap().instructions.is_empty(),
            "a focused profile still contributes its own guidance"
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
