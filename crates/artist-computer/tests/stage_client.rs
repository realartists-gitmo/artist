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
// `Buffer` and `Texture` carry the dimension and plane accessors on `Dmabuf`.
use smithay::backend::allocator::Buffer as _;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const WIDTH: i32 = 200;
const HEIGHT: i32 = 120;
/// A colour no clear-to-black could produce by accident.
const PAINT: (u8, u8, u8) = (0x20, 0xC0, 0x60);
/// A second, equally unmistakable colour, for telling two windows apart.
const OTHER_PAINT: (u8, u8, u8) = (0xE0, 0x40, 0xA0);

/// A blocky 5x7 font, thick enough for a detector trained on screenshots.
const GLYPHS: &[(char, [u8; 7])] = &[
    (
        'S',
        [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
    ),
    (
        'E',
        [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
    ),
    (
        'N',
        [
            0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001,
        ],
    ),
    (
        'D',
        [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
    ),
    (
        'O',
        [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        'K',
        [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
    ),
];

/// Where the word is drawn, so a test can check the click landed on it.
const WORD_ORIGIN: (usize, usize) = (20, 30);
const WORD_SCALE: usize = 6;

/// Draw black glyphs into a white BGRA buffer.
fn draw_word(pixels: &mut [u8], width: usize, word: &str) {
    let mut cursor = WORD_ORIGIN.0;
    for character in word.chars() {
        if let Some((_, rows)) = GLYPHS.iter().find(|(glyph, _)| *glyph == character) {
            for (row, bits) in rows.iter().enumerate() {
                for column in 0..5usize {
                    if bits & (1 << (4 - column)) == 0 {
                        continue;
                    }
                    for dy in 0..WORD_SCALE {
                        for dx in 0..WORD_SCALE {
                            let x = cursor + column * WORD_SCALE + dx;
                            let y = WORD_ORIGIN.1 + row * WORD_SCALE + dy;
                            let offset = (y * width + x) * 4;
                            if let Some(pixel) = pixels.get_mut(offset..offset + 4) {
                                pixel.copy_from_slice(&[0, 0, 0, 0xFF]);
                            }
                        }
                    }
                }
            }
        }
        cursor += 6 * WORD_SCALE;
    }
}

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
    /// Where the pointer was when it arrived, in surface coordinates.
    ///
    /// Recorded because "a button reached the client" is a weaker claim than
    /// the one rung 3 actually makes: that a click aimed at text we *read off
    /// the screen* lands on that text.
    points: Arc<Mutex<Vec<(f64, f64)>>>,

    /// Every global the registry advertised, as a real client sees it:
    /// `(interface, version)`. A toolkit decides what it is capable of from
    /// exactly this list, so it is worth asserting on directly.
    globals: Arc<Mutex<Vec<(String, u32)>>>,
    output: Option<wl_output::WlOutput>,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    /// The mode the output reported, in pixels, once its `done` arrived.
    output_mode: Arc<Mutex<Option<(i32, i32)>>>,
    /// Set when the compositor tells the surface which output it is on.
    /// Several toolkits will not draw until they have been told.
    entered_output: Arc<Mutex<bool>>,
    /// The decoration mode the compositor configured — which the stage decides,
    /// not the client.
    decoration_mode: Arc<Mutex<Option<zxdg_toplevel_decoration_v1::Mode>>>,
    /// `main_device` from dmabuf feedback, as a `dev_t`.
    dmabuf_device: Arc<Mutex<Option<u64>>>,
    /// How many format tranches the feedback carried.
    dmabuf_tranches: Arc<Mutex<usize>>,
    dmabuf_feedback_done: Arc<Mutex<bool>>,
    presentation: Option<wp_presentation::WpPresentation>,
    /// The clock the compositor says its presentation timestamps are on.
    presentation_clock: Arc<Mutex<Option<u32>>>,
    /// Whether the frame we committed was ever reported as presented. A
    /// compositor that advertises `wp_presentation` and never answers leaves a
    /// client that paces off it waiting.
    presented: Arc<Mutex<bool>>,
    /// The dmabuf-backed `wl_buffer`, once the compositor has accepted it.
    dmabuf_buffer: Arc<Mutex<Option<wl_buffer::WlBuffer>>>,
    /// Set when the compositor *refuses* a dmabuf, which is the failure this
    /// test exists to catch: the global is advertised, the import fails, and
    /// the only symptom a real client shows is a window that stays blank.
    dmabuf_rejected: Arc<Mutex<bool>>,
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
            name,
            interface,
            version,
        } = event
        {
            state
                .globals
                .lock()
                .unwrap()
                .push((interface.clone(), version));
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, 4, queue, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, queue, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, 5, queue, ())),
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, queue, ())),
                // Bound at the version the compositor offers, capped at what
                // this fixture knows how to speak. Binding above the advertised
                // version is a protocol error, so the `min` is not defensive
                // padding — it is the rule.
                "wl_output" => {
                    state.output = Some(registry.bind(name, version.min(4), queue, ()));
                }
                "zwp_linux_dmabuf_v1" => {
                    state.dmabuf = Some(registry.bind(name, version.min(4), queue, ()));
                }
                "zxdg_decoration_manager_v1" => {
                    state.decoration_manager = Some(registry.bind(name, 1, queue, ()));
                }
                "wp_presentation" => {
                    state.presentation = Some(registry.bind(name, version.min(1), queue, ()));
                }
                _ => {}
            }
        }
    }
}

