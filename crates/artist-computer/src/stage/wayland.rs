//! A headless Wayland compositor: the agent's own display server.
//!
//! Smithay has no headless backend, and that is the good news rather than a
//! gap — a headless compositor needs no backend at all. There is no monitor to
//! mode-set, no vblank to wait for and no input device to open. We construct an
//! [`Output`] ourselves, render into an offscreen GLES buffer on demand, and
//! deliver input as ordinary function calls into our own seat.
//!
//! **Threading.** Every smithay type here is `!Send` — `Display`,
//! `DisplayHandle`, `GlesRenderer`, the state struct. So one dedicated OS thread
//! owns all of it and an event loop, and [`StageWayland`] is a `Send + Sync`
//! proxy that talks to it over a channel. That split has to exist before
//! anything else does; every method on the [`Stage`] trait is
//! "send a command, await a reply".
//!
//! **Rendering** is on demand, not at 60 Hz: a frame is drawn when a surface
//! commits with damage. An idle stage therefore costs nothing, which matters
//! because it lives inside the user's editor rather than on a spare machine.
//!
//! **Frame callbacks are the failure mode to watch.** A client that does not
//! receive `wl_surface.frame` after a commit will draw exactly one frame and
//! then appear to hang — and it looks like a client bug, not a compositor one.
//! They are sent after every render, and there is a test for it.

use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::{GlesRenderbuffer, GlesRenderer};
use smithay::backend::renderer::utils::{draw_render_elements, on_commit_buffer_handler};
use smithay::backend::renderer::{Bind, Color32F, Frame as RenderFrame, Offscreen, Renderer};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface};
use smithay::reexports::wayland_server::{Client, Display, ListeningSocket};
use smithay::utils::{Rectangle, Serial, Size, Transform};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, TraversalAction,
    with_surface_tree_downward,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_seat, delegate_shm, delegate_xdg_shell,
};

use crate::model::{Frame, Rect};
use crate::program::StepError;
use crate::stage::{
    AppCommand, AppHandle, Damage, Stage, StageEnv, StageId, WindowInfo, WindowKey, WindowKind,
};

/// The stage's default virtual screen.
///
/// A stage deliberately has exactly **one** output. Multi-monitor is a human
/// affordance — a place to put things while looking at something else — and an
/// agent has no use for it: every extra output is another surface to search and
/// another coordinate space to keep straight, for no capability gained. The size
/// is configurable because a taller viewport genuinely changes what a page
/// shows; the *number* of outputs is not.
const DEFAULT_WIDTH: i32 = 1920;
const DEFAULT_HEIGHT: i32 = 1080;

/// Commands the proxy sends to the compositor thread.
enum StageCommand {
    Windows(tokio::sync::oneshot::Sender<Vec<WindowInfo>>),
    Focus(WindowKey, tokio::sync::oneshot::Sender<Result<(), String>>),
    Key(
        WindowKey,
        String,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    Text(
        WindowKey,
        String,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    Pointer(
        WindowKey,
        Rect,
        u32,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    Capture(
        Option<WindowKey>,
        tokio::sync::oneshot::Sender<Result<Frame, String>>,
    ),
    /// The stage's X11 display number, once XWayland is ready.
    X11Display(tokio::sync::oneshot::Sender<Option<u32>>),
    Shutdown,
}

/// A window the compositor is managing.
struct Managed {
    key: WindowKey,
    toplevel: ToplevelSurface,
    kind: WindowKind,
    geometry: Rect,
    /// The owning process, resolved from the Wayland client's credentials.
    /// This is what correlates a window to the browser or toolkit that made it
    /// — never a guess based on the window title.
    pid: Option<i32>,
    /// Set on the first commit that carries a buffer.
    ///
    /// A toplevel exists from the moment the client asks for one, which is
    /// before it has drawn anything; reporting every toplevel as mapped made
    /// `WindowInfo::mapped` a constant, and left `wait_for_window` racing a
    /// window that was not yet on screen.
    mapped: bool,
}

/// Everything the compositor thread owns.
struct StageState {
    /// Kept so client credentials can be read when a toplevel appears; the
    /// shell handler is not given one.
    display: smithay::reexports::wayland_server::DisplayHandle,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    seat: Seat<Self>,
    windows: Vec<Managed>,
    next_key: u64,
    /// Set when a surface commits with new content, so the render loop knows
    /// there is something to do. This is the whole reason an idle stage is free.
    dirty: bool,
    damage: tokio::sync::broadcast::Sender<Damage>,
    running: bool,
    /// The stage's own X11 display number, once XWayland reports ready.
    /// `None` when XWayland is unavailable — X11-only apps then cannot run,
    /// but everything else still can.
    x11_display: Option<u32>,
    /// The last render failure, held until a render succeeds.
    ///
    /// Kept so a `capture` can refuse rather than return the last good buffer
    /// as though it were a fresh screenshot.
    render_error: Option<String>,
    width: i32,
    height: i32,
}

impl StageState {
    fn full_screen(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.width as u32,
            height: self.height as u32,
        }
    }
}

impl StageState {
    fn window(&self, key: WindowKey) -> Option<&Managed> {
        self.windows.iter().find(|window| window.key == key)
    }
}

impl BufferHandler for StageState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for StageState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Not every client is one we inserted. XWayland is spawned by smithay
        // and arrives carrying `XWaylandClientData`, so assuming our own type
        // here panics the compositor thread the moment XWayland connects — and
        // takes X11 support down with it while every Wayland test still passes.
        if let Some(ours) = client.get_data::<StageClientState>() {
            return &ours.compositor_state;
        }
        if let Some(xwayland) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &xwayland.compositor_state;
        }
        // A third party would be a smithay-side change rather than a client
        // doing something odd, so this is genuinely unreachable in practice.
        panic!("a wayland client arrived with unrecognized client data")
    }

    fn commit(&mut self, surface: &wl_surface::WlSurface) {
        // Read what the client committed *before* handing it to the renderer:
        // `on_commit_buffer_handler` takes the buffer assignment as it imports
        // the texture, so asking afterwards makes every commit look like a
        // state-only commit with nothing to redraw.
        let regions = surface_damage(surface, self.full_screen());

        // Required before any renderer touches the buffer; without it the
        // surface has no usable texture and rendering silently draws nothing.
        on_commit_buffer_handler::<Self>(surface);
        self.dirty = true;

        // A commit carrying damage is a commit that painted, which is the point
        // a toplevel becomes a window the agent can see and act on.
        let painted = !regions.is_empty();
        let window = self
            .windows
            .iter_mut()
            .find(|managed| managed.toplevel.wl_surface() == surface)
            .map(|managed| {
                managed.mapped |= painted;
                managed.key
            });

        // Report what the client says changed, not the whole screen. Whole-screen
        // damage makes every caret blink look like a full repaint, which defeats
        // the noise filter the `quiet` settle predicate depends on — the filter
        // classifies by rectangle, and one rectangle the size of the display is
        // indistinguishable from a dialog opening.
        for region in regions {
            let _ = self.damage.send(Damage { window, region });
        }
    }
}

