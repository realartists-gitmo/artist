//! Talking to the container over adb.
//!
//! Waydroid gives every container an address on its own NAT (`waydroid0`,
//! 192.168.240.0/24) and Android's `adbd` listens there on 5555 from boot. That
//! is the whole transport: no image modification, no `lxc-attach`, and — the
//! reason it was chosen over `waydroid shell` — **no root**. `lxc-attach` needs
//! privileges an unattended agent should not be asking for, and a sudo prompt
//! in the middle of a task is not an ergonomics wrinkle but a hard stop.
//!
//! Every command is addressed to our device explicitly with `-s`. A bare
//! `adb shell` picks whatever single device it can find, which on a machine
//! with a phone plugged in is *the user's phone* — and the commands here
//! install packages, change secure settings and drive input.

use std::time::Duration;

use crate::program::StepError;

/// Where `adbd` listens inside the container.
const ADB_PORT: u16 = 5555;

/// A long-running shell command is a bug, not a slow success.
///
/// Everything here is a property read, an intent, or a dump. The one legitimate
/// exception is an install, which gets its own longer bound.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// A connection to the container's adb daemon.
#[derive(Clone, Debug)]
pub struct Adb {
    /// `ip:port`, used as the adb serial so every command names its target.
    serial: String,
}

impl Adb {
    /// Connect to a container at this address.
    ///
    /// Retried, because `adbd` accepts connections a little after the point
    /// where Android reports `sys.boot_completed`, and a single attempt turns
    /// that race into "Android is unreachable".
    pub async fn connect(ip: &str) -> Result<Self, StepError> {
        let serial = format!("{ip}:{ADB_PORT}");
        ensure_adb_exists().await?;

        let mut last = String::new();
        for attempt in 0..10 {
            let output = run(&["connect", &serial], COMMAND_TIMEOUT).await?;
            // adb reports failure on stdout with a zero exit status, so the
            // status alone says nothing.
            if output.contains("connected to") {
                return Ok(Self { serial });
            }
            last = output.trim().to_owned();
            tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
        }
        Err(StepError::Backend(format!(
            "could not reach the Android container over adb at {serial}: {last}"
        )))
    }

    /// Run a shell command inside Android and return its stdout.
    ///
    /// Arguments are quoted individually. `adb shell` joins its arguments with
    /// spaces and hands the result to the device's shell, so an unquoted
    /// argument with a space in it — a package label, a URL with a query, a
    /// settings value — silently becomes two arguments.
    pub async fn shell(&self, args: &[&str]) -> Result<String, StepError> {
        let line = args
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let argv = vec!["-s", &self.serial, "shell", &line];
        run(&argv, COMMAND_TIMEOUT).await
    }

    /// A single Android property.
    pub async fn getprop(&self, key: &str) -> Result<String, StepError> {
        Ok(self.shell(&["getprop", key]).await?.trim().to_owned())
    }

    /// Install an APK, replacing any earlier copy.
    ///
    /// `-r` because the agent's own service is reinstalled on every version
    /// change, and `-g` because a service that has to ask a human for runtime
    /// permissions is one an unattended agent can never finish installing.
    pub async fn install(&self, apk: &std::path::Path) -> Result<(), StepError> {
        let path = apk.to_string_lossy().into_owned();
        let output = run(
            &["-s", &self.serial, "install", "-r", "-g", &path],
            INSTALL_TIMEOUT,
        )
        .await?;
        if output.contains("Success") {
            return Ok(());
        }
        Err(StepError::Backend(format!(
            "installing {} failed: {}",
            apk.display(),
            output.trim()
        )))
    }

    /// Whether a package is installed.
    pub async fn has_package(&self, package: &str) -> Result<bool, StepError> {
        let list = self
            .shell(&["pm", "list", "packages", package])
            .await?;
        Ok(list
            .lines()
            .any(|line| line.trim() == format!("package:{package}")))
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }
}

/// Quote one argument for the device's shell.
///
/// Single quotes with the standard `'\''` escape: everything inside is literal,
/// which is what we want for text that came from a UI rather than from us.
fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:=@,+".contains(&byte))
    {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

async fn ensure_adb_exists() -> Result<(), StepError> {
    match tokio::process::Command::new("adb")
        .arg("version")
        .output()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(StepError::Backend(
            "adb is not installed, and it is how artist drives Android \
             (intents, settings, dumpsys, and installing the accessibility service). \
             Install the platform tools — on Arch that is `pacman -S android-tools`."
                .into(),
        )),
        Err(error) => Err(StepError::Backend(format!("could not run adb: {error}"))),
    }
}

async fn run(args: &[&str], timeout: Duration) -> Result<String, StepError> {
    let future = tokio::process::Command::new("adb").args(args).output();
    let output = match tokio::time::timeout(timeout, future).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(StepError::Backend(format!(
                "could not run `adb {}`: {error}",
                args.join(" ")
            )));
        }
        Err(_) => {
            return Err(StepError::Backend(format!(
                "`adb {}` did not finish within {}s",
                args.join(" "),
                timeout.as_secs()
            )));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StepError::Backend(format!(
            "`adb {}` failed: {}",
            args.join(" "),
            if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            }
        )));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_arguments_are_left_alone() {
        assert_eq!(quote("com.android.settings"), "com.android.settings");
        assert_eq!(quote("-n"), "-n");
        assert_eq!(quote("com.example/.MainActivity"), "com.example/.MainActivity");
    }

    #[test]
    fn a_url_with_a_query_is_quoted() {
        // `?` and `&` are glob and job-control characters to the device's
        // shell, so a deep link passed bare either expands against the
        // container's filesystem or backgrounds half of itself. Deep links are
        // rung 0's whole vocabulary on Android, so this is the common case
        // rather than an exotic one.
        assert_eq!(
            quote("https://example.com/a?b=c&d=e"),
            "'https://example.com/a?b=c&d=e'"
        );
    }

    #[test]
    fn arguments_with_spaces_are_quoted() {
        // The failure this prevents: `settings put system name My Phone` sets
        // `name` to `My` and passes `Phone` as a fourth argument, which the
        // device's shell accepts silently.
        assert_eq!(quote("My Phone"), "'My Phone'");
    }

    #[test]
    fn embedded_quotes_survive() {
        assert_eq!(quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn an_empty_argument_stays_an_argument() {
        // Not the same as no argument at all: `am start -e key ''` is a
        // deliberate empty extra.
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn shell_metacharacters_are_neutralized() {
        assert_eq!(quote("a; rm -rf /"), "'a; rm -rf /'");
        assert_eq!(quote("$(id)"), "'$(id)'");
    }
}
