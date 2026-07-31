//! Drive the stage compositor with a Wayland client of our own.
//!
//! This is the fixture that makes the compositor testable at all. Without it,
//! verifying that a window appears, that input is delivered, or — critically —
//! that frame callbacks fire would require a real toolkit application and a real
//! display. With it, all of that runs headless in an ordinary `cargo test`.
//!
//! The frame-callback assertion is the one that earns its keep. A compositor
//! that never sends `wl_surface.frame` lets a client draw exactly one frame and
//! then stall forever, and every symptom points at the client.

#![cfg(all(target_os = "linux", feature = "stage-wayland"))]

use std::os::unix::io::AsFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use artist_computer::stage::{Stage, StageId, wayland::StageWayland};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const WIDTH: i32 = 200;
const HEIGHT: i32 = 120;
/// A colour no clear-to-black could produce by accident.
const PAINT: (u8, u8, u8) = (0x20, 0xC0, 0x60);

#[derive(Default)]
struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    configured: bool,
    /// Set when the compositor answers our frame request. The whole point.
    frame_done: Arc<Mutex<bool>>,
    /// Keycodes the compositor delivered, in order.
    keys: Arc<Mutex<Vec<u32>>>,
    /// Pointer buttons the compositor delivered.
    buttons: Arc<Mutex<Vec<u32>>>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, 4, queue, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, queue, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, 5, queue, ())),
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, queue, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

/// The frame callback. Its arrival is the assertion.
impl Dispatch<wayland_client::protocol::wl_callback::WlCallback, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wayland_client::protocol::wl_callback::WlCallback,
        _: wayland_client::protocol::wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        *state.frame_done.lock().unwrap() = true;
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Client {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(queue, ());
            }
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(queue, ());
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            state.keys.lock().unwrap().push(key);
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            button,
            state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        {
            state.buttons.lock().unwrap().push(button);
        }
    }
}

delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore wl_buffer::WlBuffer);
delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);

struct Painted {
    _connection: Connection,
    queue: EventQueue<Client>,
    state: Client,
}