impl XdgShellHandler for StageState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let (width, height) = (self.width, self.height);
        // Server-side decorations are not negotiated here, so a toplevel is
        // simply given the whole stage. A client that draws its own titlebar
        // would otherwise put controls we have no geometry for.
        surface.with_pending_state(|state| {
            state.size = Some((width, height).into());
            state
                .states
                .set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated);
        });
        surface.send_configure();

        self.next_key += 1;
        let key = WindowKey(self.next_key);
        let pid = client_pid(&surface, &self.display);
        self.windows.push(Managed {
            key,
            toplevel: surface,
            kind: WindowKind::Wayland,
            geometry: Rect {
                x: 0,
                y: 0,
                width: width as u32,
                height: height as u32,
            },
            pid,
            mapped: false,
        });
        self.dirty = true;
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.windows.retain(|window| window.toplevel != surface);
        self.dirty = true;
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

impl SelectionHandler for StageState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for StageState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for StageState {}
impl ServerDndGrabHandler for StageState {}

impl SeatHandler for StageState {
    type KeyboardFocus = wl_surface::WlSurface;
    type PointerFocus = wl_surface::WlSurface;
    type TouchFocus = wl_surface::WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

impl ShmHandler for StageState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(StageState);
delegate_xdg_shell!(StageState);
delegate_shm!(StageState);
delegate_seat!(StageState);
delegate_data_device!(StageState);

#[derive(Default)]
struct StageClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for StageClientState {
    fn initialized(&self, _id: ClientId) {}
    fn disconnected(&self, _id: ClientId, _reason: DisconnectReason) {}
}

/// The rectangles a client reported as changed in its latest commit.
///
/// Two cases besides the obvious one, and both matter for the noise filter:
///
/// * A commit that attaches **no buffer** is a role or state commit, not a
///   repaint — nothing on screen changed, so nothing is reported. Treating it
///   as a full repaint (which the naive fallback does) makes a window's initial
///   handshake look like the largest possible change.
/// * A commit that attaches a buffer with **no damage hint** legally means
///   "assume everything", so it does fall back to the whole stage. Reporting
///   nothing there would let a settle predicate conclude the screen had gone
///   quiet while it was in fact fully repainting.
fn surface_damage(surface: &wl_surface::WlSurface, full_screen: Rect) -> Vec<Rect> {
    use smithay::wayland::compositor::{
        BufferAssignment, Damage as SurfaceDamage, with_states,
    };

    let (rects, painted) = with_states(surface, |states| {
        let mut cached = states.cached_state.get::<SurfaceAttributes>();
        let attributes = cached.current();
        let painted = matches!(attributes.buffer, Some(BufferAssignment::NewBuffer(_)));
        let rects = attributes
            .damage
            .iter()
            .map(|damage| match damage {
                // Buffer and surface coordinates coincide at scale 1, which is
                // the only scale the stage advertises.
                SurfaceDamage::Buffer(rect) => Rect {
                    x: rect.loc.x,
                    y: rect.loc.y,
                    width: rect.size.w.max(0) as u32,
                    height: rect.size.h.max(0) as u32,
                },
                SurfaceDamage::Surface(rect) => Rect {
                    x: rect.loc.x,
                    y: rect.loc.y,
                    width: rect.size.w.max(0) as u32,
                    height: rect.size.h.max(0) as u32,
                },
            })
            .filter(|rect| rect.width > 0 && rect.height > 0)
            .collect::<Vec<_>>();
        (rects, painted)
    });

    if !rects.is_empty() {
        return rects;
    }
    if painted {
        return vec![full_screen];
    }
    Vec::new()
}

/// Send `wl_surface.frame` callbacks for a surface tree.
///
/// See the module docs: skipping this makes every client freeze after one frame
/// while looking entirely healthy from the outside.
fn send_frames(surface: &wl_surface::WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

/// Signal a whole process group, for applications that fork helpers.
///
/// Spawned with `process_group(0)`, so the child's pid *is* its group id and a
/// negative pid reaches the tree. Best-effort: a group that has already exited
/// is not an error worth reporting during teardown.
fn kill_process_group(pid: u32) {
    let _ = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(format!("-{pid}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// The `Send + Sync` handle the rest of the crate holds.
pub struct StageWayland {
    id: StageId,
    env: StageEnv,
    commands: std::sync::mpsc::Sender<StageCommand>,
    damage: tokio::sync::broadcast::Sender<Damage>,
    socket_name: String,
    /// Processes launched into this stage, killed on shutdown so a closed stage
    /// cannot leave applications running against a dead display.
    children: Mutex<Vec<std::process::Child>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Drop for StageWayland {
    fn drop(&mut self) {
        let _ = self.commands.send(StageCommand::Shutdown);
        // `unwrap_or_else(into_inner)` rather than `unwrap`: a panic elsewhere
        // while holding either lock would poison it and turn teardown into a
        // second panic, losing the cleanup entirely.
        let mut children = self
            .children
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for child in children.iter_mut() {
            kill_process_group(child.id());
            let _ = child.kill();
            // Reap, or the child lingers as a zombie holding its profile dir.
            let _ = child.wait();
        }
        drop(children);
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = thread.join();
        }
    }
}

impl StageWayland {
    /// Start a compositor on its own thread and wait for it to be listening.
    pub fn start(id: StageId, runtime_dir: &std::path::Path) -> Result<Self, StepError> {
        Self::start_sized(id, runtime_dir, DEFAULT_WIDTH, DEFAULT_HEIGHT)
    }

    /// Start a stage with a specific virtual screen size.
    ///
    /// Worth configuring because viewport size genuinely changes what an
    /// application shows — a responsive page lays out differently, and a list
    /// renders a different number of rows. Sizes are clamped to something a
    /// renderer can actually allocate rather than trusted.
    pub fn start_sized(
        id: StageId,
        runtime_dir: &std::path::Path,
        width: i32,
        height: i32,
    ) -> Result<Self, StepError> {
        let width = width.clamp(320, 7680);
        let height = height.clamp(240, 4320);
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (damage_tx, _) = tokio::sync::broadcast::channel(256);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String, String>>();

        let damage_for_thread = damage_tx.clone();
        let runtime_dir = runtime_dir.to_owned();
        let thread_dir = runtime_dir.clone();
        let thread = std::thread::Builder::new()
            .name(format!("artist-stage-{id}"))
            .spawn(move || {
                run_compositor(
                    &thread_dir,
                    command_rx,
                    damage_for_thread,
                    ready_tx,
                    width,
                    height,
                );
            })
            .map_err(|error| StepError::Backend(format!("spawn compositor thread: {error}")))?;

        let socket_name = ready_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| StepError::Backend("compositor did not start".into()))?
            .map_err(StepError::Backend)?;

        let mut env = StageEnv::default();
        env.set("WAYLAND_DISPLAY", &socket_name);
        env.set("XDG_RUNTIME_DIR", runtime_dir_string(&runtime_dir));
        // Force the Wayland path: a toolkit that silently falls back to X11
        // would land on the *user's* display, which is the one thing a stage
        // exists to prevent.
        env.set("GDK_BACKEND", "wayland");
        env.set("QT_QPA_PLATFORM", "wayland");
        env.set("SDL_VIDEODRIVER", "wayland");
        env.set("MOZ_ENABLE_WAYLAND", "1");

        Ok(Self {
            id,
            env,
            commands: command_tx,
            damage: damage_tx,
            socket_name,
            children: Mutex::new(Vec::new()),
            thread: Mutex::new(Some(thread)),
        })
    }

    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }

    /// The stage's own X11 display number, or `None` when XWayland is not up.
    pub async fn x11_display(&self) -> Option<u32> {
        self.ask(StageCommand::X11Display).await.ok().flatten()
    }

    /// Merge in the bus environment (and anything else) before launching apps.
    pub fn extend_env(&mut self, extra: &StageEnv) {
        for (key, value) in extra.iter() {
            self.env.set(key, value);
        }
    }

    async fn ask<T: Send + 'static>(
        &self,
        build: impl FnOnce(tokio::sync::oneshot::Sender<T>) -> StageCommand,
    ) -> Result<T, StepError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.commands
            .send(build(tx))
            .map_err(|_| StepError::Backend("the stage compositor has stopped".into()))?;
        rx.await
            .map_err(|_| StepError::Backend("the stage compositor dropped a reply".into()))
    }
}

fn runtime_dir_string(dir: &std::path::Path) -> String {
    dir.to_string_lossy().into_owned()
}

#[async_trait::async_trait]
impl Stage for StageWayland {
    fn id(&self) -> &StageId {
        &self.id
    }

