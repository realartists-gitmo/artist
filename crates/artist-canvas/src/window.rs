//! Opening a canvas in a window artist owns.
//!
//! # Why this re-execs instead of opening a window in-process
//!
//! A webview needs a platform event loop, and on macOS that loop must own the
//! main thread. Artist's main thread is the TUI: a blocking crossterm poll and
//! ratatui's inline viewport. Those two cannot share a thread, and running the
//! window loop off-thread is allowed on Linux and Windows but not on macOS —
//! so it would work in development here and fail for half the users.
//!
//! Re-exec sidesteps the conflict entirely. Artist is one binary, so it can
//! launch itself with a hidden subcommand and let the child give the event loop
//! the main thread it wants. The window then dies with its process, the TUI
//! never yields its own thread, and the platform differences stay in one place.

use std::{
    io,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

/// The hidden subcommand the child runs. Not in `--help`: it is an
/// implementation detail of `canvas open`, not something to invoke by hand.
pub const WINDOW_SUBCOMMAND: &str = "__canvas-window";

/// Launch a window showing `url`.
///
/// Returns the child so a caller can kill it; dropping the handle leaves the
/// window open, which is what a user expects when the agent moves on.
pub fn open(url: &str, title: &str) -> io::Result<Child> {
    let executable: PathBuf = std::env::current_exe()?;
    Command::new(executable)
        .arg(WINDOW_SUBCOMMAND)
        .arg(url)
        .arg(title)
        // The child must not write to the terminal the TUI is drawing in.
        // A stray line from a GTK warning would corrupt the viewport.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Whether a windowing system is available at all.
///
/// Over SSH or on a headless box there is no display, and spawning would give
/// the user a silently dead child. Callers fall back to printing the URL.
pub fn available() -> bool {
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        return true;
    }
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The child is dispatched by matching this exact string before clap runs,
    /// so it must stay stable and stay unlikely to collide with a real verb.
    #[test]
    fn the_subcommand_is_stable_and_clearly_internal() {
        assert_eq!(WINDOW_SUBCOMMAND, "__canvas-window");
        assert!(WINDOW_SUBCOMMAND.starts_with("__"));
    }

    /// A headless session must fall back to printing a URL rather than
    /// spawning a child that dies without saying why.
    #[test]
    fn availability_follows_the_display_environment() {
        // SAFETY: single-threaded test, restoring both variables before it ends.
        let wayland = std::env::var_os("WAYLAND_DISPLAY");
        let x11 = std::env::var_os("DISPLAY");

        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
            std::env::remove_var("DISPLAY");
        }
        if cfg!(target_os = "linux") {
            assert!(!available(), "no display should mean no window");
            unsafe { std::env::set_var("DISPLAY", ":0") };
            assert!(available(), "X11 display should be enough");
        }

        unsafe {
            match wayland {
                Some(value) => std::env::set_var("WAYLAND_DISPLAY", value),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
            match x11 {
                Some(value) => std::env::set_var("DISPLAY", value),
                None => std::env::remove_var("DISPLAY"),
            }
        }
    }
}