/// Connect, create a toplevel, paint it a solid colour, and request a frame.
///
/// Connects by explicit socket path rather than through `WAYLAND_DISPLAY`.
/// Those variables are process-global, and these tests run in parallel against
/// separate stages — routing through the environment would have each test
/// stealing the others' display.
fn paint_a_window(socket: &str, runtime_dir: &std::path::Path, title: &str) -> Option<Painted> {
    let stream = std::os::unix::net::UnixStream::connect(runtime_dir.join(socket)).ok()?;
    let connection = Connection::from_socket(stream).ok()?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    let display = connection.display();
    display.get_registry(&handle, ());

    let mut state = Client::default();
    queue.roundtrip(&mut state).ok()?;

    let compositor = state.compositor.clone()?;
    let shm = state.shm.clone()?;
    let wm_base = state.wm_base.clone()?;

    let surface = compositor.create_surface(&handle, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg_surface.get_toplevel(&handle, ());
    toplevel.set_title(title.to_owned());
    toplevel.set_app_id("artist.test.Fixture".to_owned());
    surface.commit();
    queue.roundtrip(&mut state).ok()?;

    // A solid ARGB buffer in a shm pool, painted a colour the capture test can
    // look for. Writing real pixels matters: a zeroed buffer is transparent
    // black, which is indistinguishable from the compositor never having
    // composited the surface at all.
    let stride = WIDTH * 4;
    let size = (stride * HEIGHT) as usize;
    let file = tempfile::tempfile().ok()?;
    file.set_len(size as u64).ok()?;
    {
        use std::io::Write as _;
        let mut writer = &file;
        // ARGB8888 is little-endian BGRA in memory.
        let pixel = [PAINT.2, PAINT.1, PAINT.0, 0xFF];
        let row: Vec<u8> = pixel.iter().copied().cycle().take(stride as usize).collect();
        for _ in 0..HEIGHT {
            writer.write_all(&row).ok()?;
        }
        writer.flush().ok()?;
    }
    let pool = shm.create_pool(file.as_fd(), size as i32, &handle, ());
    let buffer = pool.create_buffer(
        0,
        WIDTH,
        HEIGHT,
        stride,
        wl_shm::Format::Argb8888,
        &handle,
        (),
    );

    surface.frame(&handle, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, WIDTH, HEIGHT);
    surface.commit();
    queue.roundtrip(&mut state).ok()?;

    Some(Painted {
        _connection: connection,
        queue,
        state,
    })
}

fn stage_or_skip(dir: &std::path::Path) -> Option<StageWayland> {
    if !std::path::Path::new("/dev/dri/renderD128").exists() {
        eprintln!("skipping: no DRM render node on this machine");
        return None;
    }
    match StageWayland::start(StageId("test".into()), dir) {
        Ok(stage) => Some(stage),
        Err(error) => {
            eprintln!("skipping: stage unavailable here: {error}");
            None
        }
    }
}

#[tokio::test]
async fn a_client_connects_and_its_window_appears_on_the_stage() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();

    assert!(
        stage.windows().await.unwrap().is_empty(),
        "a fresh stage has no windows"
    );

    let painted = paint_a_window(&socket, dir.path(), "fixture window")
        .expect("the fixture client should connect to the stage");
    assert!(
        painted.state.configured,
        "the compositor must configure a new toplevel"
    );

    // The compositor dispatches on its own thread; give it a beat to see the
    // toplevel, but assert on the condition rather than on the sleep.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let windows = stage.windows().await.unwrap();
        if let Some(window) = windows.first() {
            assert_eq!(windows.len(), 1);
            // Identity comes from what the client committed and from its socket
            // credentials — never inferred from anything guessable.
            assert_eq!(window.title, "fixture window");
            assert_eq!(window.app_id, "artist.test.Fixture");
            assert_eq!(
                window.pid,
                Some(std::process::id() as i32),
                "the window must be attributed to the process that made it"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the client's window never appeared on the stage"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The regression test for the failure mode that eats days: a compositor that
/// renders but never sends frame callbacks, so every client freezes after one
/// frame while looking perfectly healthy.
#[tokio::test]
async fn the_compositor_sends_frame_callbacks() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();
    let mut painted =
        paint_a_window(&socket, dir.path(), "frame callback").expect("client should connect");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !*painted.state.frame_done.lock().unwrap() {
        painted
            .queue
            .roundtrip(&mut painted.state)
            .expect("roundtrip");
        assert!(
            Instant::now() < deadline,
            "no wl_surface.frame callback arrived: every client on this stage would freeze \
             after its first frame"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Input must actually reach the client, and only the focused one.
///
/// Delivering to our own seat is what makes the stage safe to use while the
/// user works — but "safe" is worthless if it also means "goes nowhere", so the
/// arrival is asserted from the client's side of the protocol.
#[tokio::test]
async fn keyboard_and_pointer_input_reach_the_focused_client() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();
    let mut painted = paint_a_window(&socket, dir.path(), "input").expect("client should connect");

    // Wait for the compositor to see the window.
    let deadline = Instant::now() + Duration::from_secs(5);
    let key = loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        if let Some(window) = stage.windows().await.unwrap().first() {
            break window.key;
        }
        assert!(Instant::now() < deadline, "window never appeared");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    stage.focus(key).await.expect("focus");
    stage.key(key, "a").await.expect("key");
    stage
        .pointer(
            key,
            artist_computer::Rect {
                x: 10,
                y: 10,
                width: 20,
                height: 20,
            },
            0x110, // BTN_LEFT
        )
        .await
        .expect("pointer");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        let keys = painted.state.keys.lock().unwrap().clone();
        let buttons = painted.state.buttons.lock().unwrap().clone();
        if !keys.is_empty() && !buttons.is_empty() {
            // evdev `a` is 30; the compositor converts to xkb (+8) internally
            // and the protocol carries the evdev code back to the client.
            assert_eq!(keys[0], 30, "the wrong key arrived: {keys:?}");
            assert_eq!(buttons[0], 0x110, "the wrong button arrived: {buttons:?}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "input never reached the client (keys={keys:?} buttons={buttons:?})"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Capture must return what the client actually drew.
///
/// Without this, `capture()` could return a black frame, a stale frame, or a
/// buffer of the wrong stride and every test would still pass — the model would
/// simply be shown a picture of nothing and asked to act on it.
#[tokio::test]
async fn capture_contains_what_the_client_painted() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();
    let mut painted = paint_a_window(&socket, dir.path(), "capture").expect("client should connect");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Keep the client's queue moving so its commits reach the compositor.
        let _ = painted.queue.roundtrip(&mut painted.state);
        let frame = stage.capture(None).await.expect("capture should succeed");

        assert_eq!(frame.rgba.len(), (frame.width * frame.height * 4) as usize);

        // Sample inside the painted region, away from its edges.
        if let Some((r, g, b, _)) = frame.pixel(20, 20)
            && (r, g, b) == PAINT
        {
            // And it must encode to a real PNG, since that is what reaches the
            // attachment store and the model.
            let png = frame.to_png().expect("frame should encode");
            assert_eq!(
                &png[..8],
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
                "capture must produce a valid PNG signature"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the client's colour never appeared in a capture"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The virtual screen size is configurable, because viewport size changes what
/// an application actually shows.
#[tokio::test]
async fn a_stage_can_be_given_a_different_screen_size() {
    use artist_computer::stage::{Stage, StageId, wayland::StageWayland};

    if !std::path::Path::new("/dev/dri/renderD128").exists() {
        eprintln!("skipping: no DRM render node");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Ok(stage) = StageWayland::start_sized(StageId("sized".into()), dir.path(), 800, 600) else {
        eprintln!("skipping: stage unavailable here");
        return;
    };

    let frame = stage.capture(None).await.expect("capture");
    assert_eq!((frame.width, frame.height), (800, 600));
    assert_eq!(frame.rgba.len(), 800 * 600 * 4);
}

#[tokio::test]
async fn absurd_screen_sizes_are_clamped_rather_than_attempted() {
    use artist_computer::stage::{Stage, StageId, wayland::StageWayland};

    if !std::path::Path::new("/dev/dri/renderD128").exists() {
        eprintln!("skipping: no DRM render node");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // A renderer asked for a 2-pixel or 100k-pixel buffer fails in ways that
    // surface far from the cause; clamping keeps the failure impossible.
    let Ok(stage) = StageWayland::start_sized(StageId("tiny".into()), dir.path(), 1, 1) else {
        eprintln!("skipping: stage unavailable here");
        return;
    };
    let frame = stage.capture(None).await.expect("capture");
    assert_eq!((frame.width, frame.height), (320, 240));
}

/// Damage must reflect what the client actually changed.
///
/// Whole-screen damage on every commit would make a blinking caret
/// indistinguishable from a dialog opening, and the noise filter behind the
/// `quiet` settle predicate classifies precisely by rectangle size — so coarse
/// damage silently degrades settling everywhere.
#[tokio::test]
async fn damage_reports_the_region_the_client_changed() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();
    let mut damage = stage.damage();

    let mut painted =
        paint_a_window(&socket, dir.path(), "damage").expect("client should connect");
    let _ = painted.queue.roundtrip(&mut painted.state);

    // The fixture damages exactly its own 200x120 area, not the 1920x1080 stage.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout(Duration::from_millis(250), damage.recv()).await {
            Ok(Ok(event)) => {
                assert!(
                    event.region.width <= WIDTH as u32 && event.region.height <= HEIGHT as u32,
                    "damage should be the client's region, not the whole screen: {:?}",
                    event.region
                );
                return;
            }
            _ => {
                let _ = painted.queue.roundtrip(&mut painted.state);
                assert!(Instant::now() < deadline, "no damage was ever reported");
            }
        }
    }
}

/// XWayland must come up on the stage's *own* display, never the user's.
///
/// The dangerous failure here is subtle: if XWayland is missing, an X11-only
/// application launched into the stage would inherit whatever `DISPLAY` the
/// harness process has — and put its window on the user's screen. So this
/// asserts both halves: that the stage has its own display number when XWayland
/// works, and that it is never the user's when it does not.
#[tokio::test]
async fn xwayland_runs_on_the_stages_own_display() {
    let user_display = std::env::var("DISPLAY").ok();
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };

    // XWayland reports asynchronously; give it a bounded chance to arrive.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut display = None;
    while Instant::now() < deadline {
        display = stage.x11_display().await;
        if display.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    match display {
        Some(number) => {
            let stage_display = format!(":{number}");
            assert_ne!(
                Some(stage_display.as_str()),
                user_display.as_deref(),
                "the stage must never hand out the user's X display"
            );
        }
        None => {
            // Not fatal — everything modern is Wayland-native — but the stage
            // must then advertise no DISPLAY at all rather than the user's.
            eprintln!("note: XWayland did not start; X11-only apps will not run on this stage");
            assert!(stage.env().get("DISPLAY").is_none());
        }
    }
    assert_eq!(std::env::var("DISPLAY").ok(), user_display);
}

#[tokio::test]
async fn the_stage_never_touches_the_users_session() {
    // The isolation property is the entire product, so it gets its own test.
    let user_wayland = std::env::var("WAYLAND_DISPLAY").ok();
    let user_bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();

    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };

    assert_ne!(
        Some(stage.socket_name()),
        user_wayland.as_deref(),
        "the stage must not serve the user's display"
    );
    // The environment handed to launched applications points at the stage,
    // while the harness process itself is left exactly as it was found.
    assert_eq!(
        stage.env().get("WAYLAND_DISPLAY"),
        Some(stage.socket_name())
    );
    drop(stage);

    assert_eq!(std::env::var("WAYLAND_DISPLAY").ok(), user_wayland);
    assert_eq!(std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(), user_bus);
}