/// `wl_surface.enter` is the event the output tests turn on.
impl Dispatch<wl_surface::WlSurface, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::Enter { .. } = event {
            *state.entered_output.lock().unwrap() = true;
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Mode { width, height, .. } = event {
            *state.output_mode.lock().unwrap() = Some((width, height));
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: wayland_client::WEnum::Value(mode),
        } = event
        {
            *state.decoration_mode.lock().unwrap() = Some(mode);
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => {
                // A `dev_t` on the wire, in native byte order.
                if let Ok(bytes) = <[u8; 8]>::try_from(device.as_slice()) {
                    *state.dmabuf_device.lock().unwrap() = Some(u64::from_ne_bytes(bytes));
                }
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheDone => {
                *state.dmabuf_tranches.lock().unwrap() += 1;
            }
            zwp_linux_dmabuf_feedback_v1::Event::Done => {
                *state.dmabuf_feedback_done.lock().unwrap() = true;
            }
            _ => {}
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
        if let wl_pointer::Event::Motion {
            surface_x,
            surface_y,
            ..
        } = &event
        {
            state.points.lock().unwrap().push((*surface_x, *surface_y));
        }
        if let wl_pointer::Event::Enter {
            surface_x,
            surface_y,
            ..
        } = &event
        {
            state.points.lock().unwrap().push((*surface_x, *surface_y));
        }
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
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore wl_buffer::WlBuffer);
delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);
delegate_noop!(Client: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
delegate_noop!(Client: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);

impl Dispatch<wp_presentation::WpPresentation, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            *state.presentation_clock.lock().unwrap() = Some(clk_id);
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation_feedback::Event::Presented { .. } = event {
            *state.presented.lock().unwrap() = true;
        }
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                *state.dmabuf_buffer.lock().unwrap() = Some(buffer);
            }
            zwp_linux_buffer_params_v1::Event::Failed => {
                *state.dmabuf_rejected.lock().unwrap() = true;
            }
            _ => {}
        }
    }

    // `created` carries a brand new `wl_buffer`, and wayland-client will not
    // invent the child object for us — without this the event panics the queue
    // rather than arriving.
    wayland_client::event_created_child!(Client, zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (wl_buffer::WlBuffer, ()),
    ]);
}

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
    paint_window(socket, runtime_dir, title, None, PAINT)
}

/// The same, in a colour of your choosing.
fn paint_colour_window(
    socket: &str,
    runtime_dir: &std::path::Path,
    title: &str,
    colour: (u8, u8, u8),
) -> Option<Painted> {
    paint_window(socket, runtime_dir, title, None, colour)
}

/// The same, but with a word drawn on it in black on white.
///
/// Rung 3 has to read a *real captured frame*, not a synthesized one — the whole
/// path from client buffer through the compositor's own capture to the detector
/// is what has never been exercised.
fn paint_text_window(
    socket: &str,
    runtime_dir: &std::path::Path,
    title: &str,
    word: &str,
) -> Option<Painted> {
    paint_window(socket, runtime_dir, title, Some(word), PAINT)
}

