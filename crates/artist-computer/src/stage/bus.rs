//! A private D-Bus session and accessibility registry for a stage.
//!
//! Small, and the highest value-per-line in the subsystem: it is what makes the
//! accessibility rung *usable* rather than merely present. On the user's own
//! bus every running application appears in the tree, so attributing a node to
//! the window the agent is driving is guesswork. A stage-private bus contains
//! only what the agent launched, so attribution is exact.
//!
//! Two failure modes are worth naming, because both look like a bug somewhere
//! else entirely:
//!
//! 1. **Connecting to the wrong bus.** The harness process's own environment
//!    still points at the user's session bus, so anything that reaches for the
//!    default connection silently gets it. Every connection here is made by
//!    explicit address, and there is a test that asserts the stage address is
//!    not the user's.
//! 2. **Toolkits not being told accessibility is on.** GTK and Qt only start
//!    their bridges when the a11y bus says it is enabled, and Chromium needs an
//!    explicit flag. Without this the tree is empty and it presents as a
//!    compositor fault rather than a missing environment variable.

use std::process::Stdio;
use std::time::Duration;

use crate::program::StepError;
use crate::stage::StageEnv;

/// How long to wait for a spawned daemon to announce itself.
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// A private session bus plus the accessibility services running on it.
///
/// Both daemons are killed on drop: a leaked `dbus-daemon` per session would
/// accumulate silently across a day's work.
pub struct StageBus {
    session_address: String,
    a11y_address: Option<String>,
    /// This stage's private directory.
    ///
    /// Held because the accessibility daemons need it in their *environment*,
    /// not just ours: `at-spi-bus-launcher` derives its socket path from
    /// `$XDG_RUNTIME_DIR`, so two stages that let it inherit the user's would
    /// write to the same path and the second would silently displace the first.
    runtime_dir: std::path::PathBuf,
    children: Vec<tokio::process::Child>,
}

impl Drop for StageBus {
    /// Kill the daemons without ever blocking the thread that drops us.
    ///
    /// `start_kill` signals and returns; `kill_on_drop` hands the corpse to
    /// tokio's reaper. The obvious version — `kill()` then `wait()` — blocks a
    /// runtime worker on a process that has just been signalled, which is a
    /// small stall in the good case and an unbounded one in the bad.
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.start_kill();
        }
    }
}

impl StageBus {
    /// Start a private session bus, then the accessibility stack on top of it.
    ///
    /// The a11y half is best-effort: a stage without it still drives every other
    /// rung, and reporting "no accessibility" is far better than refusing to
    /// start a display at all.
    pub async fn start(runtime_dir: &std::path::Path) -> Result<Self, StepError> {
        let mut bus = Self::start_session_bus(runtime_dir).await?;
        if let Err(error) = bus.start_accessibility().await {
            // Deliberately not fatal, and deliberately not silent.
            eprintln!("artist: stage accessibility unavailable: {error}");
        }
        Ok(bus)
    }

