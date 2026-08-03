//! A window on the user's real display showing the stage, and reaching into it.
//!
//! The stage is deliberately invisible: no monitor, nothing to watch. That is
//! right for isolation and wrong for trust — "it clicked something and said it
//! worked" is not a thing anyone should have to take on faith.
//!
//! **Zero copy, because the buffer never leaves the GPU.** The stage renders
//! into a GBM buffer object exported as a [`Dmabuf`]; this client imports those
//! same file descriptors through `zwp_linux_dmabuf_v1` and attaches them to a
//! surface on the user's compositor, which scans them out. There is no readback,
//! no encode, no transfer and no decode. The alternative — screenshot, PNG, pipe,
//! decode, blit — does four format conversions and a CPU round trip to move a
//! buffer between two processes on one graphics card.
//!
//! **Damage passes straight through.** The stage already computes which
//! rectangles changed; those become `wl_surface.damage_buffer`, so the user's
//! compositor does a partial update and an idle stage costs nothing at all.
//!
//! **The seat is shared.** A person clicking here does not pause the agent.
//! Pausing would make the only way in a way of stopping the agent working, which
//! turns "let me help" into "let me interrupt". Both drive the same seat by the
//! same route: a viewer's click becomes the same [`Stage::pointer`] call the
//! agent makes, because input here was always a function call rather than a
//! device.
//!
//! **What sharing costs, and what pays for it.** A human acting mid-program
//! means the agent's next observation contains something no step of its program
//! caused, and its `expect` can fail for a reason it cannot see. So every action
//! taken here is counted, and the count is readable at the point a program
//! reports. An unexplained failure becomes an explained one — which is the
//! difference between a shared seat that works and one that quietly makes the
//! agent look broken.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use smithay::backend::allocator::Buffer as _;
use smithay::backend::allocator::dmabuf::Dmabuf;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::model::Rect;
use crate::program::StepError;
use crate::stage::wayland::StageWayland;
use crate::stage::{Stage, WindowKey};

/// What a person has done to the stage, so the agent can be told.
///
/// A counter rather than a flag: the question worth answering is "did anything
/// happen *during my program*", and a flag cannot answer that without a reset
/// that races the next action.
#[derive(Debug, Default)]
pub struct HumanActivity {
    actions: AtomicU64,
}

impl HumanActivity {
    pub fn count(&self) -> u64 {
        self.actions.load(Ordering::Relaxed)
    }

    /// How many actions have happened since a previously taken count.
    pub fn since(&self, previous: u64) -> u64 {
        self.count().saturating_sub(previous)
    }

    fn record(&self) {
        self.actions.fetch_add(1, Ordering::Relaxed);
    }
}

/// One thing a person did in the viewer, on its way to the stage.
#[derive(Debug)]
enum HumanInput {
    Click {
        x: i32,
        y: i32,
    },
    /// A key, by the stage's own name for it.
    ///
    /// There is deliberately no separate "text" variant. A printable key comes
    /// back from [`crate::keys::name_for_evdev`] as a one-character name, which
    /// the stage parses and delivers through the same keymap it uses for the
    /// agent. Translating to a character here and sending it as text would
    /// translate the layout twice — the way a viewer ends up typing the wrong
    /// character on a non-US keyboard.
    Key(String),
}

