//! Which app a window belongs to.
//!
//! Everywhere else in this subsystem, window identity is a fact we hold: the
//! compositor reads the client's socket credentials and we know the pid, never
//! inferring anything from a title. Android breaks that, and it is worth being
//! precise about how. Every Android surface is committed by the *same* Wayland
//! client — Waydroid's `hwcomposer`, which is one process bridging
//! SurfaceFlinger to the compositor. So `client_pid` returns one number for
//! every window on the stage, and the field that identifies a window everywhere
//! else identifies nothing here.
//!
//! The replacement comes from inside the container, where the information
//! actually lives. In order of preference:
//!
//! 1. **The toplevel's own `app_id`**, when Waydroid set one that looks like a
//!    package. Free, and authoritative when present.
//! 2. **The resumed activity**, from `dumpsys activity activities`. Costs an adb
//!    round trip, so it is cached briefly — `windows()` is called on nearly
//!    every observation and a query per call would dominate the cost of looking.
//! 3. **The package we last launched**, which in multi-window mode is also the
//!    one `waydroid.active_apps` is filtering for.
//!
//! When none of them answer, the window is labelled `android` rather than
//! guessed at. A wrong package id is worse than an absent one: adapters match on
//! it, and matching the wrong adapter drives the wrong application's API.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::android::adb::Adb;
use crate::stage::{WindowInfo, WindowKey};

/// How long a resumed-activity answer is reused.
///
/// Short enough that switching apps is reflected within one observation, long
/// enough that a burst of `windows()` calls costs one adb round trip rather
/// than a dozen.
const CACHE_TTL: Duration = Duration::from_millis(500);

/// What a window is called when nothing in the container will say.
const UNKNOWN: &str = "android";

pub struct Identity {
    adb: Adb,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Packages Android says are resumed, most recent first.
    resumed: Vec<String>,
    fetched: Option<Instant>,
    /// The last package we deliberately launched.
    launched: Option<String>,
    /// Sticky per-window answers, so a window that has been identified once
    /// does not become anonymous the moment it stops being the resumed one.
    known: HashMap<WindowKey, String>,
}

impl Identity {
    pub fn new(adb: Adb) -> Self {
        Self {
            adb,
            state: Mutex::new(State::default()),
        }
    }

    /// Remember that we launched this package.
    pub async fn note_launch(&self, package: &str) {
        let mut state = self.state.lock().await;
        state.launched = Some(package.to_owned());
        // The resumed set is now stale by construction.
        state.fetched = None;
    }

    /// Fill in package identity for a set of windows.
    pub async fn label(&self, windows: Vec<WindowInfo>) -> Vec<WindowInfo> {
        // Nothing to ask Android about if every window already said who it is.
        let needs_lookup = windows.iter().any(|window| package_of(window).is_none());
        if needs_lookup {
            self.refresh().await;
        }

        let mut state = self.state.lock().await;
        let mut labelled = Vec::with_capacity(windows.len());
        for mut window in windows {
            let package = package_of(&window)
                .or_else(|| state.known.get(&window.key).cloned())
                .or_else(|| state.resumed.first().cloned())
                .or_else(|| state.launched.clone());

            match package {
                Some(package) => {
                    state.known.insert(window.key, package.clone());
                    window.app_id = package;
                }
                None => window.app_id = UNKNOWN.to_owned(),
            }

            // Deliberately cleared. The pid the compositor read is real, but it
            // is the bridge's for every window, so anything that correlates on
            // it would correlate every Android window to every other. `None`
            // means "we do not know", which is true and safe; a number that is
            // the same for everything is neither.
            window.pid = None;
            labelled.push(window);
        }
        labelled
    }

    async fn refresh(&self) {
        {
            let state = self.state.lock().await;
            if state
                .fetched
                .is_some_and(|when| when.elapsed() < CACHE_TTL)
            {
                return;
            }
        }

        // A failure here is not an error: it means we fall back to the launched
        // package or to `android`, both of which are honest. Propagating it
        // would turn "we could not name this window" into "listing windows
        // failed", which is far more disruptive than the missing label.
        let dump = self
            .adb
            .shell(&["dumpsys", "activity", "activities"])
            .await
            .unwrap_or_default();

        let mut state = self.state.lock().await;
        state.resumed = resumed_packages(&dump);
        state.fetched = Some(Instant::now());
    }
}

