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
//!
//! **Globals are a compatibility surface, not a feature list.** A client
//! decides what it is capable of by looking at the registry, and a missing
//! global is not a graceful degradation — it is usually a hard exit or a
//! silent fall back to a slower path. Three of the ones here are load-bearing
//! for that reason:
//!
//! * `zwp_linux_dmabuf` is how a GPU client hands us a texture it already has.
//!   Without it Chromium, any Vulkan or GL application, and Waydroid's Android
//!   surfaces either read back through shared memory every frame or refuse to
//!   start. It is advertised at version 4 with a feedback tranche naming our
//!   render node, so a client allocates on the device we can actually import
//!   from rather than guessing.
//! * `wl_output` is how a client learns the screen exists. Toolkits that find
//!   no output pick a default size, skip scale setup, and in several cases
//!   never map a window at all — which reads from the outside as "the stage is
//!   broken", not "an optional global is absent".
//! * `xdg_decoration` lets us *insist* on server-side decorations. A client
//!   drawing its own titlebar puts a close button on the stage that no rung
//!   knows the geometry of, and the agent would be one stray click from
//!   destroying the window it is working in.
//!
//! `wp_viewporter` and `wp_presentation` are the quieter two: the first because
//! clients that scale their own buffers commit a viewport and expect it
//! honoured, the second because a client that asks for presentation feedback
//! and never receives it can throttle itself waiting. Advertising a global we
//! do not service would be worse than not advertising it, so feedback is
//! answered on every render.

use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::{draw_render_elements, on_commit_buffer_handler};
use smithay::backend::renderer::{Bind, Color32F, Frame as RenderFrame, ImportDma, Renderer};
use smithay::desktop::utils::{OutputPresentationFeedback, take_presentation_feedback_surface_tree};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface};
use smithay::reexports::wayland_server::{Client, Display, ListeningSocket};
use smithay::utils::{Rectangle, Serial, Size, Transform};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, TraversalAction,
    with_surface_tree_downward,
};
use smithay::wayland::dmabuf::{
    DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::presentation::{PresentationState, Refresh};
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::desktop::{
    PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, PopupUngrabStrategy,
    find_popup_root_surface,
};
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::tablet_manager::{
    TabletDescriptor, TabletManagerState, TabletSeatHandler, TabletSeatTrait,
};
use smithay::wayland::viewporter::ViewporterState;
use smithay::{
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output,
    delegate_presentation, delegate_seat, delegate_shm, delegate_viewporter,
    delegate_xdg_decoration, delegate_xdg_shell, delegate_tablet_manager,
};

use crate::model::{Frame, Rect};
use crate::program::StepError;
use crate::stage::{
    AppCommand, AppHandle, Damage, Gesture, Stage, StageEnv, StageId, WindowInfo, WindowKey,
    WindowKind,
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

/// The refresh rate the stage reports, in mHz — 60 Hz.
///
/// Nothing here is actually paced by it: rendering happens when a client
/// commits damage and not otherwise. It is reported because a client that reads
/// a refresh of zero may treat the output as disabled, and because presentation
/// feedback carries the figure a client uses to decide how far ahead to draw.
const STAGE_REFRESH_MHZ: i32 = 60_000;

/// `CLOCK_MONOTONIC`, the clock presentation timestamps are on.
///
/// It is the only one that cannot step backwards, which is exactly the property
/// a client pacing itself off these timestamps depends on.
const CLOCK_MONOTONIC: u32 = 1;

/// How many buffers the stage cycles through.
///
/// Three, not two. Two is enough to stop a viewer seeing a frame *being drawn*,
/// but not enough to stop the stage drawing into one the compositor is *still
/// displaying* — it has only one other choice, and if that one is held there is
/// nowhere to go. A third gives the renderer somewhere to be while the
/// compositor finishes with the other two, which is what removes the last of
/// the tearing under rapid damage such as pointer motion.
pub const RENDER_BUFFERS: usize = 3;

/// The stage's buffers, which one holds a finished frame, and which are still
/// held by a viewer.
///
/// `held` is written by the viewer and read by the compositor thread: a bit per
/// buffer, set when it is attached and cleared on `wl_buffer.release`. Without
/// it the renderer has no way to know a buffer is on screen, which is exactly
/// how a half-drawn frame reached the display.
type RenderTargets = (
    [Dmabuf; RENDER_BUFFERS],
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<[std::sync::atomic::AtomicBool; RENDER_BUFFERS]>,
);
type RenderTargetReply = tokio::sync::oneshot::Sender<RenderTargets>;

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
        crate::stage::Pointing,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    /// One primitive of a pointer sequence.
    ///
    /// Split out for the same reason [`StageCommand::Touch`] is: a drag is a
    /// press, real motion over real time, and a release. The motion is what
    /// makes it a drag rather than two clicks — every toolkit starts dragging
    /// from a movement threshold — and sleeping between the points has to
    /// happen on the caller's task, never on the compositor thread.
    Pointing(
        PointerStep,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    Scroll(
        WindowKey,
        Rect,
        i32,
        crate::program::Axis,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    /// Hold a key down, or let it go. `bool` is `pressed`.
    KeyHold(
        WindowKey,
        String,
        bool,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    ClipboardSet(String, tokio::sync::oneshot::Sender<Result<(), String>>),
    ClipboardGet(tokio::sync::oneshot::Sender<Result<ClipboardRead, String>>),
    CloseWindow(WindowKey, tokio::sync::oneshot::Sender<Result<(), String>>),
    Resize(u32, u32, tokio::sync::oneshot::Sender<Result<(), String>>),
    /// Release every button, key and contact this seat is holding.
    Relax(tokio::sync::oneshot::Sender<Result<(), String>>),
    /// One primitive of a stylus stroke.
    Stylus(StylusStep, tokio::sync::oneshot::Sender<Result<(), String>>),
    /// One primitive of a touch sequence.
    ///
    /// Gestures are assembled by the *proxy*, not here, because the timing is
    /// the gesture: a long press is a contact held for half a second and a fling
    /// is one moved over sixteen-millisecond steps. Sleeping for either on the
    /// compositor thread would stall rendering and the command channel for
    /// exactly as long as the gesture lasts — so the thread only ever handles
    /// instantaneous events, and the waiting happens on the caller's task.
    Touch(TouchStep, tokio::sync::oneshot::Sender<Result<(), String>>),
    Capture(
        Option<WindowKey>,
        tokio::sync::oneshot::Sender<Result<Frame, String>>,
    ),
    /// The stage's X11 display number, once XWayland is ready.
    X11Display(tokio::sync::oneshot::Sender<Option<u32>>),
    /// Both render buffers and the index of the one holding a finished frame.
    RenderTarget(RenderTargetReply),
    Shutdown,
}

/// One instantaneous step of a touch sequence.
///
/// `slot` is the contact identifier: 0 and 1 are two fingers of a pinch, and a
/// client tracks them independently. Reusing a slot that is still down is what
/// makes a two-finger gesture arrive as one confused finger.
#[derive(Clone, Copy, Debug)]
enum TouchStep {
    Down {
        window: WindowKey,
        at: (i32, i32),
        slot: u32,
    },
    Motion {
        window: WindowKey,
        at: (i32, i32),
        slot: u32,
    },
    Up {
        slot: u32,
    },
    /// Withdraw every contact. Used when a gesture fails part-way: a contact
    /// left down is worse than a gesture that did not happen, because the next
    /// interaction inherits it.
    Cancel,
}

/// The answer to a clipboard read, which arrives one of two ways.
///
/// When the agent owns the selection the text is simply there. When a *client*
/// owns it the data has to be asked for over the protocol and written into a
/// pipe, and the read cannot happen here — the client only answers once the
/// compositor loop runs again, so reading on this thread would deadlock against
/// the very event that fills the pipe. The fd goes back to the proxy, which
/// reads it on its own task.
enum ClipboardRead {
    Text(Option<String>),
    Pipe(std::os::fd::OwnedFd),
}

/// One primitive of a stylus stroke.
///
/// Split from the pointer for the same reason touch is: a stylus is not a mouse
/// that reports extra numbers. Pressure and tilt are what a drawing application
/// reads to decide stroke width and shape, and a tool that goes from "not
/// there" to "pressing" without a proximity event is one many applications
/// ignore entirely.
#[derive(Clone, Copy, Debug)]
enum StylusStep {
    /// The tool comes within range of the tablet, over a window.
    ProximityIn { window: WindowKey, at: Rect },
    /// The tip touches down.
    Down,
    /// The tool moves, at a pressure and tilt.
    Motion {
        window: WindowKey,
        at: Rect,
        pressure: f64,
        tilt: (f64, f64),
    },
    /// The tip lifts.
    Up,
    /// The tool leaves range. Paired with `ProximityIn`, always: a stylus left
    /// in proximity keeps an application in its hover state forever.
    ProximityOut,
}

/// One primitive of a pointer sequence, assembled by the proxy.
#[derive(Clone, Copy, Debug)]
enum PointerStep {
    /// Move the pointer, pressing nothing.
    Motion { window: WindowKey, at: Rect },
    /// Press or release one button where the pointer already is.
    Button { button: u32, pressed: bool },
    /// Hold or release the modifier keys of a chord, so a ctrl+click and a
    /// shift+drag are expressible as the sequence a person performs.
    Modifiers {
        window: WindowKey,
        modifiers: crate::keys::Modifiers,
        pressed: bool,
    },
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
    dmabuf_state: DmabufState,
    /// The dmabuf global's identity, compared in [`DmabufHandler::dmabuf_imported`]
    /// so a buffer offered against some other global is refused rather than
    /// imported into the wrong renderer.
    dmabuf_global: DmabufGlobal,
    /// Held only to keep their globals alive. Dropping either withdraws the
    /// global from the registry, which for a client that already bound it is a
    /// protocol error rather than a graceful loss of a feature.
    _xdg_decoration_state: XdgDecorationState,
    _viewporter_state: ViewporterState,
    _presentation_state: PresentationState,
    _output_manager_state: OutputManagerState,
    /// The stage's single virtual screen. Surfaces are entered onto it as they
    /// appear, because a client that has never received `wl_surface.enter` does
    /// not know which output it is on and cannot pick a scale.
    output: Output,
    /// The renderer lives in the state rather than beside it because a dmabuf
    /// import is a *protocol* event: the client asks, and we have to answer with
    /// the renderer before replying. Every other user of it goes through a
    /// disjoint field borrow.
    renderer: GlesRenderer,
    /// Presentation feedback is sequenced, and a client that sees the counter
    /// go backwards is entitled to treat it as a compositor fault.
    frame_sequence: u64,
    seat: Seat<Self>,
    windows: Vec<Managed>,
    next_key: u64,
    /// Buttons and evdev keycodes this seat is currently holding down.
    ///
    /// Tracked because a seat outlives the program that used it: a `press` with
    /// no `release`, or a program that failed between the two, leaves the
    /// client believing the button is still down and turns the *next* program's
    /// click into a drag. `Relax` reads these and lets go of exactly what is
    /// held, rather than blindly sending releases for things that were never
    /// pressed — a spurious release is itself an event a client can act on.
    held_buttons: Vec<u32>,
    held_keys: Vec<u32>,
    /// What the agent last put on the clipboard.
    ///
    /// The stage is the data source for its own selection, so the text has to
    /// live somewhere the offer can be served from. A client that copies
    /// something replaces this through the ordinary data-device path.
    clipboard: Option<String>,
    /// The `zwp_tablet_manager_v2` global.
    ///
    /// Held to keep the global alive, like the decoration and viewporter
    /// states: a drawing application binds it at start-up to discover whether a
    /// stylus exists at all, and one that finds none takes the mouse path
    /// permanently — no pressure, no tilt, and no way to ask for them later.
    _tablet_state: TabletManagerState,
    /// Menus, dropdowns, tooltips — every transient surface a client puts up.
    ///
    /// Held separately from `windows` because a popup is not a window: it has
    /// no independent identity the agent should see, it lives and dies with its
    /// parent, and it is positioned relative to that parent rather than to the
    /// output. What it *does* need is to be configured, rendered above its
    /// parent, and given input — none of which was happening.
    popups: PopupManager,
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
    /// When the stage came up, so input events can carry real timestamps.
    ///
    /// Every event used to be stamped `0`. Clients that only ask "did something
    /// happen" never noticed, but anything that reads the clock did: two clicks
    /// at the same millisecond are a double-click, a contact that goes down and
    /// up at the same instant has no duration to distinguish a tap from a long
    /// press, and a swipe delivered with no elapsed time has infinite velocity.
    /// Android reads all three.
    started: std::time::Instant,
}

impl StageState {
    /// Milliseconds since the stage started, which is what input events carry.
    ///
    /// Wrapping at `u32` is the protocol's own behaviour — clients are required
    /// to treat these as a wrapping counter — and 49 days of uptime is not a
    /// case worth carrying a second clock for.
    fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

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
        // doing something odd, so this should be unreachable — but the signature
        // demands a reference and the compositor thread is the *whole* display.
        // Panicking here has already cost X11 support once, silently: the thread
        // died, every later command failed, and the failure surfaced as
        // "XWayland is not available". A leaked default costs one allocation per
        // strange client and keeps the display alive.
        eprintln!(
            "artist: a wayland client arrived with unrecognized client data; \
             giving it fresh compositor state rather than taking the stage down"
        );
        Box::leak(Box::new(CompositorClientState::default()))
    }

    fn commit(&mut self, surface: &wl_surface::WlSurface) {
        // Read what the client committed *before* handing it to the renderer:
        // `on_commit_buffer_handler` takes the buffer assignment as it imports
        // the texture, so asking afterwards makes every commit look like a
        // state-only commit with nothing to redraw.
        let (regions, precise) = surface_damage(surface, self.full_screen());

        // Required before any renderer touches the buffer; without it the
        // surface has no usable texture and rendering silently draws nothing.
        on_commit_buffer_handler::<Self>(surface);
        self.dirty = true;

        // Advances a popup's configure state, and — on the initial commit —
        // sends the configure the client is waiting for. Without this a popup
        // that was tracked still never reaches a mapped state.
        self.popups.commit(surface);

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
            let _ = self.damage.send(Damage {
                window,
                region,
                precise,
            });
        }
    }
}

impl XdgShellHandler for StageState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let (width, height) = (self.width, self.height);
        // A toplevel is simply given the whole stage, undecorated. The mode is
        // set in the *initial* configure rather than waiting for the client to
        // ask through xdg-decoration: a client that never binds that global
        // would otherwise still draw its own titlebar, and this is the one
        // configure every client is guaranteed to read.
        surface.with_pending_state(|state| {
            state.size = Some((width, height).into());
            state.decoration_mode = Some(DecorationMode::ServerSide);
            state
                .states
                .set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated);
        });
        surface.send_configure();

        // Put the surface on the output. A client that has not been told which
        // output it is on cannot resolve its scale, and several toolkits will
        // sit waiting for that before they draw anything at all — which looks
        // from here like a window that never maps.
        self.output.enter(surface.wl_surface());

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
        // Paired with the `enter` above so the output does not accumulate a
        // dead weak reference per window for the life of the stage.
        self.output.leave(surface.wl_surface());
        self.windows.retain(|window| window.toplevel != surface);
        self.dirty = true;
    }

    /// A menu, a dropdown, a combo box list, a tooltip.
    ///
    /// This was an empty stub, and the consequences were larger than they look.
    /// A popup is a *separate* xdg surface, not a subsurface of its parent, so
    /// it appears in none of the toplevel surface trees `draw` walks — and
    /// xdg-shell requires a configure before the client may attach a buffer, so
    /// a popup that is never configured is never even painted. The result was
    /// that every context menu, every `<select>` dropdown and every tooltip on
    /// the stage was invisible to capture and unreachable by the pointer, with
    /// no error anywhere: the click that opened the menu succeeded, and the menu
    /// simply did not exist.
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        // The geometry the client asked for, honoured as asked. There is
        // nowhere to slide it to anyway: the stage is one output and its
        // toplevels already fill it, so the constraint-adjustment dance a real
        // compositor performs would have no better answer than the client's.
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface.clone())) {
            eprintln!("artist: a popup could not be tracked: {error}");
            return;
        }
        // Unconditional, for the same reason the decoration configure is: this
        // is the event the client is waiting on before it draws anything.
        if let Err(error) = surface.send_configure() {
            eprintln!("artist: a popup could not be configured: {error}");
        }
        self.dirty = true;
    }

    /// A client asking to own input while its menu is open.
    ///
    /// Honoured rather than ignored, because a menu that does not hold a grab
    /// closes on the next click *anywhere* in many toolkits — including the
    /// click meant to choose an item from it.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        use smithay::input::Seat;

        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let popup = PopupKind::Xdg(surface.clone());
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let mut grab = match self.popups.grab_popup(root, popup, &seat, serial) {
            Ok(grab) => grab,
            Err(error) => {
                eprintln!("artist: a popup grab was refused: {error}");
                return;
            }
        };
        if let Some(keyboard) = seat.get_keyboard()
            && keyboard.is_grabbed()
            && !(keyboard.has_grab(serial)
                || grab.previous_serial().is_some_and(|s| keyboard.has_grab(s)))
        {
            // Another grab is already in force and this one does not chain onto
            // it. Ungrabbing the new popup is what the protocol expects; taking
            // the grab anyway would strand the first menu open with no input.
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        if let Some(keyboard) = seat.get_keyboard() {
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            pointer.set_grab(
                self,
                PopupPointerGrab::new(&grab),
                serial,
                smithay::input::pointer::Focus::Keep,
            );
        }
    }

    /// A popup asking to move — a submenu flipping to the other side, a
    /// dropdown repositioning after its anchor scrolled.
    ///
    /// Acknowledged with the geometry the client computed. Leaving this unanswered
    /// leaves the popup where it was while the client believes it moved.
    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
        if let Err(error) = surface.send_configure() {
            eprintln!("artist: a popup could not be repositioned: {error}");
        }
        self.dirty = true;
    }

    /// A popup went away — the menu closed, the tooltip faded.
    ///
    /// The manager has to be told, or it keeps yielding a dead surface from
    /// `popups_for_surface` and the renderer walks a tree that no longer has
    /// buffers.
    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.popups.cleanup();
        self.dirty = true;
    }
}

