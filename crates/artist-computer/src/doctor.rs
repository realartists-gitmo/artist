//! One report answering "will computer use work here, and if not, why not?"
//!
//! Every check below already existed — as a runtime error, thrown at the moment
//! a task failed, from wherever in the stack noticed. That is the worst possible
//! time and place to learn any of it, and it is why this module exists.
//!
//! The trap the subsystem's own documentation names is the motivating case: a
//! toolkit that was never told accessibility is on produces an *empty tree*,
//! which looks exactly like the display being broken. Two unrelated problems,
//! one symptom, and no way to tell them apart from inside a failing task.
//!
//! So every check carries three things: what was looked for, whether it is a
//! **blocker** or merely degrades something, and **what to do about it**. A
//! report that says `at-spi2-registryd: missing` and stops has moved the problem
//! rather than solved it.

use std::path::Path;

/// How much a failed check costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Everything works.
    Ok,
    /// Works, but something is unavailable — a rung, a feature, a speedup.
    Degraded,
    /// Nothing that needs a display will work at all.
    Blocker,
}

impl Severity {
    fn mark(self) -> &'static str {
        match self {
            Self::Ok => "ok  ",
            Self::Degraded => "warn",
            Self::Blocker => "FAIL",
        }
    }
}

/// One thing that was looked at.
#[derive(Clone, Debug)]
pub struct Check {
    pub name: &'static str,
    pub severity: Severity,
    /// What was found. Present tense, specific, no jargon the reader has to
    /// decode — this is the line a person reads first.
    pub detail: String,
    /// What to do about it. `None` when there is nothing to do.
    pub fix: Option<String>,
    /// A repair this process can perform itself, when one exists.
    ///
    /// Deliberately rare. Most of what `doctor` finds needs a package manager
    /// and root, and a tool that silently installs system packages because a
    /// check failed is doing something the user did not ask for and cannot
    /// easily undo. What qualifies is work that is *ours*: a directory in our
    /// own state, weights we fetch into our own model path.
    pub repair: Option<Repair>,
}

/// Something `doctor` can put right without asking anything of the system.
///
/// One shape, because there turned out to be exactly one honest case: run a
/// script that ships with the repository, to fetch weights into our own model
/// directory. An earlier version also carried a "create a private directory"
/// action, which had no caller — the stage already makes its own directories at
/// mode 0700 — and a speculative variant is worse than a narrow type.
#[derive(Clone, Debug, PartialEq)]
pub struct Repair {
    /// What will happen, in the words of someone deciding whether to allow it.
    pub summary: String,
    script: &'static str,
}

impl Repair {
    /// Perform it. Returns what changed, or why it could not.
    pub fn apply(&self) -> Result<String, String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(self.script);
        if !path.is_file() {
            return Err(format!("{} is not in this checkout", self.script));
        }
        let output = std::process::Command::new("sh")
            .arg(&path)
            .output()
            .map_err(|error| format!("run {}: {error}", self.script))?;
        if output.status.success() {
            Ok(format!("ran {}", self.script))
        } else {
            Err(format!(
                "{} failed: {}",
                self.script,
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .last()
                    .unwrap_or("no output")
            ))
        }
    }
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            severity: Severity::Ok,
            detail: detail.into(),
            fix: None,
            repair: None,
        }
    }

    fn bad(
        name: &'static str,
        severity: Severity,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            name,
            severity,
            detail: detail.into(),
            fix: Some(fix.into()),
            repair: None,
        }
    }

    fn repairable(mut self, summary: impl Into<String>, script: &'static str) -> Self {
        self.repair = Some(Repair {
            summary: summary.into(),
            script,
        });
        self
    }
}

/// The whole report.
#[derive(Clone, Debug)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// The worst thing found.
    pub fn severity(&self) -> Severity {
        if self.checks.iter().any(|c| c.severity == Severity::Blocker) {
            Severity::Blocker
        } else if self.checks.iter().any(|c| c.severity == Severity::Degraded) {
            Severity::Degraded
        } else {
            Severity::Ok
        }
    }

    /// Whether a graphical stage can start at all.
    pub fn can_start_a_stage(&self) -> bool {
        self.severity() != Severity::Blocker
    }

    /// The checks this process could put right itself.
    pub fn repairable(&self) -> impl Iterator<Item = (&Check, &Repair)> {
        self.checks
            .iter()
            .filter(|check| check.severity != Severity::Ok)
            .filter_map(|check| check.repair.as_ref().map(|repair| (check, repair)))
    }
}

