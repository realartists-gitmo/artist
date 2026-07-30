use std::path::{Path, PathBuf};

pub(crate) const ROLE_NAMES: [&str; 5] = ["default", "worker", "explorer", "planner", "reviewer"];

pub(crate) fn config_root() -> Option<PathBuf> {
    std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|p| p.join("artist")))
}

/// Creates the editable defaults, but never changes an existing file.
pub(crate) fn scaffold(root: &Path) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let prompt_dir = root.join("prompts/subagents");
    if let Err(error) = std::fs::create_dir_all(&prompt_dir) {
        diagnostics.push(format!(
            "{}: cannot create prompt directory: {error}",
            prompt_dir.display()
        ));
        return diagnostics;
    }
    create(
        &root.join("prompts/main.md"),
        include_str!("system_prompt.md"),
        &mut diagnostics,
    );
    for name in ROLE_NAMES {
        create(
            &prompt_dir.join(format!("{name}.md")),
            role_prompt(name),
            &mut diagnostics,
        );
    }
    let config = root.join("subagents.toml");
    let body = ROLE_NAMES.iter().map(|name| format!(
        "[agents.{name}]\ndescription = \"{}\"\ninstructions_file = \"prompts/subagents/{name}.md\"\n\n",
        role_description(name)
    )).collect::<String>();
    create(
        &config,
        &format!("[settings]\nmax_concurrent = 4\n\n{body}"),
        &mut diagnostics,
    );
    diagnostics
}

fn create(path: &Path, contents: &str, diagnostics: &mut Vec<String>) {
    if path.exists() {
        return;
    }
    if let Err(error) = std::fs::write(path, contents) {
        diagnostics.push(format!(
            "{}: cannot create default: {error}",
            path.display()
        ));
    }
}

pub(crate) fn main_prompt() -> (String, Vec<String>) {
    load_main(config_root().as_deref())
}

fn load_main(root: Option<&Path>) -> (String, Vec<String>) {
    let fallback = include_str!("system_prompt.md").trim_end().to_owned();
    let Some(root) = root else {
        return (fallback, vec![]);
    };
    let mut diagnostics = scaffold(root);
    let path = root.join("prompts/main.md");
    match std::fs::read_to_string(&path) {
        Ok(value) if !value.trim().is_empty() => (value.trim_end().to_owned(), diagnostics),
        Ok(_) => {
            diagnostics.push(format!(
                "{}: custom main prompt is empty; using built-in",
                path.display()
            ));
            (fallback, diagnostics)
        }
        Err(error) => {
            diagnostics.push(format!(
                "{}: cannot read custom main prompt ({error}); using built-in",
                path.display()
            ));
            (fallback, diagnostics)
        }
    }
}

pub(crate) fn role_description(name: &str) -> &'static str {
    match name {
        "default" => "General-purpose agent inheriting the parent configuration",
        "worker" => "Implementation-focused agent for bounded changes and verification",
        "explorer" => "Read-heavy agent for tracing code and gathering evidence",
        "planner" => {
            "Planning agent that turns requirements and code evidence into an executable plan"
        }
        _ => "Review agent focused on correctness, regressions, security, and missing tests",
    }
}
pub(crate) fn role_prompt(name: &str) -> &'static str {
    match name {
        "default" => "Complete the delegated task and return concise findings with evidence.\n",
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
        let main = d.path().join("prompts/main.md");
        std::fs::write(&main, "mine").unwrap();
        let config = d.path().join("subagents.toml");
        std::fs::write(&config, "mine-config").unwrap();
        scaffold(d.path());
        assert_eq!(std::fs::read_to_string(main).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(config).unwrap(), "mine-config");
        for name in ROLE_NAMES {
            assert!(
                d.path()
                    .join(format!("prompts/subagents/{name}.md"))
                    .is_file()
            );
        }
    }

    #[test]
    fn unreadable_main_uses_embedded_fallback() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("prompts/main.md")).unwrap();
        let (prompt, diagnostics) = load_main(Some(d.path()));
        assert_eq!(prompt, include_str!("system_prompt.md").trim_end());
        assert!(diagnostics.iter().any(|d| d.contains("using built-in")));
    }
}
