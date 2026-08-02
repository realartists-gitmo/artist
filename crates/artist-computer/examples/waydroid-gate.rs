//! The Android gate: does Waydroid render onto our stage, and is its damage tight?
//!
//! Everything in the Android plan is conditioned on one measurement. Waydroid's
//! `hwcomposer` reads `hwc_layer_1::surfaceDamage` — SurfaceFlinger's own
//! per-layer dirty region — and forwards each rect to `wl_surface_damage`, which
//! is exactly what [`artist_computer::stage::damage`] consumes. If those rects
//! arrive tight, settle predicates and the incremental OCR path carry over to
//! Android unchanged. If they arrive degenerate — one full-surface rect per
//! frame — then "has anything happened yet?" has no cheap answer on Android and
//! every settle predicate needs a different design.
//!
//! This example answers that, and proves the two things that have to be true
//! before it can even be asked: that Android's gralloc buffers import through
//! `zwp_linux_dmabuf`, and that its tasks arrive as toplevels we can see.
//!
//! ```text
//! cargo run -p artist-computer --example waydroid-gate -- \
//!     --package com.android.settings --seconds 20
//! ```
//!
//! It does not need your compositor: the stage is headless and the verdict is
//! text. Pass `--viewer` to also open a window onto it, which needs
//! `WAYLAND_DISPLAY` to name your own session.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use artist_computer::model::Rect;
use artist_computer::stage::wayland::StageWayland;
use artist_computer::stage::{AppCommand, Damage, Stage, StageId, WindowInfo};

/// How long to wait for the container to boot and paint something.
///
/// Generous because a first boot after `waydroid init` provisions the data
/// image, which is minutes rather than seconds on a cold machine. A gate that
/// gave up early would report "Android never mapped a window" for what is
/// actually a working system doing first-run setup.
const BOOT_TIMEOUT: Duration = Duration::from_secs(300);

/// A damage rect covering at least this fraction of its window is degenerate:
/// it carries no more information than "something changed somewhere".
const DEGENERATE: f64 = 0.9;

