//! Is the process that made this claim still running?
//!
//! Every durable claim in this crate — a delegation seat, a running job — is
//! held by a process that can die without cleaning up. Liveness is what lets a
//! survivor reclaim the claim, and it is deliberately the *only* reclaim rule:
//! a process that is alive but wedged still holds its seat, because a wedged
//! subagent genuinely is occupying one. Treating slowness as death would hand
//! the same seat to two runs.

use serde::{Deserialize, Serialize};

/// A process identity that survives PID reuse.
///
/// A bare PID is not enough: the kernel reuses them, and a reused PID makes a
/// dead process look alive, which would pin a seat until the unrelated new
/// process exits. Pairing the PID with the process start time makes the
/// identity unique — the pair can only repeat if the same PID starts at the
/// same clock tick, which the kernel will not do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    /// Start time in kernel ticks since boot, where the platform reports it.
    /// `None` means the platform could not tell us, and liveness degrades to a
    /// bare PID check — see [`Owner::is_alive`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
}

impl Owner {
    /// This process.
    pub fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            started: start_time(pid),
        }
    }

    /// Whether the process that made a claim is still the process running under
    /// that PID.
    ///
    /// Answers `true` when the platform cannot tell us — an unknown liveness
    /// must not free a seat that may still be held, because over-counting
    /// concurrency is a stall and under-counting it is a double-spend.
    pub fn is_alive(self) -> bool {
        if !pid_exists(self.pid) {
            return false;
        }
        match (self.started, start_time(self.pid)) {
            // A different start time under the same PID means the PID was
            // reused and the original holder is gone.
            (Some(claimed), Some(actual)) => claimed == actual,
            _ => true,
        }
    }
}

#[cfg(unix)]
fn pid_exists(pid: u32) -> bool {
    // Signal 0 performs the permission and existence checks without delivering
    // anything. `EPERM` means the process exists but is not ours, which is
    // still alive for our purposes.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_exists(_pid: u32) -> bool {
    true
}

/// Field 22 of `/proc/{pid}/stat`, which is the process start time.
///
/// Parsed from the last `)` rather than by splitting the whole line, because
/// field 2 is the executable name and may itself contain spaces and
/// parentheses.
#[cfg(target_os = "linux")]
fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn start_time(_pid: u32) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive() {
        assert!(Owner::current().is_alive());
    }

    /// The reclaim rule is what frees a seat after a crash, so a claim from a
    /// process that no longer exists must read as dead.
    #[test]
    fn a_vanished_process_is_not_alive() {
        // A PID the kernel will not have allocated: the maximum is far below
        // this on every platform we run on.
        let ghost = Owner {
            pid: u32::MAX - 1,
            started: None,
        };
        assert!(!ghost.is_alive());
    }

    /// PID reuse is the failure this pairing exists to prevent: our own PID
    /// with someone else's start time is not us.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_reused_pid_is_not_the_original_holder() {
        let mut impostor = Owner::current();
        impostor.started = Some(impostor.started.expect("linux reports start time") + 1);
        assert!(!impostor.is_alive());
    }
}