/// The package a toplevel named itself, if it looks like one.
///
/// Waydroid's own surfaces are called things like `Waydroid` and `waydroid.
/// desktop`, which are not packages and must not be treated as ones — an
/// adapter matching `waydroid*` would then claim every window on the stage.
fn package_of(window: &WindowInfo) -> Option<String> {
    let app_id = window.app_id.trim();
    if app_id.is_empty() || app_id.eq_ignore_ascii_case("waydroid") {
        return None;
    }
    // Waydroid names each toplevel `waydroid.<package>` — VERIFIED against a
    // running container, which reported `waydroid.com.android.settings` for the
    // Settings app. The prefix has to come off: every adapter, every profile and
    // every trajectory names the package the way Android does, so leaving it on
    // means nothing anyone writes by hand ever matches.
    let app_id = app_id.strip_prefix("waydroid.").unwrap_or(app_id);
    // A package id is dotted, and has something on both sides of every dot.
    let dotted = app_id.contains('.')
        && app_id
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_'));
    dotted.then(|| app_id.to_ascii_lowercase())
}

/// Packages with a resumed activity, most recently resumed first.
///
/// Parsed from `dumpsys activity activities`, whose shape has changed across
/// Android versions — so this looks for the *component* pattern on any line
/// mentioning a resumed activity rather than for a fixed line format.
fn resumed_packages(dump: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in dump.lines() {
        let line = line.trim();
        if !(line.contains("topResumedActivity=")
            || line.contains("mResumedActivity=")
            || line.contains("ResumedActivity:"))
        {
            continue;
        }
        if let Some(package) = component_package(line)
            && !found.contains(&package)
        {
            found.push(package);
        }
    }
    found
}

/// The package out of a `com.example.app/.MainActivity` component reference.
fn component_package(line: &str) -> Option<String> {
    // Scan for a `pkg/activity` token, which is how every component is written
    // regardless of what surrounds it on the line.
    line.split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '/');
            let (package, activity) = token.split_once('/')?;
            (package.contains('.') && !activity.is_empty()).then(|| package.to_ascii_lowercase())
        })
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;
    use crate::stage::WindowKind;

    fn window(app_id: &str) -> WindowInfo {
        WindowInfo {
            key: WindowKey(1),
            title: String::new(),
            app_id: app_id.to_owned(),
            pid: Some(4242),
            geometry: Rect::default(),
            kind: WindowKind::Wayland,
            mapped: true,
        }
    }

    #[test]
    fn a_dotted_app_id_is_a_package() {
        assert_eq!(
            package_of(&window("com.android.settings")).as_deref(),
            Some("com.android.settings")
        );
    }

    #[test]
    fn waydroids_own_surfaces_are_not_packages() {
        // The failure this prevents: an adapter matching `waydroid*` claiming
        // every Android window, and driving the wrong application's API.
        assert!(package_of(&window("Waydroid")).is_none());
        assert!(package_of(&window("")).is_none());
    }

    #[test]
    fn an_undotted_name_is_not_a_package() {
        assert!(package_of(&window("chromium")).is_none());
    }

    #[test]
    fn the_resumed_activity_is_read_out_of_a_dump() {
        let dump = "
            Display #0 (activities from top to bottom):
              topResumedActivity=ActivityRecord{a1b2c3 u0 com.android.settings/.Settings t42}
        ";
        assert_eq!(resumed_packages(dump), vec!["com.android.settings"]);
    }

    #[test]
    fn several_resumed_activities_keep_their_order_and_do_not_repeat() {
        let dump = "
              topResumedActivity=ActivityRecord{aa u0 com.foo.one/.Main t1}
              mResumedActivity=ActivityRecord{bb u0 com.bar.two/.Main t2}
              mResumedActivity=ActivityRecord{cc u0 com.foo.one/.Main t1}
        ";
        assert_eq!(resumed_packages(dump), vec!["com.foo.one", "com.bar.two"]);
    }

    #[test]
    fn a_dump_with_nothing_resumed_yields_nothing() {
        assert!(resumed_packages("Display #0 (activities from top to bottom):").is_empty());
    }

    #[test]
    fn a_component_is_found_wherever_it_sits_on_the_line() {
        assert_eq!(
            component_package("  mResumedActivity: ActivityRecord{x u0 com.a.b/.C t7}").as_deref(),
            Some("com.a.b")
        );
    }
}