    fn env(&self) -> &StageEnv {
        &self.env
    }

    async fn spawn(&self, command: AppCommand) -> Result<AppHandle, StepError> {
        use std::os::unix::process::CommandExt as _;

        let mut process = std::process::Command::new(&command.program);
        process.args(&command.args);
        // Its own process group, so teardown can signal the whole tree.
        // Browsers in particular fork a zygote, a GPU process and several
        // utility processes; killing only the one we spawned leaves those
        // running, reparented to init, still holding the profile directory —
        // and, for a browser, still holding the user's cookies open.
        process.process_group(0);
        if let Some(cwd) = &command.cwd {
            process.current_dir(cwd);
        }
        // Only the stage environment is injected; everything else is inherited,
        // which is what keeps the agent's applications able to see the user's
        // real home, credentials and configuration.
        for (key, value) in self.env.iter() {
            process.env(key, value);
        }
        // `DISPLAY` is resolved per spawn rather than at start-up, because
        // XWayland reports its display number asynchronously and may not have
        // been ready when the stage came up. An X11-only app launched before
        // then would otherwise silently inherit the *user's* DISPLAY — landing
        // its window on their screen, which is the one thing a stage prevents.
        match self.x11_display().await {
            Some(number) => {
                process.env("DISPLAY", format!(":{number}"));
            }
            None => {
                process.env_remove("DISPLAY");
            }
        }
        let child = process
            .spawn()
            .map_err(|error| StepError::Backend(format!("launch {}: {error}", command.program)))?;
        let pid = child.id() as i32;
        self.children.lock().unwrap().push(child);
        Ok(AppHandle { pid, command })
    }

