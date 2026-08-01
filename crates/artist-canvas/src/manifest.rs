//! `canvas.toml` — what a canvas declares about itself.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Everything a canvas states up front.
///
/// Unknown keys are rejected rather than ignored, matching `settings.toml`: a
/// typo in a permission name should fail loudly instead of silently granting
/// nothing.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Shown in the browser tab and in `canvas list`.
    #[serde(default)]
    pub title: String,
    /// Module the page boots from, relative to the canvas directory.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// Load the Tailwind browser build on this page.
    #[serde(default = "default_true")]
    pub tailwind: bool,
    #[serde(default)]
    pub permissions: Permissions,
    /// Extra bare specifiers for the import map, pointing at absolute URLs.
    /// These need the network at page load; the vendored set does not.
    #[serde(default)]
    pub deps: BTreeMap<String, String>,
    #[serde(default)]
    pub limits: Limits,
}

/// Ceilings this canvas needs raised above the defaults.
///
/// The defaults exist because a canvas is model-written and nothing about it is
/// rate-limited — a render loop that writes state grows a file in the user's
/// repo until the disk is gone. They are set well past normal use, so meeting
/// one usually means a bug rather than an ambitious canvas. A canvas that
/// genuinely needs more says so here, where the user can see the claim in their
/// own repo rather than discovering it from disk usage.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Bytes of durable state this canvas may hold, overriding
    /// `state::DEFAULT_MAX_BYTES`.
    #[serde(default)]
    pub state_bytes: Option<usize>,
}

/// Tools this canvas may invoke.
///
/// This can only narrow what the profile and settings already allow — a canvas
/// cannot grant itself anything the model running it could not call directly.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub allow: Vec<String>,
    /// Other canvases in this project whose shared state this one may read.
    ///
    /// Declared rather than open for the same reason `allow` is. Canvases in a
    /// project are one trust domain, but they are also independent surfaces the
    /// model wrote at different times, and a form quietly reading a dashboard's
    /// state is a coupling nobody chose. Naming it makes it a decision, and
    /// makes it visible in the file the user can read.
    ///
    /// Read-only on purpose: a canvas writing into another's state would give
    /// two surfaces a shared mutable store with no arbiter, and the interesting
    /// case — a form whose decision a dashboard reflects — is served by the
    /// dashboard reading the form.
    #[serde(default)]
    pub canvases: Vec<String>,
}

fn default_entry() -> String {
    "main.jsx".to_owned()
}

fn default_true() -> bool {
    true
}

/// Derived `Default` would disagree with `parse("")`, handing out an empty
/// entry path and silently dropping Tailwind. The two must describe the same
/// canvas, so this is written out rather than derived.
impl Default for Manifest {
    fn default() -> Self {
        Self {
            title: String::new(),
            entry: default_entry(),
            tailwind: default_true(),
            permissions: Permissions::default(),
            deps: BTreeMap::new(),
            limits: Limits::default(),
        }
    }
}

impl Manifest {
    pub fn parse(source: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(source)
    }

    pub fn render(&self) -> String {
        toml::to_string_pretty(self).expect("a canvas manifest is always serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_manifest_still_boots_a_canvas() {
        let manifest = Manifest::parse("").expect("empty is valid");
        assert_eq!(manifest.entry, "main.jsx");
        assert!(manifest.tailwind);
        assert!(manifest.permissions.allow.is_empty());
    }

    /// An omitted `canvas.toml` and an empty one must describe the same canvas.
    /// When these drifted, `Manifest::default()` handed out an empty entry path
    /// and quietly turned Tailwind off.
    #[test]
    fn the_default_manifest_matches_an_empty_file() {
        assert_eq!(Manifest::default(), Manifest::parse("").expect("empty"));
    }

    #[test]
    fn declarations_round_trip() {
        let manifest = Manifest {
            title: "Perf".into(),
            entry: "app.tsx".into(),
            tailwind: false,
            permissions: Permissions {
                allow: vec!["read".into(), "grep".into()],
                canvases: vec!["dashboard".into()],
            },
            deps: BTreeMap::from([("three".to_owned(), "https://esm.sh/three".to_owned())]),
            limits: Limits {
                state_bytes: Some(8 * 1024 * 1024),
            },
        };

        let reparsed = Manifest::parse(&manifest.render()).expect("round trip");
        assert_eq!(reparsed, manifest);
    }

    /// A canvas that says nothing about limits gets the defaults, and one that
    /// raises a limit has to be taken at its word — that is the whole point of
    /// the override.
    #[test]
    fn limits_default_to_absent_and_survive_being_declared() {
        assert_eq!(
            Manifest::parse("")
                .expect("empty parses")
                .limits
                .state_bytes,
            None
        );
        let raised = Manifest::parse("[limits]\nstate_bytes = 16777216\n").expect("parses");
        assert_eq!(raised.limits.state_bytes, Some(16 * 1024 * 1024));
    }

    /// A misspelled key that silently did nothing would hand the model a canvas
    /// whose permissions it believes it set.
    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Manifest::parse("titel = \"oops\"").is_err());
        assert!(Manifest::parse("[permissions]\nalow = [\"read\"]").is_err());
    }
}
