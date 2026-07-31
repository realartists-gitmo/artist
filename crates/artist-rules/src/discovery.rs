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

/// The guardrail on irreversible GUI actions.
///
/// This is the reason the `computer` tool requires a `label` on every anchor
/// reference. The matcher regexes the streamed tool-argument text, and an
/// argument of `{"click":{"anchor":"kv7"}}` says nothing about what `kv7` is —
/// so without the model's own echo of the element's name there would be
/// nothing here to match on, and no way to catch a destructive click before it
/// happens.
///
/// `per-turn`, not `once`: a once-per-session guardrail protects the cheap
/// first mistake and is dormant for the expensive later one.
///
/// The patterns anchor on the verb rather than requiring a closed quote,
/// because `on_tool_call_delta` matches mid-JSON and may only have seen
/// `"label":"Delete acc` when it fires. Four classes, and each earns its place:
///
/// 1. **English destructive verbs**, unbounded in where they may appear. The
///    earlier `{0,60}` prefix bound meant a long button name pushed its own verb
///    out of range — a silent hole that widened with the label.
/// 2. **The same verbs in other languages.** A stage runs whatever the user
///    runs; "Supprimer" is not less irreversible for being French.
/// 3. **Bare confirmations** — `Yes`, `OK`, `Continue`, `Proceed` — anchored to
///    the *whole* label, because those words are only alarming when they are the
///    entire button. This is the class that catches the second half of a
///    two-step destructive flow, where the first click was innocuous and the
///    dialog it opened is where the damage happens.
/// 4. **Delete keys**, including modified forms.
///
/// What this still cannot see is a key press aimed at nothing named: that is why
/// `Step::Key` takes an optional `label` naming what the model believes holds
/// focus, so `{"key":{"chord":"Enter","label":"Delete account"}}` matches class
/// 1 exactly as the click would. A bare `{"key":"Enter"}` on a focused
/// destructive default button remains outside every pattern here — no regex over
/// the arguments can recover a name the arguments do not contain — and the
/// harness-side focus cross-check is what covers it.
const COMPUTER_DESTRUCTIVE_ACTIONS: (&str, &str) = (
    r#"name: computer-destructive-actions
description: Confirm with the user before an irreversible action in a GUI
targets: [tool-args]
patterns:
  - '"label"\s*:\s*"[^"]*?(?i)\b(delete|delet|remove|discard|erase|trash|bin|send|pay|purchase|buy|order|checkout|confirm|transfer|deactivate|deregister|unsubscribe|publish|revoke|reset|format|wipe|overwrite|shut\s*down|restart|log\s*out|sign\s*out)'
  - '"label"\s*:\s*"[^"]*?(?i)(supprimer|effacer|l(ö|oe)schen|entfernen|eliminar|borrar|excluir|apagar|elimina|verwijderen|slett|ta\s*bort|poista|usu(ń|n)|udal|удалить|отправить|削除|送信|삭제|删除|刪除|发送|移除)'
  - '"label"\s*:\s*"(?i)(yes|ok|okay|continue|proceed|accept|agree|apply|submit|confirm)\s*("|$)'
  - '"key"\s*:\s*"(?i)(ctrl\+|shift\+|alt\+)*delete"'
tools: [computer]
fire: per-turn"#,
    "That step looks irreversible. Confirm with the user before doing anything \
that deletes, sends, pays, publishes, or otherwise cannot be undone — describe \
exactly what you are about to do and wait for an answer.\n\n\
Note that this aborted the whole program before any step ran, so anything \
earlier in it still needs doing. Re-issue the safe steps, then put the \
irreversible step in its own single-step call once the user has agreed.",
);

pub fn builtin_rules() -> Vec<DeclarativeRule> {
    [NO_SWALLOWED_ERRORS, COMPUTER_DESTRUCTIVE_ACTIONS]
        .into_iter()
        .map(|(yaml, body)| {
            let mut rule = parse_parts(yaml, body, None).expect("builtin rule parses");
            rule.id = crate::types::RuleId(format!("builtin:{}", rule.id.0));
            rule
        })
        .collect()
}

