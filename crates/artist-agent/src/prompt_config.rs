use std::path::{Path, PathBuf};

pub(crate) fn config_root() -> Option<PathBuf> {
    std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|p| p.join("artist")))
}

/// Creates the editable defaults, but never changes an existing file.
pub(crate) fn scaffold(root: &Path) -> Vec<String> {
    let mut diagnostics = Vec::new();
    create(
        &root.join("prompts/main.md"),
        include_str!("system_prompt.md"),
        &mut diagnostics,
    );
    diagnostics
}

fn create(path: &Path, contents: &str, diagnostics: &mut Vec<String>) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scaffold_does_not_overwrite() {
        let d = tempfile::tempdir().unwrap();
        scaffold(d.path());
        let main = d.path().join("prompts/main.md");
        std::fs::write(&main, "mine").unwrap();
        scaffold(d.path());
        assert_eq!(std::fs::read_to_string(main).unwrap(), "mine");
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
