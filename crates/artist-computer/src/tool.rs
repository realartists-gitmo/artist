//! The `computer` tool: the model-facing surface of the whole subsystem.
//!
//! Thin by design — parse, dispatch, format. Every decision worth making lives
//! in [`crate::anchors`], [`crate::render`] or a backend; this module only
//! routes between them and owns the surface registry.
//!
//! The registry copies [`artist_tools::BashTool`]'s session model deliberately:
//! a `DashMap` keyed by id, an in-flight guard against duplicate creation, and
//! tombstone-once reaping in `list`. Two long-lived resource maps in one harness
//! behaving differently would be a needless second thing to learn.

use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use rig_core::tool::{PortableTool, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::anchors::AnchorBook;
use crate::model::SurfaceId;
use crate::program::{Program, StepError};
use crate::render;
use crate::surface::{Surface, run_program};

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
    /// Guards against two concurrent opens racing to claim one id, exactly as
    /// `BashTool` guards session creation.
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
        Self::with_host_opt(default_state_dir(), Default::default())
    }

    /// Build a registry rooted at a state directory, with adapters discovered
    /// for a project.
    pub fn for_project(state_dir: impl Into<std::path::PathBuf>, project: &std::path::Path) -> Self {
        Self::with_host(state_dir, crate::ladder::adapters::AdapterSet::discover(project))
    }

    pub fn with_host(
        state_dir: impl Into<std::path::PathBuf>,
        adapters: crate::ladder::adapters::AdapterSet,
    ) -> Self {
        Self::with_host_opt(Some(state_dir.into()), adapters)
    }

    fn with_host_opt(
        state_dir: Option<std::path::PathBuf>,
        adapters: crate::ladder::adapters::AdapterSet,
    ) -> Self {
        Self {
            surfaces: Arc::new(DashMap::new()),
            opening: Arc::new(DashSet::new()),
            host: Arc::new(crate::host::Host::new(state_dir, adapters)),
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
        )
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
}

impl ComputerTool {
    pub fn new(registry: SurfaceRegistry) -> Self {
        Self { registry }
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
- `close` — release a surface.

Naming things: every element is shown as `role "name" (anchor)`. Use the bare anchor token to refer to it. NEVER use screen coordinates — they are deliberately not shown, and there is no way to act on one.

Every step that names an anchor must also carry a `label`: your own copy of that element's name from the most recent observation. It is checked against reality before anything runs, so a wrong anchor fails loudly instead of clicking the wrong thing.

`expect` is required, and is stated by NAME rather than by anchor — an element that appears because of your program has no anchor you could know yet. One of:
- `{"appears":"Message sent"}` — this should be on the surface afterwards
- `{"gone":"Compose"}` — this should no longer be there
- `{"still":{"anchor":"kv7","label":"Save"}}` — this exact element should still be there and still be called that

If an anchor is rejected as stale, do not retry it and do not guess another — `observe` that surface again to get current anchors.

Steps stop at the first failure, and a guardrail can abort the whole program before ANY step runs. So put an irreversible step (delete, send, pay, confirm) in its own single-step call, after the rest has already succeeded.

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
                "mode": {"enum": ["surfaces", "launch", "observe", "do", "close"], "description": "Defaults to `do` when steps are given, otherwise `surfaces`."},
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
                                    "text": {"type": "string"}
                                },
                                "required": ["anchor", "text"],
                                "additionalProperties": false
                            },
                            "key": {"type": "string", "description": "A key or chord, e.g. `Enter`, `ctrl+c`, `Down`."},
                            "scroll": {
                                "type": "object",
                                "properties": {
                                    "anchor": {"type": "string"},
                                    "amount": {"type": "integer", "description": "Positive scrolls down."}
                                },
                                "required": ["amount"],
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
                    let mut parts = command.split_whitespace();
                    let program = parts.next().unwrap_or(&command).to_owned();
                    let rest: Vec<String> = parts.map(str::to_owned).collect();
                    let surface = self.registry.host().launch(&program, &rest).await?;
                    let id = self.registry.attach(surface);
                    // A browser yields two surfaces at two rungs; attach the
                    // chrome half too so tabs are addressable.
                    if let Some(extra) = self.registry.host().take_pending_surface().await {
                        self.registry.attach(extra);
                    }
                    id
                } else {
                    let id = surface_id("term").as_str().to_owned();
                    let surface = crate::surface::pty::PtySurface::spawn(
                        id.clone(),
                        &command,
                        args.cwd.as_deref().map(std::path::Path::new),
                        40,
                        120,
                    )?;
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
                Ok(ToolOutput::text(format!(
                    "launched {command:?} as {id}\n\n{}",
                    render::observation(&id, &observed, None)
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
                let snapshot = attached.surface.snapshot().await?;
                let mut book = attached.book.lock().await;
                let observed = book.observe(&snapshot, args.full);
                Ok(ToolOutput::text(render::observation(&id, &observed, None)))
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
                Ok(ToolOutput::text(render_report(&id, &report)))
            }
            other => Err(StepError::Backend(format!(
                "unknown mode {other:?}; expected surfaces, observe, do, or close"
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

    fn render_surfaces(&self) -> String {
        let rows = self.registry.list();
        if rows.is_empty() {
            return "no surfaces are open".to_owned();
        }
        rows.into_iter()
            .map(|(id, title, rung)| format!("{id}\trung {rung}\t{title}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
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
}