/// Run every check that needs no running stage.
pub fn run() -> Report {
    Report {
        checks: vec![
            runtime_dir(),
            render_node(),
            graphics_libraries(),
            xwayland(),
            dbus(),
            accessibility_daemons(),
            chromium(),
            ocr_models(),
        ],
    }
}

/// `$XDG_RUNTIME_DIR`, and whether it is private.
///
/// Checked first because it is the only blocker with no software to install —
/// and because the mode check is a real refusal the stage makes, so a user who
/// hits it deserves to see it named rather than discovering it as a launch
/// failure.
fn runtime_dir() -> Check {
    use std::os::unix::fs::PermissionsExt;

    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from) else {
        return Check::bad(
            "runtime directory",
            Severity::Blocker,
            "$XDG_RUNTIME_DIR is not set",
            "Terminal, browser-attach and adapter surfaces still work without it. \
             For a graphical stage, run inside a normal desktop session, or set \
             $XDG_RUNTIME_DIR to a directory only you can enter (mode 0700).",
        );
    };

    match std::fs::metadata(&dir) {
        Ok(meta) if meta.permissions().mode() & 0o077 != 0 => Check::bad(
            "runtime directory",
            Severity::Blocker,
            format!(
                "{} is readable by other users (mode {:o})",
                dir.display(),
                meta.permissions().mode() & 0o777
            ),
            format!("chmod 700 {}", dir.display()),
        ),
        Ok(_) => Check::ok("runtime directory", format!("{} is private", dir.display())),
        Err(error) => Check::bad(
            "runtime directory",
            Severity::Blocker,
            format!("{} cannot be read: {error}", dir.display()),
            "Check that the directory exists and belongs to you.",
        ),
    }
}

fn render_node() -> Check {
    if Path::new("/dev/dri/renderD128").exists() {
        Check::ok("gpu render node", "/dev/dri/renderD128 is present")
    } else {
        Check::bad(
            "gpu render node",
            Severity::Blocker,
            "no /dev/dri/renderD128",
            "The compositor renders through it. On a headless server install \
             Mesa and load a DRM driver; in a container, pass /dev/dri through. \
             Terminal and browser-attach surfaces work without it.",
        )
    }
}

/// The libraries the compositor links against.
///
/// A missing one is a link error at stage start, which reads as a crash rather
/// than a missing package.
fn graphics_libraries() -> Check {
    let missing: Vec<&str> = [("libgbm.so.1", "mesa"), ("libEGL.so.1", "mesa")]
        .into_iter()
        .filter(|(library, _)| !library_present(library))
        .map(|(library, _)| library)
        .collect();

    if missing.is_empty() {
        Check::ok("graphics libraries", "libgbm and libEGL are present")
    } else {
        Check::bad(
            "graphics libraries",
            Severity::Blocker,
            format!("missing {}", missing.join(", ")),
            "Install Mesa (`mesa` on Arch, `libgbm1 libegl1` on Debian).",
        )
    }
}

fn xwayland() -> Check {
    if which("Xwayland").is_some() {
        Check::ok("xwayland", "X11-only applications can run")
    } else {
        Check::bad(
            "xwayland",
            Severity::Degraded,
            "Xwayland is not installed",
            "Everything Wayland-native still works; X11-only applications will \
             not start. Install `xorg-xwayland`.",
        )
    }
}

fn dbus() -> Check {
    match which("dbus-daemon") {
        Some(path) => Check::ok("session bus", format!("{path} is available")),
        None => Check::bad(
            "session bus",
            Severity::Blocker,
            "dbus-daemon is not installed",
            "The stage runs its own session bus so applications on it never \
             touch yours. Install `dbus`.",
        ),
    }
}

/// The accessibility stack — **the check this module exists for**.
///
/// Without these the tree is empty, which is indistinguishable from a broken
/// display unless something says so plainly. It is degraded rather than a
/// blocker because browsers and terminals do not need it.
fn accessibility_daemons() -> Check {
    let launcher = [
        "/usr/lib/at-spi-bus-launcher",
        "/usr/libexec/at-spi-bus-launcher",
    ]
    .into_iter()
    .find(|path| Path::new(path).exists());
    let registry = [
        "/usr/lib/at-spi2-registryd",
        "/usr/libexec/at-spi2-registryd",
    ]
    .into_iter()
    .find(|path| Path::new(path).exists());

    match (launcher, registry) {
        (Some(_), Some(_)) => Check::ok(
            "accessibility",
            "at-spi bus launcher and registry are installed",
        ),
        _ => Check::bad(
            "accessibility",
            Severity::Degraded,
            "at-spi2-core is not installed",
            "Desktop applications will show an EMPTY element tree, which looks \
             exactly like a broken display. Browsers and terminals are \
             unaffected. Install `at-spi2-core`.",
        ),
    }
}

