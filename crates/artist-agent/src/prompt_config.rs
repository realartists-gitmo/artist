use std::path::{Path, PathBuf};

pub(crate) fn config_root() -> Option<PathBuf> {
    std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|p| p.join("artist")))
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
    let mut diagnostics = Vec::new();
    let path = root.join("prompts/main.md");
    if !path.exists() {
        // Absent is the normal case: the shipped prompt is used, and future
        // improvements to it land without anyone editing a file.
        return (fallback, diagnostics);
    }
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

pub(crate) fn profile_description(name: &str) -> &'static str {
    match name {
        "default" => "General-purpose agent with no additional specialization",
        "worker" => "Implementation-focused agent for bounded changes and verification",
        "explorer" => "Read-heavy agent for tracing code and gathering evidence",
        "planner" => {
            "Planning agent that turns requirements and code evidence into an executable plan"
        }
        "reviewer" => "Review agent focused on correctness, regressions, security, and missing tests",
        // A name with no text of its own, rather than the last arm's. This used
        // to fall through to the reviewer, so a sixth built-in would have
        // shipped describing itself as one.
        _ => "",
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
        "reviewer" => {
            "Review like a code owner. Lead with concrete findings ordered by severity, cite files and symbols, explain impact and reproduction, and avoid style-only feedback. Do not edit files.\n"
        }
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::BUILTIN_NAMES;

    /// Nothing is written to disk. Built-ins live in the binary so that
    /// improving them reaches every install, and a file is purely an override.
    #[test]
    fn an_untouched_config_directory_stays_empty() {
        let d = tempfile::tempdir().unwrap();
        let (base, diagnostics) = load_base(Some(d.path()));
        assert_eq!(base, include_str!("system_prompt.md").trim_end());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(
            std::fs::read_dir(d.path()).unwrap().next().is_none(),
            "config directory should not be populated"
        );

        let profiles = crate::profiles::Profiles::discover_from(d.path(), Some(d.path()));
        for name in BUILTIN_NAMES {
            assert!(profiles.get(name).is_ok(), "missing built-in {name}");
        }
    }

    /// A built-in with no text of its own is a built-in that ships wearing
    /// someone else's. Descriptions are what the delegating model chooses from,
    /// so a blank one is unpickable; `default` is the one profile that adds
    /// nothing to the shared prompt, and no other may share that emptiness.
    #[test]
    fn every_builtin_has_text_of_its_own() {
        let mut prompts = std::collections::BTreeSet::new();
        for name in BUILTIN_NAMES {
            assert!(
                !profile_description(name).is_empty(),
                "{name} has no description to be chosen by"
            );
            assert!(
                prompts.insert(profile_prompt(name)),
                "{name} shares another built-in's prompt"
            );
        }
    }

    #[test]
    fn a_shared_prompt_file_overrides_the_builtin() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("prompts")).unwrap();
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

    #[test]
    fn the_default_profile_adds_nothing_to_the_shared_prompt() {
        let d = tempfile::tempdir().unwrap();
        let profiles = crate::profiles::Profiles::discover_from(d.path(), Some(d.path()));
        assert!(profiles.get("default").unwrap().instructions.is_empty());
        assert!(!profiles.get("planner").unwrap().instructions.is_empty());
    }
}