/// A window onto a running stage.
pub struct Viewer {
    human: Arc<HumanActivity>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Viewer {
    pub fn human(&self) -> &Arc<HumanActivity> {
        &self.human
    }

    /// Open a window on the user's display showing this stage.
    ///
    /// Connects through the process's own `WAYLAND_DISPLAY` — the user's
    /// compositor, deliberately, and the only place in the crate that does. Every
    /// other connection is to the stage's own socket by absolute path, and
    /// confusing the two would put the agent's windows on the user's screen.
    /// Takes the Wayland stage concretely rather than `dyn Stage`, because
    /// exporting a render target is not something every stage can do — a PTY has
    /// no pixels — and a trait method returning `None` for most implementors
    /// would be pretending otherwise.
    pub async fn open(stage: Arc<StageWayland>) -> Result<Self, StepError> {
        Self::open_on(stage, None).await
    }

    /// The same, on a named display.
    ///
    /// Exists so a test can point at a compositor without setting
    /// `WAYLAND_DISPLAY`, which is process-global: doing that from one test
    /// changed what every other test in the binary saw, and broke the one
    /// asserting the stage leaves the user's session alone. The isolation test
    /// was right and the convenience was wrong.
    pub async fn open_on(
        stage: Arc<StageWayland>,
        display: Option<&str>,
    ) -> Result<Self, StepError> {
        let (targets, front, held) = stage.render_target().await.ok_or_else(|| {
            StepError::Backend(
                "this stage cannot export its render target, so there is nothing to show".into(),
            )
        })?;

        let connection = match display {
            Some(name) => {
                let runtime = std::env::var("XDG_RUNTIME_DIR")
                    .map_err(|_| StepError::Backend("XDG_RUNTIME_DIR is not set".into()))?;
                let socket = std::path::Path::new(&runtime).join(name);
                let stream = std::os::unix::net::UnixStream::connect(&socket).map_err(|error| {
                    StepError::Backend(format!("connect to {}: {error}", socket.display()))
                })?;
                Connection::from_socket(stream).map_err(|error| {
                    StepError::Backend(format!("connect to your display: {error}"))
                })?
            }
            None => Connection::connect_to_env().map_err(|error| {
                StepError::Backend(format!(
                    "connect to your display: {error}. A viewer needs a running Wayland \
                     session; over SSH there is no screen to put it on."
                ))
            })?,
        };

        let human = Arc::new(HumanActivity::default());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<HumanInput>();

        // Input arrives on the viewer thread and has to reach the stage, whose
        // methods are async. A channel rather than a runtime handle inside the
        // client loop: blocking that loop on a stage call would stop the window
        // repainting while the agent held the seat.
        let acting_stage = Arc::clone(&stage);
        let acting_human = Arc::clone(&human);
        tokio::spawn(async move {
            while let Some(input) = input_rx.recv().await {
                let Ok(windows) = acting_stage.windows().await else {
                    continue;
                };
                // The first window, because every toplevel is given the whole
                // output and the viewer is looking at the composite. Attributing
                // a click by geometry would need hit-testing the viewer has no
                // basis for.
                let Some(window) = windows.first().map(|window| window.key) else {
                    continue;
                };
                let sent = deliver(acting_stage.as_ref(), window, input).await;
                if sent {
                    // Counted only once it lands, so the agent is never told
                    // about an action the stage refused.
                    acting_human.record();
                }
            }
        });

        // Bridge the stage's damage signal into the client loop. The
        // subscription is a tokio broadcast and the loop is an ordinary thread,
        // so a task forwards rects across a sync channel. This is what makes an
        // idle stage cost *nothing*: no timer, no polling, no commit until the
        // compositor says something actually changed.
        let (damage_tx, damage_rx) = std::sync::mpsc::channel::<Rect>();
        let mut damage = stage.damage();
        let damage_stop = Arc::clone(&stop);
        tokio::spawn(async move {
            while !damage_stop.load(Ordering::Relaxed) {
                match damage.recv().await {
                    Ok(event) => {
                        if damage_tx.send(event.region).is_err() {
                            return;
                        }
                    }
                    // Lagged means the viewer fell behind a burst. The right
                    // recovery is to repaint everything once rather than to
                    // replay a queue of stale rectangles.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if damage_tx.send(FULL_REPAINT).is_err() {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        // The thread reports whether it got as far as a mapped window with the
        // stage's buffer attached. Without this, a compositor refusing the
        // import left `open` returning `Ok` and a dead thread behind it — the
        // caller would be told a window was showing when nothing was, which is
        // the worst shape a failure can take here.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("artist-stage-viewer".into())
            .spawn(move || {
                run(
                    connection,
                    Session {
                        targets,
                        front,
                        held,
                        input: input_tx,
                        damage: damage_rx,
                        ready: ready_tx,
                        stop: thread_stop,
                    },
                )
            })
            .map_err(|error| StepError::Backend(format!("start the viewer thread: {error}")))?;

        match tokio::task::spawn_blocking(move || ready_rx.recv_timeout(READY_TIMEOUT))
            .await
            .map_err(|error| StepError::Backend(format!("viewer startup: {error}")))?
        {
            Ok(Ok(())) => Ok(Self { human, stop }),
            Ok(Err(error)) => Err(StepError::Backend(error)),
            Err(_) => Err(StepError::Backend(
                "the viewer did not manage to show a window within \
                 the time allowed; your compositor may not accept the stage's buffer"
                    .into(),
            )),
        }
    }
}

async fn deliver(stage: &StageWayland, window: WindowKey, input: HumanInput) -> bool {
    let result = match input {
        HumanInput::Click { x, y } => {
            // A one-pixel rectangle: `pointer` aims at the centre of an area
            // because that is what an anchor's bounds give it. A click has none.
            let at = Rect {
                x,
                y,
                width: 1,
                height: 1,
            };
            stage
                .pointer(
                    window,
                    crate::stage::Pointing::at(at).with_button(LEFT_BUTTON),
                )
                .await
        }
        HumanInput::Key(chord) => stage.key(window, &chord).await,
    };
    result.is_ok()
}

/// `BTN_LEFT` from `linux/input-event-codes.h`.
const LEFT_BUTTON: u32 = 0x110;

/// The sentinel meaning "redraw all of it".
///
/// A zero-sized rect, which is not a rectangle any real damage event produces —
/// the compositor only reports regions that were painted.
const FULL_REPAINT: Rect = Rect {
    x: 0,
    y: 0,
    width: 0,
    height: 0,
};

/// How long to wait for the viewer to get a window on screen.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the client loop waits for damage before servicing the socket.
///
/// Not a frame interval: nothing is drawn on this tick. It exists so the loop
/// still reads Wayland events — configures, input, a close request — on a stage
/// that is perfectly still.
const SERVICE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    seat: Option<wl_seat::WlSeat>,
    decorations: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    /// Set when the compositor configures a new window size, so the viewport
    /// can be resized to match before the next commit.
    resized: bool,
    buffer: Option<wl_buffer::WlBuffer>,
    /// Cleared when the compositor gives a buffer back, so the stage knows it
    /// may draw into it again. Without this the renderer has no idea a buffer
    /// is still on screen — which is how a frame being drawn reached the
    /// display under rapid damage like pointer motion.
    held: Option<Arc<[std::sync::atomic::AtomicBool; crate::stage::wayland::RENDER_BUFFERS]>>,
    /// The imported buffers, in stage order, so a `release` can be attributed.
    imported: Vec<wl_buffer::WlBuffer>,
    configured: bool,
    closed: bool,
    /// Stage pixels, for scaling a click from window coordinates.
    stage_size: (i32, i32),
    /// The size the compositor last configured this window to.
    window_size: (i32, i32),
    /// Where the pointer is, in window coordinates.
    pointer_at: (f64, f64),
    input: tokio::sync::mpsc::UnboundedSender<HumanInput>,
}

/// Everything the client thread needs, gathered so the signature stays readable.
struct Session {
    targets: [Dmabuf; crate::stage::wayland::RENDER_BUFFERS],
    front: Arc<std::sync::atomic::AtomicUsize>,
    held: Arc<[std::sync::atomic::AtomicBool; crate::stage::wayland::RENDER_BUFFERS]>,
    input: tokio::sync::mpsc::UnboundedSender<HumanInput>,
    damage: std::sync::mpsc::Receiver<Rect>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

fn run(connection: Connection, session: Session) {
    let Session {
        targets,
        front,
        held,
        input,
        damage,
        ready,
        stop,
    } = session;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());

    let mut client = Client {
        compositor: None,
        wm_base: None,
        dmabuf: None,
        seat: None,
        held: Some(Arc::clone(&held)),
        imported: Vec::new(),
        decorations: None,
        viewporter: None,
        resized: false,
        buffer: None,
        configured: false,
        closed: false,
        stage_size: (targets[0].width() as i32, targets[0].height() as i32),
        window_size: (targets[0].width() as i32, targets[0].height() as i32),
        pointer_at: (0.0, 0.0),
        input,
    };
    if queue.roundtrip(&mut client).is_err() {
        let _ = ready.send(Err("lost the connection to your display".into()));
        return;
    }

    let (Some(compositor), Some(wm_base), Some(dmabuf)) = (
        client.compositor.clone(),
        client.wm_base.clone(),
        client.dmabuf.clone(),
    ) else {
        let _ = ready.send(Err(
            "your compositor does not offer the globals a viewer needs \
             (wl_compositor, xdg_wm_base, zwp_linux_dmabuf)"
                .into(),
        ));
        return;
    };

    let surface = compositor.create_surface(&handle, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg_surface.get_toplevel(&handle, ());
    toplevel.set_title("artist — the agent's screen".into());
    toplevel.set_app_id("com.artist.StageViewer".into());

    // Ask for a titlebar. A Wayland client gets no decorations unless it either
    // draws them or asks the compositor to — and without one this window cannot
    // be moved, resized or closed by ordinary means. Server-side, so it matches
    // the rest of the user's desktop rather than something we invented.
    if let Some(manager) = client.decorations.clone() {
        let decoration = manager.get_toplevel_decoration(&toplevel, &handle, ());
        decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    }

    // Present the stage's buffer scaled into whatever size the window is. The
    // buffer is the stage's full resolution — 1920x1080 by default — and asking
    // for a window that size makes it land like a fullscreen surface on a
    // 1920x1080 screen: unplaceable, and covering everything. A viewport lets
    // the window be an ordinary size while the buffer stays exactly as rendered.
    let viewport = client
        .viewporter
        .clone()
        .map(|viewporter| viewporter.get_viewport(&surface, &handle, ()));
    if viewport.is_some() {
        client.window_size = default_window_size(client.stage_size);
    }
    surface.commit();
    let _ = queue.roundtrip(&mut client);

    // Import *both* of the stage's buffers. These are the compositor's own file
    // descriptors: what is attached below is the memory being rendered into,
    // not a copy of it. Both, because the stage alternates — attaching only one
    // would show a buffer being drawn into half the time, which is the tearing
    // this whole arrangement exists to remove.
    let mut buffers = Vec::with_capacity(2);
    for target in &targets {
        client.buffer = None;
        let params = dmabuf.create_params(&handle, ());
        for (index, ((handle_fd, offset), stride)) in target
            .handles()
            .zip(target.offsets())
            .zip(target.strides())
            .enumerate()
        {
            let modifier: u64 = target.format().modifier.into();
            params.add(
                handle_fd,
                index as u32,
                offset,
                stride,
                (modifier >> 32) as u32,
                (modifier & 0xFFFF_FFFF) as u32,
            );
        }
        params.create(
            target.width() as i32,
            target.height() as i32,
            target.format().code as u32,
            zwp_linux_buffer_params_v1::Flags::empty(),
        );
        for _ in 0..4 {
            if queue.roundtrip(&mut client).is_err() || client.buffer.is_some() {
                break;
            }
        }
        if let Some(imported) = client.buffer.clone() {
            // Recorded in the client too, so a `release` can be matched back to
            // the index the stage knows this buffer by.
            client.imported.push(imported.clone());
            buffers.push(imported);
        }
    }
    let Some(buffer) = buffers.first().cloned() else {
        let _ = ready.send(Err(
            "your compositor refused the stage's buffer, so there is nothing to show. \
             It may not accept a linear ARGB dmabuf from another device."
                .into(),
        ));
        return;
    };

    if let Some(viewport) = &viewport {
        viewport.set_destination(client.window_size.0, client.window_size.1);
    }
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    // Imported and attached: from here a window exists, whatever happens next.
    let _ = ready.send(Ok(()));

    // Driven by damage, not by a clock. The stage already computes which
    // rectangles changed for its own settle predicates, so the viewer commits
    // exactly those and nothing at all when the screen is still. That is the
    // advantage of owning the compositor, and a timer would throw it away.
    let mut first_frame = true;
    while !stop.load(Ordering::Relaxed) && !client.closed {
        if queue.roundtrip(&mut client).is_err() {
            break;
        }
        if !client.configured {
            continue;
        }

        // Collect everything that arrived, so a burst is one commit rather than
        // one per rectangle. `recv_timeout` for the first, then drain.
        let mut rects = Vec::new();
        match damage.recv_timeout(SERVICE_INTERVAL) {
            Ok(rect) => rects.push(rect),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        rects.extend(damage.try_iter());

        // The very first commit has to paint everything: the client has just
        // attached a buffer full of whatever the stage had already drawn, and no
        // damage event describes pixels that were painted before we arrived.
        // A resize is checked *before* the no-damage early exit, not after.
        // After it, a window resized while the stage happened to be still did
        // nothing at all until something else repainted — the window snapped
        // back to a corner and stayed there until you touched the application
        // inside it. A resize changes where the buffer is drawn, which is our
        // business and has nothing to do with whether the stage drew anything.
        if client.resized {
            client.resized = false;
            if let Some(viewport) = &viewport {
                viewport.set_destination(client.window_size.0, client.window_size.1);
            }
            rects.clear();
            rects.push(FULL_REPAINT);
        } else if first_frame {
            rects.clear();
            rects.push(FULL_REPAINT);
            first_frame = false;
        } else if rects.is_empty() {
            continue;
        }

        for rect in &rects {
            if *rect == FULL_REPAINT {
                surface.damage_buffer(0, 0, client.stage_size.0, client.stage_size.1);
            } else {
                surface.damage_buffer(rect.x, rect.y, rect.width as i32, rect.height as i32);
            }
        }
        // Whichever buffer holds a *finished* frame. The stage publishes the
        // index after its draw fence has been waited on, so this is never one
        // being rendered into.
        let showing = front
            .load(std::sync::atomic::Ordering::Acquire)
            .min(buffers.len().saturating_sub(1));
        // Marked as ours *before* the commit. The compositor may take it the
        // instant the commit lands, and a stage that read the flag in between
        // would think the buffer was free and start drawing into it.
        held[showing].store(true, std::sync::atomic::Ordering::Release);
        // Re-attached every frame: the buffer contents changed underneath, and a
        // compositor is entitled to assume an unattached surface is unchanged.
        surface.attach(Some(&buffers[showing]), 0, 0);
        surface.commit();
    }
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
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), queue, ()));
                }
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, queue, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(5), queue, ())),
                "zwp_linux_dmabuf_v1" => {
                    state.dmabuf = Some(registry.bind(name, version.min(3), queue, ()));
                }
                "zxdg_decoration_manager_v1" => {
                    state.decorations = Some(registry.bind(name, 1, queue, ()));
                }
                "wp_viewporter" => {
                    state.viewporter = Some(registry.bind(name, 1, queue, ()));
                }
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } if width > 0 && height > 0 => {
                if state.window_size != (width, height) {
                    state.window_size = (width, height);
                    state.resized = true;
                }
            }
            xdg_toplevel::Event::Close => state.closed = true,
            _ => {}
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
        if let zwp_linux_buffer_params_v1::Event::Created { buffer } = event {
            state.buffer = Some(buffer);
        }
    }

    wayland_client::event_created_child!(Client, zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (wl_buffer::WlBuffer, ()),
    ]);
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
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(queue, ());
            }
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(queue, ());
            }
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
        match event {
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => state.pointer_at = (surface_x, surface_y),
            wl_pointer::Event::Button {
                state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => {
                // Window coordinates are not stage coordinates: the window is
                // resizable and the stage is not. Without this scaling every
                // click on a resized viewer lands somewhere else — the single
                // most likely bug in any remote view of a screen.
                let (window_w, window_h) = state.window_size;
                let (stage_w, stage_h) = state.stage_size;
                if window_w <= 0 || window_h <= 0 {
                    return;
                }
                let x = (state.pointer_at.0 * stage_w as f64 / window_w as f64).round() as i32;
                let y = (state.pointer_at.1 * stage_h as f64 / window_h as f64).round() as i32;
                let _ = state.input.send(HumanInput::Click { x, y });
            }
            _ => {}
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
            // The evdev code, passed through as the stage's own key vocabulary
            // understands it. Deliberately not translated to a keysym here: the
            // stage has the keymap and translating twice is how a viewer ends up
            // typing the wrong character on a non-US layout.
            if let Some(name) = crate::keys::name_for_evdev(key) {
                let _ = state.input.send(HumanInput::Key(name.to_owned()));
            }
        }
    }
}