fn chromium() -> Check {
    match [
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
    ]
    .into_iter()
    .find_map(which)
    {
        Some(path) => Check::ok("browser", format!("{path} is available")),
        None => Check::bad(
            "browser",
            Severity::Degraded,
            "no Chromium-family browser found",
            "Web surfaces are driven over the DevTools protocol, which needs \
             Chromium, Chrome, Brave or Edge. Install `chromium`.",
        ),
    }
}

/// The OCR weights that make the pixel rung actionable.
fn ocr_models() -> Check {
    let dir = std::env::var_os("ARTIST_OCR_MODELS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("models"));

    let missing: Vec<&str> = [
        "ppocrv5-mobile-det.onnx",
        "ppocrv5-mobile-rec.onnx",
        "ppocrv5_dict.txt",
    ]
    .into_iter()
    .filter(|name| !dir.join(name).is_file())
    .collect();

    if missing.is_empty() {
        Check::ok(
            "text recognition",
            format!("weights are in {}", dir.display()),
        )
    } else {
        Check::bad(
            "text recognition",
            Severity::Degraded,
            format!("missing {} in {}", missing.join(", "), dir.display()),
            "Applications with no accessibility tree — games, canvases — cannot \
             be clicked without these. Run `scripts/fetch-ocr-models.sh`.",
        )
        .repairable(
            format!("download PP-OCRv5 weights into {}", dir.display()),
            "scripts/fetch-ocr-models.sh",
        )
    }
}

fn which(program: &str) -> Option<String> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

/// Whether the dynamic linker can find a library.
///
/// Searched by path rather than by dlopen so the check costs nothing and cannot
/// itself crash on a broken driver.
fn library_present(name: &str) -> bool {
    [
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ]
    .iter()
    .any(|dir| Path::new(dir).join(name).exists())
}

/// Render for a person.
pub fn render(report: &Report) -> String {
    let mut out = String::from("computer use readiness\n\n");
    for check in &report.checks {
        out.push_str(&format!(
            "  {}  {:<20} {}\n",
            check.severity.mark(),
            check.name,
            check.detail
        ));
        if let Some(fix) = &check.fix {
            for line in textwrap(fix, 68) {
                out.push_str(&format!("        {line}\n"));
            }
        }
    }

    out.push('\n');
    out.push_str(match report.severity() {
        Severity::Ok => "Everything needed is present.\n",
        Severity::Degraded => "Usable. Some surfaces are unavailable — see the warnings above.\n",
        Severity::Blocker => {
            "A graphical stage cannot start here. Terminal, browser-attach and \
             adapter surfaces still work.\n"
        }
    });

    // Offered, never performed. Everything else `doctor` finds needs a package
    // manager and root; a tool that installed system packages because a check
    // failed would be doing something nobody asked for and cannot easily undo.
    let repairable: Vec<&Repair> = report.repairable().map(|(_, repair)| repair).collect();
    if !repairable.is_empty() {
        out.push_str(
            "\nSome of this can be put right from here — `artist computer doctor --fix`:\n",
        );
        for repair in repairable {
            out.push_str(&format!("  · {}\n", repair.summary));
        }
    }
    out
}

/// Apply every repair `doctor` offered, and say what happened to each.
///
/// Reports failures rather than stopping at the first: a machine missing two
/// things should learn about both in one run, and a repair that cannot proceed
/// is information, not a reason to abandon the others.
pub fn repair(report: &Report) -> String {
    let repairs: Vec<(&Check, &Repair)> = report.repairable().collect();
    if repairs.is_empty() {
        return "nothing here can be repaired automatically.\n".to_owned();
    }
    let mut out = String::new();
    for (check, repair) in repairs {
        match repair.apply() {
            Ok(done) => out.push_str(&format!("  ok    {:<20} {done}\n", check.name)),
            Err(error) => out.push_str(&format!("  FAIL  {:<20} {error}\n", check.name)),
        }
    }
    out.push_str("\nRe-run `artist computer doctor` to confirm.\n");
    out
}

/// Wrap at a width, breaking on spaces.
fn textwrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failing_check_says_what_to_do_about_it() {
        // The property that separates this from an error message. A report that
        // names a missing package and stops has moved the problem, not solved
        // it — which is exactly the experience this module replaces.
        for check in run().checks {
            if check.severity != Severity::Ok {
                let fix = check.fix.unwrap_or_default();
                assert!(
                    !fix.trim().is_empty(),
                    "{} failed without a recommended fix",
                    check.name
                );
            }
        }
    }

    #[test]
    fn a_missing_accessibility_stack_is_degraded_rather_than_fatal() {
        // Browsers and terminals do not need it, and calling it a blocker would
        // send people installing packages they do not need.
        let check = accessibility_daemons();
        assert_ne!(check.severity, Severity::Blocker);
        if check.severity == Severity::Degraded {
            // And it must name the symptom, because the symptom is the whole
            // reason this check exists: an empty tree looks like a broken
            // display.
            assert!(
                check.detail.contains("at-spi")
                    || check.fix.as_deref().unwrap_or("").contains("EMPTY"),
                "{check:?}"
            );
        }
    }

    #[test]
    fn severity_is_the_worst_thing_found() {
        let report = Report {
            checks: vec![
                Check::ok("a", "fine"),
                Check::bad("b", Severity::Degraded, "meh", "do this"),
            ],
        };
        assert_eq!(report.severity(), Severity::Degraded);
        assert!(report.can_start_a_stage());

        let report = Report {
            checks: vec![
                Check::ok("a", "fine"),
                Check::bad("b", Severity::Blocker, "no", "do that"),
            ],
        };
        assert_eq!(report.severity(), Severity::Blocker);
        assert!(!report.can_start_a_stage());
    }

    #[test]
    fn the_report_renders_every_check_and_a_conclusion() {
        let rendered = render(&run());
        for name in [
            "runtime directory",
            "gpu render node",
            "graphics libraries",
            "xwayland",
            "session bus",
            "accessibility",
            "browser",
            "text recognition",
        ] {
            assert!(rendered.contains(name), "{name} missing from:\n{rendered}");
        }
        assert!(
            rendered.contains("Everything needed")
                || rendered.contains("Usable")
                || rendered.contains("cannot start"),
            "no conclusion:\n{rendered}"
        );
    }

    /// Not an assertion — a way to read the report this machine produces.
    /// `cargo test -p artist-computer --lib doctor::tests::print -- --nocapture`
    #[test]
    fn print_the_report_for_a_human() {
        eprintln!("\n{}", render(&run()));
    }

    #[test]
    fn wrapping_does_not_lose_or_split_words() {
        let text = "The stage runs its own session bus so applications on it never touch yours.";
        let lines = textwrap(text, 20);
        assert!(
            lines
                .iter()
                .all(|line| line.len() <= 20 || !line.contains(' '))
        );
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn only_our_own_state_is_ever_repaired_automatically() {
        // The line this holds: `doctor` may fetch weights into our model
        // directory or create a directory in our own state, because those are
        // ours. It must never install a system package, because that needs root
        // and is not easily undone — a tool that did it because a check failed
        // would be acting well outside what was asked.
        let report = run();
        for (check, repair) in report.repairable() {
            let summary = repair.summary.to_lowercase();
            for forbidden in ["apt", "pacman", "dnf", "install ", "sudo"] {
                assert!(
                    !summary.contains(forbidden),
                    "{} offers to run a package manager: {:?}",
                    check.name,
                    repair.summary
                );
            }
        }
    }

    #[test]
    fn a_repair_is_offered_but_never_taken_by_rendering() {
        // `render` describes; `repair` acts. If rendering a report could change
        // the machine, every status display would be a side effect.
        let before = run();
        let _ = render(&before);
        let after = run();
        assert_eq!(
            before.checks.iter().map(|c| c.severity).collect::<Vec<_>>(),
            after.checks.iter().map(|c| c.severity).collect::<Vec<_>>(),
            "rendering a report must not change anything"
        );
    }

    #[test]
    fn a_report_with_nothing_to_repair_says_so_rather_than_pretending() {
        let clean = Report {
            checks: vec![Check::ok("everything", "fine")],
        };
        assert!(clean.repairable().next().is_none());
        assert!(
            repair(&clean).contains("nothing here can be repaired"),
            "{}",
            repair(&clean)
        );
        // And it is not advertised in the report either.
        assert!(!render(&clean).contains("--fix"), "{}", render(&clean));
    }

    #[test]
    fn a_healthy_check_is_never_repaired_even_if_it_carries_one() {
        // Repairs hang off the check, so a check that passes must not be
        // "fixed" — re-downloading weights that are already there on every run
        // would be a slow, silent surprise.
        let passing = Report {
            checks: vec![
                Check::ok("text recognition", "weights are present")
                    .repairable("download weights", "scripts/fetch-ocr-models.sh"),
            ],
        };
        assert!(
            passing.repairable().next().is_none(),
            "a passing check must not be repaired"
        );
    }
}
