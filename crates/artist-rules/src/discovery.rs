//! Rule discovery: built-ins + rules directories, with an mtime fingerprint
//! for cheap between-turn hot-reload checks.
//!
//! Roots mirror skills discovery (`artist-agent/src/resources/skills.rs`):
//! global config, `~/.agents`, then each ancestor from the git root down —
//! later scopes shadow earlier ones by rule name.

use std::path::{Path, PathBuf};

use crate::declarative::{self, parse_parts};
use crate::types::DeclarativeRule;

pub const MAX_RULES: usize = 200;
const MAX_DIR_ENTRIES: usize = 512;

/// The one curated built-in that ships enabled. Users can disable it from
/// `/rules`; a project rule with the same name shadows it.
const NO_SWALLOWED_ERRORS: (&str, &str) = (
    r#"name: no-swallowed-errors
description: Catch edits that bury failures instead of handling or propagating them
targets: [tool-args]
patterns:
  - 'catch\s*(\([^)]{0,80}\))?\s*\{\s*\}'
  - '\.unwrap_or_default\(\)'
  - 'except[^:\n]{0,80}:\s*\n\s*pass\b'
  - '(?i)//\s*ignore (the )?error'
tools: [write, edit]"#,
    "Do not swallow errors. An empty catch block, `except: pass`, or a \
blanket `.unwrap_or_default()` hides failures from the user and from \
yourself. Handle the error, propagate it, or explain in a comment why \
ignoring it is genuinely correct here.",
);

pub fn builtin_rules() -> Vec<DeclarativeRule> {
    let (yaml, body) = NO_SWALLOWED_ERRORS;
    let mut rule = parse_parts(yaml, body, None).expect("builtin rule parses");
    rule.id = crate::types::RuleId(format!("builtin:{}", rule.id.0));
    vec![rule]
}

/// The rule directories consulted for a project, in precedence order
/// (later shadows earlier).
pub fn roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
        roots.push(config.join("artist/rules"));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".agents/rules"));
    }
    let start = workspace
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(workspace);
    let mut directories = workspace
        .ancestors()
        .take_while(|path| path.starts_with(start))
        .collect::<Vec<_>>();
    directories.reverse();
    for directory in directories {
        roots.push(directory.join(".artist/rules"));
        roots.push(directory.join(".agents/rules"));
    }
    roots
}

/// Discover all rules: built-ins first, then rule files (which shadow
/// built-ins and earlier roots on name collision).
pub fn discover(workspace: &Path, diagnostics: &mut Vec<String>) -> Vec<DeclarativeRule> {
    discover_roots(&roots(workspace), diagnostics)
}

pub fn discover_roots(roots: &[PathBuf], diagnostics: &mut Vec<String>) -> Vec<DeclarativeRule> {
    let mut rules: Vec<DeclarativeRule> = builtin_rules();
    for root in roots {
        for file in rule_files(root) {
            match declarative::parse(&file) {
                Ok(rule) => {
                    if let Some(existing) = rules.iter_mut().find(|existing| {
                        existing.id == rule.id || existing.id.0 == format!("builtin:{}", rule.id.0)
                    }) {
                        diagnostics.push(format!(
                            "rule collision resolved by later scope: {}",
                            file.display()
                        ));
                        *existing = rule;
                    } else if rules.len() >= MAX_RULES {
                        diagnostics
                            .push(format!("rule catalog capped; skipped {}", file.display()));
                    } else {
                        rules.push(rule);
                    }
                }
                Err(error) => diagnostics.push(error),
            }
        }
    }
    rules
}

/// `*.md` files directly inside a rules dir (no recursion — rules are flat;
/// wasm plugins arrive as `*.wasm` + manifest later).
fn rule_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .take(MAX_DIR_ENTRIES)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file() && !kind.is_symlink())
                .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    found.sort();
    found
}

/// Cheap fingerprint of every rules dir: (path, mtime, len) of each rule
/// file, hashed structurally. Compared between turns to decide reload.
pub fn fingerprint(roots: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    for root in roots {
        for file in rule_files(root) {
            file.hash(&mut hasher);
            if let Ok(metadata) = std::fs::metadata(&file) {
                metadata.len().hash(&mut hasher);
                if let Ok(modified) = metadata.modified() {
                    modified.hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_parses_and_is_enabled() {
        let rules = builtin_rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id.0, "builtin:no-swallowed-errors");
        assert!(rules[0].enabled);
        assert_eq!(rules[0].tools, vec!["write", "edit"]);
    }

    #[test]
    fn project_rule_shadows_builtin_by_bare_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("rules");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("no-swallowed-errors.md"),
            "---\nname: no-swallowed-errors\ndescription: mine\npatterns: ['x']\n---\nmine\n",
        )
        .unwrap();
        let mut diagnostics = Vec::new();
        let rules = discover_roots(&[root], &mut diagnostics);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].reminder, "mine");
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn invalid_rule_becomes_diagnostic_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("rules");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("broken.md"), "no frontmatter here").unwrap();
        let mut diagnostics = Vec::new();
        let rules = discover_roots(&[root], &mut diagnostics);
        assert_eq!(rules.len(), builtin_rules().len());
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn fingerprint_changes_on_edit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("rules");
        std::fs::create_dir_all(&root).unwrap();
        let roots = vec![root.clone()];
        let before = fingerprint(&roots);
        std::fs::write(
            root.join("a.md"),
            "---\nname: a\ndescription: d\npatterns: ['x']\n---\nbody\n",
        )
        .unwrap();
        let after = fingerprint(&roots);
        assert_ne!(before, after);
    }
}
