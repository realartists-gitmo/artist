//! The `computer` tool: the model-facing surface of the whole subsystem.
//!
//! Thin by design — parse, dispatch, format. Every decision worth making lives
//! in [`crate::anchors`], [`crate::render`] or a backend; this module only
//! routes between them and owns the surface registry.
//!
//! The registry copies [`artist_tools::BashTool`]'s session model where the two
//! genuinely share a problem — a `DashMap` keyed by id, `short_id` ids, and
//! resources released on drop — and not where they do not. `BashTool` reaps
//! exited shells because a shell exits on its own; a surface does not, so there
//! is nothing to tombstone and `list` does no reaping. Claiming otherwise in a
//! comment is worse than not having it: the next reader looks for the mechanism
//! and finds nothing, and cannot tell whether it was removed or never written.

use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use rig_core::tool::{PortableTool, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use artist_session::{ComputerActed, ComputerObserved, ComputerStep, Recorder};

use crate::anchors::AnchorBook;
use crate::model::SurfaceId;
use crate::program::{Program, SettleOutcome, StepError};
use crate::render;
use crate::surface::{ProgramReport, Surface, run_program};

/// A surface plus the anchor state that names its elements.
///
/// They are locked together because they must move together: resolving an
/// anchor against a book that has drifted from its surface is exactly the
/// silent-wrong-target failure the design exists to prevent.
pub struct Attached {
    pub surface: Arc<dyn Surface>,
    pub book: Mutex<AnchorBook>,
}

/// Every surface this session can drive, plus the display they run on.
#[derive(Clone)]
pub struct SurfaceRegistry {
    surfaces: Arc<DashMap<String, Arc<Attached>>>,
    /// Ids currently being claimed by an in-flight open.
    ///
    /// A surface takes real time to become driveable — a browser has to start
    /// and bind a debugging port, a toolkit has to build its accessibility tree
    /// — and until `attach` lands there is nothing in `surfaces` to collide
    /// with. Two opens naming the same id would both proceed, and the second's
    /// `attach` would silently replace the first's surface while the first
    /// caller still holds its id.
    opening: Arc<DashSet<String>>,
    /// The graphical half. Shared so a stage opened on one turn is still there
    /// on the next.
    host: Arc<crate::host::Host>,
    /// Held for the duration of an action program.
    ///
    /// A stage has one seat and one keyboard focus. Delivering a keystroke is
    /// "focus this window, then send" — two programs interleaving between those
    /// two steps would land A's keystroke in B's window. The per-surface anchor
    /// lock does not help, because the surfaces are different; the contended
    /// resource is the *stage*.
    ///
    /// Not reachable while the agent issues one tool call at a time, which is
    /// exactly why it is worth holding now: the failure would first appear
    /// under concurrency, as a keystroke silently going to the wrong window.
    input: Arc<Mutex<()>>,
}

impl Default for SurfaceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceRegistry {
    /// A registry whose stage directory comes from `$XDG_RUNTIME_DIR`.
    ///
    /// When that is unset the registry still works for every rung that needs no
    /// display — PTY, CDP-attach, adapters — and `ensure_stage` reports why it
    /// cannot bring one up. That is better than either refusing outright or
    /// silently using a world-readable directory.
    pub fn new() -> Self {
        Self::with_host_opt(
            default_state_dir(),
            Default::default(),
            crate::host::DEFAULT_SCREEN,
        )
    }

    /// The registry the agent actually runs with.
    ///
    /// Discovers this project's rung-0 adapters and takes the configured screen
    /// size. Both were previously reachable only through constructors nothing
    /// called, which is why `select()` could never return `Programmatic` in the
    /// shipped agent and `[computer] screen` resolved into a config field that
    /// no code read.
    pub fn for_project(project: &std::path::Path, screen: (i32, i32)) -> Self {
        Self::with_host_opt(
            default_state_dir(),
            crate::ladder::adapters::AdapterSet::discover(project),
            screen,
        )
    }

    pub fn with_host(
        state_dir: impl Into<std::path::PathBuf>,
        adapters: crate::ladder::adapters::AdapterSet,
    ) -> Self {
        Self::with_host_opt(
            Some(state_dir.into()),
            adapters,
            crate::host::DEFAULT_SCREEN,
        )
    }

    fn with_host_opt(
        state_dir: Option<std::path::PathBuf>,
        adapters: crate::ladder::adapters::AdapterSet,
        screen: (i32, i32),
    ) -> Self {
        Self {
            surfaces: Arc::new(DashMap::new()),
            opening: Arc::new(DashSet::new()),
            host: Arc::new(crate::host::Host::sized(state_dir, adapters, screen)),
            input: Arc::new(Mutex::new(())),
        }
    }

    /// Take the stage's input lease for the duration of a program.
    pub async fn input_lease(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.input.lock().await
    }

    pub fn host(&self) -> &crate::host::Host {
        &self.host
    }

    /// An independent registry with its own stage, for a subagent.
    ///
    /// A stage is one seat with one keyboard focus, so siblings sharing one
    /// would serialize behind the input lease — safe, but useless: a delegate
    /// that blocks for minutes waiting for the seat is not doing parallel work.
    /// Each gets its own display instead.
    ///
    /// Cheap to create because a stage is lazy: a delegate that never touches a
    /// GUI never starts a compositor. And it is genuinely independent — dropping
    /// the child registry when the delegate finishes tears down its compositor,
    /// session bus and browser profile with it.
    pub fn for_delegate(&self) -> Self {
        Self::with_host_opt(
            self.host.state_dir().map(std::path::Path::to_owned),
            self.host.adapters().clone(),
            self.host.screen(),
        )
    }

    /// Claim an id for a surface that is still being brought up.
    ///
    /// Returns `false` when another open already holds it, which the caller
    /// must treat as "someone else is making this" rather than racing it.
    pub fn claim(&self, id: &str) -> bool {
        !self.surfaces.contains_key(id) && self.opening.insert(id.to_owned())
    }

    /// Release a claim that will never become a surface.
    pub fn abandon(&self, id: &str) {
        self.opening.remove(id);
    }

    /// Whether an id is spoken for, either open or opening.
    pub fn is_claimed(&self, id: &str) -> bool {
        self.surfaces.contains_key(id) || self.opening.contains(id)
    }

    pub fn attach(&self, surface: Arc<dyn Surface>) -> String {
        let id = surface.id().as_str().to_owned();
        self.surfaces.insert(
            id.clone(),
            Arc::new(Attached {
                surface,
                book: Mutex::new(AnchorBook::new()),
            }),
        );
        self.opening.remove(&id);
        id
    }

    pub fn get(&self, id: &str) -> Option<Arc<Attached>> {
        self.surfaces.get(id).map(|entry| Arc::clone(entry.value()))
    }

    pub fn close(&self, id: &str) -> bool {
        self.surfaces.remove(id).is_some()
    }

    pub fn list(&self) -> Vec<(String, String, u8)> {
        let mut rows: Vec<_> = self
            .surfaces
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.value().surface.title(),
                    entry.value().surface.rung().as_u8(),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }
}

#[derive(Clone)]
pub struct ComputerTool {
    registry: SurfaceRegistry,
    /// Where `computer.*` events go.
    ///
    /// The tool records its own events, exactly as `TodoTool` does. Without
    /// this the events were declared, consumed by `artist computer log` and
    /// `distill`, and never produced — so both commands always reported nothing
    /// and macro distillation had no input at all.
    ///
    /// Deliberately operational: no `history.rs` arm reads these, so they cost
    /// the model no context. The observation text the model sees is the tool
    /// result; what is recorded here is the metadata that makes a run auditable
    /// and replayable afterwards.
    recorder: Recorder,
    /// Where screenshots are kept.
    ///
    /// `None` for a session that records nothing: the model still gets the
    /// picture inline, there is simply nothing to retrieve it from later.
    attachments: Option<artist_session::AttachmentStore>,
}

impl ComputerTool {
    /// A tool that records nothing — for tests, and for callers with no session.
    pub fn new(registry: SurfaceRegistry) -> Self {
        Self::with_recorder(registry, Recorder::noop(), None)
    }

    pub fn with_recorder(
        registry: SurfaceRegistry,
        recorder: Recorder,
        attachments: Option<artist_session::AttachmentStore>,
    ) -> Self {
        Self {
            registry,
            recorder,
            attachments,
        }
    }

    pub fn registry(&self) -> &SurfaceRegistry {
        &self.registry
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerArgs {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    surface: Option<String>,
    /// Force a complete observation rather than a delta.
    #[serde(default)]
    full: bool,
    /// For `launch`: the command to run on a terminal surface. Named `command`
    /// rather than `program` because `program` flattens into the action program
    /// below and the two would collide.
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    /// Launch into the isolated display rather than onto a terminal.
    #[serde(default)]
    gui: bool,
    #[serde(flatten)]
    program: Option<Program>,
}

impl PortableTool for ComputerTool {
    const NAME: &'static str = "computer";
    type Error = StepError;
    type Args = ComputerArgs;
    type Output = ToolOutput;

    fn description(&self) -> String {
        // Every word of model-facing guidance lives here: the system prompt has
        // no per-tool section, so this is the only channel.
        r#"Drive an application through its most efficient interface.

Modes:
- `surfaces` — list what can be driven, with each surface's id and abstraction level.
- `launch` — start a program on a new surface and observe it. Terminal programs (`htop`, `vim notes.md`) run on a PTY; pass `gui: true` for a browser or desktop application, which runs on an isolated display of its own and never touches the user's screen or keyboard. For a plain command whose output you just want to read, `bash` is simpler.
- `observe` — read a surface. The first look returns everything; later looks return only what CHANGED (`+` added, `~` changed, `-` gone). Pass `full: true` to re-read everything.
- `do` — run a short program of steps against a surface.
- `screenshot` — a picture of a surface, alongside the usual structured view. Use it only when the structured view cannot answer the question — a chart, a canvas, a rendering fault, "does this look right". It costs far more context than `observe` and you still cannot act on a coordinate.
- `close` — release a surface.

Naming things: every element is shown as `role "name" (anchor)`. Use the bare anchor token to refer to it. NEVER use screen coordinates — they are deliberately not shown, and there is no way to act on one.

Every step that names an anchor must also carry a `label`: your own copy of that element's name from the most recent observation. It is checked against reality before anything runs, so a wrong anchor fails loudly instead of clicking the wrong thing.

`expect` is required, and is stated by NAME rather than by anchor — an element that appears because of your program has no anchor you could know yet. One of:
- `{"appears":"Message sent"}` — this should be on the surface afterwards
- `{"gone":"Compose"}` — this should no longer be there
- `{"still":{"anchor":"kv7","label":"Save"}}` — this exact element should still be there and still be called that

If an anchor is rejected as stale, do not retry it and do not guess another — `observe` that surface again to get current anchors.

Steps stop at the first failure, and a guardrail can abort the whole program before ANY step runs. So put an irreversible step (delete, send, pay, confirm) in its own single-step call, after the rest has already succeeded.

The steps:
- `{"click":{"anchor":…,"label":…}}` — activate an element.
- `{"type":{"anchor":…,"label":…,"text":…}}` — REPLACES what is in the field. Pass `"clear":false` to append instead.
- `{"key":"Enter"}` — send a key to whatever holds focus. When the key will activate something in particular, name it: `{"key":{"chord":"Enter","label":"Delete account"}}`. That claim is checked against the focused element, and it is what lets a guardrail see a destructive Enter coming.
- `{"scroll":{"amount":3}}` — positive scrolls down.
- `{"navigate":{"url":…}}`, `{"back":{}}`, `{"forward":{}}` — browser surfaces. Use these rather than launching a second browser.
- `{"invoke":{"anchor":…,"label":…,"action":…}}` — run one of the verbs an element lists after its name, e.g. a tab's `close`.

Example:
{"mode":"do","surface":"pty:1",
 "steps":[{"click":{"anchor":"kv7","label":"Compose"}},
          {"type":{"anchor":"m2q","label":"To","text":"adam@example.com"}},
          {"key":"Enter"}],
 "settle":{"until":"quiet","timeoutMs":3000},
 "expect":{"appears":"Message sent"}}"#
            .to_owned()
    }

    fn parameters(&self) -> Value {
        let target = json!({
            "type": "object",
            "properties": {
                "anchor": {"type": "string", "description": "A token from the most recent observation of this surface."},
                "label": {"type": "string", "description": "That element's name, copied from the observation. Checked before the step runs."}
            },
            "required": ["anchor"],
            "additionalProperties": false
        });
        json!({
            "type": "object",
            "properties": {
                "mode": {"enum": ["surfaces", "launch", "observe", "do", "screenshot", "close"], "description": "Defaults to `do` when steps are given, otherwise `surfaces`."},
                "surface": {"type": "string", "description": "Surface id, from `surfaces`."},
                "command": {"type": "string", "description": "For `launch`: the command to run, e.g. `htop`, `vim notes.md`, or `chromium https://example.com`."},
                "cwd": {"type": "string", "description": "For `launch`: the working directory."},
                "gui": {"type": "boolean", "default": false, "description": "For `launch`: run the program on the isolated display instead of a terminal. Use this for browsers and desktop applications."},
                "full": {"type": "boolean", "default": false, "description": "Return the whole surface rather than a delta."},
                "steps": {
                    "type": "array",
                    "description": "Actions, run in order, stopping at the first failure.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "click": target,
                            "type": {
                                "type": "object",
                                "properties": {
                                    "anchor": {"type": "string"},
                                    "label": {"type": "string"},
                                    "text": {"type": "string"},
                                    "clear": {"type": "boolean", "default": true, "description": "Replace the field's contents. Set false to append."}
                                },
                                "required": ["anchor", "text"],
                                "additionalProperties": false
                            },
                            "key": {
                                "description": "A key or chord, e.g. `Enter`, `ctrl+c`, `Down`. Give the object form to name what the key will activate.",
                                "oneOf": [
                                    {"type": "string"},
                                    {
                                        "type": "object",
                                        "properties": {
                                            "chord": {"type": "string"},
                                            "label": {"type": "string", "description": "The focused element's name, checked before the key is sent."}
                                        },
                                        "required": ["chord", "label"],
                                        "additionalProperties": false
                                    }
                                ]
                            },
                            "scroll": {
                                "type": "object",
                                "description": "Omit the anchor to scroll the surface; name an element to scroll the container holding it.",
                                "properties": {
                                    "anchor": {"type": "string"},
                                    "label": {"type": "string"},
                                    "amount": {"type": "integer", "description": "Positive scrolls down."}
                                },
                                "required": ["amount"],
                                "additionalProperties": false
                            },
                            "navigate": {
                                "type": "object",
                                "properties": {"url": {"type": "string"}},
                                "required": ["url"],
                                "additionalProperties": false
                            },
                            "back": {"type": "object", "additionalProperties": false},
                            "forward": {"type": "object", "additionalProperties": false},
                            "invoke": {
                                "type": "object",
                                "description": "Run one of the verbs an element lists after its name.",
                                "properties": {
                                    "anchor": {"type": "string"},
                                    "label": {"type": "string"},
                                    "action": {"type": "string"}
                                },
                                "required": ["anchor", "action"],
                                "additionalProperties": false
                            }
                        },
                        "additionalProperties": false
                    }
                },
                "settle": {
                    "type": "object",
                    "properties": {
                        "until": {"enum": ["quiet", "networkIdle", "none"], "description": "How to know the surface finished reacting. Never use a sleep."},
                        "timeoutMs": {"type": "integer", "minimum": 1}
                    },
                    "additionalProperties": false
                },
                "expect": {
                    "type": "object",
                    "description": "What should be true when the program finishes. Stated by name, not by anchor — an element that appears because of your program has no anchor you could know yet.",
                    "properties": {
                        "appears": {"type": "string", "description": "This element should be on the surface afterwards."},
                        "gone": {"type": "string", "description": "This element should no longer be there."},
                        "still": target
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        })
    }

    async fn call(&self, args: ComputerArgs) -> Result<ToolOutput, StepError> {
        let mode = args.mode.clone().unwrap_or_else(|| {
            if args.program.is_some() {
                "do".to_owned()
            } else {
                "surfaces".to_owned()
            }
        });

        match mode.as_str() {
            "surfaces" => Ok(ToolOutput::text(self.render_surfaces())),
            "launch" => {
                let command = args.command.clone().ok_or_else(|| {
                    StepError::Backend("`launch` needs a `command` to run".into())
                })?;
                // A graphical launch goes to the stage and gets whichever rung
                // the probe finds; a terminal launch gets a PTY. Explicit rather
                // than sniffed, because guessing wrong means either an invisible
                // window or a display brought up for `ls`.
                let id = if args.gui {
                    let mut words = split_command(&command);
                    if words.is_empty() {
                        return Err(StepError::Backend("`command` is empty".into()));
                    }
                    let program = words.remove(0);
                    let surface = self
                        .registry
                        .host()
                        .launch(&program, &words, args.cwd.as_deref().map(std::path::Path::new))
                        .await?;
                    let id = self.registry.attach(surface);
                    // A browser yields two surfaces at two rungs; attach the
                    // chrome half too so tabs are addressable.
                    if let Some(extra) = self.registry.host().take_pending_surface().await {
                        self.registry.attach(extra);
                    }
                    id
                } else {
                    // The id is claimed before the spawn, which can take
                    // seconds: until `attach` lands there is nothing in the map
                    // for a second open to collide with, so two would both
                    // proceed and the later `attach` would silently replace the
                    // earlier surface under a caller still holding its id.
                    let id = surface_id("term").as_str().to_owned();
                    if !self.registry.claim(&id) {
                        return Err(StepError::Backend(format!(
                            "{id} is already being opened"
                        )));
                    }
                    let surface = crate::surface::pty::PtySurface::spawn(
                        id.clone(),
                        &command,
                        args.cwd.as_deref().map(std::path::Path::new),
                        40,
                        120,
                    )
                    .inspect_err(|_| self.registry.abandon(&id))?;
                    self.registry.attach(Arc::new(surface))
                };
                let attached = self.attached(&id)?;
                // Wait for first paint rather than sleeping a fixed interval.
                // A program that has just started has usually drawn nothing
                // yet, and how long it takes depends on the program and on how
                // loaded the machine is — a fixed delay is either too short
                // (an empty first observation) or wasted time on every launch.
                let snapshot = wait_for_first_paint(attached.surface.as_ref()).await?;
                let mut book = attached.book.lock().await;
                let observed = book.observe(&snapshot, true);
                let rendered = render::observation(&id, &observed, None);
                self.record_observation(&id, attached.surface.rung(), &observed, &rendered, None);
                Ok(ToolOutput::text(format!(
                    "launched {command:?} as {id}\n\n{rendered}"
                )))
            }
            "close" => {
                let id = self.require_surface(&args)?;
                let closed = self.registry.close(&id);
                Ok(ToolOutput::text(if closed {
                    format!("closed {id}")
                } else {
                    format!("no such surface: {id}")
                }))
            }
            "observe" => {
                let id = self.require_surface(&args)?;
                let attached = self.attached(&id)?;
                // `full` is not only a rendering choice on a streaming surface:
                // it is the difference between "what is new" and "everything
                // still held", and the latter is the only way back after an
                // observation has been elided.
                let snapshot = if args.full {
                    attached.surface.snapshot_full().await?
                } else {
                    attached.surface.snapshot().await?
                };
                let mut book = attached.book.lock().await;
                let observed = book.observe(&snapshot, args.full);
                let rendered = render::observation(&id, &observed, None);
                self.record_observation(&id, attached.surface.rung(), &observed, &rendered, None);
                Ok(ToolOutput::text(rendered))
            }
            "do" => {
                let id = self.require_surface(&args)?;
                let program = args.program.ok_or_else(|| {
                    StepError::Backend(
                        "mode `do` needs `steps` and `expect`; see the tool description".into(),
                    )
                })?;
                if program.steps.is_empty() {
                    return Err(StepError::Backend("`steps` must not be empty".into()));
                }
                let attached = self.attached(&id)?;
                // Order matters: the stage-wide input lease before the
                // per-surface anchor lock, always, or two programs on different
                // surfaces could deadlock taking them in opposite orders.
                let _input = self.registry.input_lease().await;
                let mut book = attached.book.lock().await;
                let report = run_program(attached.surface.as_ref(), &mut book, &program).await?;
                self.record_program(&id, &program, &report);
                Ok(ToolOutput::text(render_report(&id, &report)))
            }
            // Rung 3, and deliberately opt-in. A picture costs far more context
            // than the structured view and cannot be acted on — there is no way
            // to click a coordinate — so it is for the cases the tree genuinely
            // cannot answer: a rendering fault, a canvas, a chart.
            "screenshot" => {
                let id = self.require_surface(&args)?;
                let attached = self.attached(&id)?;
                let Some(frame) = attached.surface.pixels().await? else {
                    return Err(StepError::Backend(format!(
                        "{id} has no picture to take — it is a {} surface, which has geometry \
                         only where a display is involved. Use mode=\"observe\" to read it.",
                        attached.surface.rung().label()
                    )));
                };
                let png = frame.to_png().map_err(StepError::Backend)?;
                let digest = self.store_image(&png)?;

                // The structured view rides along, because a picture alone
                // gives the model nothing it can name in a later step.
                let snapshot = attached.surface.snapshot().await?;
                let mut book = attached.book.lock().await;
                let observed = book.observe(&snapshot, args.full);
                let rendered = render::observation(&id, &observed, digest.as_deref());
                self.record_observation(
                    &id,
                    attached.surface.rung(),
                    &observed,
                    &rendered,
                    digest.clone(),
                );

                Ok(ToolOutput::content(rig_core::OneOrMany::many([
                    rig_core::completion::message::ToolResultContent::text(format!(
                        "{rendered}\n{}×{} picture of {id}",
                        frame.width, frame.height
                    )),
                    rig_core::completion::message::ToolResultContent::image_base64(
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png),
                        Some(rig_core::completion::message::ImageMediaType::PNG),
                        None,
                    ),
                ])
                .expect("two blocks")))
            }
            other => Err(StepError::Backend(format!(
                "unknown mode {other:?}; expected surfaces, launch, observe, do, screenshot or close"
            ))),
        }
    }
}

impl ComputerTool {
    fn require_surface(&self, args: &ComputerArgs) -> Result<String, StepError> {
        args.surface.clone().ok_or_else(|| {
            StepError::Backend(format!(
                "this mode needs a `surface`. Available:\n{}",
                self.render_surfaces()
            ))
        })
    }

    fn attached(&self, id: &str) -> Result<Arc<Attached>, StepError> {
        self.registry.get(id).ok_or_else(|| {
            StepError::Backend(format!(
                "no surface {id:?}. Available:\n{}",
                self.render_surfaces()
            ))
        })
    }

    /// Keep a durable copy of a frame, and name it.
    ///
    /// The model gets the picture inline; this is the copy `artist computer
    /// frame <sha>` can hand back afterwards, and the one the observation
    /// sentinel names so an elided observation still says what was seen. A store
    /// that fails is not worth failing the screenshot over — the model has the
    /// image either way — so it degrades to an unnamed frame.
    fn store_image(&self, png: &[u8]) -> Result<Option<String>, StepError> {
        let Some(store) = &self.attachments else {
            return Ok(None);
        };
        match store.put(png) {
            Ok(digest) => Ok(Some(digest)),
            Err(error) => {
                eprintln!("artist: could not store a computer-use frame: {error}");
                Ok(None)
            }
        }
    }

    /// Record one look at a surface.
    ///
    /// Metadata only, deliberately: the node text already reached the model as
    /// the tool result, and duplicating it here would grow the log with every
    /// look at an unchanged screen. `bytes` is what that result actually cost,
    /// which is the number worth having when deciding whether decay is earning
    /// its keep.
    fn record_observation(
        &self,
        surface: &str,
        rung: crate::model::Rung,
        observed: &crate::anchors::Observation,
        rendered: &str,
        image: Option<String>,
    ) {
        self.recorder.record(ComputerObserved {
            internal_call_id: artist_tools::short_id("obs"),
            surface: surface.to_owned(),
            epoch: observed.epoch,
            rung: rung.as_u8(),
            full: observed.full,
            nodes: observed.entries.len() as u32,
            bytes: rendered.len() as u64,
            image,
        });
    }

    /// Record one program, as executed.
    ///
    /// This is `distill`'s only input. Every field a replay needs has to survive
    /// here — the payload especially, without which a `key` step replays as an
    /// empty chord and a `type` step types nothing, both reporting success.
    fn record_program(&self, surface: &str, program: &Program, report: &ProgramReport) {
        self.recorder.record(ComputerActed {
            internal_call_id: artist_tools::short_id("act"),
            surface: surface.to_owned(),
            epoch: report.observation.epoch,
            steps: report
                .steps
                .iter()
                .map(|step| ComputerStep {
                    action: step.action.to_owned(),
                    anchor: step.anchor.clone(),
                    label: step.label.clone(),
                    resolved_name: step.resolved_name.clone(),
                    payload: step.payload.clone(),
                    outcome: step.outcome.clone(),
                })
                .collect(),
            settled_ms: report.settled.as_ref().and_then(|outcome| match outcome {
                SettleOutcome::Settled { after_ms } | SettleOutcome::TimedOut { after_ms } => {
                    Some(*after_ms)
                }
                SettleOutcome::Unsupported => None,
            }),
            expect: program.expect.label().map(str::to_owned),
            expect_met: report.expect_met,
            failed_step: report.failed_step,
        });
    }

    /// The surface list, including what each one can be asked to do.
    ///
    /// `Caps` exists so the tool can refuse rather than pretend, and it was
    /// never shown — so the model discovered that a terminal has no pointer by
    /// clicking one and reading the error, at the cost of a round trip every
    /// time. Saying it up front is strictly cheaper.
    fn render_surfaces(&self) -> String {
        let mut rows: Vec<(String, String, u8, String)> = self
            .registry
            .surfaces
            .iter()
            .map(|entry| {
                let surface = &entry.value().surface;
                (
                    entry.key().clone(),
                    surface.title(),
                    surface.rung().as_u8(),
                    verbs(&surface.caps()),
                )
            })
            .collect();
        if rows.is_empty() {
            return "no surfaces are open".to_owned();
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows.into_iter()
            .map(|(id, title, rung, verbs)| format!("{id}\trung {rung}\t{title}\t[{verbs}]"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Split a command line into words, honouring quotes.
///
/// `split_whitespace` breaks the two things a GUI launch is most likely to
/// carry: a URL with a query string is fine, but a path with a space in it or a
/// quoted argument is silently torn into pieces and the application is started
/// with arguments nobody wrote. Not a shell — no expansion, no globbing, no
/// substitution — deliberately: this splits a command, it does not interpret one.
fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;

    for character in command.chars() {
        match (quote, character) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => current.push(c),
            (None, c @ ('\'' | '"')) => {
                // An empty quoted argument is still an argument.
                any = true;
                quote = Some(c);
            }
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() || any {
                    words.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if !current.is_empty() || any {
        words.push(current);
    }
    words
}

/// The step verbs a surface actually accepts.
fn verbs(caps: &crate::model::Caps) -> String {
    let mut verbs = Vec::new();
    for (supported, verb) in [
        (caps.click, "click"),
        (caps.type_text, "type"),
        (caps.key, "key"),
        (caps.scroll, "scroll"),
        (caps.pixels, "screenshot"),
    ] {
        if supported {
            verbs.push(verb);
        }
    }
    if verbs.is_empty() {
        return "observe only".to_owned();
    }
    verbs.join(" ")
}

/// Render a finished program for the model.
///
/// A failure reports which step failed and why, then the surface as it actually
/// is — so the next move is informed by reality rather than by the model's
/// stale belief about it.
pub fn render_report(surface: &str, report: &crate::surface::ProgramReport) -> String {
    let mut out = String::new();
    for (index, step) in report.steps.iter().enumerate() {
        let target = step
            .label
            .as_deref()
            .or(step.resolved_name.as_deref())
            .unwrap_or("");
        out.push_str(&format!(
            "{}. {} {target:?} — {}\n",
            index + 1,
            step.action,
            step.outcome
        ));
    }
    if let Some(failed) = report.failed_step {
        out.push_str(&format!(
            "\nstopped at step {}; the remaining steps did not run.\n",
            failed + 1
        ));
    }
    match report.expect_met {
        Some(true) => out.push_str("expect: met\n"),
        Some(false) => out.push_str(
            "expect: NOT met — the program finished somewhere other than you predicted. Read the observation below before acting again.\n",
        ),
        None => {}
    }
    if let Some(settled) = &report.settled {
        out.push_str(&format!("settle: {settled:?}\n"));
    }
    out.push('\n');
    out.push_str(&render::observation(surface, &report.observation, None));
    out
}

/// Where a stage keeps its sockets.
///
/// `$XDG_RUNTIME_DIR` rather than the project's state directory or the shared
/// temp dir, for three reasons that all matter here:
///
/// * **Length.** A stage's Wayland and D-Bus sockets are `AF_UNIX` paths, and
///   `sun_path` caps out around 108 bytes. A project-keyed config path plus a
///   stage id can overrun that, and the failure — a bind error deep inside the
///   compositor — points nowhere near the cause.
/// * **Permissions.** The runtime dir is already `0700` and on tmpfs. A stage
///   shares the user's `$HOME`, so its control sockets should not sit in a
///   world-traversable directory where another user could pre-create paths.
/// * **Lifetime.** It is cleared on logout, so a crashed stage leaves nothing
///   behind.
///
/// Process-scoped, because two agents must never land on the same stage
/// directory: they would contend for the same sockets and browser profile.
/// There is deliberately **no fallback to `/tmp`.** A stage directory holds the
/// Wayland socket (and with it the agent's clipboard), the private D-Bus socket,
/// and a browser profile containing the user's cookies and session tokens. In a
/// world-traversable directory those are readable by any local user, and
/// `create_dir_all` on an enumerable path can be pre-created or symlinked by an
/// attacker who wins the race. Refusing to start is the correct outcome.
pub fn default_state_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .filter(|dir| dir.is_absolute())?;
    Some(base.join(format!("artist-stage-{}", std::process::id())))
}

/// Build a surface id that is stable and readable in a transcript.
pub fn surface_id(kind: &str) -> SurfaceId {
    SurfaceId::new(artist_tools::short_id(kind))
}

/// Poll until a freshly launched surface has drawn something, or give up.
///
/// Returning the empty snapshot on timeout rather than erroring is deliberate:
/// a program that legitimately paints nothing (a daemon, a silent command) has
/// still launched, and the model is better served by an honest empty
/// observation than by a failure that suggests the launch itself went wrong.
async fn wait_for_first_paint(surface: &dyn Surface) -> Result<crate::Snapshot, StepError> {
    use std::time::{Duration, Instant};

    const DEADLINE: Duration = Duration::from_secs(5);
    const POLL: Duration = Duration::from_millis(25);

    let started = Instant::now();
    loop {
        let snapshot = surface.snapshot().await?;
        // Named nodes only. A terminal on the alternate screen always reports a
        // cursor, which carries a value from the very first frame — counting it
        // would make every launch look painted before anything was drawn.
        let painted = snapshot
            .nodes
            .iter()
            .any(|node| !node.name.trim().is_empty());
        if painted || started.elapsed() >= DEADLINE {
            return Ok(snapshot);
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::pty::PtySurface;

    fn registry_with_terminal() -> (SurfaceRegistry, String) {
        let registry = SurfaceRegistry::new();
        let surface = PtySurface::detached("pty:1", 6, 40).with_title("scripted terminal");
        surface.feed(b"\x1b[?1049h");
        surface.feed(b"\x1b[1;1HREADY");
        let id = registry.attach(Arc::new(surface));
        (registry, id)
    }

    async fn call(tool: &ComputerTool, value: Value) -> String {
        let args = serde_json::from_value(value).unwrap();
        tool.call(args).await.unwrap().render()
    }

    #[tokio::test]
    async fn surfaces_lists_what_can_be_driven() {
        let (registry, _) = registry_with_terminal();
        let tool = ComputerTool::new(registry);
        let out = call(&tool, json!({"mode": "surfaces"})).await;
        assert!(out.contains("pty:1"));
        assert!(out.contains("scripted terminal"));
        assert!(out.contains("rung 1"));
    }

    #[tokio::test]
    async fn observe_is_full_once_then_delta() {
        let (registry, id) = registry_with_terminal();
        let tool = ComputerTool::new(registry.clone());

        let first = call(&tool, json!({"mode": "observe", "surface": id})).await;
        assert!(first.contains("READY"), "{first}");

        let second = call(&tool, json!({"mode": "observe", "surface": id})).await;
        assert!(
            second.contains("(no change)"),
            "an unchanged surface must not be re-dumped: {second}"
        );
    }

    #[tokio::test]
    async fn a_missing_surface_names_the_ones_that_exist() {
        let (registry, _) = registry_with_terminal();
        let tool = ComputerTool::new(registry);
        let args = serde_json::from_value(json!({"mode": "observe", "surface": "nope"})).unwrap();
        let error = tool.call(args).await.unwrap_err().to_string();
        assert!(error.contains("no surface"));
        assert!(error.contains("pty:1"), "the error must be actionable: {error}");
    }

    #[tokio::test]
    async fn a_program_reports_steps_and_verifies_expect() {
        let (registry, id) = registry_with_terminal();
        let tool = ComputerTool::new(registry.clone());

        // Learn a live anchor.
        let observed = call(&tool, json!({"mode": "observe", "surface": id})).await;
        let anchor = observed
            .lines()
            .find(|line| line.contains("READY"))
            .and_then(|line| line.rsplit('(').next())
            .map(|tail| tail.trim_end_matches(')').to_owned())
            .expect("an anchor for the READY row");

        let out = call(
            &tool,
            json!({
                "mode": "do",
                "surface": id,
                "steps": [{"key": "Enter"}],
                "settle": {"until": "none"},
                "expect": {"still": {"anchor": anchor, "label": "READY"}}
            }),
        )
        .await;

        assert!(out.contains("1. key"), "{out}");
        assert!(out.contains("expect: met"), "{out}");
    }

    #[tokio::test]
    async fn a_program_needs_steps() {
        let (registry, id) = registry_with_terminal();
        let tool = ComputerTool::new(registry);
        let args = serde_json::from_value(json!({
            "mode": "do",
            "surface": id,
            "steps": [],
            "expect": {"appears": "x"}
        }))
        .unwrap();
        assert!(tool.call(args).await.is_err());
    }

    #[tokio::test]
    async fn closing_removes_the_surface() {
        let (registry, id) = registry_with_terminal();
        let tool = ComputerTool::new(registry.clone());
        call(&tool, json!({"mode": "close", "surface": id.clone()})).await;
        assert!(registry.is_empty());
    }

    /// End-to-end against a real process on a real PTY.
    ///
    /// `less` is used because it is on every POSIX box, takes the alternate
    /// screen, and quits on `q` — so the whole loop (launch, observe, act,
    /// settle, re-observe) is exercised without depending on anything exotic.
    #[tokio::test]
    async fn drives_a_real_curses_program_end_to_end() {
        if !std::path::Path::new("/usr/bin/less").exists() {
            eprintln!("skipping: less is not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "ALPHA\nBRAVO\nCHARLIE\n").unwrap();

        let tool = ComputerTool::new(SurfaceRegistry::new());
        let launched = call(
            &tool,
            json!({"mode": "launch", "command": format!("less {}", file.display())}),
        )
        .await;

        assert!(launched.contains("ALPHA"), "less should have painted: {launched}");
        let id = launched
            .split_whitespace()
            .find(|word| word.starts_with("term"))
            .expect("a surface id")
            .to_owned();

        // Quit, then confirm the surface reflects it rather than reporting
        // success blindly.
        let out = call(
            &tool,
            json!({
                "mode": "do",
                "surface": id,
                "steps": [{"key": "q"}],
                "settle": {"until": "quiet", "timeoutMs": 1500},
                "expect": {"appears": "unlikelytoexist"}
            }),
        )
        .await;
        assert!(out.contains("1. key"), "{out}");
        assert!(
            out.contains("expect: NOT met"),
            "a wrong expectation must be reported, not glossed over: {out}"
        );
    }

    #[test]
    fn stage_state_directories_are_process_scoped_and_never_world_readable() {
        let Some(dir) = default_state_dir() else {
            // No `$XDG_RUNTIME_DIR`: the correct answer is None, not /tmp.
            assert!(std::env::var_os("XDG_RUNTIME_DIR").is_none());
            return;
        };
        // Two agents on one machine must not land on the same stage directory:
        // they would contend for the same Wayland socket, D-Bus socket and
        // browser profile.
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(&std::process::id().to_string()), "{name}");
        assert!(dir.is_absolute());
        // Short enough to hold an AF_UNIX socket path underneath it.
        assert!(
            dir.as_os_str().len() < 70,
            "stage sockets must fit in sun_path: {dir:?}"
        );
        // And never the shared temp dir, whatever the environment says.
        assert!(
            !dir.starts_with(std::env::temp_dir()) || std::env::temp_dir().starts_with("/run"),
            "stage sockets must not live in a world-traversable directory: {dir:?}"
        );
    }

    #[tokio::test]
    async fn the_input_lease_serializes_programs_against_one_stage() {
        // Delivering a keystroke is "focus this window, then send". Two
        // programs interleaving between those steps would land one's keystroke
        // in the other's window, so the lease must be exclusive.
        let registry = SurfaceRegistry::new();
        let held = registry.input_lease().await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                registry.input_lease()
            )
            .await
            .is_err(),
            "a second program must wait for the first to finish with the seat"
        );
        drop(held);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                registry.input_lease()
            )
            .await
            .is_ok(),
            "the lease must be released when a program ends"
        );
    }

    #[tokio::test]
    async fn a_delegate_registry_is_independent_of_its_parent() {
        // A subagent gets its own display rather than sharing the parent's:
        // one stage is one seat, and siblings sharing it would serialize behind
        // the input lease into uselessness.
        let parent = SurfaceRegistry::new();
        let child = parent.for_delegate();

        // Surfaces do not leak between them.
        parent.attach(Arc::new(
            crate::surface::pty::PtySurface::detached("pty:parent", 4, 20),
        ));
        assert!(parent.get("pty:parent").is_some());
        assert!(
            child.get("pty:parent").is_none(),
            "a delegate must not inherit the parent's surfaces"
        );

        // And the input lease is per-stage, so a delegate is never blocked by
        // whatever the parent is doing with its own seat.
        let _held = parent.input_lease().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), child.input_lease())
                .await
                .is_ok(),
            "a delegate's seat must be independent of its parent's"
        );
    }

    #[test]
    fn the_description_forbids_coordinates_and_demands_labels() {
        let tool = ComputerTool::new(SurfaceRegistry::new());
        let description = tool.description();
        assert!(description.contains("NEVER use screen coordinates"));
        assert!(description.contains("label"));
        assert!(description.contains("observe"));
        assert!(
            description.contains("irreversible"),
            "the model must be told to isolate destructive steps"
        );
    }

    /// A tool wired to a real recorder, so what it writes can be read back.
    struct Recording {
        tool: ComputerTool,
        surface: String,
        reader: artist_session::EventLogReader,
        recorder: artist_session::Recorder,
        _writer: artist_session::WriterTask,
        _dir: tempfile::TempDir,
    }

    impl Recording {
        /// Every event written so far.
        ///
        /// The writer is a separate task, so a read that did not wait would race
        /// it and pass or fail depending on scheduling.
        async fn events(&self) -> Vec<artist_session::Envelope> {
            self.recorder.flush().await;
            self.reader.read_all().unwrap()
        }
    }

    async fn recording_tool() -> Recording {
        let dir = tempfile::tempdir().unwrap();
        let writer = artist_session::EventLogWriter::open(dir.path(), "test").unwrap();
        let (recorder, task) = artist_session::spawn_writer(writer, None);
        let (registry, surface) = registry_with_terminal();
        Recording {
            tool: ComputerTool::with_recorder(registry, recorder.clone(), None),
            surface,
            reader: artist_session::EventLogReader::new(dir.path()),
            recorder,
            _writer: task,
            _dir: dir,
        }
    }

    #[tokio::test]
    async fn observing_records_an_event_the_inspector_can_read() {
        // The events were declared and consumed and never produced, so
        // `artist computer log` and `distill` always reported nothing at all.
        let fixture = recording_tool().await;
        let (tool, id) = (&fixture.tool, fixture.surface.clone());
        call(tool, json!({"mode": "observe", "surface": id})).await;

        let events = fixture.events().await;
        let observed: Vec<_> = events
            .iter()
            .filter_map(|envelope| match envelope.event() {
                artist_session::SessionEvent::ComputerObserved(observed) => Some(observed),
                _ => None,
            })
            .collect();

        assert_eq!(observed.len(), 1, "one look, one record");
        assert_eq!(observed[0].surface, id);
        assert_eq!(observed[0].rung, 1, "a terminal is the engine rung");
        assert!(observed[0].bytes > 0, "the cost of the result is the point");
    }

    #[tokio::test]
    async fn a_program_records_every_step_with_its_payload() {
        let fixture = recording_tool().await;
        let (tool, id) = (&fixture.tool, fixture.surface.clone());
        let observation = call(tool, json!({"mode": "observe", "surface": id})).await;
        let anchor = observation
            .lines()
            .find(|line| line.contains("READY"))
            .and_then(|line| line.rsplit('(').next())
            .map(|tail| tail.trim_end_matches(')').to_owned())
            .expect("an anchor for the READY row");

        call(
            tool,
            json!({
                "mode": "do",
                "surface": id,
                "steps": [{"key": "Enter"}, {"type": {"anchor": anchor, "label": "READY", "text": "hello"}}],
                "settle": {"until": "none"},
                "expect": {"appears": "READY"}
            }),
        )
        .await;

        let events = fixture.events().await;
        let acted = events
            .iter()
            .find_map(|envelope| match envelope.event() {
                artist_session::SessionEvent::ComputerActed(acted) => Some(acted),
                _ => None,
            })
            .expect("the program must be recorded");

        assert_eq!(acted.surface, id);
        assert_eq!(acted.steps.len(), 2);
        // Without the payload a distilled macro replays as a silent no-op: the
        // key becomes an empty chord and the type step types nothing.
        assert_eq!(acted.steps[0].action, "key");
        assert_eq!(acted.steps[0].payload.as_deref(), Some("Enter"));
        assert_eq!(acted.steps[1].action, "type");
        assert_eq!(acted.steps[1].payload.as_deref(), Some("hello"));
        assert_eq!(acted.expect.as_deref(), Some("READY"));
        assert_eq!(acted.expect_met, Some(true));
    }

    #[tokio::test]
    async fn a_surface_with_no_picture_says_so_rather_than_failing_obscurely() {
        let (registry, id) = registry_with_terminal();
        let tool = ComputerTool::new(registry);
        let args = serde_json::from_value(json!({"mode": "screenshot", "surface": id})).unwrap();
        let error = tool.call(args).await.unwrap_err().to_string();

        assert!(error.contains("no picture"), "{error}");
        assert!(
            error.contains("observe"),
            "an error must name the thing to do instead: {error}"
        );
    }

    #[test]
    fn a_command_line_survives_quotes_and_spaces() {
        // `split_whitespace` tore a quoted path into pieces and started the
        // application with arguments nobody wrote.
        assert_eq!(split_command("chromium"), ["chromium"]);
        assert_eq!(
            split_command("chromium https://example.com/a?b=c&d=e"),
            ["chromium", "https://example.com/a?b=c&d=e"]
        );
        assert_eq!(
            split_command(r#"gedit "/home/a/My Notes.txt""#),
            ["gedit", "/home/a/My Notes.txt"]
        );
        assert_eq!(
            split_command("code --folder-uri 'file:///tmp/my project'"),
            ["code", "--folder-uri", "file:///tmp/my project"]
        );
        // An empty quoted argument is still an argument.
        assert_eq!(split_command(r#"app "" x"#), ["app", "", "x"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn the_surface_list_says_what_each_surface_can_do() {
        // `Caps` exists so the tool can refuse rather than pretend, and was
        // never rendered — so the model learned a terminal has no pointer by
        // clicking one and reading the error, at a round trip every time.
        let (registry, _) = registry_with_terminal();
        let listing = ComputerTool::new(registry).render_surfaces();

        assert!(listing.contains("key"), "{listing}");
        assert!(listing.contains("type"), "{listing}");
        assert!(
            !listing.contains("click"),
            "a terminal has no pointer and must not advertise one: {listing}"
        );
    }

    #[tokio::test]
    async fn an_id_being_opened_cannot_be_claimed_twice() {
        let registry = SurfaceRegistry::new();
        assert!(registry.claim("term:1"));
        assert!(!registry.claim("term:1"), "the second open must lose");
        assert!(registry.is_claimed("term:1"));

        // A failed open releases its claim rather than burning the id.
        registry.abandon("term:1");
        assert!(!registry.is_claimed("term:1"));
        assert!(registry.claim("term:1"));
    }
}