    /// Start the private session bus and read the address it prints.
    ///
    /// The read is `tokio`'s and is bounded. A plain `read_line` here blocks a
    /// runtime worker with no timeout, so a `dbus-daemon` that starts but never
    /// prints — a stale socket it will not overwrite, a broken install — hung
    /// the whole agent rather than reporting a failure it could recover from.
    async fn start_session_bus(runtime_dir: &std::path::Path) -> Result<Self, StepError> {
        let socket = runtime_dir.join("bus");
        let mut child = tokio::process::Command::new("dbus-daemon")
            .args([
                "--session",
                "--print-address",
                "--nofork",
                "--nopidfile",
                &format!("--address=unix:path={}", socket.display()),
            ])
            // The daemon's environment is inherited by every service it
            // *activates*, and that is the leak. `org.a11y.Bus` has a
            // `.service` file, so the first `GetAddress` on this bus can start
            // a second `at-spi-bus-launcher` through activation — before ours
            // has claimed the name — and an activated launcher carrying the
            // user's `XDG_RUNTIME_DIR` answers with the user's accessibility
            // bus at `/run/user/1000/at-spi/bus`. The stage then attaches to
            // the user's a11y bus and can see the user's own applications,
            // which is precisely the isolation this whole module exists to
            // provide. It is a race, so it presents as an occasional
            // "Server GUID mismatch" under load rather than as a constant
            // failure — the worst possible shape for a correctness bug.
            .env("XDG_RUNTIME_DIR", runtime_dir)
            //
            // Same reason, different channel: `at-spi-bus-launcher` will read
            // the `AT_SPI_BUS` property off the X root window when it has a
            // display, and the user's root window names the user's bus. The
            // stage's own X display is handed to launched applications, never
            // to these daemons.
            .env_remove("DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                StepError::Backend(format!("start dbus-daemon (is dbus installed?): {error}"))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| StepError::Backend("dbus-daemon gave no stdout".into()))?;

        let address = {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut line = String::new();
            match tokio::time::timeout(READY_TIMEOUT, BufReader::new(stdout).read_line(&mut line))
                .await
            {
                Ok(Ok(_)) => line.trim().to_owned(),
                Ok(Err(error)) => {
                    let _ = child.start_kill();
                    return Err(StepError::Backend(format!("read bus address: {error}")));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(StepError::Backend(format!(
                        "dbus-daemon did not print its address within {READY_TIMEOUT:?}"
                    )));
                }
            }
        };

        if address.is_empty() {
            let _ = child.start_kill();
            return Err(StepError::Backend(
                "dbus-daemon printed no address".to_owned(),
            ));
        }

        Ok(Self {
            session_address: address,
            a11y_address: None,
            runtime_dir: runtime_dir.to_owned(),
            children: vec![child],
        })
    }

    async fn start_accessibility(&mut self) -> Result<(), StepError> {
        let launcher = [
            "/usr/lib/at-spi-bus-launcher",
            "/usr/libexec/at-spi-bus-launcher",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or_else(|| StepError::Backend("at-spi-bus-launcher not found".into()))?;

        self.children.push(
            tokio::process::Command::new(launcher)
                .arg("--launch-immediately")
                .env("DBUS_SESSION_BUS_ADDRESS", &self.session_address)
                // Without this the launcher puts its socket at the *user's*
                // runtime path, which every stage shares. Two stages then race,
                // and the loser's stored address reaches the winner's daemon —
                // surfacing as "D-Bus handshake failed: Server GUID mismatch"
                // and silently dropping that stage a rung.
                .env("XDG_RUNTIME_DIR", &self.runtime_dir)
                // See `start_session_bus`: with a display, the launcher takes
                // the bus address off the X root window instead of creating one.
                .env_remove("DISPLAY")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| StepError::Backend(format!("spawn {launcher}: {error}")))?,
        );

        let address = self.await_a11y_address().await?;
        self.a11y_address = Some(address);

        let registryd = [
            "/usr/lib/at-spi2-registryd",
            "/usr/libexec/at-spi2-registryd",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or_else(|| StepError::Backend("at-spi2-registryd not found".into()))?;
        self.children.push(
            tokio::process::Command::new(registryd)
                .arg("--use-gnome-session=no")
                .env("DBUS_SESSION_BUS_ADDRESS", &self.session_address)
                .env("XDG_RUNTIME_DIR", &self.runtime_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| StepError::Backend(format!("spawn {registryd}: {error}")))?,
        );
        Ok(())
    }

    /// Poll `org.a11y.Bus.GetAddress` until the launcher has claimed the name.
    async fn await_a11y_address(&self) -> Result<String, StepError> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(address) = self.query_a11y_address().await {
                return Ok(address);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(StepError::Backend(
                    "timed out waiting for org.a11y.Bus".to_owned(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn query_a11y_address(&self) -> Result<String, StepError> {
        // `busctl` rather than a zbus dependency: this runs once per stage, and
        // shelling out keeps the bus module free of a client-library version
        // constraint that the accessibility surface will pick anyway.
        let output = tokio::process::Command::new("busctl")
            .args([
                "--user",
                "call",
                "org.a11y.Bus",
                "/org/a11y/bus",
                "org.a11y.Bus",
                "GetAddress",
            ])
            .env("DBUS_SESSION_BUS_ADDRESS", &self.session_address)
            .output()
            .await
            .map_err(|error| StepError::Backend(format!("busctl: {error}")))?;
        if !output.status.success() {
            return Err(StepError::Backend("org.a11y.Bus not up yet".to_owned()));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // `s "unix:path=..."` — take what is inside the quotes.
        text.split('"')
            .nth(1)
            .map(str::to_owned)
            .ok_or_else(|| StepError::Backend(format!("unparsed a11y address: {text}")))
    }

    pub fn session_address(&self) -> &str {
        &self.session_address
    }

    pub fn a11y_address(&self) -> Option<&str> {
        self.a11y_address.as_deref()
    }

    /// The environment an application must inherit to appear on this bus *and*
    /// expose an accessibility tree.
    ///
    /// The a11y flags are not optional decoration. GTK4 and Qt start their
    /// bridges only when told to, and Chromium needs
    /// `--force-renderer-accessibility` on its command line besides. Omit any of
    /// these and rung 2 reports an empty tree for a perfectly healthy app.
    pub fn apply_to(&self, env: &mut StageEnv) {
        env.set("DBUS_SESSION_BUS_ADDRESS", &self.session_address);
        if let Some(address) = &self.a11y_address {
            env.set("AT_SPI_BUS_ADDRESS", address);
        }
        env.set("GTK_A11Y", "atspi");
        env.set("QT_ACCESSIBILITY", "1");
        env.set("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1");
        env.set("ACCESSIBILITY_ENABLED", "1");
        env.set("NO_AT_BRIDGE", "0");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dbus_available() -> bool {
        std::process::Command::new("dbus-daemon")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    #[tokio::test]
    async fn the_stage_bus_is_not_the_users_bus() {
        if !dbus_available() {
            eprintln!("skipping: dbus-daemon is not installed");
            return;
        }
        let user_bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        let dir = tempfile::tempdir().unwrap();
        let bus = StageBus::start_session_bus(dir.path()).await.unwrap();

        assert!(bus.session_address().starts_with("unix:"));
        if let Some(user_bus) = &user_bus {
            assert_ne!(
                bus.session_address(),
                user_bus,
                "the stage must never share the user's session bus"
            );
        }

        // The isolation property is the product, so the test also asserts the
        // parent process was left exactly as it was found.
        assert_eq!(std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(), user_bus);
    }

    #[tokio::test]
    async fn the_accessibility_bus_is_the_stages_own_and_never_the_users() {
        // The bug this pins was invisible in every other test. `org.a11y.Bus`
        // has a D-Bus `.service` file, so asking a bus for it *starts* a
        // launcher if none has claimed the name — and an activated launcher
        // inherits the **daemon's** environment. A stage daemon carrying the
        // user's `XDG_RUNTIME_DIR` therefore answered with the user's own
        // accessibility bus, and the stage attached to it: the agent could see
        // the user's applications, and the tree it called private was not.
        //
        // It raced with our explicitly-spawned launcher, so it only lost under
        // load, and it surfaced as an intermittent "Server GUID mismatch"
        // rather than as anything resembling a leak.
        if !dbus_available() {
            eprintln!("skipping: dbus-daemon is not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // `start_session_bus`, deliberately not `start`: with no launcher of
        // ours running, the query below has no choice but to take the
        // activation path — which is the path that was wrong. Going through
        // `start` would race our own launcher against activation, and our
        // launcher usually wins, so the test would pass either way and pin
        // nothing. (Checked: it does.)
        let bus = match StageBus::start_session_bus(dir.path()).await {
            Ok(bus) => bus,
            Err(error) => {
                eprintln!("skipping: no bus here ({error})");
                return;
            }
        };
        let address = match bus.query_a11y_address().await {
            Ok(address) => address,
            Err(error) => {
                eprintln!("skipping: at-spi is not installed on this machine ({error})");
                return;
            }
        };

        let path = address
            .strip_prefix("unix:path=")
            .and_then(|rest| rest.split(',').next())
            .expect("an a11y address is a unix socket path");
        assert!(
            std::path::Path::new(path).starts_with(dir.path()),
            "the stage's accessibility bus must live inside its own runtime \
             directory ({}), but it is at {path} — which means the stage is \
             attached to somebody else's a11y bus",
            dir.path().display()
        );
    }

    #[tokio::test]
    async fn the_bus_dies_with_the_stage() {
        if !dbus_available() {
            eprintln!("skipping: dbus-daemon is not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let pid = {
            let bus = StageBus::start_session_bus(dir.path()).await.unwrap();
            bus.children[0].id().expect("the daemon is still running")
        };
        // Give the kill a moment to land before checking.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
        assert!(!alive, "dropping the stage must not leak a dbus-daemon");
    }

    #[test]
    fn the_environment_tells_every_toolkit_accessibility_is_on() {
        let bus = StageBus {
            session_address: "unix:path=/tmp/fake".into(),
            a11y_address: Some("unix:path=/tmp/fake-a11y".into()),
            runtime_dir: std::path::PathBuf::from("/tmp/fake-stage"),
            children: Vec::new(),
        };
        let mut env = StageEnv::default();
        bus.apply_to(&mut env);

        assert_eq!(
            env.get("DBUS_SESSION_BUS_ADDRESS"),
            Some("unix:path=/tmp/fake")
        );
        assert_eq!(
            env.get("AT_SPI_BUS_ADDRESS"),
            Some("unix:path=/tmp/fake-a11y")
        );
        // Each of these is load-bearing for a different toolkit; a missing one
        // shows up as an empty tree rather than as an error.
        assert_eq!(env.get("GTK_A11Y"), Some("atspi"));
        assert_eq!(env.get("QT_ACCESSIBILITY"), Some("1"));
        assert_eq!(env.get("QT_LINUX_ACCESSIBILITY_ALWAYS_ON"), Some("1"));
        assert_eq!(env.get("ACCESSIBILITY_ENABLED"), Some("1"));
    }
}