impl SelectionHandler for StageState {
    type SelectionUserData = ();

    /// A client took the clipboard.
    ///
    /// Our cached copy is dropped rather than kept, because keeping it would
    /// make `getClipboard` answer with what the *agent* last copied long after
    /// an application replaced it — a stale read that looks exactly like a
    /// successful one. `None` here means "ask whoever owns it", which is the
    /// only answer that stays true.
    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if ty == SelectionTarget::Clipboard && source.is_some() {
            self.clipboard = None;
        }
    }

    /// A client is reading the selection we own. Write it into the pipe.
    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        _mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        use std::io::Write;

        if ty != SelectionTarget::Clipboard {
            return;
        }
        let Some(text) = self.clipboard.clone() else {
            return;
        };
        // Dropped at the end of the scope, which closes the write end — a
        // reader blocks until EOF, so a pipe left open is a client that hangs
        // rather than one that gets nothing.
        let mut pipe = std::fs::File::from(fd);
        // A failed write is the client having gone away mid-request, which is
        // ordinary and not ours to report anywhere.
        let _ = pipe.write_all(text.as_bytes());
        let _ = pipe.flush();
    }
}

impl DataDeviceHandler for StageState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for StageState {}
impl ServerDndGrabHandler for StageState {}

/// The stage has no cursor to draw, so a tool asking for one changes nothing.
///
/// Implemented rather than left off because `delegate_tablet_manager!` requires
/// it, and because a client that sets a tool image is telling us it is
/// genuinely using the tablet — which is worth nothing here but is not an
/// error.
impl TabletSeatHandler for StageState {}

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

impl OutputHandler for StageState {}

impl DmabufHandler for StageState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        // The import is attempted *now*, not deferred to the first render.
        // A client that is told its buffer was accepted and then gets a blank
        // window has no way to recover; one that is told the import failed
        // falls back to shared memory and keeps working. Answering honestly
        // here is the difference between a slow client and a broken one.
        if global != &self.dmabuf_global {
            notifier.failed();
            return;
        }
        match self.renderer.import_dmabuf(&dmabuf, None) {
            Ok(_) => {
                let _ = notifier.successful::<StageState>();
            }
            Err(error) => {
                eprintln!("artist: stage refused a client dmabuf: {error}");
                notifier.failed();
            }
        }
    }
}