/// The rule directories consulted for a project, in precedence order
/// (later shadows earlier).
pub fn roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(config_root) = std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("artist")))
    {
        roots.push(config_root.join("rules"));
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

/// Programmable plugins loaded from rules dirs (empty without the `wasm`
/// feature).
#[cfg(feature = "wasm")]
pub type WasmRules = Vec<std::sync::Arc<crate::wasm::WasmRule>>;
#[cfg(not(feature = "wasm"))]
pub type WasmRules = Vec<std::convert::Infallible>;

/// Discover declarative rules plus wasm plugin manifests. Each plugin
/// contributes an ordinary declarative rule (its mandatory prefilter) with
/// a `wasm:` id; the plugin judges prefilter hits at match time.
pub fn discover_all(
    roots: &[PathBuf],
    diagnostics: &mut Vec<String>,
) -> (Vec<DeclarativeRule>, WasmRules) {
    let mut rules = discover_roots(roots, diagnostics);
    #[cfg(feature = "wasm")]
    let mut wasm: WasmRules = Vec::new();
    #[cfg(not(feature = "wasm"))]
    let wasm: WasmRules = Vec::new();
    for root in roots {
        for manifest_path in wasm_manifests(root) {
            match load_wasm_rule(&manifest_path) {
                Ok((rule, plugin)) => {
                    if rules.iter().any(|existing| existing.id == rule.id) {
                        diagnostics.push(format!(
                            "duplicate wasm rule skipped: {}",
                            manifest_path.display()
                        ));
                        continue;
                    }
                    if rules.len() >= MAX_RULES {
                        diagnostics.push(format!(
                            "rule catalog capped; skipped {}",
                            manifest_path.display()
                        ));
                        continue;
                    }
                    rules.push(rule);
                    #[cfg(feature = "wasm")]
                    wasm.push(plugin);
                    #[cfg(not(feature = "wasm"))]
                    let _ = plugin;
                }
                Err(error) => diagnostics.push(format!("{}: {error:#}", manifest_path.display())),
            }
        }
    }
    (rules, wasm)
}

/// `<name>.toml` manifests with a sibling `<name>.wasm`.
fn wasm_manifests(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .take(MAX_DIR_ENTRIES)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
                && path.with_extension("wasm").exists()
        })
        .collect();
    found.sort();
    found
}

#[cfg(feature = "wasm")]
fn load_wasm_rule(
    manifest_path: &Path,
) -> anyhow::Result<(DeclarativeRule, std::sync::Arc<crate::wasm::WasmRule>)> {
    use anyhow::Context as _;
    let name = manifest_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("manifest has no stem")?
        .to_owned();
    let manifest: crate::wasm::WasmManifest =
        toml::from_str(&std::fs::read_to_string(manifest_path)?).context("parse manifest")?;
    anyhow::ensure!(
        !manifest.prefilter.is_empty(),
        "empty prefilter (mandatory)"
    );
    let rule = crate::types::DeclarativeRule {
        id: crate::types::RuleId(format!("wasm:{name}")),
        description: manifest.description.clone(),
        targets: manifest
            .targets
            .clone()
            .unwrap_or_else(|| vec![crate::types::MatchTarget::AssistantText]),
        patterns: manifest.prefilter.clone(),
        tools: manifest.tools.clone().unwrap_or_default(),
        window: crate::types::DEFAULT_WINDOW,
        fire: manifest.fire.unwrap_or_default(),
        persistence: Default::default(),
        scope: match &manifest.scope {
            None => Default::default(),
            Some(entries) => crate::types::RuleScope {
                main: entries.iter().any(|scope| scope == "main"),
                delegate: entries.iter().any(|scope| scope == "delegate"),
            },
        },
        enabled: manifest.enabled.unwrap_or(true),
        // Placeholder — the plugin's verdict supplies the real reminder.
        reminder: "(judged by wasm plugin)".to_owned(),
        source: Some(manifest_path.to_owned()),
    };
    // Validate the prefilter regexes like ordinary patterns.
    for pattern in &rule.patterns {
        regex::RegexBuilder::new(pattern)
            .size_limit(crate::declarative::REGEX_SIZE_LIMIT)
            .build()
            .map_err(|error| anyhow::anyhow!("prefilter `{pattern}`: {error}"))?;
    }
    let plugin = std::sync::Arc::new(crate::wasm::WasmRule::load(
        rule.id.clone(),
        &manifest_path.with_extension("wasm"),
    )?);
    Ok((rule, plugin))
}