/// A window size that behaves like a window.
///
/// Half the stage, so a 1920x1080 stage opens as a 960x540 window that the
/// compositor can place normally. Asking for the stage's full size on a screen
/// of the same size produces something that behaves like a fullscreen surface:
/// it covers everything and there is nowhere to put it.
fn default_window_size((width, height): (i32, i32)) -> (i32, i32) {
    ((width / 2).max(320), (height / 2).max(240))
}

delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);

/// `wl_buffer.release` is the compositor saying it has finished with a buffer.
///
/// Previously ignored, which is what left the last of the tearing: the stage had
/// no way to know a buffer was still on screen and would start drawing into it.
impl Dispatch<wl_buffer::WlBuffer, ()> for Client {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Identified by object id rather than user data: `event_created_child!`
        // fixes the child's udata type at the macro, so the index cannot be
        // attached there. The list is three long, so a scan is free.
        if matches!(event, wl_buffer::Event::Release)
            && let Some(held) = &state.held
            && let Some(index) = state
                .imported
                .iter()
                .position(|imported| imported.id() == buffer.id())
            && let Some(flag) = held.get(index)
        {
            flag.store(false, std::sync::atomic::Ordering::Release);
        }
    }
}

delegate_noop!(Client: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
delegate_noop!(Client: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(Client: ignore zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1);
delegate_noop!(Client: ignore wp_viewporter::WpViewporter);
delegate_noop!(Client: ignore wp_viewport::WpViewport);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_counts_rather_than_flags() {
        // "Did anything happen during my program" cannot be answered by a flag
        // without a reset that races the next action.
        let human = HumanActivity::default();
        assert_eq!(human.since(0), 0);
        human.record();
        human.record();
        assert_eq!(human.count(), 2);
        assert_eq!(human.since(1), 1);
    }

    #[test]
    fn a_stale_baseline_never_reports_negative_activity() {
        let human = HumanActivity::default();
        human.record();
        assert_eq!(human.since(99), 0);
    }

    #[test]
    fn the_full_repaint_sentinel_is_not_a_rectangle_damage_can_produce() {
        // The compositor only reports regions that were painted, so a
        // zero-sized rect is unambiguous. If it were a real value, a genuine
        // damage event would silently trigger a whole-screen commit.
        assert_eq!(FULL_REPAINT.width, 0);
        assert_eq!(FULL_REPAINT.height, 0);
    }

    #[test]
    fn the_service_interval_is_not_a_frame_rate() {
        // Nothing is drawn on this tick — it exists only so the loop still
        // reads configures, input and close requests on a still stage. If it
        // were ever used to pace painting, an idle stage would stop being free.
        assert!(
            SERVICE_INTERVAL >= std::time::Duration::from_millis(50),
            "a short interval here would look like a frame rate and behave like one"
        );
    }
}