impl XdgDecorationHandler for StageState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        force_server_side_decorations(&toplevel);
    }

    /// The client's preference is read and then overruled, deliberately.
    ///
    /// A client-drawn titlebar is pixels the compositor has no geometry for:
    /// its close button does not appear at any rung, so the agent cannot avoid
    /// it and cannot deliberately use it either. Server-side means the stage
    /// owns the whole window frame, which here is no frame at all — the
    /// toplevel gets the entire output.
    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        force_server_side_decorations(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        force_server_side_decorations(&toplevel);
    }
}

fn force_server_side_decorations(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(DecorationMode::ServerSide);
    });
    // Unconditional, and `send_pending_configure` would be the bug here.
    // The mode is already `ServerSide` from the initial configure, so "has the
    // pending state changed?" answers *no* in the ordinary case — a client
    // binds xdg-decoration after its first configure, asks for client-side,
    // and smithay suppresses the reply because nothing changed. The client is
    // then left waiting for a configure the protocol promises it, and settles
    // on its own default. This cost a test.
    //
    // Before the initial configure there is nothing to do: the mode is in the
    // pending state and the initial configure will carry it.
    if toplevel.is_initial_configure_sent() {
        toplevel.send_configure();
    }
}

delegate_compositor!(StageState);
delegate_xdg_shell!(StageState);
delegate_xdg_decoration!(StageState);
delegate_shm!(StageState);
delegate_seat!(StageState);
delegate_data_device!(StageState);
delegate_tablet_manager!(StageState);
delegate_dmabuf!(StageState);
delegate_output!(StageState);
delegate_viewporter!(StageState);
delegate_presentation!(StageState);

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
fn surface_damage(surface: &wl_surface::WlSurface, full_screen: Rect) -> (Vec<Rect>, bool) {
    use smithay::wayland::compositor::{BufferAssignment, Damage as SurfaceDamage, with_states};

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
        return (rects, true);
    }
    // A new buffer with no damage attached. The client is entitled to do this —
    // it means "assume all of it" — so the whole surface is the honest bound,
    // reported as imprecise so nothing downstream mistakes it for a located
    // change.
    if painted {
        return (vec![full_screen], false);
    }
    (Vec::new(), true)
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
    /// Whether the seat got a tablet and a gamepad at start-up.
    ///
    /// Decided once, when the seat is built, and reported through
    /// [`Stage::seat`] rather than assumed — a surface derives its own `Caps`
    /// from this, and advertising a device the seat does not have is how a step
    /// comes back `ok` having done nothing.
    has_tablet: bool,
    has_gamepad: bool,
}

impl Drop for StageWayland {
    /// Tear the stage down — off the async runtime, if we are on one.
    ///
    /// Teardown genuinely blocks: reaping a killed browser and joining the
    /// compositor thread both wait on another process or thread. That is fine
    /// on an ordinary thread and not fine on a tokio worker, where it stalls
    /// every other task on that worker. So the work is moved onto a blocking
    /// thread when there is a runtime to move it to, and done inline when there
    /// is not — a `Drop` that silently skipped the kill because no runtime was
    /// handy would leak a compositor and a browser.
    fn drop(&mut self) {
        let _ = self.commands.send(StageCommand::Shutdown);
        // `unwrap_or_else(into_inner)` rather than `unwrap`: a panic elsewhere
        // while holding either lock would poison it and turn teardown into a
        // second panic, losing the cleanup entirely.
        let children = std::mem::take(
            &mut *self
                .children
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();

        let reap = move || {
            let mut children = children;
            for child in children.iter_mut() {
                kill_process_group(child.id());
                let _ = child.kill();
                // Reap, or the child lingers as a zombie holding its profile dir.
                let _ = child.wait();
            }
            if let Some(thread) = thread {
                let _ = thread.join();
            }
        };

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(reap);
            }
            Err(_) => reap(),
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
            // Both devices are added to the seat unconditionally when the
            // compositor thread builds it, so both are always present here. The
            // fields exist because a future stage — a remote one, a phone —
            // may not have them, and `Caps` has to report the truth rather than
            // this backend's assumption.
            has_tablet: true,
            has_gamepad: true,
        })
    }

    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }

    /// The stage's own X11 display number, or `None` when XWayland is not up.
    pub async fn x11_display(&self) -> Option<u32> {
        self.ask(StageCommand::X11Display).await.ok().flatten()
    }

    /// The buffer the stage draws into, for a viewer to import.
    ///
    /// A handle to the very memory the compositor renders into, not a copy: a
    /// viewer that imports this and attaches it to a surface on the user's own
    /// display is showing the stage with no readback, no encode and no transfer.
    /// That is only possible because the target is allocated through GBM — an
    /// opaque `GlesRenderbuffer` could not leave the process at all.
    ///
    /// **The frame it holds is whatever was drawn last.** There is no implicit
    /// synchronisation here; a viewer redraws on the stage's damage signal,
    /// which is the same signal the settle predicates use.
    pub async fn render_target(&self) -> Option<RenderTargets> {
        self.ask(StageCommand::RenderTarget).await.ok()
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

/// One step of a swipe or pinch, at roughly a 60 Hz frame.
///
/// Android's velocity tracker integrates the last handful of points, so the
/// step rate is what a fling's distance is computed from. Much coarser and a
/// swipe reads as a teleport with no velocity; much finer and the events
/// outpace the frames that would consume them.
const GESTURE_STEP_MS: u64 = 16;

/// The shortest a contact stays down.
///
/// A down and an up in the same millisecond is discarded by some input stacks
/// as a spurious contact, and read by others as a zero-duration tap that never
/// crosses the threshold to become a click at all.
const MIN_CONTACT_MS: u64 = 40;

impl StageWayland {
    async fn touch_step(&self, step: TouchStep) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Touch(step, tx))
            .await?
            .map_err(StepError::Backend)
    }

    /// Play a gesture out over wall-clock time.
    ///
    /// The waiting happens here, on the caller's task, rather than on the
    /// compositor thread — which is why the thread only ever sees instantaneous
    /// events. A 500 ms long press otherwise stalls rendering, damage delivery
    /// and every other command for half a second.
    /// Send one pointer primitive to the compositor thread.
    async fn pointing(&self, step: PointerStep) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Pointing(step, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn stylus_step(&self, step: StylusStep) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Stylus(step, tx))
            .await?
            .map_err(StepError::Backend)
    }

    /// Approach, touch down, draw, lift, withdraw.
    ///
    /// The five phases are what a drawing application expects, and skipping any
    /// of them is a stroke it will not record: proximity is how it learns which
    /// tool is in use, and a tip-down with no preceding proximity event is
    /// discarded outright by several.
    async fn run_stroke(
        &self,
        window: WindowKey,
        from: Rect,
        to: Rect,
        pressure: f32,
        tilt: (f32, f32),
    ) -> Result<(), StepError> {
        let start = centre_i32(from);
        let end = centre_i32(to);
        let steps = 24;
        let pressure = f64::from(pressure.clamp(0.0, 1.0));
        let tilt = (f64::from(tilt.0), f64::from(tilt.1));
        let point = |x: i32, y: i32| Rect {
            x,
            y,
            width: 0,
            height: 0,
        };

        self.stylus_step(StylusStep::ProximityIn { window, at: from })
            .await?;
        self.stylus_step(StylusStep::Motion {
            window,
            at: from,
            pressure,
            tilt,
        })
        .await?;
        self.stylus_step(StylusStep::Down).await?;

        // More samples than a drag uses. A stroke's *shape* is the output here,
        // not just its endpoints, so the sample rate is the resolution of the
        // line the application draws.
        for step in 1..=steps {
            tokio::time::sleep(std::time::Duration::from_millis(GESTURE_STEP_MS)).await;
            let (x, y) = lerp(start, end, step, steps);
            self.stylus_step(StylusStep::Motion {
                window,
                at: point(x, y),
                pressure,
                tilt,
            })
            .await?;
        }

        self.stylus_step(StylusStep::Up).await?;
        self.stylus_step(StylusStep::ProximityOut).await
    }

    /// Press, move in steps, release.
    ///
    /// The intermediate motion is the whole point. Every toolkit begins a drag
    /// only after the pointer has travelled past a threshold — typically a few
    /// pixels — so a press and a release at two points is not a short drag, it
    /// is a click at the first point and nothing at the second. The waiting
    /// happens here, on the caller's task, for the same reason gestures do it
    /// here: sleeping on the compositor thread would stall rendering and the
    /// command channel for the length of the drag.
    async fn run_drag(
        &self,
        window: WindowKey,
        from: Rect,
        to: Rect,
        button: u32,
        modifiers: crate::keys::Modifiers,
    ) -> Result<(), StepError> {
        let start = centre_i32(from);
        let end = centre_i32(to);
        let steps = 16;

        if modifiers.any() {
            self.pointing(PointerStep::Modifiers {
                window,
                modifiers,
                pressed: true,
            })
            .await?;
        }
        self.pointing(PointerStep::Motion { window, at: from })
            .await?;
        self.pointing(PointerStep::Button {
            button,
            pressed: true,
        })
        .await?;

        for step in 1..=steps {
            tokio::time::sleep(std::time::Duration::from_millis(GESTURE_STEP_MS)).await;
            let (x, y) = lerp(start, end, step, steps);
            self.pointing(PointerStep::Motion {
                window,
                at: Rect {
                    x,
                    y,
                    width: 0,
                    height: 0,
                },
            })
            .await?;
        }
        // A settling pause before the release. A drop target that highlights on
        // hover often commits on the *next* frame, and releasing in the same
        // millisecond as the final motion lands the drop before the target has
        // accepted it.
        tokio::time::sleep(std::time::Duration::from_millis(GESTURE_STEP_MS * 2)).await;
        self.pointing(PointerStep::Button {
            button,
            pressed: false,
        })
        .await?;
        if modifiers.any() {
            self.pointing(PointerStep::Modifiers {
                window,
                modifiers,
                pressed: false,
            })
            .await?;
        }
        Ok(())
    }

    async fn run_gesture(&self, window: WindowKey, gesture: &Gesture) -> Result<(), StepError> {
        match *gesture {
            Gesture::Tap { at, hold_ms } => {
                let point = centre_i32(at);
                self.touch_step(TouchStep::Down {
                    window,
                    at: point,
                    slot: 0,
                })
                .await?;
                tokio::time::sleep(std::time::Duration::from_millis(
                    hold_ms.max(MIN_CONTACT_MS),
                ))
                .await;
                self.touch_step(TouchStep::Up { slot: 0 }).await
            }
            Gesture::Swipe {
                from,
                to,
                duration_ms,
            } => {
                let start = centre_i32(from);
                let end = centre_i32(to);
                let steps = (duration_ms / GESTURE_STEP_MS).clamp(2, 240) as i32;
                self.touch_step(TouchStep::Down {
                    window,
                    at: start,
                    slot: 0,
                })
                .await?;
                for step in 1..=steps {
                    tokio::time::sleep(std::time::Duration::from_millis(GESTURE_STEP_MS)).await;
                    self.touch_step(TouchStep::Motion {
                        window,
                        at: lerp(start, end, step, steps),
                        slot: 0,
                    })
                    .await?;
                }
                self.touch_step(TouchStep::Up { slot: 0 }).await
            }
            Gesture::Pinch {
                at,
                from_gap,
                to_gap,
                duration_ms,
            } => {
                let (cx, cy) = centre_i32(at);
                let steps = (duration_ms / GESTURE_STEP_MS).clamp(2, 240) as i32;
                // Horizontal, about the centre. The axis is arbitrary — every
                // pinch handler cares about the distance between contacts and
                // not their orientation — but it has to be *consistent*, or a
                // two-finger gesture reads as a rotation as well as a scale.
                let half = |gap: u32| (gap / 2) as i32;
                let left = |gap: u32| (cx - half(gap), cy);
                let right = |gap: u32| (cx + half(gap), cy);

                self.touch_step(TouchStep::Down {
                    window,
                    at: left(from_gap),
                    slot: 0,
                })
                .await?;
                self.touch_step(TouchStep::Down {
                    window,
                    at: right(from_gap),
                    slot: 1,
                })
                .await?;
                for step in 1..=steps {
                    tokio::time::sleep(std::time::Duration::from_millis(GESTURE_STEP_MS)).await;
                    let gap = lerp_u32(from_gap, to_gap, step, steps);
                    self.touch_step(TouchStep::Motion {
                        window,
                        at: left(gap),
                        slot: 0,
                    })
                    .await?;
                    self.touch_step(TouchStep::Motion {
                        window,
                        at: right(gap),
                        slot: 1,
                    })
                    .await?;
                }
                // Lifted in the order they were placed. Simultaneous lifts are
                // not expressible — each is its own event — and lifting the
                // second first is what a real hand does least often.
                self.touch_step(TouchStep::Up { slot: 0 }).await?;
                self.touch_step(TouchStep::Up { slot: 1 }).await
            }
        }
    }
}

