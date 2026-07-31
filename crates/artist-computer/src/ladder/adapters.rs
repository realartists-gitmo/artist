//! Rung 0: driving an application through its own interface.
//!
//! This is the rung everyone omits and the one that carries most of the value.
//! Pausing a media player is one D-Bus call; doing it by clicking is a capture, a
//! tree walk, an anchor resolution and a synthetic input event, each of which can
//! fail on its own. Where an application has an API, using the API is not an
//! optimization — it is the difference between an operation that works and one
//! that usually works.
//!
//! Adapters are declarative TOML rather than Rust so the knowledge can grow
//! without touching the crate. Discovery is layered exactly like stream rules and
//! skills: a global directory, then a project one, with later scopes shadowing
//! earlier by name.
//!
//! ```toml
//! # ~/.config/artist/computer/adapters/mpris.toml
//! name = "mpris"
//! match_app_id = ["org.mpris.MediaPlayer2.*", "spotify"]
//!
//! [[action]]
//! name = "pause"
//! description = "Pause playback"
//! dbus = { service = "org.mpris.MediaPlayer2.spotify",
//!          path = "/org/mpris/MediaPlayer2",
//!          interface = "org.mpris.MediaPlayer2.Player",
//!          method = "Pause" }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One declarative adapter.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub name: String,
    /// Glob patterns matched against a window's app id or a command's argv[0].
    #[serde(default)]
    pub match_app_id: Vec<String>,
    #[serde(default, rename = "action")]
    pub actions: Vec<Action>,
}

/// One thing an adapter can do, and how.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub call: Call,
}

/// The transports an adapter can speak.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Call {
    Dbus(DbusCall),
    /// A command line. `{value}` is substituted from the step's text.
    Cli { argv: Vec<String> },
    /// An HTTP request against a loopback endpoint.
    Http { method: String, url: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DbusCall {
    pub service: String,
    pub path: String,
    pub interface: String,
    pub method: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl Adapter {
    /// Whether this adapter claims an application.
    pub fn matches(&self, app_id: &str) -> bool {
        self.match_app_id
            .iter()
            .any(|pattern| glob_match(pattern, app_id))
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.name == name)
    }
}

/// A very small glob: `*` matches any run, everything else is literal.
///
/// Deliberately not a full glob crate. Adapter patterns are application ids like
/// `org.mpris.MediaPlayer2.*`, and a matcher with character classes and
/// alternation would be more to get wrong than to gain.
fn glob_match(pattern: &str, value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return false;
    };
    if !value.starts_with(first) {
        return false;
    }
    let mut cursor = first.len();
    let mut last_was_wildcard = pattern.len() > first.len();
    for part in parts {
        if part.is_empty() {
            last_was_wildcard = true;
            continue;
        }
        match value[cursor..].find(part) {
            Some(offset) => cursor += offset + part.len(),
            None => return false,
        }
        last_was_wildcard = false;
    }
    last_was_wildcard || cursor == value.len()
}

/// The loaded adapter set for a project.
#[derive(Clone, Debug, Default)]
pub struct AdapterSet {
    adapters: BTreeMap<String, Adapter>,
    diagnostics: Vec<String>,
}

impl AdapterSet {
    /// Discover adapters across every scope, later shadowing earlier by name.
    pub fn discover(project: &Path) -> Self {
        Self::discover_roots(&roots(project))
    }

    pub fn discover_roots(roots: &[PathBuf]) -> Self {
        let mut set = Self::default();
        for root in roots {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            let mut paths: Vec<_> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
                .collect();
            // Deterministic order so a collision resolves the same way twice.
            paths.sort();
            for path in paths {
                match std::fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| toml::from_str::<Adapter>(&text).map_err(|e| e.to_string()))
                {
                    Ok(adapter) => {
                        // A malformed adapter must never take the whole set
                        // down: the other applications still need driving.
                        if set.adapters.insert(adapter.name.clone(), adapter).is_some() {
                            set.diagnostics
                                .push(format!("{} shadows an earlier adapter", path.display()));
                        }
                    }
                    Err(error) => set
                        .diagnostics
                        .push(format!("{}: {error}", path.display())),
                }
            }
        }
        set
    }

    /// The adapter claiming an application, if any.
    pub fn for_app(&self, app_id: &str) -> Option<&Adapter> {
        self.adapters
            .values()
            .find(|adapter| adapter.matches(app_id))
    }

    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