struct Args {
    package: Option<String>,
    seconds: u64,
    viewer: bool,
    mode: WindowMode,
    /// Deliver this many clicks during the measurement window.
    ///
    /// Without traffic the measurement is vacuous: an idle Android screen
    /// genuinely damages nothing, which tells us the settle predicate will be
    /// quiet but nothing about whether its rects are tight. Clicks are the only
    /// input verb the stage currently has, which is itself the finding that
    /// motivates the touch work.
    poke: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowMode {
    /// `persist.waydroid.multi_windows=true`: one xdg_toplevel per Android task.
    Multi,
    /// One fullscreen surface, with Android's own window manager inside it.
    Full,
    /// Whatever the container is already set to. The default, because changing
    /// it costs a session restart.
    AsFound,
}

fn parse_args() -> Args {
    let mut args = Args {
        package: None,
        seconds: 20,
        viewer: false,
        mode: WindowMode::AsFound,
        poke: 6,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--package" | "-p" => args.package = argv.next(),
            "--seconds" | "-s" => {
                args.seconds = argv.next().and_then(|v| v.parse().ok()).unwrap_or(20);
            }
            "--viewer" => args.viewer = true,
            "--poke" => {
                args.poke = argv.next().and_then(|v| v.parse().ok()).unwrap_or(6);
            }
            "--mode" => {
                args.mode = match argv.next().as_deref() {
                    Some("multi") => WindowMode::Multi,
                    Some("full") => WindowMode::Full,
                    _ => WindowMode::AsFound,
                }
            }
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    args
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    let dir = std::path::Path::new(&runtime_dir).join("artist-waydroid-gate");
    std::fs::create_dir_all(&dir)?;

    println!("== stage ==");
    let stage = Arc::new(StageWayland::start(StageId("waydroid-gate".into()), &dir)?);
    let wayland_display = stage.env().get("WAYLAND_DISPLAY").unwrap_or("<unset>");
    let stage_runtime = stage.env().get("XDG_RUNTIME_DIR").unwrap_or("<unset>");
    println!("  WAYLAND_DISPLAY = {wayland_display}");
    println!("  XDG_RUNTIME_DIR = {stage_runtime}");
    println!("  (waydroid bind-mounts exactly this socket into the container)");

    // Subscribed before anything is launched. Damage that arrives while we are
    // still setting up is damage we would otherwise attribute to the wrong
    // phase — and the very first frames are the interesting ones.
    let mut damage = stage.damage();

    println!();
    println!("== container ==");
    report_preflight();

    if args.mode != WindowMode::AsFound {
        // Deliberately before the session starts: the property is read when the
        // Android window manager comes up, so setting it afterwards means a
        // restart. Doing it here is the difference between one boot and two.
        set_window_mode(&stage, args.mode);
    }

    println!();
    println!("== session ==");
    println!("  waydroid session start");
    let session = stage
        .spawn(
            AppCommand::new("waydroid")
                .arg("session".to_owned())
                .arg("start".to_owned()),
        )
        .await;
    match &session {
        Ok(handle) => println!("  started, pid {}", handle.pid),
        Err(error) => {
            eprintln!("  could not start the session: {error}");
            eprintln!("  is waydroid installed and initialised? try: waydroid status");
            return Ok(());
        }
    }

    println!();
    println!("== waiting for a toplevel ==");
    let deadline = Instant::now() + BOOT_TIMEOUT;
    let mut windows = Vec::new();
    let mut announced = false;
    while Instant::now() < deadline {
        windows = stage.windows().await.unwrap_or_default();
        if windows.iter().any(|window| window.mapped) {
            break;
        }
        if !announced && Instant::now().elapsed() > Duration::from_secs(30) {
            println!("  still nothing after 30s — first boot provisions data, so this is normal");
            announced = true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if !windows.iter().any(|window| window.mapped) {
        println!();
        println!("  VERDICT: no Android toplevel ever mapped.");
        println!("  Nothing below can be measured. Check `waydroid log` for a hwcomposer");
        println!("  failure — the usual causes are a missing binder node, a dmabuf import");
        println!("  refusal, or a client that found no wl_output and never sized itself.");
        return Ok(());
    }

    println!("  Android windows on the stage:");
    for window in &windows {
        describe(window);
    }
    println!();
    println!("  NOTE: pid is the HWComposer's for every one of these — it does not");
    println!("  discriminate between Android apps. Package identity has to come from");
    println!("  the container, which is what task 5 exists to do.");

    if let Some(package) = &args.package {
        println!();
        println!("== launching {package} ==");
        match stage
            .spawn(
                AppCommand::new("waydroid")
                    .arg("app".to_owned())
                    .arg("launch".to_owned())
                    .arg(package.clone()),
            )
            .await
        {
            Ok(_) => println!("  launched"),
            Err(error) => eprintln!("  could not launch: {error}"),
        }
        // Give the activity a moment to arrive before we start counting, so the
        // measurement is of the app rather than of the launcher animating away.
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    let viewer = if args.viewer {
        match artist_computer::stage::viewer::Viewer::open(Arc::clone(&stage)).await {
            Ok(viewer) => {
                println!();
                println!("  a viewer is open on your screen — drive Android by hand if you like");
                Some(viewer)
            }
            Err(error) => {
                eprintln!("  could not open a viewer: {error}");
                None
            }
        }
    } else {
        None
    };

    println!();
    println!("== measuring damage for {}s ==", args.seconds);
    println!("  interact with the viewer, or let an idle screen speak for itself");

    // Drain whatever queued while the app was starting: those rects describe a
    // launch animation, not steady-state behaviour.
    while damage.try_recv().is_ok() {}

    let poking = poke(&stage, &windows, &args);

    let mut stats = Stats::default();
    let until = Instant::now() + Duration::from_secs(args.seconds);
    while Instant::now() < until {
        let remaining = until.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, damage.recv()).await {
            Ok(Ok(event)) => stats.record(&event, &windows),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(count))) => {
                stats.lagged += count;
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
            Err(_) => break,
        }
    }

    if let Some(handle) = poking {
        handle.abort();
    }

    println!();
    stats.report();

    if let Ok(frame) = stage.capture(None).await {
        let path = dir.join("waydroid-gate.png");
        match frame.to_png() {
            Ok(png) => {
                std::fs::write(&path, png)?;
                println!();
                println!("== capture ==");
                println!("  {}x{} written to {}", frame.width, frame.height, path.display());
                println!("  a non-black frame here is also the dmabuf import path proving itself");
            }
            Err(error) => eprintln!("  could not encode the capture: {error}"),
        }
    }

    drop(viewer);
    println!();
    println!("== teardown ==");
    let _ = std::process::Command::new("waydroid")
        .args(["session", "stop"])
        .status();
    let _ = stage.shutdown().await;
    Ok(())
}

/// Generate traffic to measure, by clicking down the middle of the screen.
///
/// Alternated with Escape, which Waydroid maps to Android's Back: without it the
/// clicks wander steadily deeper into whatever was launched and the later ones
/// land on nothing. Back also *is* a UI transition, so it damages honestly.
fn poke(
    stage: &Arc<StageWayland>,
    windows: &[WindowInfo],
    args: &Args,
) -> Option<tokio::task::JoinHandle<()>> {
    if args.poke == 0 {
        return None;
    }
    let window = windows.iter().find(|window| window.mapped)?;
    let key = window.key;
    let geometry = window.geometry;
    let count = args.poke;
    let gap = Duration::from_secs_f64(args.seconds as f64 / f64::from(count + 1));
    let stage = Arc::clone(stage);

    Some(tokio::spawn(async move {
        // Down the middle column at four heights, so a list screen gets its rows
        // hit rather than the same row four times.
        let fractions = [0.30_f64, 0.45, 0.60, 0.75];
        for index in 0..count {
            tokio::time::sleep(gap).await;
            let fraction = fractions[(index as usize) % fractions.len()];
            let point = Rect {
                x: geometry.x + (geometry.width / 2) as i32,
                y: geometry.y + (f64::from(geometry.height) * fraction) as i32,
                width: 0,
                height: 0,
            };
            if index % 2 == 0 {
                let _ = stage.pointer(key, point, 0x110).await;
            } else {
                let _ = stage.key(key, "Escape").await;
            }
        }
    }))
}

fn describe(window: &WindowInfo) {
    println!(
        "    key {:?} app_id {:?} title {:?} {}x{} at ({},{}) mapped={} pid={:?}",
        window.key.0,
        window.app_id,
        window.title,
        window.geometry.width,
        window.geometry.height,
        window.geometry.x,
        window.geometry.y,
        window.mapped,
        window.pid,
    );
}

/// The checks whose failure otherwise presents as "the compositor is broken".
fn report_preflight() {
    let modules = std::fs::read_to_string("/proc/modules").unwrap_or_default();
    let binder = modules.contains("binder_linux") || std::path::Path::new("/dev/binder").exists();
    println!("  binder            {}", yes_no(binder));

    let images = std::path::Path::new("/var/lib/waydroid/images/system.img").exists();
    println!("  system image      {}", yes_no(images));

    let status = std::process::Command::new("waydroid")
        .arg("status")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    for line in status.lines() {
        println!("  {}", line.trim());
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "NO" }
}

fn set_window_mode(stage: &StageWayland, mode: WindowMode) {
    let value = if mode == WindowMode::Multi {
        "true"
    } else {
        "false"
    };
    println!("  persist.waydroid.multi_windows := {value}");
    let mut command = std::process::Command::new("waydroid");
    command.args(["prop", "set", "persist.waydroid.multi_windows", value]);
    for (key, env_value) in stage.env().iter() {
        command.env(key, env_value);
    }
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(_) => println!("  (prop set failed — it needs a running session; measuring as-found)"),
        Err(error) => println!("  (could not run waydroid prop: {error})"),
    }
}

/// What the damage stream looked like.
#[derive(Default)]
struct Stats {
    events: u64,
    lagged: u64,
    /// Area fraction buckets, as a share of the damaged window's own area.
    buckets: BTreeMap<&'static str, u64>,
    degenerate: u64,
    unattributed: u64,
    total_fraction: f64,
    largest: f64,
    smallest: f64,
}

impl Stats {
    fn record(&mut self, event: &Damage, windows: &[WindowInfo]) {
        self.events += 1;
        if self.events == 1 {
            self.smallest = 1.0;
        }

        // Against the window's own area where we can attribute it, and against
        // the output otherwise. A rect that fills its window is degenerate even
        // if the window is small, which is the distinction that matters.
        let reference = event
            .window
            .and_then(|key| windows.iter().find(|window| window.key == key))
            .map(|window| area(window.geometry))
            .unwrap_or_else(|| {
                self.unattributed += 1;
                1920.0 * 1080.0
            });
        if reference <= 0.0 {
            return;
        }

        let fraction = (area(event.region) / reference).clamp(0.0, 1.0);
        self.total_fraction += fraction;
        self.largest = self.largest.max(fraction);
        self.smallest = self.smallest.min(fraction);
        if fraction >= DEGENERATE {
            self.degenerate += 1;
        }
        let bucket = match fraction {
            f if f < 0.001 => "<0.1%",
            f if f < 0.01 => "0.1-1%",
            f if f < 0.1 => "1-10%",
            f if f < 0.5 => "10-50%",
            f if f < DEGENERATE => "50-90%",
            _ => ">90%",
        };
        *self.buckets.entry(bucket).or_default() += 1;
    }

    fn report(&self) {
        println!("== damage ==");
        if self.events == 0 {
            println!("  no damage at all in the measurement window.");
            println!();
            println!("  VERDICT: INCONCLUSIVE. An idle Android screen genuinely produces");
            println!("  nothing, which is itself good news for settle — but re-run with");
            println!("  --viewer and interact, or with --package, to see real traffic.");
            return;
        }

        println!("  {} events ({} lagged, {} unattributed to a window)",
            self.events, self.lagged, self.unattributed);
        for (bucket, count) in &self.buckets {
            let share = 100.0 * *count as f64 / self.events as f64;
            println!("    {bucket:>8}  {count:>6}  {share:>5.1}%");
        }
        let mean = self.total_fraction / self.events as f64;
        println!("  mean {:.2}% of window, smallest {:.3}%, largest {:.1}%",
            100.0 * mean, 100.0 * self.smallest, 100.0 * self.largest);

        let degenerate_share = self.degenerate as f64 / self.events as f64;
        println!();
        if degenerate_share > 0.5 {
            println!("  VERDICT: DEGENERATE. {:.0}% of frames damage the whole surface.",
                100.0 * degenerate_share);
            println!("  SurfaceFlinger's rects are not surviving the trip, so damage carries");
            println!("  no location information on Android. Settle must fall back to frame");
            println!("  differencing, and incremental OCR loses its 15x — plan accordingly.");
        } else {
            println!("  VERDICT: TIGHT. {:.0}% of frames damage a real sub-region.",
                100.0 * (1.0 - degenerate_share));
            println!("  damage.rs needs no Android-specific work: settle predicates and");
            println!("  incremental OCR carry over as designed.");
        }
    }
}

fn area(rect: Rect) -> f64 {
    f64::from(rect.width) * f64::from(rect.height)
}
