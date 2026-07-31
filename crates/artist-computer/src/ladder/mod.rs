//! Choosing how to drive a surface.
//!
//! The ladder is the "generalized" part of generalized computer use. It is not
//! one verb list that works everywhere; it is a ranked set of abstraction levels
//! with a probe that finds the highest one a given surface supports, and a cache
//! so the answer is paid for once.
//!
//! Rung choice is **per surface, not per application**. A browser window is
//! rung 0 for its own chrome — `Target.createTarget` and `Page.navigate` are
//! programmatic calls, and reaching for the accessibility tree to click a tab
//! would be both fragile and unnecessary — and rung 1 for page content. A native
//! file dialog opened by that same browser is a separate toplevel and gets
//! probed on its own.

pub mod adapters;

use crate::model::Rung;

/// What the probe learned about one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub rung: Rung,
    /// Which maker answered, for diagnostics and for the adapter cache.
    pub maker: &'static str,
}

/// Everything the probe is allowed to look at.
///
/// Deliberately concrete facts rather than guesses: the pid we spawned, the
/// app id the window reported, the debugging port we asked for. Nothing here is
/// inferred from a window title.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub app_id: String,
    pub argv0: String,
    pub pid: Option<i32>,
    /// Set when this target is a PTY we started — a fact we hold, never sniffed.
    pub is_pty: bool,
    /// Chromium's user-data dir, when the target was launched as a browser.
    pub user_data_dir: Option<std::path::PathBuf>,
    /// Whether an accessibility tree with real content was found.
    pub accessible_nodes: usize,
    pub mapped: bool,
}

/// An accessibility tree smaller than this is a stub, not an attachment.
///
/// Toolkits frequently expose a single bare application node whether or not the
/// bridge is really working. Accepting that would strand the surface on a rung
/// that cannot actually drive it, which is worse than descending to pixels.
pub const MIN_TREE_NODES: usize = 8;

/// Descend the ladder until something answers.
pub fn select(probe: &Probe, adapters: &adapters::AdapterSet) -> Attachment {
    // Rung 0: does an adapter claim this application?
    if adapters.for_app(&probe.app_id).is_some() || adapters.for_app(&probe.argv0).is_some() {
        return Attachment {
            rung: Rung::Programmatic,
            maker: "adapter",
        };
    }

    // Rung 1: an engine protocol that already models the content.
    if probe.user_data_dir.is_some() {
        return Attachment {
            rung: Rung::Engine,
            maker: "cdp",
        };
    }
    if probe.is_pty {
        return Attachment {
            rung: Rung::Engine,
            maker: "pty",
        };
    }

    // Rung 2: a real accessibility tree.
    if probe.accessible_nodes >= MIN_TREE_NODES {
        return Attachment {
            rung: Rung::Accessibility,
            maker: "atspi",
        };
    }

    // Rung 3: pixels. Observation only — see the crate docs.
    Attachment {
        rung: Rung::Pixels,
        maker: "pixels",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapters_with(toml: &str) -> adapters::AdapterSet {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("adapters");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.toml"), toml).unwrap();
        // The TempDir is dropped here, but discovery has already read the file.
        adapters::AdapterSet::discover_roots(&[root])
    }

    #[test]
    fn an_adapter_wins_over_every_other_rung() {
        // The whole point of rung 0: even when a browser protocol and an
        // accessibility tree are both available, the application's own API is
        // cheaper and more reliable than either.
        let set = adapters_with("name = \"vlc\"\nmatch_app_id = [\"org.videolan.*\"]\n");
        let probe = Probe {
            app_id: "org.videolan.vlc".into(),
            user_data_dir: Some("/tmp/profile".into()),
            accessible_nodes: 400,
            ..Probe::default()
        };
        assert_eq!(select(&probe, &set).rung, Rung::Programmatic);
    }

    #[test]
    fn a_browser_is_engine_rung() {
        let probe = Probe {
            app_id: "chromium".into(),
            user_data_dir: Some("/tmp/profile".into()),
            accessible_nodes: 500,
            ..Probe::default()
        };
        let attachment = select(&probe, &adapters::AdapterSet::default());
        assert_eq!(attachment.rung, Rung::Engine);
        assert_eq!(attachment.maker, "cdp");
    }

    #[test]
    fn a_terminal_is_engine_rung_because_we_know_we_started_it() {
        let probe = Probe {
            is_pty: true,
            ..Probe::default()
        };
        let attachment = select(&probe, &adapters::AdapterSet::default());
        assert_eq!(attachment.rung, Rung::Engine);
        assert_eq!(attachment.maker, "pty");
    }

    #[test]
    fn a_toolkit_app_with_a_real_tree_is_accessibility_rung() {
        let probe = Probe {
            app_id: "org.gnome.TextEditor".into(),
            accessible_nodes: 120,
            ..Probe::default()
        };
        assert_eq!(
            select(&probe, &adapters::AdapterSet::default()).rung,
            Rung::Accessibility
        );
    }

    #[test]
    fn a_stub_tree_falls_through_to_pixels() {
        // One bare application node is what a toolkit reports when its bridge
        // never started. Treating that as rung 2 would strand the surface
        // somewhere it cannot be driven.
        let probe = Probe {
            app_id: "some.game".into(),
            accessible_nodes: 1,
            ..Probe::default()
        };
        assert_eq!(
            select(&probe, &adapters::AdapterSet::default()).rung,
            Rung::Pixels
        );
    }

    #[test]
    fn rungs_are_ordered_cheapest_first() {
        assert!(Rung::Programmatic < Rung::Engine);
        assert!(Rung::Engine < Rung::Accessibility);
        assert!(Rung::Accessibility < Rung::Pixels);
    }
}