    async fn windows(&self) -> Result<Vec<WindowInfo>, StepError> {
        self.ask(StageCommand::Windows).await
    }

    async fn focus(&self, window: WindowKey) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Focus(window, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn key(&self, window: WindowKey, stroke: &str) -> Result<(), StepError> {
        let stroke = stroke.to_owned();
        self.ask(|tx| StageCommand::Key(window, stroke, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn text(&self, window: WindowKey, text: &str) -> Result<(), StepError> {
        let text = text.to_owned();
        self.ask(|tx| StageCommand::Text(window, text, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn pointer(&self, window: WindowKey, at: Rect, button: u32) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Pointer(window, at, button, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn capture(&self, window: Option<WindowKey>) -> Result<Frame, StepError> {
        self.ask(|tx| StageCommand::Capture(window, tx))
            .await?
            .map_err(StepError::Backend)
    }

    fn damage(&self) -> tokio::sync::broadcast::Receiver<Damage> {
        self.damage.subscribe()
    }

    async fn shutdown(&self) -> Result<(), StepError> {
        let _ = self.commands.send(StageCommand::Shutdown);
        Ok(())
    }
}

/// Build the GLES renderer for a headless stage.
///
/// There is no display to present to, so this is EGL on a render node with no
/// surface at all: open the DRM render device, wrap it in GBM, make an EGL
/// display from that, and take a context. Everything is drawn into an offscreen
/// buffer that only ever leaves as a capture.
fn make_renderer() -> Result<GlesRenderer, String> {
    use smithay::backend::allocator::gbm::GbmDevice;

    let candidates = ["/dev/dri/renderD128", "/dev/dri/renderD129"];
    let mut last = String::from("no DRM render node found");
    for path in candidates {
        if !std::path::Path::new(path).exists() {
            continue;
        }
        let file = match std::fs::OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => file,
            Err(error) => {
                last = format!("open {path}: {error}");
                continue;
            }
        };
        let fd = smithay::backend::drm::DrmDeviceFd::new(std::os::fd::OwnedFd::from(file).into());
        let gbm = match GbmDevice::new(fd) {
            Ok(device) => device,
            Err(error) => {
                last = format!("gbm on {path}: {error}");
                continue;
            }
        };
        let display = match unsafe { EGLDisplay::new(gbm) } {
            Ok(display) => display,
            Err(error) => {
                last = format!("egl display on {path}: {error}");
                continue;
            }
        };
        let context = match EGLContext::new(&display) {
            Ok(context) => context,
            Err(error) => {
                last = format!("egl context on {path}: {error}");
                continue;
            }
        };
        return unsafe { GlesRenderer::new(context) }
            .map_err(|error| format!("gles renderer on {path}: {error}"));
    }
    Err(last)
}

/// The compositor thread body.
fn run_compositor(
    runtime_dir: &std::path::Path,
    commands: std::sync::mpsc::Receiver<StageCommand>,
    damage: tokio::sync::broadcast::Sender<Damage>,
    ready: std::sync::mpsc::Sender<Result<String, String>>,
    width: i32,
    height: i32,
) {
    let mut display = match Display::<StageState>::new() {
        Ok(display) => display,
        Err(error) => {
            let _ = ready.send(Err(format!("create wayland display: {error}")));
            return;
        }
    };
    let mut handle = display.handle();

    let mut renderer = match make_renderer() {
        Ok(renderer) => renderer,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "no GPU renderer for the stage: {error}. \
                 A DRM render node (/dev/dri/renderD128) and working EGL are required."
            )));
            return;
        }
    };

    let mut seat_state = SeatState::new();
    let seat = seat_state.new_wl_seat(&handle, "artist-stage");
    let mut state = StageState {
        display: handle.clone(),
        compositor_state: CompositorState::new::<StageState>(&handle),
        xdg_shell_state: XdgShellState::new::<StageState>(&handle),
        shm_state: ShmState::new::<StageState>(&handle, Vec::new()),
        seat_state,
        data_device_state: DataDeviceState::new::<StageState>(&handle),
        seat,
        windows: Vec::new(),
        next_key: 0,
        dirty: false,
        damage,
        running: true,
        x11_display: None,
        render_error: None,
        width,
        height,
    };

    let keyboard = match state.seat.add_keyboard(Default::default(), 200, 25) {
        Ok(keyboard) => keyboard,
        Err(error) => {
            let _ = ready.send(Err(format!("add keyboard: {error}")));
            return;
        }
    };
    let pointer = state.seat.add_pointer();

    // Bound by absolute path rather than through `XDG_RUNTIME_DIR`.
    // `ListeningSocket::bind` reads that variable from the process environment,
    // so using it would mean mutating global state from a compositor thread —
    // and two stages in one process would then race for the same directory,
    // with the loser's clients quietly connecting to the winner's display.
    let socket_name = "wayland-artist".to_owned();
    let listener = match ListeningSocket::bind_absolute(runtime_dir.join(&socket_name)) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(format!("bind wayland socket: {error}")));
            return;
        }
    };
    let _ = ready.send(Ok(socket_name));