/// Adapter directories, in precedence order (later shadows earlier).
pub fn roots(project: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(config) = std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|dir| dir.join("artist")))
    {
        roots.push(config.join("computer").join("adapters"));
    }
    roots.push(project.join(".artist").join("computer").join("adapters"));
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    const MPRIS: &str = r#"
name = "mpris"
match_app_id = ["org.mpris.MediaPlayer2.*"]

[[action]]
name = "pause"
description = "Pause playback"
dbus = { service = "org.mpris.MediaPlayer2.spotify", path = "/org/mpris/MediaPlayer2", interface = "org.mpris.MediaPlayer2.Player", method = "Pause" }
"#;

    #[test]
    fn an_adapter_parses_and_claims_its_application() {
        let adapter: Adapter = toml::from_str(MPRIS).unwrap();
        assert_eq!(adapter.name, "mpris");
        assert!(adapter.matches("org.mpris.MediaPlayer2.spotify"));
        assert!(!adapter.matches("org.gnome.Calculator"));

        let action = adapter.action("pause").expect("pause action");
        let Call::Dbus(call) = &action.call else {
            panic!("expected a dbus call");
        };
        assert_eq!(call.method, "Pause");
    }

    #[test]
    fn globs_match_prefixes_suffixes_and_middles() {
        assert!(glob_match("org.mpris.*", "org.mpris.MediaPlayer2.vlc"));
        assert!(glob_match("*firefox", "/usr/bin/firefox"));
        assert!(glob_match("*chrom*", "/usr/bin/chromium"));
        assert!(glob_match("exact", "exact"));

        assert!(!glob_match("exact", "exactly"));
        assert!(!glob_match("org.mpris.*", "com.spotify.Client"));
        assert!(!glob_match("*firefox", "firefox-esr"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(glob_match("*Firefox", "/usr/bin/firefox"));
        assert!(glob_match("*firefox", "/usr/bin/Firefox"));
    }

    #[test]
    fn a_project_adapter_shadows_a_global_one_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        let project = dir.path().join("project");
        write(&global, "mpris.toml", MPRIS);
        write(
            &project,
            "mpris.toml",
            r#"
name = "mpris"
match_app_id = ["mine"]
"#,
        );

        let set = AdapterSet::discover_roots(&[global, project]);
        assert_eq!(set.len(), 1, "the adapter must be replaced, not duplicated");
        assert!(set.for_app("mine").is_some());
        assert!(set.for_app("org.mpris.MediaPlayer2.vlc").is_none());
        assert_eq!(set.diagnostics().len(), 1);
    }

    #[test]
    fn a_malformed_adapter_becomes_a_diagnostic_rather_than_killing_the_set() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("adapters");
        write(&root, "good.toml", MPRIS);
        write(&root, "broken.toml", "this is not toml = = =");

        let set = AdapterSet::discover_roots(&[root]);
        assert_eq!(set.len(), 1, "the good adapter must still load");
        assert_eq!(set.diagnostics().len(), 1);
        assert!(set.diagnostics()[0].contains("broken.toml"));
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_silently_ignored() {
        // A typo in an adapter should be reported, not quietly do nothing.
        let result = toml::from_str::<Adapter>(
            r#"
name = "typo"
match_appid = ["x"]
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn cli_and_http_transports_parse() {
        let adapter: Adapter = toml::from_str(
            r#"
name = "mixed"
match_app_id = ["*"]

[[action]]
name = "open"
cli = { argv = ["xdg-open", "{value}"] }

[[action]]
name = "health"
http = { method = "GET", url = "http://127.0.0.1:8080/health" }
"#,
        )
        .unwrap();

        assert!(matches!(adapter.action("open").unwrap().call, Call::Cli { .. }));
        assert!(matches!(
            adapter.action("health").unwrap().call,
            Call::Http { .. }
        ));
    }

    #[test]
    fn discovery_of_a_missing_directory_is_not_an_error() {
        let set = AdapterSet::discover_roots(&[PathBuf::from("/nonexistent/adapters")]);
        assert!(set.is_empty());
        assert!(set.diagnostics().is_empty());
    }
}