fn centre_i32(rect: Rect) -> (i32, i32) {
    (
        rect.x + (rect.width / 2) as i32,
        rect.y + (rect.height / 2) as i32,
    )
}

fn lerp(from: (i32, i32), to: (i32, i32), step: i32, steps: i32) -> (i32, i32) {
    let fraction = f64::from(step) / f64::from(steps);
    (
        from.0 + ((f64::from(to.0 - from.0)) * fraction).round() as i32,
        from.1 + ((f64::from(to.1 - from.1)) * fraction).round() as i32,
    )
}

fn lerp_u32(from: u32, to: u32, step: i32, steps: i32) -> u32 {
    let fraction = f64::from(step) / f64::from(steps);
    let span = f64::from(to) - f64::from(from);
    (f64::from(from) + span * fraction).round().max(0.0) as u32
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
        // GUI applications must not inherit the harness terminal's stdio. Browsers
        // are especially noisy (D-Bus, portal, and web-service diagnostics), and
        // leaking their stderr into Artist makes harmless child warnings look like
        // harness failures. GUI diagnostics belong to the surface/process log;
        // until that channel exists, discard both streams rather than corrupting
        // the agent's terminal.
        process.stdout(std::process::Stdio::null());
        process.stderr(std::process::Stdio::null());
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
        //
        // The two failures are told apart deliberately. `Err` means the
        // compositor thread is gone; `Ok(None)` means it is alive and XWayland
        // simply is not up. Collapsing them — as `.ok().flatten()` did — turned
        // a dead compositor into "no X11" and launched the application against a
        // socket nobody is listening on, so it hung or died with no explanation
        // that pointed anywhere near the cause.
        match self.ask(StageCommand::X11Display).await {
            Ok(Some(number)) => {
                process.env("DISPLAY", format!(":{number}"));
            }
            Ok(None) => {
                process.env_remove("DISPLAY");
            }
            Err(error) => {
                return Err(StepError::Backend(format!(
                    "the stage's compositor is not running, so {} cannot be launched into it: \
                     {error}",
                    command.program
                )));
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

    async fn pointer(
        &self,
        window: WindowKey,
        pointing: crate::stage::Pointing,
    ) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Pointer(window, pointing, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn hover(&self, window: WindowKey, at: Rect) -> Result<(), StepError> {
        self.pointing(PointerStep::Motion { window, at }).await
    }

    async fn press(&self, window: WindowKey, at: Rect, button: u32) -> Result<(), StepError> {
        self.pointing(PointerStep::Motion { window, at }).await?;
        self.pointing(PointerStep::Button {
            button,
            pressed: true,
        })
        .await
    }

    async fn release(&self, _window: WindowKey, button: u32) -> Result<(), StepError> {
        self.pointing(PointerStep::Button {
            button,
            pressed: false,
        })
        .await
    }

    async fn drag(
        &self,
        window: WindowKey,
        from: Rect,
        to: Rect,
        button: u32,
        modifiers: crate::keys::Modifiers,
    ) -> Result<(), StepError> {
        let result = self.run_drag(window, from, to, button, modifiers).await;
        if result.is_err() {
            // Same guarantee `gesture` gives a touch contact: a button left
            // down outlives the failed drag and turns the next click into one,
            // so the withdrawal is unconditional and its own failure is
            // discarded rather than replacing the real error.
            let _ = self
                .pointing(PointerStep::Button {
                    button,
                    pressed: false,
                })
                .await;
            let _ = self
                .pointing(PointerStep::Modifiers {
                    window,
                    modifiers,
                    pressed: false,
                })
                .await;
        }
        result
    }

    async fn key_hold(
        &self,
        window: WindowKey,
        stroke: &str,
        pressed: bool,
    ) -> Result<(), StepError> {
        let stroke = stroke.to_owned();
        self.ask(|tx| StageCommand::KeyHold(window, stroke, pressed, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn clipboard_set(&self, text: &str) -> Result<(), StepError> {
        let text = text.to_owned();
        self.ask(|tx| StageCommand::ClipboardSet(text, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn clipboard_get(&self) -> Result<Option<String>, StepError> {
        match self
            .ask(StageCommand::ClipboardGet)
            .await?
            .map_err(StepError::Backend)?
        {
            ClipboardRead::Text(text) => Ok(text),
            // The owning client writes into the pipe only once the compositor
            // loop runs again, so this read happens on a blocking task and is
            // bounded: a client that never answers must not hang the agent.
            ClipboardRead::Pipe(fd) => {
                let read = tokio::task::spawn_blocking(move || {
                    use std::io::Read;
                    let mut buffer = String::new();
                    let mut pipe = std::fs::File::from(fd);
                    pipe.read_to_string(&mut buffer).map(|_| buffer)
                });
                match tokio::time::timeout(std::time::Duration::from_secs(2), read).await {
                    Ok(Ok(Ok(text))) if text.is_empty() => Ok(None),
                    Ok(Ok(Ok(text))) => Ok(Some(text)),
                    Ok(Ok(Err(error))) => Err(StepError::Backend(format!(
                        "reading the clipboard from the application that owns it: {error}"
                    ))),
                    Ok(Err(error)) => Err(StepError::Backend(format!("clipboard read: {error}"))),
                    Err(_) => Err(StepError::Backend(
                        "the application holding the clipboard did not answer within 2s".into(),
                    )),
                }
            }
        }
    }

    async fn close_window(&self, window: WindowKey) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::CloseWindow(window, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn resize(&self, width: u32, height: u32) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Resize(width, height, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn stylus(
        &self,
        window: WindowKey,
        from: Rect,
        to: Rect,
        pressure: f32,
        tilt: (f32, f32),
    ) -> Result<(), StepError> {
        let result = self.run_stroke(window, from, to, pressure, tilt).await;
        if result.is_err() {
            // A tip left down, or a tool left in proximity, outlives the failed
            // stroke — the first keeps drawing on the next motion, the second
            // holds the application in its hover state for good.
            let _ = self.stylus_step(StylusStep::Up).await;
            let _ = self.stylus_step(StylusStep::ProximityOut).await;
        }
        result
    }

    async fn relax(&self, _window: WindowKey) -> Result<(), StepError> {
        // Touch contacts first: `Cancel` is idempotent and withdrawing a
        // contact that was never placed costs nothing, whereas leaving one down
        // makes the next gesture start mid-sequence.
        let _ = self.touch_step(TouchStep::Cancel).await;
        self.ask(StageCommand::Relax)
            .await?
            .map_err(StepError::Backend)
    }

    async fn capture(&self, window: Option<WindowKey>) -> Result<Frame, StepError> {
        self.ask(|tx| StageCommand::Capture(window, tx))
            .await?
            .map_err(StepError::Backend)
    }

    fn seat(&self) -> crate::stage::SeatCaps {
        crate::stage::SeatCaps {
            keyboard: true,
            pointer: true,
            scroll: true,
            touch: true,
            buttons: true,
            clipboard: true,
            // Both are advertised only when their device was actually created;
            // the compositor decides that at start-up rather than here.
            tablet: self.has_tablet,
            gamepad: self.has_gamepad,
        }
    }

    async fn scroll(
        &self,
        window: WindowKey,
        at: Rect,
        amount: i32,
        axis: crate::program::Axis,
    ) -> Result<(), StepError> {
        self.ask(|tx| StageCommand::Scroll(window, at, amount, axis, tx))
            .await?
            .map_err(StepError::Backend)
    }

    async fn gesture(&self, window: WindowKey, gesture: &Gesture) -> Result<(), StepError> {
        let result = self.run_gesture(window, gesture).await;
        if result.is_err() {
            // A contact left down outlives the failed gesture and poisons the
            // next one, so the withdrawal is unconditional and its own failure
            // is ignored: there is nothing better to do with it, and reporting
            // it would replace the real error with a less useful one.
            let _ = self.touch_step(TouchStep::Cancel).await;
        }
        result
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
///
/// The device's `dev_t` comes back with the renderer because dmabuf feedback
/// has to name it. A client picks its allocator from that number, so getting it
/// wrong on a multi-GPU machine means every buffer it hands us is one we cannot
/// import — and the failure appears as a client that renders nothing rather
/// than as a device mismatch.
/// The stage's buffer allocator.
///
/// `GbmAllocator` rather than the bare `GbmDevice`: the allocation flags belong
/// with the allocator, and every buffer the stage makes wants the same ones.
type StageGbm = smithay::backend::allocator::gbm::GbmAllocator<smithay::backend::drm::DrmDeviceFd>;

fn make_renderer() -> Result<(GlesRenderer, StageGbm, u64), String> {
    use smithay::backend::allocator::gbm::GbmDevice;
    use std::os::linux::fs::MetadataExt;

    let candidates = ["/dev/dri/renderD128", "/dev/dri/renderD129"];
    let mut last = String::from("no DRM render node found");
    for path in candidates {
        if !std::path::Path::new(path).exists() {
            continue;
        }
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        {
            Ok(file) => file,
            Err(error) => {
                last = format!("open {path}: {error}");
                continue;
            }
        };
        // Read the device id from the open handle, not the path: the two can
        // differ if the node is replaced between the open and the stat, and the
        // fd is what the renderer is actually bound to.
        let device = match file.metadata() {
            Ok(metadata) => metadata.st_rdev(),
            Err(error) => {
                last = format!("stat {path}: {error}");
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
        // Cloned rather than moved: `EGLDisplay::new` consumes the device, and
        // the stage needs it afterwards to allocate a render target that can be
        // *exported*. A `GlesRenderbuffer` cannot be — it is an opaque GL object
        // — so a viewer on the user's display would have no way to receive the
        // frame except a full readback and re-encode per frame, which is the
        // whole cost this avoids.
        let allocator = smithay::backend::allocator::gbm::GbmAllocator::new(
            gbm.clone(),
            smithay::backend::allocator::gbm::GbmBufferFlags::RENDERING,
        );
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
            .map(|renderer| (renderer, allocator, device))
            .map_err(|error| format!("gles renderer on {path}: {error}"));
    }
    Err(last)
}

/// Allocate the stage's render target as an exportable GPU buffer.
///
/// Two steps that have to stay together: GBM allocates it, and `export` turns it
/// into a `Dmabuf` — a set of file descriptors another process can import. The
/// renderer binds the `Dmabuf` rather than the `GbmBuffer`, so the thing being
/// drawn into and the thing handed out are the same memory, not a copy of it.
fn allocate_target(allocator: &mut StageGbm, width: i32, height: i32) -> Result<Dmabuf, String> {
    use smithay::backend::allocator::Modifier;
    use smithay::backend::allocator::gbm::GbmBufferFlags;

    let buffer = allocator
        .create_buffer_with_flags(
            width as u32,
            height as u32,
            Fourcc::Abgr8888,
            &[Modifier::Linear],
            // `RENDERING` alone. Mesa's iris rejects `WRITE` and `LINEAR` as
            // flags outright when a modifier is named, which is the same trap
            // the dmabuf client test hit from the other side.
            GbmBufferFlags::RENDERING,
        )
        .map_err(|error| format!("allocate a {width}x{height} render target: {error}"))?;
    buffer
        .export()
        .map_err(|error| format!("export the render target as a dmabuf: {error}"))
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

    // Bound *before* any GPU state exists, and that ordering is a crash fix
    // rather than a preference. Binding is the failure that actually happens —
    // a stale socket, or a second stage on the same path — and when it happened
    // after the renderer was built, the early return dropped an EGL context and
    // its GBM buffers from this thread while the main thread was still unwinding
    // the same objects. That is not a clean error: it aborts the whole process
    // with `corrupted double-linked list`, so a name collision presented as a
    // heap bug. With nothing on the GPU yet, the same failure is just an `Err`.
    let socket_name = "wayland-artist".to_owned();
    let listener = match ListeningSocket::bind_absolute(runtime_dir.join(&socket_name)) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(format!("bind wayland socket: {error}")));
            return;
        }
    };

    let (renderer, mut allocator, render_device) = match make_renderer() {
        Ok(renderer) => renderer,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "no GPU renderer for the stage: {error}. \
                 A DRM render node (/dev/dri/renderD128) and working EGL are required."
            )));
            return;
        }
    };

    // The render target, allocated through GBM and exported as a dmabuf.
    //
    // Linear explicitly. A tiled buffer renders and reads back fine, but the
    // point of exporting is that another process imports it, and linear is the
    // one layout every consumer can accept. The stage is not fill-rate bound —
    // it renders on damage, not at 60 Hz — so the tiling loss costs nothing
    // measurable here and buys a zero-copy path out.
    // *Two* targets, not one, and this is not an optimisation — it is a
    // correctness fix. A viewer attaches one of these to a surface on the
    // user's compositor, which then scans it out whenever it likes. Drawing
    // into that same buffer means the compositor can sample a half-drawn frame,
    // which showed up on screen as black geometry flashing over a window while
    // it was being typed into. So the stage draws into the *back* buffer and
    // publishes the finished one; a viewer only ever attaches a completed
    // frame.
    let mut allocated = Vec::with_capacity(RENDER_BUFFERS);
    for _ in 0..RENDER_BUFFERS {
        match allocate_target(&mut allocator, width, height) {
            Ok(target) => allocated.push(target),
            Err(error) => {
                let _ = ready.send(Err(format!("stage render target: {error}")));
                return;
            }
        }
    }
    let mut targets: [Dmabuf; RENDER_BUFFERS] = allocated
        .try_into()
        .expect("allocated exactly RENDER_BUFFERS");
    let held: Arc<[std::sync::atomic::AtomicBool; RENDER_BUFFERS]> =
        Arc::new(std::array::from_fn(|_| {
            std::sync::atomic::AtomicBool::new(false)
        }));
    // Which of the two holds a finished frame. Shared with the viewer as an
    // atomic rather than sent per frame: the viewer reads it on each repaint,
    // so publishing is a single store and costs the compositor nothing.
    let front = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut back = 1usize;

    // The formats we advertise are the formats this renderer can genuinely
    // import — asked of it directly rather than hardcoded, because the answer
    // depends on the driver and the EGL extensions actually present. A
    // hardcoded list is a promise the import path then breaks one client at a
    // time.
    let dmabuf_formats: Vec<_> = renderer.dmabuf_formats().iter().copied().collect();
    let mut dmabuf_state = DmabufState::new();
    let dmabuf_global = match DmabufFeedbackBuilder::new(render_device, dmabuf_formats).build() {
        Ok(feedback) => {
            dmabuf_state.create_global_with_default_feedback::<StageState>(&handle, &feedback)
        }
        Err(error) => {
            // Version 3 is a real fallback rather than a failure: it carries the
            // same format list without the device hint, which is all a
            // single-GPU machine needs anyway.
            eprintln!("artist: stage dmabuf feedback unavailable ({error}); advertising v3");
            dmabuf_state.create_global::<StageState>(
                &handle,
                renderer
                    .dmabuf_formats()
                    .iter()
                    .copied()
                    .collect::<Vec<_>>(),
            )
        }
    };

    let output = Output::new(
        "artist-stage".to_owned(),
        PhysicalProperties {
            // A virtual screen has no physical size, but reporting zero makes
            // clients that compute DPI divide by it. These are the millimetres
            // of a common 24" panel, which yields an ordinary ~96 DPI.
            size: (530, 300).into(),
            subpixel: Subpixel::Unknown,
            make: "artist".to_owned(),
            model: "stage".to_owned(),
        },
    );
    let mode = OutputMode {
        size: (width, height).into(),
        refresh: STAGE_REFRESH_MHZ,
    };
    let _output_global = output.create_global::<StageState>(&handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    let mut seat_state = SeatState::new();
    let seat = seat_state.new_wl_seat(&handle, "artist-stage");
    let mut state = StageState {
        display: handle.clone(),
        compositor_state: CompositorState::new::<StageState>(&handle),
        xdg_shell_state: XdgShellState::new::<StageState>(&handle),
        shm_state: ShmState::new::<StageState>(&handle, Vec::new()),
        seat_state,
        data_device_state: DataDeviceState::new::<StageState>(&handle),
        dmabuf_state,
        dmabuf_global,
        _xdg_decoration_state: XdgDecorationState::new::<StageState>(&handle),
        _viewporter_state: ViewporterState::new::<StageState>(&handle),
        // CLOCK_MONOTONIC. Presentation timestamps must be on the clock the
        // client is told about, and it is the only one that does not step.
        _presentation_state: PresentationState::new::<StageState>(&handle, CLOCK_MONOTONIC),
        // With xdg-output, so clients get the output's logical geometry and
        // name rather than inferring them from the mode.
        _output_manager_state: OutputManagerState::new_with_xdg_output::<StageState>(&handle),
        output,
        renderer,
        frame_sequence: 0,
        seat,
        windows: Vec::new(),
        next_key: 0,
        held_buttons: Vec::new(),
        held_keys: Vec::new(),
        clipboard: None,
        _tablet_state: TabletManagerState::new::<StageState>(&handle),
        popups: PopupManager::default(),
        dirty: false,
        damage,
        running: true,
        x11_display: None,
        render_error: None,
        width,
        height,
        started: std::time::Instant::now(),
    };

    let keyboard = match state.seat.add_keyboard(Default::default(), 200, 25) {
        Ok(keyboard) => keyboard,
        Err(error) => {
            let _ = ready.send(Err(format!("add keyboard: {error}")));
            return;
        }
    };
    let pointer = state.seat.add_pointer();
    // The seat advertises `wl_touch` from the moment it is created, because a
    // client reads the seat's capabilities once and decides from them what it
    // can do. Adding touch later would be invisible to anything already running
    // — and for Android that is the whole application.
    let touch = state.seat.add_touch();

    // A stylus, for the same reason: a drawing application asks the tablet seat
    // what tools exist when it starts, and one added afterwards is invisible to
    // it. The descriptor is deliberately plain — no usb id and no syspath,
    // because there is no device and claiming a real one would be a lie a
    // client could act on (several match on vendor id to pick a pressure curve).
    let display_handle = state.display.clone();
    let tablet_seat = state.seat.tablet_seat();
    let tablet = tablet_seat.add_tablet::<StageState>(
        &display_handle,
        &TabletDescriptor {
            name: "artist-stage-stylus".to_owned(),
            usb_id: None,
            syspath: None,
        },
    );
    // One pen, declaring only the axes we actually send. Advertising
    // `DISTANCE`, `ROTATION` or the rest and never reporting them is the same
    // trap as an unserviced global: an application that reads a declared axis
    // and always gets its initial value draws with it.
    let tool = tablet_seat.add_tool::<StageState>(
        &mut state,
        &display_handle,
        &smithay::backend::input::TabletToolDescriptor {
            tool_type: smithay::backend::input::TabletToolType::Pen,
            hardware_serial: 0,
            hardware_id_wacom: 0,
            capabilities: smithay::backend::input::TabletToolCapabilities::PRESSURE
                | smithay::backend::input::TabletToolCapabilities::TILT,
        },
    );

    // Bound by absolute path rather than through `XDG_RUNTIME_DIR`.
    // `ListeningSocket::bind` reads that variable from the process environment,
    // so using it would mean mutating global state from a compositor thread —
    // and two stages in one process would then race for the same directory,
    // with the loser's clients quietly connecting to the winner's display.
    let _ = ready.send(Ok(socket_name));

    // XWayland needs a calloop `LoopHandle`, so the loop is calloop-driven —
    // but it deliberately keeps the same poll-and-service shape rather than
    // becoming fully event-driven. Blocking on readiness would starve the
    // command channel, which is not a pollable source here.
    let mut event_loop: calloop::EventLoop<'static, StageState> =
        match calloop::EventLoop::try_new() {
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
                &touch,
                &tablet,
                &tool,
                Buffers {
                    targets: &mut targets,
                    back,
                    front: &front,
                    held: &held,
                },
                command,
            );
        }

        if state.dirty {
            state.dirty = false;
            // Choose somewhere to draw that is neither on screen nor still held
            // by the viewer. Falling back to the current `back` when everything
            // is held costs a torn frame in a situation that should not arise;
            // refusing to draw at all would freeze the stage, which is worse.
            back = (0..RENDER_BUFFERS)
                .find(|index| {
                    *index != front.load(std::sync::atomic::Ordering::Acquire)
                        && !held[*index].load(std::sync::atomic::Ordering::Acquire)
                })
                .unwrap_or(back);
            match render(&mut state, &mut targets[back], start) {
                Ok(()) => {
                    // Published only once the draw has finished and its fence
                    // has been waited on, so what a viewer attaches is never
                    // mid-frame.
                    front.store(back, std::sync::atomic::Ordering::Release);
                    state.render_error = None;
                }
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

    let inserted =
        loop_handle.insert_source(
            xwayland,
            move |event, _, state: &mut StageState| match event {
                XWaylandEvent::Ready { display_number, .. } => {
                    state.x11_display = Some(display_number);
                }
                XWaylandEvent::Error => {
                    state.x11_display = None;
                }
            },
        );
    if let Err(error) = inserted {
        eprintln!("artist: stage XWayland source failed ({error})");
    }
    // The client handle keeps XWayland's Wayland connection alive.
    std::mem::drop(client);
}

/// The render buffers and their bookkeeping, kept together because they are
/// only ever meaningful together: a buffer without knowing whether it is on
/// screen or held is a buffer you cannot safely draw into.
struct Buffers<'a> {
    targets: &'a mut [Dmabuf; RENDER_BUFFERS],
    back: usize,
    front: &'a Arc<std::sync::atomic::AtomicUsize>,
    held: &'a Arc<[std::sync::atomic::AtomicBool; RENDER_BUFFERS]>,
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    touch: &smithay::input::touch::TouchHandle<StageState>,
    tablet: &smithay::wayland::tablet_manager::TabletHandle,
    tool: &smithay::wayland::tablet_manager::TabletToolHandle,
    buffers: Buffers<'_>,
    command: StageCommand,
) {
    let Buffers {
        targets,
        back,
        front,
        held,
    } = buffers;
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
        StageCommand::Pointer(key, pointing, reply) => {
            let _ = reply.send(deliver_click(state, pointer, keyboard, key, pointing));
        }
        StageCommand::Pointing(step, reply) => {
            let result = match step {
                PointerStep::Motion { window, at } => {
                    deliver_motion(state, pointer, window, at).map(|_| ())
                }
                PointerStep::Button { button, pressed } => {
                    deliver_button(state, pointer, button, pressed);
                    Ok(())
                }
                PointerStep::Modifiers {
                    window,
                    modifiers,
                    pressed,
                } => deliver_modifiers(state, keyboard, window, modifiers, pressed),
            };
            let _ = reply.send(result);
        }
        StageCommand::Scroll(key, at, amount, axis, reply) => {
            let _ = reply.send(deliver_scroll(state, pointer, key, at, amount, axis));
        }
        StageCommand::KeyHold(key, stroke, pressed, reply) => {
            let _ = reply.send(deliver_key_hold(state, keyboard, key, &stroke, pressed));
        }
        StageCommand::Relax(reply) => {
            deliver_relax(state, pointer, keyboard);
            let _ = reply.send(Ok(()));
        }
        StageCommand::Stylus(step, reply) => {
            let _ = reply.send(deliver_stylus(state, tablet, tool, step));
        }
        StageCommand::ClipboardSet(text, reply) => {
            let display = state.display.clone();
            let seat = state.seat.clone();
            // Announced in the three spellings a Wayland client actually asks
            // for. A toolkit that finds none of its preferred types treats the
            // offer as empty, which presents as a paste that silently does
            // nothing.
            set_data_device_selection(
                &display,
                &seat,
                vec![
                    "text/plain;charset=utf-8".to_owned(),
                    "text/plain".to_owned(),
                    "UTF8_STRING".to_owned(),
                ],
                (),
            );
            // After the call, not before: `set_data_device_selection` can drive
            // `new_selection`, and that handler clears this field.
            state.clipboard = Some(text);
            let _ = reply.send(Ok(()));
        }
        StageCommand::ClipboardGet(reply) => {
            let _ = reply.send(read_clipboard(state));
        }
        StageCommand::CloseWindow(key, reply) => {
            let result = match state.window(key) {
                Some(window) => {
                    // A request, not a kill. The client may put up "save your
                    // work?" instead of closing, and that dialog is a surface
                    // the agent can drive like any other — which is the whole
                    // reason not to destroy the toplevel from here.
                    window.toplevel.send_close();
                    Ok(())
                }
                None => Err(format!("no window {key:?}")),
            };
            let _ = reply.send(result);
        }
        StageCommand::Resize(width, height, reply) => {
            let _ = reply.send(resize_output(state, width, height));
        }
        StageCommand::Touch(step, reply) => {
            let _ = reply.send(deliver_touch(state, touch, step));
        }
        StageCommand::Capture(window, reply) => {
            let full = state.full_screen();
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
                // For one window, recomposite with *only that window's* surface
                // tree before reading back. Cropping the finished screen is not
                // enough: every toplevel here is given the whole output, so the
                // crop is the whole screen and a rung-3 surface would read every
                // application's text as though it were its own.
                if window.is_some() {
                    // Drawn into the back buffer and read straight back, never
                    // published: this is a partial composite of one window and
                    // a viewer showing it would be showing a lie.
                    draw(state, &mut targets[back], window)?;
                    // The buffer now holds one window. Whatever is on screen
                    // next has to be composited again from scratch, or the other
                    // windows stay missing until something else happens to
                    // damage them.
                    state.dirty = true;
                }
                // One window reads what was just drawn; the whole screen reads
                // the last *finished* frame rather than whatever the back
                // buffer happens to hold mid-draw.
                let index = if window.is_some() {
                    back
                } else {
                    front.load(std::sync::atomic::Ordering::Acquire)
                };
                capture(&mut state.renderer, &mut targets[index], full, region)
            });
            let _ = reply.send(result);
        }
        StageCommand::X11Display(reply) => {
            let _ = reply.send(state.x11_display);
        }
        StageCommand::RenderTarget(reply) => {
            // A clone of the handle, not of the memory: `Dmabuf` is a set of
            // file descriptors behind an `Arc`, so a viewer importing this is
            // reading the very pixels the compositor draws — which is the whole
            // point of allocating it through GBM rather than as a renderbuffer.
            let _ = reply.send((targets.clone(), Arc::clone(front), Arc::clone(held)));
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
/// Move the pointer onto a window, without pressing anything.
///
/// Factored out because every pointer action starts this way: a wheel event, a
/// press and a click all go to whatever is under the cursor, so arriving there
/// first is not a nicety. Returns the timestamp used, so a caller sequencing
/// several events can keep them coherent.
fn deliver_motion(
    state: &mut StageState,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    key: WindowKey,
    at: Rect,
) -> Result<u32, String> {
    use smithay::input::pointer::MotionEvent;
    use smithay::utils::SERIAL_COUNTER;

    let Some(window) = state.window(key) else {
        return Err(format!("no window {key:?}"));
    };
    let toplevel = window.toplevel.wl_surface().clone();
    let point = centre_of(at);
    let time = state.now_ms();

    // What is actually under the point, which is not always the toplevel: an
    // open menu sits above it, and focusing the toplevel regardless would send
    // the click straight through the menu to whatever it covers. That is worse
    // than the menu being invisible, because the click lands somewhere and
    // reports success.
    let focus = surface_under(&toplevel, point).unwrap_or((toplevel, (0.0, 0.0).into()));

    pointer.motion(
        state,
        Some(focus),
        &MotionEvent {
            location: point,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pointer.frame(state);
    Ok(time)
}

/// The surface at a point, preferring popups over the window beneath them.
///
/// Returns the surface and the point's position *within* it, which is the pair
/// `PointerHandle::motion` wants — a click carries surface-local coordinates,
/// so handing it the popup with the toplevel's origin would land the press at
/// the wrong place inside the menu.
fn surface_under(
    toplevel: &wl_surface::WlSurface,
    point: smithay::utils::Point<f64, smithay::utils::Logical>,
) -> Option<(
    wl_surface::WlSurface,
    smithay::utils::Point<f64, smithay::utils::Logical>,
)> {
    // Reversed: `popups_for_surface` yields parents before children, and a
    // submenu overlapping its parent menu must win.
    let popups: Vec<_> = PopupManager::popups_for_surface(toplevel).collect();
    for (popup, offset) in popups.into_iter().rev() {
        let geometry = popup.geometry();
        let origin = offset - geometry.loc;
        let rect = smithay::utils::Rectangle::new(
            smithay::utils::Point::from((origin.x, origin.y)),
            geometry.size,
        );
        if rect.to_f64().contains(point) {
            return Some((
                popup.wl_surface().clone(),
                point - smithay::utils::Point::from((f64::from(origin.x), f64::from(origin.y))),
            ));
        }
    }
    None
}

/// Press or release one button where the pointer already is.
///
/// Records what is held so [`StageCommand::Relax`] can let go of exactly that.
fn deliver_button(
    state: &mut StageState,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    button: u32,
    pressed: bool,
) {
    use smithay::input::pointer::ButtonEvent;
    use smithay::utils::SERIAL_COUNTER;

    let time = state.now_ms();
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
    pointer.frame(state);

    if pressed {
        if !state.held_buttons.contains(&button) {
            state.held_buttons.push(button);
        }
    } else {
        state.held_buttons.retain(|held| *held != button);
    }
}

/// A click: move there, then press and release `count` times.
///
/// The repeats are deliberately *not* spaced out. A double click is two presses
/// inside the toolkit's double-click window — typically 400 ms — and delivering
/// them as fast as the seat allows is the only way to be reliably inside it.
/// The single-click case is unchanged from what it always was.
fn deliver_click(
    state: &mut StageState,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    key: WindowKey,
    pointing: crate::stage::Pointing,
) -> Result<(), String> {
    deliver_motion(state, pointer, key, pointing.at)?;

    // Modifiers are held across the whole click, the way a hand holds them:
    // ctrl+click extends a selection only if ctrl is down when the button goes
    // down, so pressing it afterwards would be an ordinary click.
    let modifiers = pointing.modifiers;
    if modifiers.any() {
        deliver_modifiers(state, keyboard, key, modifiers, true)?;
    }
    // A press with no matching release leaves the client believing the button
    // is still held, which breaks the very next interaction.
    for _ in 0..pointing.count.clamp(1, 3) {
        deliver_button(state, pointer, pointing.button, true);
        deliver_button(state, pointer, pointing.button, false);
    }
    if modifiers.any() {
        deliver_modifiers(state, keyboard, key, modifiers, false)?;
    }
    Ok(())
}

/// Hold or release the modifier keys of a chord.
fn deliver_modifiers(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    key: WindowKey,
    modifiers: crate::keys::Modifiers,
    pressed: bool,
) -> Result<(), String> {
    focus_window(state, keyboard, key)?;
    // Released in the reverse of the order they were pressed, so a client
    // tracking modifier state never sees an impossible intermediate — the same
    // ordering `tap` already keeps.
    let mut codes = modifier_codes(modifiers);
    if !pressed {
        codes.reverse();
    }
    for code in codes {
        hold_key(state, keyboard, code, pressed);
    }
    Ok(())
}

/// Press or release one evdev keycode, recording what is held.
fn hold_key(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    code: u32,
    pressed: bool,
) {
    use smithay::backend::input::KeyState;
    use smithay::input::keyboard::{FilterResult, Keycode};
    use smithay::utils::SERIAL_COUNTER;

    // The same evdev -> xkb offset `tap` applies. Sending the raw code here
    // would hold a key eight places along the keymap from the one asked for —
    // and because a held key produces no visible character, it would be wrong
    // silently.
    const OFFSET: u32 = 8;

    let time = state.now_ms();
    keyboard.input::<(), _>(
        state,
        Keycode::from(code + OFFSET),
        if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        },
        SERIAL_COUNTER.next_serial(),
        time,
        |_, _, _| FilterResult::Forward,
    );

    if pressed {
        if !state.held_keys.contains(&code) {
            state.held_keys.push(code);
        }
    } else {
        state.held_keys.retain(|held| *held != code);
    }
}

/// Deliver one instantaneous step of a stylus stroke.
fn deliver_stylus(
    state: &mut StageState,
    tablet: &smithay::wayland::tablet_manager::TabletHandle,
    tool: &smithay::wayland::tablet_manager::TabletToolHandle,
    step: StylusStep,
) -> Result<(), String> {
    use smithay::utils::SERIAL_COUNTER;

    let time = state.now_ms();
    match step {
        StylusStep::ProximityIn { window, at } => {
            let Some(managed) = state.window(window) else {
                return Err(format!("no window {window:?}"));
            };
            let surface = managed.toplevel.wl_surface().clone();
            let point = centre_of(at);
            tool.proximity_in(
                point,
                (surface, (0.0, 0.0).into()),
                tablet,
                SERIAL_COUNTER.next_serial(),
                time,
            );
        }
        StylusStep::Down => tool.tip_down(SERIAL_COUNTER.next_serial(), time),
        StylusStep::Motion {
            window,
            at,
            pressure,
            tilt,
        } => {
            let Some(managed) = state.window(window) else {
                return Err(format!("no window {window:?}"));
            };
            let surface = managed.toplevel.wl_surface().clone();
            let point = centre_of(at);
            tool.motion(
                point,
                Some((surface, (0.0, 0.0).into())),
                tablet,
                SERIAL_COUNTER.next_serial(),
                time,
            );
            // Sent after the motion, not before: the axes describe the point
            // just reported, and an application that reads them in the other
            // order attributes this sample's pressure to the previous position.
            tool.pressure(pressure);
            tool.tilt(tilt);
        }
        StylusStep::Up => tool.tip_up(time),
        StylusStep::ProximityOut => tool.proximity_out(time),
    }
    // No explicit frame: smithay's tablet tool emits one per event itself,
    // unlike the pointer handle where framing is the caller's job.
    Ok(())
}

/// Let go of every button and key this seat is holding.
///
/// Runs after every program. Iterating over a *copy* of the held lists because
/// each release mutates them, and releasing only what is actually held keeps a
/// spurious release — itself an event a client acts on — from being sent.
fn deliver_relax(
    state: &mut StageState,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
) {
    for button in state.held_buttons.clone() {
        deliver_button(state, pointer, button, false);
    }
    for code in state.held_keys.clone() {
        hold_key(state, keyboard, code, false);
    }
}

/// The centre of a rectangle, in compositor-logical coordinates.
fn centre_of(rect: Rect) -> smithay::utils::Point<f64, smithay::utils::Logical> {
    smithay::utils::Point::from((
        f64::from(rect.x) + f64::from(rect.width) / 2.0,
        f64::from(rect.y) + f64::from(rect.height) / 2.0,
    ))
}

/// How many logical pixels one wheel notch scrolls.
///
/// The conventional figure, and the one wlroots and smithay's own examples use.
/// It matters because `v120` and the continuous value have to agree: a client
/// that reads the discrete steps and one that reads the pixel value must scroll
/// by the same amount, or the same call moves a GTK list and an Android list by
/// visibly different distances.
const PIXELS_PER_NOTCH: f64 = 15.0;

/// Scroll at a point, as a wheel would.
fn deliver_scroll(
    state: &mut StageState,
    pointer: &smithay::input::pointer::PointerHandle<StageState>,
    key: WindowKey,
    at: Rect,
    amount: i32,
    axis: crate::program::Axis,
) -> Result<(), String> {
    use smithay::backend::input::{Axis, AxisSource};
    use smithay::input::pointer::AxisFrame;

    if amount == 0 {
        return Ok(());
    }
    // The pointer has to be over the thing being scrolled first: a wheel event
    // goes to whatever is under the cursor, so scrolling without moving there
    // scrolls whatever was last clicked instead.
    deliver_motion(state, pointer, key, at)?;

    // A horizontal wheel is what a tilt wheel or a two-finger sideways swipe
    // produces, and it is the only way to reach a wide table, a timeline or a
    // carousel. Clients read the two axes from the same frame, so the only
    // difference is which one carries the value.
    let axis = match axis {
        crate::program::Axis::Vertical => Axis::Vertical,
        crate::program::Axis::Horizontal => Axis::Horizontal,
    };

    // Delivered a notch at a time rather than as one large value. A real wheel
    // never sends 900 pixels in one frame, and a list that animates per notch
    // treats a single huge delta as one jump — losing the intermediate layout
    // that a virtualized list needs in order to realize its rows.
    let notches = (f64::from(amount).abs() / PIXELS_PER_NOTCH).ceil() as i32;
    let notches = notches.clamp(1, 40);
    let per_notch = f64::from(amount) / f64::from(notches);
    for _ in 0..notches {
        let time = state.now_ms();
        let frame = AxisFrame::new(time)
            .source(AxisSource::Wheel)
            .value(axis, per_notch)
            .v120(axis, (per_notch / PIXELS_PER_NOTCH * 120.0).round() as i32);
        pointer.axis(state, frame);
        pointer.frame(state);
    }
    Ok(())
}

/// Deliver one instantaneous step of a touch sequence.
fn deliver_touch(
    state: &mut StageState,
    touch: &smithay::input::touch::TouchHandle<StageState>,
    step: TouchStep,
) -> Result<(), String> {
    use smithay::backend::input::TouchSlot;
    use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
    use smithay::utils::SERIAL_COUNTER;

    let time = state.now_ms();
    match step {
        TouchStep::Down { window, at, slot } => {
            let Some(managed) = state.window(window) else {
                return Err(format!("no window {window:?}"));
            };
            let surface = managed.toplevel.wl_surface().clone();
            let location = smithay::utils::Point::from((f64::from(at.0), f64::from(at.1)));
            touch.down(
                state,
                Some((surface, (0.0, 0.0).into())),
                &DownEvent {
                    slot: TouchSlot::from(Some(slot)),
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            touch.frame(state);
        }
        TouchStep::Motion { window, at, slot } => {
            let Some(managed) = state.window(window) else {
                return Err(format!("no window {window:?}"));
            };
            let surface = managed.toplevel.wl_surface().clone();
            let location = smithay::utils::Point::from((f64::from(at.0), f64::from(at.1)));
            touch.motion(
                state,
                Some((surface, (0.0, 0.0).into())),
                &MotionEvent {
                    slot: TouchSlot::from(Some(slot)),
                    location,
                    time,
                },
            );
            touch.frame(state);
        }
        TouchStep::Up { slot } => {
            touch.up(
                state,
                &UpEvent {
                    slot: TouchSlot::from(Some(slot)),
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            touch.frame(state);
        }
        TouchStep::Cancel => {
            touch.cancel(state);
            touch.frame(state);
        }
    }
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
        // Real time and a fresh serial, for the same reason clicks carry them:
        // a client that measures key repeat, or that matches a request to the
        // event that authorized it, is reading both.
        let time = state.now_ms();
        keyboard.input::<(), _>(
            state,
            Keycode::from(code + OFFSET),
            pressed,
            smithay::utils::SERIAL_COUNTER.next_serial(),
            time,
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

/// Hold a chord down, or let it go.
///
/// The modifiers are pressed before the base key and released after it, so a
/// held `ctrl+shift` is genuinely held rather than tapped either side.
fn deliver_key_hold(
    state: &mut StageState,
    keyboard: &smithay::input::keyboard::KeyboardHandle<StageState>,
    key: WindowKey,
    stroke: &str,
    pressed: bool,
) -> Result<(), String> {
    focus_window(state, keyboard, key)?;
    let chord = crate::keys::parse(stroke).map_err(|error| error.to_string())?;
    let code = chord
        .key
        .evdev()
        .ok_or_else(|| format!("key {stroke:?} is not on the stage keyboard layout"))?;

    if pressed {
        for modifier in modifier_codes(chord.modifiers) {
            hold_key(state, keyboard, modifier, true);
        }
        hold_key(state, keyboard, code, true);
    } else {
        hold_key(state, keyboard, code, false);
        for modifier in modifier_codes(chord.modifiers).into_iter().rev() {
            hold_key(state, keyboard, modifier, false);
        }
    }
    Ok(())
}

/// Read the clipboard, either from our own copy or from whoever owns it.
fn read_clipboard(state: &mut StageState) -> Result<ClipboardRead, String> {
    if let Some(text) = &state.clipboard {
        return Ok(ClipboardRead::Text(Some(text.clone())));
    }

    let (read, write) = std::io::pipe().map_err(|error| format!("clipboard pipe: {error}"))?;
    let seat = state.seat.clone();
    match request_data_device_client_selection::<StageState>(
        &seat,
        "text/plain;charset=utf-8".to_owned(),
        write.into(),
    ) {
        Ok(()) => Ok(ClipboardRead::Pipe(read.into())),
        // No client owns a clipboard selection either, so the clipboard is
        // genuinely empty. That is an answer, not a failure.
        Err(_) => Ok(ClipboardRead::Text(None)),
    }
}

/// Resize the single output every window is given.
///
/// Each toplevel is reconfigured to the new size, because a client that is not
/// told has no reason to repaint and would keep drawing at the old dimensions
/// into a differently sized screen.
fn resize_output(state: &mut StageState, width: u32, height: u32) -> Result<(), String> {
    use smithay::output::{Mode, Scale};

    if width == 0 || height == 0 {
        return Err("a stage cannot be zero pixels across".into());
    }
    // An upper bound, because the size becomes a GPU allocation: the render
    // target is reallocated at this size and an unchecked value from the model
    // is a way to ask for a buffer that cannot exist.
    if width > 7680 || height > 4320 {
        return Err(format!(
            "{width}x{height} is past the largest stage we allocate (7680x4320)"
        ));
    }

    state.width = width as i32;
    state.height = height as i32;
    let mode = Mode {
        size: (width as i32, height as i32).into(),
        refresh: 60_000,
    };
    state.output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        None,
    );
    state.output.set_preferred(mode);

    for window in &mut state.windows {
        window.geometry = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        window.toplevel.with_pending_state(|pending| {
            pending.size = Some((width as i32, height as i32).into());
        });
        window.toplevel.send_configure();
    }
    state.dirty = true;
    Ok(())
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
        //
        // Deliberately not falling back to the clipboard automatically, even
        // though the seat now has one. Pasting means sending ctrl+v, and that
        // is a *convention* rather than something observable here — a field
        // that does not honour it would receive nothing while this reported
        // success, which is the failure class the whole subsystem exists to
        // remove. The remedy is named instead, and the model performs it
        // deliberately.
        return Err(format!(
            "character {character:?} is not on the stage's US keyboard layout. \
             Put the text on the clipboard with a `setClipboard` step and paste it \
             with `key: ctrl+v`, or drive this element at a rung that takes text \
             directly."
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
    state: &mut StageState,
    target: &mut Dmabuf,
    start: std::time::Instant,
) -> Result<(), String> {
    let outcome = draw(state, target, None);

    // After every render, without exception. See the module docs.
    let elapsed = start.elapsed();
    let time = elapsed.as_millis() as u32;
    for window in &state.windows {
        send_frames(window.toplevel.wl_surface(), time);
    }
    answer_presentation_feedback(state, elapsed, outcome.is_ok());
    outcome
}

/// Settle every outstanding `wp_presentation` request from this frame.
///
/// Advertising the global obliges us to reply to each request exactly once. A
/// client that asked for feedback and never hears back may hold off committing
/// its next frame — so a compositor that offers the global and then stays
/// silent is worse for that client than one that never offered it.
///
/// A failed render reports `discarded` rather than `presented`. The distinction
/// is the whole content of the protocol: `presented` asserts those pixels
/// reached a screen at that timestamp, and saying so about a frame that never
/// drew would corrupt exactly the pacing this is for.
fn answer_presentation_feedback(state: &mut StageState, elapsed: std::time::Duration, drawn: bool) {
    let mut feedback = OutputPresentationFeedback::new(&state.output);
    let output = state.output.clone();
    for window in &state.windows {
        take_presentation_feedback_surface_tree(
            window.toplevel.wl_surface(),
            &mut feedback,
            // One output, and every surface is on it — so this is not a lookup
            // so much as a statement of the stage's shape.
            |_, _| Some(output.clone()),
            // Nothing here is scanned out directly; the composite always goes
            // through the renderer into the offscreen buffer.
            |_, _| wp_presentation_feedback::Kind::empty(),
        );
    }

    if !drawn {
        feedback.discarded();
        return;
    }
    state.frame_sequence += 1;
    feedback.presented::<_, smithay::utils::Monotonic>(
        elapsed,
        Refresh::fixed(std::time::Duration::from_nanos(
            1_000_000_000_000 / STAGE_REFRESH_MHZ as u64,
        )),
        state.frame_sequence,
        wp_presentation_feedback::Kind::empty(),
    );
}

/// Draw the stage, or one window of it.
///
/// `only` restricts the composite to a single window's surface tree, which is
/// what makes per-window capture mean anything. Cropping the finished composite
/// is not enough: every toplevel on this stage is given the whole output, so a
/// crop of the screen is the screen — and a rung-3 surface reading text off it
/// would read *every* application's text, not its own.
fn draw(
    state: &mut StageState,
    target: &mut Dmabuf,
    only: Option<WindowKey>,
) -> Result<(), String> {
    // Disjoint field borrows: the renderer is taken mutably while the window
    // list is read. This is why the renderer lives in the state at all — a
    // dmabuf import has to reach it from a protocol handler — and it costs
    // nothing here beyond naming the two halves.
    let StageState {
        renderer,
        windows,
        width,
        height,
        ..
    } = state;
    let size: Size<i32, smithay::utils::Physical> = (*width, *height).into();
    let full = Rectangle::from_size(size);

    let mut framebuffer = renderer
        .bind(target)
        .map_err(|error| format!("bind for render: {error}"))?;
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = windows
        .iter()
        .filter(|window| only.is_none_or(|key| window.key == key))
        .flat_map(|window| {
            let surface = window.toplevel.wl_surface();
            // The toplevel, then everything popped up over it.
            //
            // Order matters and is the reason this is not one call: render
            // elements are drawn front-to-back, so a menu has to come *first*
            // in the list to land on top of the window that opened it. A popup
            // is a separate xdg surface rather than a subsurface, so walking
            // the toplevel's tree alone never reaches it — which is exactly why
            // menus used to be invisible.
            //
            // Popups are collected under the same `only` filter as their
            // parent, so a per-window capture of the window that owns a menu
            // contains the menu, and a capture of its neighbour does not.
            let popups: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                PopupManager::popups_for_surface(surface)
                    .flat_map(|(popup, offset)| {
                        // The offset is the popup's position relative to the
                        // toplevel, which is what makes a submenu land beside
                        // its parent item rather than at the window's corner.
                        let location = offset - popup.geometry().loc;
                        render_elements_from_surface_tree(
                            renderer,
                            popup.wl_surface(),
                            (location.x, location.y),
                            1.0,
                            1.0,
                            Kind::Unspecified,
                        )
                    })
                    .collect();
            popups.into_iter().chain(render_elements_from_surface_tree(
                renderer,
                surface,
                (0, 0),
                1.0,
                1.0,
                Kind::Unspecified,
            ))
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
    // Wait on the fence rather than dropping it. `capture` reads this very
    // buffer back with `copy_framebuffer`, and reading before the GPU has
    // finished writing returns a half-drawn frame — a screenshot that is
    // plausible, wrong, and reported as a success.
    frame
        .finish()
        .map_err(|error| format!("finish: {error}"))?
        .wait()
        .map_err(|error| format!("wait for the frame to finish: {error}"))?;
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
    target: &mut Dmabuf,
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
