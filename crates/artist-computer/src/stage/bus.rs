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
    children: Vec<std::process::Child>,
}

impl Drop for StageBus {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
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
        let mut bus = Self::start_session_bus(runtime_dir)?;
        if let Err(error) = bus.start_accessibility().await {
            // Deliberately not fatal, and deliberately not silent.
            eprintln!("artist: stage accessibility unavailable: {error}");
        }
        Ok(bus)
    }

    fn start_session_bus(runtime_dir: &std::path::Path) -> Result<Self, StepError> {
        let socket = runtime_dir.join("bus");
        let mut child = std::process::Command::new("dbus-daemon")
            .args([
                "--session",
                "--print-address",
                "--nofork",
                "--nopidfile",
                &format!("--address=unix:path={}", socket.display()),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                StepError::Backend(format!(
                    "start dbus-daemon (is dbus installed?): {error}"
                ))
            })?;

        let address = {
            use std::io::{BufRead, BufReader};
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| StepError::Backend("dbus-daemon gave no stdout".into()))?;
            let mut line = String::new();
            BufReader::new(stdout)
                .read_line(&mut line)
                .map_err(|error| StepError::Backend(format!("read bus address: {error}")))?;
            line.trim().to_owned()
        };

        if address.is_empty() {
            let _ = child.kill();
            return Err(StepError::Backend(
                "dbus-daemon printed no address".to_owned(),
            ));
        }

        Ok(Self {
            session_address: address,
            a11y_address: None,
            children: vec![child],
        })
    }

    async fn start_accessibility(&mut self) -> Result<(), StepError> {
        let launcher = ["/usr/lib/at-spi-bus-launcher", "/usr/libexec/at-spi-bus-launcher"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
            .ok_or_else(|| StepError::Backend("at-spi-bus-launcher not found".into()))?;

        self.children.push(
            std::process::Command::new(launcher)
                .arg("--launch-immediately")
                .env("DBUS_SESSION_BUS_ADDRESS", &self.session_address)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| StepError::Backend(format!("spawn {launcher}: {error}")))?,
        );

        let address = self.await_a11y_address().await?;
        self.a11y_address = Some(address);

        let registryd = ["/usr/lib/at-spi2-registryd", "/usr/libexec/at-spi2-registryd"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
            .ok_or_else(|| StepError::Backend("at-spi2-registryd not found".into()))?;
        self.children.push(
            std::process::Command::new(registryd)
                .arg("--use-gnome-session=no")
                .env("DBUS_SESSION_BUS_ADDRESS", &self.session_address)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
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
        let bus = StageBus::start_session_bus(dir.path()).unwrap();

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
    async fn the_bus_dies_with_the_stage() {
        if !dbus_available() {
            eprintln!("skipping: dbus-daemon is not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let pid = {
            let bus = StageBus::start_session_bus(dir.path()).unwrap();
            bus.children[0].id()
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
            children: Vec::new(),
        };
        let mut env = StageEnv::default();
        bus.apply_to(&mut env);

        assert_eq!(
            env.get("DBUS_SESSION_BUS_ADDRESS"),
            Some("unix:path=/tmp/fake")
        );
        assert_eq!(env.get("AT_SPI_BUS_ADDRESS"), Some("unix:path=/tmp/fake-a11y"));
        // Each of these is load-bearing for a different toolkit; a missing one
        // shows up as an empty tree rather than as an error.
        assert_eq!(env.get("GTK_A11Y"), Some("atspi"));
        assert_eq!(env.get("QT_ACCESSIBILITY"), Some("1"));
        assert_eq!(env.get("QT_LINUX_ACCESSIBILITY_ALWAYS_ON"), Some("1"));
        assert_eq!(env.get("ACCESSIBILITY_ENABLED"), Some("1"));
    }
}