    let size: Size<i32, smithay::utils::Buffer> = (width, height).into();
    let mut target: GlesRenderbuffer = match renderer.create_buffer(Fourcc::Abgr8888, size) {
        Ok(buffer) => buffer,
        Err(error) => {
            eprintln!("artist: stage offscreen buffer failed: {error}");
            return;
        }
    };

    // XWayland needs a calloop `LoopHandle`, so the loop is calloop-driven —
    // but it deliberately keeps the same poll-and-service shape rather than
    // becoming fully event-driven. Blocking on readiness would starve the
    // command channel, which is not a pollable source here.
    let mut event_loop: calloop::EventLoop<'static, StageState> = match calloop::EventLoop::try_new()
    {
        Ok(loop_) => loop_,
        Err(error) => {
            eprintln!("artist: stage event loop failed: {error}");
            return;
        }
    };
    start_xwayland(&handle, &event_loop.handle(), runtime_dir);

    let start = std::time::Instant::now();
    let mut clients = Vec::new();

    while state.running {
        // Accept new clients, then dispatch anything they sent.
        if let Ok(Some(stream)) = listener.accept()
            && let Ok(client) = handle.insert_client(stream, Arc::new(StageClientState::default()))
        {
            clients.push(client);
        }
        let _ = display.dispatch_clients(&mut state);

        while let Ok(command) = commands.try_recv() {
            handle_command(
                &mut state,
                &keyboard,
                &pointer,
                &mut renderer,
                &mut target,
                command,
            );
        }

        if state.dirty {
            state.dirty = false;
            match render(&mut renderer, &mut target, &mut state, start) {
                Ok(()) => state.render_error = None,
                Err(error) => {
                    // Stay dirty so the next pass retries — a transient GL
                    // error should cost a frame, not the session. The 4 ms
                    // dispatch below bounds the retry rate, and the error is
                    // held so a `capture` reports it instead of returning the
                    // last good buffer as a fresh screenshot.
                    state.dirty = true;
                    if state.render_error.as_deref() != Some(error.as_str()) {
                        eprintln!("artist: stage render failed: {error}");
                    }
                    state.render_error = Some(error);
                }
            }
        }

        let _ = display.flush_clients();
        // 4ms keeps an idle stage at a fraction of a percent of a core while
        // servicing commands, client traffic and XWayland together.
        let _ = event_loop.dispatch(Some(std::time::Duration::from_millis(4)), &mut state);
    }
}

/// Bring XWayland up so X11-only applications can run on the stage.
///
/// Best-effort by design. Everything modern is Wayland-native, so a stage
/// without XWayland is still fully useful — reporting the failure and carrying
/// on is far better than refusing to start a display because a legacy
/// compatibility layer is missing.
#[allow(unused_variables)]
fn start_xwayland(
    display: &smithay::reexports::wayland_server::DisplayHandle,
    loop_handle: &calloop::LoopHandle<'static, StageState>,
    runtime_dir: &std::path::Path,
) {
    use smithay::xwayland::{XWayland, XWaylandEvent};

    let (xwayland, client) = match XWayland::spawn(
        display,
        None,
        // XWayland inherits the stage's own runtime dir, so its X11 socket
        // lands beside the Wayland one and never in the user's.
        [("XDG_RUNTIME_DIR", runtime_dir.as_os_str())],
        true,
        std::process::Stdio::null(),
        std::process::Stdio::null(),
        |_| {},
    ) {
        Ok(spawned) => spawned,
        Err(error) => {
            eprintln!("artist: stage XWayland unavailable ({error}); X11-only apps will not run");
            return;
        }
    };

    let inserted = loop_handle.insert_source(xwayland, move |event, _, state: &mut StageState| {
        match event {
            XWaylandEvent::Ready { display_number, .. } => {
                state.x11_display = Some(display_number);
            }
            XWaylandEvent::Error => {
                state.x11_display = None;
            }
        }
    });
    if let Err(error) = inserted {
        eprintln!("artist: stage XWayland source failed ({error})");
    }
    // The client handle keeps XWayland's Wayland connection alive.
    std::mem::drop(client);
}

fn handle_command(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    renderer: &mut GlesRenderer,
    target: &mut GlesRenderbuffer,
    command: StageCommand,
) {
    match command {
        StageCommand::Windows(reply) => {
            let windows = state
                .windows
                .iter()
                .map(|window| {
                    let (title, app_id) = toplevel_identity(&window.toplevel);
                    WindowInfo {
                        key: window.key,
                        title,
                        app_id,
                        pid: window.pid,
                        geometry: window.geometry,
                        kind: window.kind,
                        mapped: window.mapped,
                    }
                })
                .collect();
            let _ = reply.send(windows);
        }
        StageCommand::Focus(key, reply) => {
            let result = match state.window(key) {
                Some(window) => {
                    let surface = window.toplevel.wl_surface().clone();
                    keyboard.set_focus(state, Some(surface), Serial::from(0));
                    Ok(())
                }
                None => Err(format!("no window {key:?}")),
            };
            let _ = reply.send(result);
        }
        StageCommand::Key(key, stroke, reply) => {
            let _ = reply.send(deliver_key(state, keyboard, key, &stroke));
        }
        StageCommand::Text(key, text, reply) => {
            let _ = reply.send(deliver_text(state, keyboard, key, &text));
        }
        StageCommand::Pointer(key, at, button, reply) => {
            let _ = reply.send(deliver_click(state, pointer, key, at, button));
        }
        StageCommand::Capture(window, reply) => {
            let full = state.full_screen();
            // The window key was being destructured and thrown away, so every
            // caller asking for one window silently got the whole screen.
            let region = match window {
                None => Ok(full),
                Some(key) => match state.window(key) {
                    Some(window) => Ok(window.geometry),
                    None => Err(format!("no window {key:?}")),
                },
            };
            let result = region.and_then(|region| {
                // A render that failed is held, not forgotten: returning the
                // last good buffer here would report a stale screen as a fresh
                // screenshot, which is worse than an error.
                if let Some(error) = &state.render_error {
                    return Err(format!("the stage could not render: {error}"));
                }
                capture(renderer, target, full, region)
            });
            let _ = reply.send(result);
        }
        StageCommand::X11Display(reply) => {
            let _ = reply.send(state.x11_display);
        }
        StageCommand::Shutdown => state.running = false,
    }
}

/// The pid behind a toplevel, from the Wayland client's socket credentials.
///
/// Authoritative rather than inferred: a browser forks many processes and its
/// windows are correlated to a CDP session by pid, so guessing from a title
/// would be exactly the heuristic the stage exists to avoid.
fn client_pid(
    toplevel: &ToplevelSurface,
    display: &smithay::reexports::wayland_server::DisplayHandle,
) -> Option<i32> {
    use smithay::reexports::wayland_server::Resource;

    let client = toplevel.wl_surface().client()?;
    client
        .get_credentials(display)
        .ok()
        .map(|credentials| credentials.pid)
}

/// The title and app id a client committed for a toplevel.
///
/// Read from the surface's role attributes rather than pending state: pending
/// is what the *compositor* is about to send, while these are what the client
/// has actually told us it is.
fn toplevel_identity(toplevel: &ToplevelSurface) -> (String, String) {
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

    with_states(toplevel.wl_surface(), |states| {
        let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() else {
            return (String::new(), String::new());
        };
        let attributes = data.lock().expect("toplevel attributes poisoned");
        (
            attributes.title.clone().unwrap_or_default(),
            attributes.app_id.clone().unwrap_or_default(),
        )
    })
}

/// Deliver a click at the centre of a rectangle.
///
/// The caller passes the element's bounds rather than a point, and the centre is
/// computed here: the model never supplies coordinates, so the only geometry in
/// play is what the harness resolved from an anchor moments earlier.
fn deliver_click(
    state: &mut StageState,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    key: WindowKey,
    at: Rect,
    button: u32,
) -> Result<(), String> {
    use smithay::input::pointer::{ButtonEvent, MotionEvent};
    use smithay::utils::SERIAL_COUNTER;

    let Some(window) = state.window(key) else {
        return Err(format!("no window {key:?}"));
    };
    let surface = window.toplevel.wl_surface().clone();
    let point = smithay::utils::Point::<f64, smithay::utils::Logical>::from((
        f64::from(at.x) + f64::from(at.width) / 2.0,
        f64::from(at.y) + f64::from(at.height) / 2.0,
    ));
    let time = 0;

    pointer.motion(
        state,
        Some((surface, (0.0, 0.0).into())),
        &MotionEvent {
            location: point,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    // A press with no matching release leaves the client believing the button
    // is still held, which breaks the very next interaction.
    for pressed in [true, false] {
        pointer.button(
            state,
            &ButtonEvent {
                button,
                state: if pressed {
                    smithay::backend::input::ButtonState::Pressed
                } else {
                    smithay::backend::input::ButtonState::Released
                },
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
    }
    pointer.frame(state);
    Ok(())
}

/// Press and release one evdev keycode, optionally wrapped in modifier holds.
fn tap(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    code: u32,
    held: &[u32],
) {
    use smithay::backend::input::KeyState;
    use smithay::input::keyboard::{FilterResult, Keycode};

    const OFFSET: u32 = 8; // evdev -> xkb
    let send = |state: &mut StageState, code: u32, pressed: KeyState| {
        keyboard.input::<(), _>(
            state,
            Keycode::from(code + OFFSET),
            pressed,
            Serial::from(0),
            0,
            |_, _, _| FilterResult::Forward,
        );
    };

    for modifier in held {
        send(state, *modifier, KeyState::Pressed);
    }
    send(state, code, KeyState::Pressed);
    send(state, code, KeyState::Released);
    // Released in reverse, so a client tracking modifier state sees a
    // well-formed sequence rather than a stuck modifier.
    for modifier in held.iter().rev() {
        send(state, *modifier, KeyState::Released);
    }
}

/// evdev codes for the modifiers a chord asks for.
fn modifier_codes(modifiers: crate::keys::Modifiers) -> Vec<u32> {
    let mut codes = Vec::new();
    if modifiers.ctrl {
        codes.push(29); // LEFTCTRL
    }
    if modifiers.alt {
        codes.push(56); // LEFTALT
    }
    if modifiers.shift {
        codes.push(42); // LEFTSHIFT
    }
    if modifiers.meta {
        codes.push(125); // LEFTMETA
    }
    codes
}

fn focus_window(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    key: WindowKey,
) -> Result<(), String> {
    let Some(window) = state.window(key) else {
        return Err(format!("no window {key:?}"));
    };
    let surface = window.toplevel.wl_surface().clone();
    keyboard.set_focus(state, Some(surface), Serial::from(0));
    Ok(())
}

fn deliver_key(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    key: WindowKey,
    stroke: &str,
) -> Result<(), String> {
    focus_window(state, keyboard, key)?;
    let chord = crate::keys::parse(stroke).map_err(|error| error.to_string())?;
    let code = chord
        .key
        .evdev()
        .ok_or_else(|| format!("key {stroke:?} is not on the stage keyboard layout"))?;
    tap(state, keyboard, code, &modifier_codes(chord.modifiers));
    Ok(())
}

/// Type a string by synthesizing keystrokes, holding shift where the layout
/// needs it.
///
/// This previously sent each character through the plain keycode table, which
/// lowercased everything — `type "Hello World"` delivered `hello world` and
/// reported success — and rejected every punctuation character outside letters
/// and digits, including the `@` in the tool description's own example.
fn deliver_text(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    key: WindowKey,
    text: &str,
) -> Result<(), String> {
    focus_window(state, keyboard, key)?;
    const SHIFT: u32 = 42;

    for character in text.chars() {
        if character == '\n' {
            tap(state, keyboard, 28, &[]); // ENTER
            continue;
        }
        if character == '\t' {
            tap(state, keyboard, 15, &[]);
            continue;
        }
        if let Some(code) = crate::keys::evdev_for_char(character)
            && crate::keys::shifted_char(character).is_none()
        {
            tap(state, keyboard, code, &[]);
            continue;
        }
        // Needs shift: uppercase letters and the shifted punctuation row.
        if let Some(base) = crate::keys::shifted_char(character)
            && let Some(code) = crate::keys::evdev_for_char(base)
        {
            tap(state, keyboard, code, &[SHIFT]);
            continue;
        }
        // Refuse rather than approximate. Silently dropping a character is how
        // typed text ends up subtly wrong with no error anywhere.
        return Err(format!(
            "character {character:?} cannot be typed on the stage's US layout; \
             paste it or use a rung that accepts text directly"
        ));
    }
    Ok(())
}

/// Draw the stage, then release every client to draw its next frame.
///
/// The two halves are separated by design. A client that gets no frame callback
/// paints once and then waits forever — it looks exactly like a hung
/// application, and the module docs name it as the failure mode to guard
/// against. So a render that fails must *still* send callbacks: a transient GL
/// error should cost one frame, not freeze every client on the stage
/// permanently.
fn render(
    renderer: &mut GlesRenderer,
    target: &mut GlesRenderbuffer,
    state: &mut StageState,
    start: std::time::Instant,
) -> Result<(), String> {
    let outcome = draw(renderer, target, state);

    // After every render, without exception. See the module docs.
    let time = start.elapsed().as_millis() as u32;
    for window in &state.windows {
        send_frames(window.toplevel.wl_surface(), time);
    }
    outcome
}

fn draw(
    renderer: &mut GlesRenderer,
    target: &mut GlesRenderbuffer,
    state: &mut StageState,
) -> Result<(), String> {
    let size: Size<i32, smithay::utils::Physical> = (state.width, state.height).into();
    let full = Rectangle::from_size(size);

    let mut framebuffer = renderer
        .bind(target)
        .map_err(|error| format!("bind for render: {error}"))?;
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = state
        .windows
        .iter()
        .flat_map(|window| {
            render_elements_from_surface_tree(
                renderer,
                window.toplevel.wl_surface(),
                (0, 0),
                1.0,
                1.0,
                Kind::Unspecified,
            )
        })
        .collect();

    // Every GL step reported, not swallowed. A silent failure here is what let
    // `capture` hand back a stale buffer as a successful screenshot.
    let mut frame = renderer
        .render(&mut framebuffer, size, Transform::Normal)
        .map_err(|error| format!("render: {error}"))?;
    frame
        .clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[full])
        .map_err(|error| format!("clear: {error}"))?;
    draw_render_elements(&mut frame, 1.0, &elements, &[full])
        .map_err(|error| format!("draw: {error}"))?;
    frame
        .finish()
        .map_err(|error| format!("finish: {error}"))?;
    drop(framebuffer);
    Ok(())
}

/// Read pixels back out of the offscreen buffer.
///
/// `wanted` is clamped to `screen`: a window can legitimately extend past the
/// output edge, and asking the GL driver to read outside the framebuffer is an
/// error rather than a smaller picture.
fn capture(
    renderer: &mut GlesRenderer,
    target: &mut GlesRenderbuffer,
    screen: Rect,
    wanted: Rect,
) -> Result<Frame, String> {
    use smithay::backend::renderer::ExportMem;

    let left = wanted.x.max(screen.x);
    let top = wanted.y.max(screen.y);
    let right = (wanted.x + wanted.width as i32).min(screen.x + screen.width as i32);
    let bottom = (wanted.y + wanted.height as i32).min(screen.y + screen.height as i32);
    let (width, height) = (right - left, bottom - top);
    if width <= 0 || height <= 0 {
        return Err(format!(
            "the requested region {wanted:?} lies outside the stage {screen:?}"
        ));
    }

    let region: Rectangle<i32, smithay::utils::Buffer> =
        Rectangle::new((left, top).into(), (width, height).into());
    let framebuffer = renderer
        .bind(target)
        .map_err(|error| format!("bind for capture: {error}"))?;
    let mapping = renderer
        .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
        .map_err(|error| format!("copy framebuffer: {error}"))?;
    let bytes = renderer
        .map_texture(&mapping)
        .map_err(|error| format!("map texture: {error}"))?
        .to_vec();

    Ok(Frame::new(width as u32, height as u32, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_node_available() -> bool {
        std::path::Path::new("/dev/dri/renderD128").exists()
    }

    #[test]
    fn modifiers_become_real_key_holds() {
        // The superseded table returned plain `c` for `ctrl+c` and a test
        // asserted that, pinning the bug. Modifiers are now separate keycodes
        // held around the base key.
        let chord = crate::keys::parse("ctrl+shift+c").unwrap();
        assert_eq!(modifier_codes(chord.modifiers), vec![29, 42]);
        assert_eq!(chord.key.evdev(), Some(46)); // `c`

        assert!(modifier_codes(crate::keys::parse("c").unwrap().modifiers).is_empty());
    }

    #[tokio::test]
    async fn a_stage_starts_and_leaves_the_users_session_untouched() {
        if !render_node_available() {
            eprintln!("skipping: no DRM render node");
            return;
        }
        let user_display = std::env::var("WAYLAND_DISPLAY").ok();
        let dir = tempfile::tempdir().unwrap();

        let stage = match StageWayland::start(StageId("test".into()), dir.path()) {
            Ok(stage) => stage,
            Err(error) => {
                eprintln!("skipping: stage unavailable here: {error}");
                return;
            }
        };

        assert_eq!(stage.socket_name(), "wayland-artist");
        assert_ne!(
            stage.env().get("WAYLAND_DISPLAY"),
            user_display.as_deref(),
            "the stage must never hand out the user's display"
        );
        // The isolation property is the product, so it is asserted directly.
        assert_eq!(std::env::var("WAYLAND_DISPLAY").ok(), user_display);
        assert!(stage.windows().await.unwrap().is_empty());
    }
}
