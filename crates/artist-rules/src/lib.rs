//! Time-Traveling Stream Rules (TTSR) for Artist.
//!
//! Rules sit dormant at zero context cost. When one matches the model's
//! streaming output (text, tool-call args, or reasoning summaries), the
//! in-flight completion aborts mid-token, the rule injects itself as a
//! system reminder, and the request retries from the same point — the
//! offending partial output never enters context. Each rule fires at most
//! once per session (`fire: once`, the default) or once per user turn
//! (`fire: per-turn`).
//!
//! Two tiers:
//! - **Declarative** (this crate, always on): markdown rule files with YAML
//!   frontmatter — regex patterns + a reminder body. See [`declarative`].
//! - **WASM plugins** (feature `wasm`, future module): programmable matchers
//!   behind a mandatory native prefilter.
//!
//! The abort/inject/retry driver and the rig `AgentHook` live in
//! `artist-agent`, which owns the rig dependency surface; this crate is the
//! engine: parsing, discovery, matching, and session state.

pub mod declarative;
pub mod discovery;
pub mod matcher;
pub mod retro;
pub mod state;
pub mod types;
#[cfg(feature = "wasm")]
pub mod wasm;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use matcher::RuleSet;

/// The rules engine for one project: an atomically swappable compiled rule
/// set plus the reload fingerprint. Runs snapshot the `Arc<RuleSet>` so
/// hot-reload never changes rules mid-run.
pub struct RulesEngine {
    roots: Vec<PathBuf>,
    inner: RwLock<EngineInner>,
}

struct EngineInner {
    rules: Arc<RuleSet>,
    fingerprint: u64,
    diagnostics: Vec<String>,
}

impl RulesEngine {
    /// Discover and compile all rules for a project.
    pub fn discover(workspace: &Path) -> Self {
        let roots = discovery::roots(workspace);
        let mut diagnostics = Vec::new();
        let rules = Self::compile(&roots, &mut diagnostics);
        let fingerprint = discovery::fingerprint(&roots);
        Self {
            roots,
            inner: RwLock::new(EngineInner {
                rules,
                fingerprint,
                diagnostics,
            }),
        }
    }

    fn compile(roots: &[PathBuf], diagnostics: &mut Vec<String>) -> Arc<RuleSet> {
        let (rules, wasm) = discovery::discover_all(roots, diagnostics);
        #[cfg(feature = "wasm")]
        {
            Arc::new(RuleSet::compile(rules).with_wasm(wasm))
        }
        #[cfg(not(feature = "wasm"))]
        {
            let _ = wasm;
            Arc::new(RuleSet::compile(rules))
        }
    }

    /// Cheap between-turns check: recompile only when a rule file changed.
    /// Returns fresh diagnostics when a reload happened.
    pub fn reload_if_changed(&self) -> Option<Vec<String>> {
        let fingerprint = discovery::fingerprint(&self.roots);
        {
            let inner = self.read();
            if inner.fingerprint == fingerprint {
                return None;
            }
        }
        let mut diagnostics = Vec::new();
        let previous = self.snapshot();
        let rules = Self::compile(&self.roots, &mut diagnostics);
        // Preserve wasm session KV for plugins that still exist across the
        // reload (they'd otherwise reset on any rule-file change).
        rules.inherit_wasm_kv(&previous);
        let mut inner = self.write();
        inner.rules = rules;
        inner.fingerprint = fingerprint;
        inner.diagnostics = diagnostics.clone();
        Some(diagnostics)
    }

    /// The current compiled rule set (runs hold this snapshot for their
    /// duration).
    pub fn snapshot(&self) -> Arc<RuleSet> {
        Arc::clone(&self.read().rules)
    }

    pub fn diagnostics(&self) -> Vec<String> {
        self.read().diagnostics.clone()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, EngineInner> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, EngineInner> {
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_hot_reloads_on_rule_file_change() {
        let dir = tempfile::tempdir().unwrap();
        let rules_dir = dir.path().join(".artist/rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        // No .git marker: workspace itself is the discovery start.
        let engine = RulesEngine::discover(dir.path());
        let initial = engine.snapshot();
        assert_eq!(initial.rules.len(), discovery::builtin_rules().len());
        assert!(engine.reload_if_changed().is_none());

        std::fs::write(
            rules_dir.join("leak.md"),
            "---\nname: leak\ndescription: d\npatterns: ['Box::leak']\n---\nno leaks\n",
        )
        .unwrap();
        let diagnostics = engine.reload_if_changed().expect("reload triggered");
        assert!(diagnostics.is_empty());
        let reloaded = engine.snapshot();
        assert_eq!(reloaded.rules.len(), initial.rules.len() + 1);
        // The old snapshot is untouched (runs keep their rule set).
        assert_eq!(initial.rules.len(), discovery::builtin_rules().len());
    }
}