#[cfg(not(feature = "wasm"))]
fn load_wasm_rule(
    _manifest_path: &Path,
) -> anyhow::Result<(DeclarativeRule, std::convert::Infallible)> {
    anyhow::bail!("wasm rule plugins are not compiled into this build")
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
    let stamp = |file: &Path, hasher: &mut std::hash::DefaultHasher| {
        file.hash(hasher);
        if let Ok(metadata) = std::fs::metadata(file) {
            metadata.len().hash(hasher);
            if let Ok(modified) = metadata.modified() {
                modified.hash(hasher);
            }
        }
    };
    for root in roots {
        for file in rule_files(root) {
            stamp(&file, &mut hasher);
        }
        // WASM plugins reload too: hash each `.toml` manifest and its sibling
        // `.wasm` so editing or replacing a plugin triggers a reload.
        for manifest in wasm_manifests(root) {
            stamp(&manifest, &mut hasher);
            stamp(&manifest.with_extension("wasm"), &mut hasher);
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(name: &str) -> DeclarativeRule {
        builtin_rules()
            .into_iter()
            .find(|rule| rule.id.0 == format!("builtin:{name}"))
            .unwrap_or_else(|| panic!("no builtin named {name}"))
    }

    #[test]
    fn every_builtin_parses_and_ships_enabled() {
        let rules = builtin_rules();
        assert!(!rules.is_empty());
        for rule in &rules {
            assert!(rule.id.0.starts_with("builtin:"), "{}", rule.id.0);
            assert!(rule.enabled, "{} must ship enabled", rule.id.0);
            assert!(!rule.reminder.is_empty(), "{} needs a reminder", rule.id.0);
        }
    }

    #[test]
    fn the_swallowed_errors_builtin_targets_the_file_tools() {
        let rule = builtin("no-swallowed-errors");
        assert_eq!(rule.tools, vec!["write", "edit"]);
    }

    #[test]
    fn the_destructive_action_builtin_matches_a_label_not_an_anchor() {
        let rule = builtin("computer-destructive-actions");
        assert_eq!(rule.tools, vec!["computer"]);
        // Per-turn, not once: the guardrail must still be armed for the second
        // irreversible action of a session, which is usually the costly one.
        assert_eq!(rule.fire, crate::types::FirePolicy::PerTurn);

        let set = crate::matcher::RuleSet::compile(vec![rule]);
        let matches = |text: &str| {
            !set.scan_all(crate::types::MatchTarget::ToolArgs, text, Some("computer"))
                .is_empty()
        };

        assert!(matches(r#"{"click":{"anchor":"kv7","label":"Delete account"}}"#));
        assert!(matches(r#"{"click":{"anchor":"kv7","label":"Send message"}}"#));
        // Mid-stream, before the closing quote has arrived.
        assert!(matches(r#"{"click":{"anchor":"kv7","label":"Delete acc"#));
        // The anchor alone carries no meaning and must not fire anything.
        assert!(!matches(r#"{"click":{"anchor":"kv7"}}"#));
        assert!(!matches(r#"{"click":{"anchor":"kv7","label":"Compose"}}"#));
    }

    #[test]
    fn the_destructive_action_builtin_covers_what_it_claims_to() {
        let set = crate::matcher::RuleSet::compile(vec![builtin(
            "computer-destructive-actions",
        )]);
        let matches = |text: &str| {
            !set.scan_all(crate::types::MatchTarget::ToolArgs, text, Some("computer"))
                .is_empty()
        };
        let label = |name: &str| format!(r#"{{"click":{{"anchor":"kv7","label":"{name}"}}}}"#);

        // A bare confirmation is the second half of every two-step destructive
        // flow: the first click opens a dialog, and this is the one that does it.
        for name in ["Yes", "OK", "Continue", "Proceed", "Confirm", "Apply"] {
            assert!(matches(&label(name)), "{name} must be caught");
        }
        // Non-English. A stage runs whatever the user runs.
        for name in ["Supprimer", "Löschen", "删除", "Eliminar", "Удалить"] {
            assert!(matches(&label(name)), "{name} must be caught");
        }
        // The old `{0,60}` prefix bound let a long label push its own verb out
        // of range, so the guardrail weakened as the button got wordier.
        assert!(matches(&label(
            "Are you absolutely sure you want to permanently delete this workspace"
        )));
        for name in ["Move to Trash", "Empty Bin", "Place order", "Submit order"] {
            assert!(matches(&label(name)), "{name} must be caught");
        }
        // A key press that names what it will activate is caught exactly as the
        // click would be — the whole reason `key` carries an optional label.
        assert!(matches(
            r#"{"key":{"chord":"Enter","label":"Delete account"}}"#
        ));

        // And the ordinary things stay quiet, or the rule is noise and gets
        // ignored where it matters.
        for name in ["Compose", "Reply", "Settings", "Back", "Search", "Yesterday"] {
            assert!(!matches(&label(name)), "{name} must not fire");
        }
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
        let shadowed = rules
            .iter()
            .find(|rule| rule.id.0.ends_with("no-swallowed-errors"))
            .expect("the shadowing rule survives");
        assert_eq!(shadowed.reminder, "mine");
        assert_eq!(
            rules
                .iter()
                .filter(|rule| rule.id.0.ends_with("no-swallowed-errors"))
                .count(),
            1,
            "the builtin must be replaced, not duplicated"
        );
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
