//! Open a window onto a stage nothing is driving, and let a person use it.
//!
//! The point is the *idle* part. Every other exercise of the viewer has the
//! agent working while you watch; this has nobody working at all, so anything
//! that happens on screen is yours. It is the cleanest test of the shared-seat
//! decision: clicks and keystrokes go through exactly the path the agent uses,
//! and the stage cannot tell the difference.
//!
//! ```text
//! WAYLAND_DISPLAY=wayland-1 cargo run -p artist-computer --example watch -- [command...]
//! ```
//!
//! `WAYLAND_DISPLAY` must name **your** compositor: the viewer is a client of
//! your session, while everything it shows lives on a display of its own.

use std::sync::Arc;

use artist_computer::stage::viewer::Viewer;
use artist_computer::stage::wayland::StageWayland;
use artist_computer::stage::{AppCommand, Stage, StageId};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let program = args.next().unwrap_or_else(|| "zenity".to_owned());
    let rest: Vec<String> = args.collect();
    let rest = if rest.is_empty() && program == "zenity" {
        // Something with buttons, a text field and a title — enough to prove a
        // click and a keystroke both land.
        vec![
            "--entry".to_owned(),
            "--title=artist stage".to_owned(),
            "--text=Type here, then press a button. Nothing is driving this but you.".to_owned(),
            "--entry-text=hello".to_owned(),
        ]
    } else {
        rest
    };

    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| {
        "WAYLAND_DISPLAY is not set, so there is no screen to open a window on. \
         Run this from a graphical session."
    })?;

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    let dir = std::path::Path::new(&runtime_dir).join("artist-watch-demo");
    std::fs::create_dir_all(&dir)?;

    println!("starting a stage…");
    let stage = Arc::new(StageWayland::start(StageId("watch".into()), &dir)?);

    println!("launching {program}…");
    let mut command = AppCommand::new(&program);
    // Chromium defaults to whatever platform it guesses and fails outright on a
    // display it does not expect. The real launch path (`Host::launch`) sets
    // this along with a private profile and the debugging port; this example
    // talks to the stage directly, so it has to say so itself.
    if program.contains("chromium") || program.contains("chrome") {
        command = command
            .arg("--ozone-platform=wayland")
            .arg("--no-first-run")
            .arg("--no-default-browser-check");
    }
    for arg in &rest {
        command = command.arg(arg.clone());
    }
    if let Err(error) = stage.spawn(command).await {
        eprintln!("could not start {program}: {error}");
        eprintln!("pass a different command: cargo run --example watch -- <command> [args...]");
        return Ok(());
    }

    // Wait for it to actually paint, or the first frame the viewer shows is an
    // empty screen and it looks broken rather than merely early.
    for _ in 0..100 {
        if stage
            .windows()
            .await
            .map(|windows| windows.iter().any(|window| window.mapped))
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    println!("opening a viewer on {display}…");
    let viewer = Viewer::open(Arc::clone(&stage)).await?;
    println!();
    println!("  A window is now open on your screen showing a display that is not yours.");
    println!("  Click in it. Type in it. Nothing else is driving it.");
    println!();
    println!("  Ctrl-C here closes it.");
    println!();

    // Report what the person did, since counting it is the half that makes a
    // shared seat safe rather than merely possible.
    let mut reported = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let actions = viewer.human().count();
        if actions > reported {
            println!("  you have acted {actions} time(s) — the agent would be told this");
            reported = actions;
        }
    }
}