fn paint_window(
    socket: &str,
    runtime_dir: &std::path::Path,
    title: &str,
    word: Option<&str>,
    colour: (u8, u8, u8),
) -> Option<Painted> {
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

    // Ask for client-side decorations — the thing the stage must refuse.
    // Doing it here rather than in one dedicated test means *every* stage test
    // now runs against a client that asked for the wrong mode, so an override
    // that broke ordinary window handling would show up as a broad failure
    // rather than a single red test.
    if let Some(manager) = state.decoration_manager.clone() {
        let decoration = manager.get_toplevel_decoration(&toplevel, &handle, ());
        decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ClientSide);
    }

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
        match word {
            None => {
                let pixel = [colour.2, colour.1, colour.0, 0xFF];
                let row: Vec<u8> = pixel
                    .iter()
                    .copied()
                    .cycle()
                    .take(stride as usize)
                    .collect();
                for _ in 0..HEIGHT {
                    writer.write_all(&row).ok()?;
                }
            }
            Some(word) => {
                let mut pixels = vec![0xFFu8; size];
                draw_word(&mut pixels, WIDTH as usize, word);
                writer.write_all(&pixels).ok()?;
            }
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

    // Asked for *before* the commit it describes — feedback is per-frame, and a
    // request made afterwards refers to the next one. Every fixture window asks,
    // so a compositor that advertised the global and answered nobody would show
    // up here rather than only in the one test that looks for it.
    if let Some(presentation) = state.presentation.clone() {
        presentation.feedback(&surface, &handle, ());
    }

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

/// Why a GPU-buffer test could not run on this machine.
///
/// Kept distinct from a failure on purpose. "This driver will not hand out a
/// linear ARGB buffer" and "the compositor refused a buffer it advertised
/// support for" are different findings, and collapsing them into one red test
/// would hide the second behind the first.
enum NoDmabuf {
    Unavailable(String),
}

/// The same window, painted into a **GPU buffer** and handed over as a dmabuf.
///
/// This is the path that matters for real applications: Chromium, anything on
/// Vulkan or GL, and Waydroid's Android surfaces all allocate on the GPU and
/// pass a file descriptor. Shared memory is the fallback they use when the
/// compositor gives them no choice — so a stage that only ever exercised shm
/// would look completely healthy while being unusable by every one of them.
fn paint_dmabuf_window(
    socket: &str,
    runtime_dir: &std::path::Path,
    title: &str,
) -> Result<Painted, NoDmabuf> {
    use gbm::{BufferObjectFlags, Format, Modifier};

    let node = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/dri/renderD128")
        .map_err(|error| NoDmabuf::Unavailable(format!("open render node: {error}")))?;
    let device = gbm::Device::new(node)
        .map_err(|error| NoDmabuf::Unavailable(format!("gbm device: {error}")))?;

    // Linear explicitly, so the buffer can be mapped and filled with a colour
    // the capture assertion can look for. A tiled buffer would import fine but
    // leave nothing to check the pixels against.
    //
    // Three spellings of the same request, because which one a driver accepts
    // is not portable. Mesa's iris rejects `WRITE` and `LINEAR` as *flags*
    // outright (EINVAL) while happily giving out a linear buffer when the
    // modifier is named explicitly and the only flag is `RENDERING`. Trying one
    // spelling would make this test skip on that driver — which is to say, on
    // most Intel machines — and a test that skips everywhere is worse than no
    // test, because it reports green.
    let mut bo = device
        .create_buffer_object_with_modifiers2::<()>(
            WIDTH as u32,
            HEIGHT as u32,
            Format::Argb8888,
            [Modifier::Linear].into_iter(),
            BufferObjectFlags::RENDERING,
        )
        .or_else(|_| {
            device.create_buffer_object_with_modifiers::<()>(
                WIDTH as u32,
                HEIGHT as u32,
                Format::Argb8888,
                [Modifier::Linear].into_iter(),
            )
        })
        .or_else(|_| {
            device.create_buffer_object::<()>(
                WIDTH as u32,
                HEIGHT as u32,
                Format::Argb8888,
                BufferObjectFlags::LINEAR | BufferObjectFlags::RENDERING,
            )
        })
        .map_err(|error| NoDmabuf::Unavailable(format!("allocate a linear buffer: {error}")))?;

    let map = bo.map_mut(0, 0, WIDTH as u32, HEIGHT as u32, |mapped| {
        let stride = mapped.stride() as usize;
        let pixel = [PAINT.2, PAINT.1, PAINT.0, 0xFF];
        let buffer = mapped.buffer_mut();
        for row in 0..HEIGHT as usize {
            for column in 0..WIDTH as usize {
                let offset = row * stride + column * 4;
                if let Some(slot) = buffer.get_mut(offset..offset + 4) {
                    slot.copy_from_slice(&pixel);
                }
            }
        }
    });
    if let Err(error) = map {
        return Err(NoDmabuf::Unavailable(format!("map the buffer: {error}")));
    }

    let fd = bo
        .fd()
        .map_err(|error| NoDmabuf::Unavailable(format!("export the buffer: {error}")))?;
    let stride = bo.stride();
    let offset = bo.offset(0);
    let modifier: u64 = bo.modifier().into();

    let stream = std::os::unix::net::UnixStream::connect(runtime_dir.join(socket))
        .map_err(|error| NoDmabuf::Unavailable(format!("connect: {error}")))?;
    let connection = Connection::from_socket(stream)
        .map_err(|error| NoDmabuf::Unavailable(format!("wayland connection: {error}")))?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());

    let mut state = Client::default();
    queue.roundtrip(&mut state).unwrap();

    let compositor = state.compositor.clone().unwrap();
    let wm_base = state.wm_base.clone().unwrap();
    let dmabuf = state
        .dmabuf
        .clone()
        .expect("the stage advertises no dmabuf global");

    let surface = compositor.create_surface(&handle, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg_surface.get_toplevel(&handle, ());
    toplevel.set_title(title.to_owned());
    toplevel.set_app_id("artist.test.Dmabuf".to_owned());
    surface.commit();
    queue.roundtrip(&mut state).unwrap();

    // `create`, not `create_immed`: the asynchronous form gets an explicit
    // `failed` back if the compositor cannot import, where the immediate form
    // answers a refusal by killing the connection — which would surface as a
    // dead socket rather than as the refusal it is.
    let params = dmabuf.create_params(&handle, ());
    params.add(
        fd.as_fd(),
        0,
        offset,
        stride,
        (modifier >> 32) as u32,
        (modifier & 0xFFFF_FFFF) as u32,
    );
    params.create(
        WIDTH,
        HEIGHT,
        Format::Argb8888 as u32,
        zwp_linux_buffer_params_v1::Flags::empty(),
    );
    for _ in 0..4 {
        queue.roundtrip(&mut state).unwrap();
        if state.dmabuf_buffer.lock().unwrap().is_some() || *state.dmabuf_rejected.lock().unwrap() {
            break;
        }
    }

    assert!(
        !*state.dmabuf_rejected.lock().unwrap(),
        "the stage advertised zwp_linux_dmabuf and then refused a linear ARGB8888 \
         buffer allocated on its own render node — a real GPU client would show a \
         blank window and nothing would point back here"
    );
    let buffer = state
        .dmabuf_buffer
        .lock()
        .unwrap()
        .clone()
        .expect("the stage neither accepted nor refused the dmabuf");

    surface.frame(&handle, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, WIDTH, HEIGHT);
    surface.commit();
    queue.roundtrip(&mut state).unwrap();

    Ok(Painted {
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
            artist_computer::stage::Pointing::at(artist_computer::Rect {
                x: 10,
                y: 10,
                width: 20,
                height: 20,
            }),
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
    let mut painted =
        paint_a_window(&socket, dir.path(), "capture").expect("client should connect");

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

    let mut painted = paint_a_window(&socket, dir.path(), "damage").expect("client should connect");
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

/// `capture(Some(window))` must return that window, not the whole screen.
///
/// The window key was being destructured into `_window` and thrown away, so
/// every caller asking for one window silently got a full-screen frame with no
/// indication anything had been ignored.
#[tokio::test]
async fn capturing_one_window_returns_that_window_not_the_screen() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();
    let mut painted = paint_a_window(&socket, dir.path(), "one-window").expect("client connects");

    // Wait for the toplevel to be mapped — which now means "has painted",
    // rather than "the client asked for a toplevel".
    let deadline = Instant::now() + Duration::from_secs(5);
    let key = loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        let windows = stage.windows().await.expect("windows");
        if let Some(window) = windows.iter().find(|window| window.mapped) {
            break window.key;
        }
        assert!(
            Instant::now() < deadline,
            "no window ever reported itself mapped: {windows:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let whole = stage.capture(None).await.expect("full-screen capture");
    let one = stage.capture(Some(key)).await.expect("per-window capture");

    let geometry = stage
        .windows()
        .await
        .expect("windows")
        .into_iter()
        .find(|window| window.key == key)
        .expect("the window is still there")
        .geometry;

    // The capture is the window's own region. A stage toplevel is currently
    // given the whole output — decorations are server-side and there is no tiling
    // policy — so today that region happens to equal the screen, and this asserts
    // the *relationship* rather than a smaller number. X11 dialogs and
    // override-redirect windows arrive with their own geometry and are where the
    // crop becomes visible.
    assert_eq!(one.width, geometry.width, "capture must match the geometry");
    assert_eq!(one.height, geometry.height);
    assert_eq!(one.rgba.len(), (one.width * one.height * 4) as usize);
    assert!(one.width <= whole.width && one.height <= whole.height);

    // A key that names nothing must be an error, not a silent full screen.
    let missing = artist_computer::stage::WindowKey(u64::MAX);
    assert!(
        stage.capture(Some(missing)).await.is_err(),
        "capturing a window that does not exist must fail loudly"
    );
}

/// Rung 3 end to end: a real window, a real capture, text read off it, and a
/// click that lands where the text is.
///
/// The path this covers has never run before. Every piece was tested in
/// isolation — the compositor captures, the detector reads synthetic frames —
/// but "capture a live client, find its text, click it" is the claim rung 3
/// actually makes, and it was the one thing nothing exercised.
#[cfg(feature = "ocr")]
#[tokio::test]
async fn the_screen_surface_reads_a_real_window_and_clicks_what_it_read() {
    use artist_computer::ocr::Ocr;
    use artist_computer::surface::Surface;
    use artist_computer::surface::screen::ScreenSurface;

    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let ocr = match Ocr::load_default() {
        Ok(ocr) => ocr,
        Err(error) => {
            eprintln!("skipping: {error}");
            return;
        }
    };

    let socket = stage.socket_name().to_owned();
    let mut painted =
        paint_text_window(&socket, dir.path(), "screen-surface", "SEND").expect("client connects");

    // Let the client's buffer reach the compositor.
    let deadline = Instant::now() + Duration::from_secs(5);
    let key = loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        let windows = stage.windows().await.expect("windows");
        if let Some(window) = windows.iter().find(|window| window.mapped) {
            break window.key;
        }
        assert!(
            Instant::now() < deadline,
            "the client never mapped a window"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let stage: std::sync::Arc<dyn Stage> = std::sync::Arc::new(stage);
    let surface = ScreenSurface::new("screen:1", std::sync::Arc::clone(&stage), key, ocr);

    // Read the screen. This is capture -> detector -> recogniser -> nodes.
    let found;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        let snapshot = surface.snapshot_full().await.expect("snapshot");
        if let Some(node) = snapshot
            .nodes
            .iter()
            .find(|node| node.bounds.is_some_and(|b| b.width > 8 && b.height > 8))
        {
            found = node.clone();
            break;
        }
        assert!(
            Instant::now() < deadline,
            "rung 3 never found the text painted on the window"
        );
        tokio::time::sleep(Duration::from_millis(250).min(Duration::from_millis(250))).await;
    }

    let node = found;
    let bounds = node.bounds.expect("bounds");
    eprintln!("rung 3 read {:?} at {:?}", node.name, bounds);

    // The box must actually cover the word we drew, not some artifact.
    let word_left = WORD_ORIGIN.0 as i32;
    let word_top = WORD_ORIGIN.1 as i32;
    assert!(
        bounds.x <= word_left + 20 && bounds.y <= word_top + 20,
        "the box is nowhere near the drawn word at ({word_left},{word_top}): {bounds:?}"
    );

    // And the anchor must be actionable — this is the part that used to be
    // impossible, because rung 3 reported "no actionable surface".
    assert!(
        node.actions.iter().any(|action| action == "click"),
        "rung 3 must offer a click: {:?}",
        node.actions
    );

    // Click it, and check the client received a button where the text is.
    stage
        .pointer(key, artist_computer::stage::Pointing::at(bounds))
        .await
        .expect("the click should dispatch");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        let buttons = painted.state.buttons.lock().unwrap().clone();
        let points = painted.state.points.lock().unwrap().clone();
        if !buttons.is_empty() && !points.is_empty() {
            assert_eq!(buttons[0], 0x110, "wrong button: {buttons:?}");
            let (x, y) = *points.last().unwrap();
            let centre_x = f64::from(bounds.x) + f64::from(bounds.width) / 2.0;
            let centre_y = f64::from(bounds.y) + f64::from(bounds.height) / 2.0;
            assert!(
                (x - centre_x).abs() < 4.0 && (y - centre_y).abs() < 4.0,
                "the click landed at ({x},{y}) but the text we read is centred at \
                 ({centre_x},{centre_y})"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the click never reached the client (buttons={buttons:?})"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Capturing one window must not include another window's pixels.
///
/// Every toplevel on this stage is given the whole output, so a capture that
/// merely *crops* the finished composite returns the whole screen — and a rung-3
/// surface reading text off it would read every running application's text as
/// though it were its own. The fix is to recomposite with only that window's
/// surface tree, and this is the test that distinguishes the two.
#[tokio::test]
async fn capturing_one_window_excludes_another_windows_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();

    // Two clients painting two different colours.
    let mut first =
        paint_a_window(&socket, dir.path(), "first").expect("first client should connect");
    let mut second =
        paint_colour_window(&socket, dir.path(), "second", OTHER_PAINT).expect("second connects");

    let deadline = Instant::now() + Duration::from_secs(8);
    let (first_key, second_key) = loop {
        let _ = first.queue.roundtrip(&mut first.state);
        let _ = second.queue.roundtrip(&mut second.state);
        let windows = stage.windows().await.expect("windows");
        let mapped: Vec<_> = windows.iter().filter(|window| window.mapped).collect();
        if mapped.len() >= 2 {
            break (mapped[0].key, mapped[1].key);
        }
        assert!(
            Instant::now() < deadline,
            "both clients never mapped: {windows:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Whichever window we ask for, the other one's colour must be absent.
    for (key, want, avoid) in [
        (first_key, PAINT, OTHER_PAINT),
        (second_key, OTHER_PAINT, PAINT),
    ] {
        let _ = first.queue.roundtrip(&mut first.state);
        let _ = second.queue.roundtrip(&mut second.state);

        let frame = stage.capture(Some(key)).await.expect("per-window capture");
        let mut saw_wanted = false;
        let mut saw_other = false;
        for y in 0..frame.height {
            for x in 0..frame.width {
                if let Some((r, g, b, _)) = frame.pixel(x, y) {
                    if (r, g, b) == want {
                        saw_wanted = true;
                    }
                    if (r, g, b) == avoid {
                        saw_other = true;
                    }
                }
            }
        }
        // The window we asked for might not have painted yet on this pass, so
        // its own colour is a soft expectation. The other window's colour being
        // absent is the hard one — that is the property under test.
        assert!(
            !saw_other,
            "capturing {key:?} included another window's pixels (wanted {want:?}, \
             found {avoid:?}, own colour present: {saw_wanted})"
        );
    }
}

/// The globals a mainstream toolkit looks for before deciding what it can do.
///
/// Listed as a constant rather than asserted inline because the interesting
/// property is the *set*: a client reads the registry once and commits to a
/// rendering strategy from what it finds. Losing one of these does not degrade
/// the stage gracefully — it changes which applications will run on it at all.
const REQUIRED_GLOBALS: &[&str] = &[
    "wl_compositor",
    "wl_subcompositor",
    "wl_shm",
    "wl_seat",
    "wl_output",
    "wl_data_device_manager",
    "xdg_wm_base",
    "zxdg_decoration_manager_v1",
    "zxdg_output_manager_v1",
    "zwp_linux_dmabuf_v1",
    "wp_viewporter",
    "wp_presentation",
    // A drawing application asks the tablet seat what tools exist at start-up
    // and takes the mouse path permanently if it finds none — there is no
    // second chance to discover a stylus, so its absence is not a degradation
    // but a decision made once, invisibly, against us.
    "zwp_tablet_manager_v2",
];

#[tokio::test]
async fn the_registry_advertises_what_a_real_toolkit_looks_for() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let Some(painted) = paint_a_window(stage.socket_name(), dir.path(), "globals") else {
        panic!("the fixture client could not connect");
    };

    let globals = painted.state.globals.lock().unwrap().clone();
    let names: Vec<&str> = globals.iter().map(|(name, _)| name.as_str()).collect();
    for wanted in REQUIRED_GLOBALS {
        assert!(
            names.contains(wanted),
            "the stage does not advertise {wanted}; it offers {names:?}"
        );
    }

    // Version matters as much as presence for dmabuf: version 3 carries a bare
    // format list, and only version 4 carries the feedback that tells a client
    // which device to allocate on. A v3-only global on a multi-GPU machine
    // means every buffer a client sends is one we cannot import.
    let dmabuf_version = globals
        .iter()
        .find(|(name, _)| name == "zwp_linux_dmabuf_v1")
        .map(|(_, version)| *version)
        .unwrap();
    assert!(
        dmabuf_version >= 4,
        "dmabuf must be advertised at version 4 or better for feedback, got {dmabuf_version}"
    );
}

#[tokio::test]
async fn dmabuf_feedback_names_the_device_the_stage_renders_on() {
    use std::os::linux::fs::MetadataExt as _;

    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let Some(mut painted) = paint_a_window(stage.socket_name(), dir.path(), "dmabuf") else {
        panic!("the fixture client could not connect");
    };

    let handle = painted.queue.handle();
    let dmabuf = painted
        .state
        .dmabuf
        .clone()
        .expect("the stage advertises no dmabuf global");
    assert!(
        dmabuf.version() >= 4,
        "feedback needs version 4, bound {}",
        dmabuf.version()
    );
    let _feedback = dmabuf.get_default_feedback(&handle, ());

    // Two round trips: the compositor answers a feedback request with a burst
    // of events terminated by `done`, and one trip can deliver only the part of
    // it that was already queued.
    for _ in 0..4 {
        painted.queue.roundtrip(&mut painted.state).unwrap();
        if *painted.state.dmabuf_feedback_done.lock().unwrap() {
            break;
        }
    }

    assert!(
        *painted.state.dmabuf_feedback_done.lock().unwrap(),
        "the stage never finished sending dmabuf feedback"
    );
    assert!(
        *painted.state.dmabuf_tranches.lock().unwrap() > 0,
        "feedback carried no format tranche, so a client learns no usable format"
    );

    // The device we advertise has to be the device we render on. If it is not,
    // a client allocates on the wrong GPU and every buffer it hands us fails to
    // import — and the symptom is a window that draws nothing, which points at
    // the client rather than at this line.
    let advertised = painted
        .state
        .dmabuf_device
        .lock()
        .unwrap()
        .expect("feedback carried no main_device");
    let nodes: Vec<u64> = ["/dev/dri/renderD128", "/dev/dri/renderD129"]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.st_rdev())
        .collect();
    assert!(
        nodes.contains(&advertised),
        "dmabuf feedback names device {advertised}, which is not a render node on this \
         machine ({nodes:?})"
    );
}

#[tokio::test]
async fn a_surface_is_told_which_output_it_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let Some(painted) = paint_a_window(stage.socket_name(), dir.path(), "output") else {
        panic!("the fixture client could not connect");
    };

    assert!(
        *painted.state.entered_output.lock().unwrap(),
        "the surface was never sent wl_surface.enter, so the client cannot resolve \
         its scale — several toolkits will sit waiting for this and never draw"
    );
    assert_eq!(
        *painted.state.output_mode.lock().unwrap(),
        Some((1920, 1080)),
        "the output must report the stage's actual size"
    );
}

#[tokio::test]
async fn the_output_reports_the_size_the_stage_was_given() {
    // The default is 1920x1080, so a stage built at another size proves the
    // output is wired to the real dimensions rather than to the same constant
    // the previous test happens to check.
    let dir = tempfile::tempdir().unwrap();
    if !std::path::Path::new("/dev/dri/renderD128").exists() {
        eprintln!("skipping: no DRM render node on this machine");
        return;
    }
    let stage = match StageWayland::start_sized(StageId("test".into()), dir.path(), 1024, 768) {
        Ok(stage) => stage,
        Err(error) => {
            eprintln!("skipping: stage unavailable here: {error}");
            return;
        }
    };
    let Some(painted) = paint_a_window(stage.socket_name(), dir.path(), "sized") else {
        panic!("the fixture client could not connect");
    };
    assert_eq!(
        *painted.state.output_mode.lock().unwrap(),
        Some((1024, 768))
    );
}

#[tokio::test]
async fn the_stage_overrules_a_client_that_wants_to_draw_its_own_titlebar() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    // The fixture asks for `ClientSide` on every window it creates.
    let Some(painted) = paint_a_window(stage.socket_name(), dir.path(), "decorated") else {
        panic!("the fixture client could not connect");
    };

    assert_eq!(
        *painted.state.decoration_mode.lock().unwrap(),
        Some(zxdg_toplevel_decoration_v1::Mode::ServerSide),
        "a client that draws its own titlebar puts a close button on the stage that \
         no rung knows the geometry of — the agent could neither avoid it nor aim \
         at it, and would be one stray click from destroying its own window"
    );
}

#[tokio::test]
async fn a_gpu_client_hands_over_a_dmabuf_and_the_stage_composites_it() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let socket = stage.socket_name().to_owned();
    let mut painted = match paint_dmabuf_window(&socket, dir.path(), "gpu") {
        Ok(painted) => painted,
        Err(NoDmabuf::Unavailable(why)) => {
            // Not a pass and not a failure: this machine's driver would not
            // produce the buffer, so the compositor was never asked anything.
            eprintln!("skipping: no mappable GPU buffer here ({why})");
            return;
        }
    };

    // The claim is not "the buffer was accepted" — it is that the pixels inside
    // it reach the composite. An import that succeeds and then contributes
    // nothing looks, from the client's side, exactly like the blank window a
    // refused import produces.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        let frame = stage.capture(None).await.expect("capture should succeed");
        if let Some((r, g, b, _)) = frame.pixel(20, 20)
            && (r, g, b) == PAINT
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the stage accepted the client's dmabuf but composited none of its pixels"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_frame_the_client_asked_about_is_reported_as_presented() {
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let Some(mut painted) = paint_a_window(stage.socket_name(), dir.path(), "presented") else {
        panic!("the fixture client could not connect");
    };

    // Feedback arrives after the render that satisfies it, so give the
    // compositor a couple of passes rather than assuming one round trip.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && !*painted.state.presented.lock().unwrap() {
        painted.queue.roundtrip(&mut painted.state).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(
        *painted.state.presented.lock().unwrap(),
        "the stage advertises wp_presentation but never answered a feedback request; \
         a client pacing itself off presentation timestamps would stall waiting"
    );
    assert_eq!(
        *painted.state.presentation_clock.lock().unwrap(),
        Some(1),
        "presentation timestamps must be advertised on CLOCK_MONOTONIC, the clock \
         they are actually taken from"
    );
}

#[tokio::test]
async fn the_stage_hands_out_its_render_target_as_an_importable_dmabuf() {
    // The whole native-viewer design rests on this: the frame the compositor
    // draws must be able to *leave the process* as file descriptors another
    // program can import. A `GlesRenderbuffer` — what this used to be — cannot,
    // and the only way out would have been a full readback and re-encode per
    // frame, which is precisely the cost the viewer exists to avoid.
    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let mut painted = paint_a_window(stage.socket_name(), dir.path(), "exported")
        .expect("the fixture client should connect");
    let _ = painted.queue.roundtrip(&mut painted.state);

    let (targets, front, held) = stage
        .render_target()
        .await
        .expect("the stage must be able to hand out its render target");

    // Two of them, and that is the point rather than an implementation detail:
    // a viewer attaching the buffer the stage is *currently drawing into* shows
    // half-rendered frames, which looked on screen like black geometry flashing
    // over a window being typed into.
    // Three, and the count is the design rather than a detail. Two stops a
    // viewer seeing a frame *being drawn*, but leaves the renderer with only
    // one alternative — and if the compositor is still holding that one there
    // is nowhere to go, which is how tearing survived under pointer motion.
    assert_eq!(
        targets.len(),
        artist_computer::stage::wayland::RENDER_BUFFERS,
        "the stage must have somewhere to draw that is neither on screen nor held"
    );
    assert!(targets.len() >= 3, "two buffers cannot absorb a held frame");
    assert!(
        held.iter()
            .all(|flag| !flag.load(std::sync::atomic::Ordering::Acquire)),
        "nothing is attached yet, so no buffer should be marked held"
    );
    assert!(
        front.load(std::sync::atomic::Ordering::Acquire) < targets.len(),
        "the published index must name one of the buffers"
    );

    for target in &targets {
        // A dmabuf is only useful if it carries real planes with real descriptors.
        assert!(target.num_planes() > 0, "an exported buffer with no planes");
        assert_eq!(
            (target.width() as i32, target.height() as i32),
            (1920, 1080),
            "the exported buffer must be the stage's screen"
        );
        assert!(
            target.handles().count() > 0,
            "the export carried no file descriptors, so nothing could import it"
        );
    }

    // The two must be genuinely different memory. Handing out the same buffer
    // twice would satisfy every assertion above and tear exactly as before.
    let first: Vec<_> = targets[0]
        .handles()
        .map(|fd| fd.try_clone_to_owned())
        .collect();
    let second: Vec<_> = targets[1]
        .handles()
        .map(|fd| fd.try_clone_to_owned())
        .collect();
    assert_eq!(first.len(), second.len());

    // And the same buffer is still what capture reads, which is the property
    // that makes the viewer and the agent see the same screen rather than two
    // that merely agree most of the time.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _ = painted.queue.roundtrip(&mut painted.state);
        let frame = stage.capture(None).await.expect("capture should succeed");
        if let Some((r, g, b, _)) = frame.pixel(20, 20)
            && (r, g, b) == PAINT
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the exported target is not the buffer being drawn into"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The viewer, against the user's actual compositor.
///
/// Everything else about the viewer is unit-tested — the export carries real
/// descriptors, keys round-trip, clicks scale — but none of that exercises the
/// part that can only fail on a real display: whether another compositor will
/// *accept* our buffer. That is the claim the whole zero-copy design rests on,
/// and it cannot be made from a headless test.
///
/// Skipped when there is no session to open a window on, which is every CI
/// machine and every SSH shell.
#[tokio::test]
async fn a_viewer_window_opens_on_a_real_display_and_imports_the_stage_buffer() {
    use artist_computer::stage::viewer::Viewer;

    let user_display = std::env::var("ARTIST_TEST_WAYLAND_DISPLAY")
        .ok()
        .or_else(|| std::env::var("WAYLAND_DISPLAY").ok());
    let Some(display) = user_display else {
        eprintln!("skipping: no WAYLAND_DISPLAY, so there is no screen to open a window on");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    if !std::path::Path::new("/dev/dri/renderD128").exists() {
        eprintln!("skipping: no DRM render node");
        return;
    }
    let stage = match StageWayland::start(StageId("viewtest".into()), dir.path()) {
        Ok(stage) => std::sync::Arc::new(stage),
        Err(error) => {
            eprintln!("skipping: stage unavailable here: {error}");
            return;
        }
    };

    // Something on screen, so the window is not merely black.
    let painted = paint_a_window(stage.socket_name(), dir.path(), "watched");
    assert!(painted.is_some(), "the fixture client should connect");

    // Named explicitly rather than through `WAYLAND_DISPLAY`. That variable is
    // process-global, so setting it here changed what every other test in this
    // binary saw and broke the one asserting the stage leaves the user's
    // session untouched — the same class of bug the stage itself avoids by
    // binding its socket by absolute path.
    let viewer = match Viewer::open_on(std::sync::Arc::clone(&stage), Some(&display)).await {
        Ok(viewer) => viewer,
        Err(error) => {
            // A compositor that will not take a linear ARGB dmabuf is a real
            // finding, not a skip — but a missing session is not.
            let message = error.to_string();
            assert!(
                message.contains("connect to your display"),
                "the viewer failed for a reason other than an absent display: {message}"
            );
            eprintln!("skipping: could not reach {display} ({message})");
            return;
        }
    };

    // Nobody has touched it, so nothing is attributed to a human.
    assert_eq!(viewer.human().count(), 0);

    // `Viewer::open` only returns once the buffer is imported and attached, so
    // reaching here *is* the assertion: a real compositor, on a different
    // process, accepted the stage's render target and put it on screen. That is
    // the claim the whole zero-copy design rests on and the one thing no
    // headless test can make.
    eprintln!("a viewer window opened on {display} and imported the stage's buffer");

    // Dropping closes it again.
    drop(viewer);
    drop(painted);
}

/// Rung 3 against text a real toolkit rendered, at sizes a real interface uses.
///
/// Everything else about OCR here is measured on a blocky 5x7 font drawn by the
/// test itself — thick, high-contrast, and nothing like a interface. Small
/// anti-aliased text at 13–16px is the actual case, and it is the case where
/// recognition degrades. Until this passes, "rung 3 can read the screen" is a
/// claim about a font we invented.
#[cfg(feature = "ocr")]
#[tokio::test]
async fn rung_three_reads_text_a_real_toolkit_rendered() {
    use artist_computer::stage::AppCommand;

    let dir = tempfile::tempdir().unwrap();
    let Some(stage) = stage_or_skip(dir.path()) else {
        return;
    };
    let Some(chromium) = ["chromium", "chromium-browser", "google-chrome-stable"]
        .into_iter()
        .find(|name| {
            std::env::var_os("PATH").is_some_and(|paths| {
                std::env::split_paths(&paths).any(|path| path.join(name).is_file())
            })
        })
    else {
        eprintln!("skipping: no chromium to render real text with");
        return;
    };

    // Ordinary interface text: default weight, default size, dark on light.
    // Nothing bolded or enlarged to help the detector.
    let page = dir.path().join("ui.html");
    std::fs::write(
        &page,
        "<!doctype html><meta charset=utf-8>\
         <body style='font:14px system-ui;padding:24px;background:#fff;color:#111'>\
         <h1 style='font-size:20px'>Account settings</h1>\
         <p>Change how your workspace behaves.</p>\
         <button style='font:14px system-ui;padding:6px 12px'>Save changes</button>\
         <button style='font:14px system-ui;padding:6px 12px'>Discard</button>\
         </body>",
    )
    .unwrap();

    let command = AppCommand::new(chromium)
        .arg("--ozone-platform=wayland")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!(
            "--user-data-dir={}",
            dir.path().join("profile").display()
        ))
        .arg(format!("file://{}", page.display()));
    if stage.spawn(command).await.is_err() {
        eprintln!("skipping: chromium would not start on the stage");
        return;
    }

    // Wait for it to paint something.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if stage
            .windows()
            .await
            .map(|windows| windows.iter().any(|window| window.mapped))
            .unwrap_or(false)
        {
            break;
        }
        if Instant::now() >= deadline {
            eprintln!("skipping: chromium never mapped a window");
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;

    let frame = stage.capture(None).await.expect("capture");
    let Ok(ocr) = artist_computer::ocr::Ocr::load_default() else {
        eprintln!("skipping: no OCR weights");
        return;
    };
    let boxes = ocr
        .read(&frame, None, &Default::default())
        .expect("reading the screen should not fail");

    let seen: Vec<String> = boxes.iter().map(|found| found.text.clone()).collect();
    // `locate` is what the product uses — the model names a label and rung 3
    // finds its box — so assert through that rather than on raw strings.
    for label in ["Save changes", "Discard", "Account settings"] {
        assert!(
            ocr.locate(&boxes, label).is_some(),
            "rung 3 could not find {label:?} in real 14px interface text. Read: {seen:?}"
        );
    }
}
