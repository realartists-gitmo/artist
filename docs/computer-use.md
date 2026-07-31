# Generalized computer use

Artist's computer-use subsystem: one observation contract over every application
surface, selecting the cheapest abstraction each one supports.

Status: **partially implemented.** What is built and verified is marked
throughout; §Status collects what is not.

## Thesis

Computer use is not an action problem. The verb list — click, type, key, scroll —
is the trivial half, and it is where most implementations spend their design
budget. The hard parts are:

1. **Observation economy.** What the model is shown after each action, and how
   that stays bounded across a hundred steps.
2. **Referential stability.** How the model names a thing on screen such that
   naming it wrongly *fails* rather than silently doing something else.
3. **Abstraction selection.** Choosing, per surface, the cheapest rung that can
   still express the task — clicking a play button is orders of magnitude more
   expensive and more fragile than the D-Bus call that does the same thing.
4. **Isolation.** Doing all of it on the user's real machine without taking their
   keyboard, mouse or focus hostage.

Everything below follows from those four.

## What this is designed against

| Common failure | Cause | Our answer |
|---|---|---|
| Context fills with near-identical screenshots | Every action returns a full frame | Structured observation, **diffs by default**, opt-in pixels, and a decay pass that stubs stale observations |
| `click(743, 219)` is silently wrong after a 4px reflow | Coordinates in the model's output | The model emits **anchors**; a stale anchor is a hard error |
| Click, screenshot, get a spinner | No settle semantics; the model guesses `sleep(2)` | Backend **settle predicates**, with compositor damage as the signal |
| "Computer use" that is really browser use | DOM-shaped abstraction | One `Surface` contract: a PTY, a CDP page, an AT-SPI window and a pixel buffer are all surfaces |
| Driving a GUI to do what a CLI does in one line | A single fixed abstraction level | The ladder |
| The agent steals the user's mouse | Global input injection | The **Stage**: its own display server and session bus |

## The Stage

A *stage* is an isolated graphical session on the user's own machine: its own
compositor, its own D-Bus session bus, sharing `$HOME` and credentials but
sharing nothing about input focus or screen real estate.

`/dev/uinput` is reachable on a typical desktop, but kernel-level injection goes
to whatever seat has focus — the agent and the user would fight over one
keyboard. XTEST on the live display is the same problem. So the agent gets its
own display server, and once that is forced it is the better architecture anyway:

- **Input is a function call** into our own seat. No shared device, no race.
- **Damage regions are the change signal** — a compositor already computes which
  rectangles changed per frame, which beats polling a tree or differencing
  screenshots.
- **Window identity is a fact we hold.** Windows carry the client's pid from its
  socket credentials, never a guess from the title.
- **Capture is a buffer read** — no portal prompt, none of the user's windows.
- **The accessibility tree is unpolluted**, because only agent-launched apps
  appear on the stage's private bus.

**Implemented** (`crates/artist-computer/src/stage/`): a headless
[smithay](https://github.com/Smithay/smithay) compositor on `GlesRenderer` over
GBM/EGL, on its own thread behind a `Send + Sync` proxy; `wl_compositor`,
`wl_shm`, `xdg_shell`, `wl_seat`, `wl_data_device_manager`; keyboard and pointer
delivery; offscreen render target and PNG capture; a private `dbus-daemon` plus
`at-spi-bus-launcher` and `at-spi2-registryd`.

**Filesystem:** shares `$HOME`. An overlay would break the credential reuse that
is the whole reason to run locally rather than in a VM, and would contradict this
harness's "bash is unsandboxed by design" stance. The safety layer is the stream
rule and the label cross-check, not the filesystem.

### Toolkits must be told accessibility is on

Not optional decoration. GTK and Qt start their bridges only when told, and
Chromium needs a command-line flag. Omit any of these and rung 2 reports an empty
tree for a perfectly healthy application, which presents as a compositor fault:

```
GTK_A11Y=atspi  QT_ACCESSIBILITY=1  QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
ACCESSIBILITY_ENABLED=1  AT_SPI_BUS_ADDRESS=<stage bus>
chromium --force-renderer-accessibility
```

### Two traps worth naming

Both were hit during implementation, and both are silent:

- **Connecting to the wrong bus.** The harness's own environment still points at
  the user's session bus, so any default connection silently gets it. Every
  connection is made by explicit address.
- **Global environment from a compositor thread.** Binding the Wayland socket
  through `XDG_RUNTIME_DIR` means mutating process-global state; two stages then
  race and the loser's clients connect to the winner's display. The socket is
  bound by absolute path instead.

## The ladder

The generalization is not one verb list that works everywhere. It is a ranked set
of abstraction levels, **selected per surface**, with a probe that finds the
highest rung a surface supports.

| Rung | Level | Mechanism | Example |
|---|---|---|---|
| **0** | Programmatic | D-Bus, localhost HTTP, CLI, scripting server | MPRIS to pause a player; `git` instead of a git GUI |
| **1** | Engine protocol | CDP; PTY + vt100 screen buffer | Any Chromium **or Electron** app; a curses TUI |
| **2** | Accessibility | AT-SPI2 on the stage's private bus | GTK/Qt apps — and AT-SPI exposes *actions*, so most buttons need no coordinates at all |
| **3** | Pixels | Capture + set-of-mark | Games, canvas apps, anything else |

Three things make this real rather than a slogan:

- **The rung is per-surface, not per-application.** A browser window is rung 0
  for its own chrome (`Target.*`, `Page.navigate` are programmatic calls — we
  never need AT-SPI for a tab strip) and rung 1 for page content. Native dialogs
  are separate toplevels and get probed independently.
- **A probe runs at attach time**, descending until something answers.
- **Results cache to user-editable adapter TOMLs**, which is where hand-written
  rung-0 knowledge lives.

Rung 0 deserves emphasis because it is the rung everyone omits and the one that
carries most of the value.

Rung 3 is **observation-only**. Minting anchors from a pixel grid is coordinates
wearing a hat; a surface with no tree reports "no actionable surface" — a loud,
honest failure.

## Anchors

*Implemented and verified: `crates/artist-computer/src/anchors.rs`.*

The model **never emits coordinates**; `bounds` is stripped before rendering.
This is the same contract `edit` already has with mnemonic line anchors, and it
reuses the same allocator — `hashline_tools::AnchorTable`, a circular free-slot
allocator over 2983 tokenizer-screened words. File anchors and screen anchors
mint from one implementation rather than two.

**Resolution is identity-only.** There is deliberately no fallback to matching on
role and name: two buttons both called "Delete" are exactly the case anchors
exist to disambiguate, and a re-rendered list where row 3 now holds what row 4
held is exactly what a path fallback gets wrong. Three distinct errors:

- *not issued* — the token was never handed out;
- *stale* — it named something that is gone; recovery is to observe again;
- *label mismatch* — it still resolves, but not to what the model thinks.

Retired anchors are remembered as bounded tombstones. Without that, reclaiming a
handle (which is what keeps the handle space bounded) would report a genuinely
stale anchor as "never issued", losing the only error with a useful recovery.

## Observations

First contact renders in full; everything after is a **delta** (`+` added,
`~` changed, `-` gone). The *collection* is always complete — a delta is a
rendering choice, not a partial read — which is what lets the allocator reclaim
and keeps handles bounded.

Rendering is budgeted, not exhaustive: interactive nodes always survive, static
text is capped, and overflow is **named rather than silently dropped**. Node
digests deliberately exclude geometry, so a reflow produces no spurious deltas.

Whitespace is preserved inside a rendered value: in a terminal row the spacing
*is* the content, and collapsing `PID  COMMAND` destroys the column alignment the
model needs to read a table.

## Actions are programs

*Implemented: `crates/artist-computer/src/program.rs`, `surface/mod.rs`.*

Round-trips dominate the cost of driving a UI, so the unit of action is a short
program:

```json
{ "mode": "do", "surface": "win:3",
  "steps": [{"click": {"anchor": "kv7", "label": "Compose"}},
            {"type": {"anchor": "m2q", "label": "To", "text": "adam@example.com"}},
            {"key": "Enter"}],
  "settle": {"until": "quiet", "timeoutMs": 3000},
  "expect": {"anchor": "kx9", "label": "Message sent"} }
```

- Steps abort on first failure, returning the failing index and the surface as it
  actually is.
- **The settle watcher is armed before the final step dispatches.** Subscribing
  afterwards is a race: a fast surface finishes reacting before the watcher
  attaches, and the wait then burns the full timeout on an already-settled
  screen.
- `expect` is **mandatory**. An optional hypothesis is one nobody states, and its
  whole value is converting a silent wrong turn into an error.

### `settle`, and why damage-quiet alone does not work

Blinking carets, spinners, clocks and video damage forever, so "no damage for N
ms" means waiting for the timeout every time. `quiet` therefore means *no damage
outside the learned noise set*: a rect repeating ≥3 times with area <1% of the
screen is classified as noise. The window is a **gap detector keyed to the last
sighting**, not to the first — keying it to the first would declassify a caret
that has been blinking longer than the window, which is the exact case it exists
to catch. Settling also requires that something happened first, so a program
cannot "settle" instantly against a screen that ignored it.

That classification is by rectangle, so **damage granularity is load-bearing**.
The compositor reports the regions a client actually committed rather than the
whole screen; reporting whole-screen damage on every commit would make a caret
blink indistinguishable from a dialog opening and quietly defeat the filter. A
commit that attaches no buffer is a role commit, not a repaint, and reports
nothing; a commit that attaches a buffer with no damage hint legally means
"assume everything" and does fall back to the whole screen.

On a page, `networkIdle` counts requests in flight from the CDP event stream —
started, finished *and* failed, since a blocked request leaves flight too — and
waits for the count to sit at or below two for 500 ms. `document.readyState`
alone reaches `complete` once the initial document parses and is blind to the
XHR an SPA fires immediately afterwards, which is most of a real session;
`wait_for_navigation` has the same blind spot.

### The label, and why the guardrail depends on it

Every anchor reference carries `label` — the model's echo of the element's name.
It exists for two independent reasons, either of which would justify it:

1. **The stream-rule guardrail needs something to match on.** Rules regex the
   *streamed tool-argument text*. An argument of `{"click":{"anchor":"kv7"}}`
   contains no evidence of intent; `"label":"Delete account"` does. Without the
   label there is nothing a rule could fire on, and the destructive-action
   guardrail could not exist.
2. **It is a second referential check.** A stale-but-still-live anchor that no
   longer means what the model thinks is caught before dispatch.

Matching normalizes (NFKC, casefold, strip `&` mnemonics and trailing ellipses,
collapse whitespace) then accepts equality **or containment either way** —
`"Save"` against `"Save…"`, `"Delete"` against `"Delete account permanently"`.
There is deliberately **no edit-distance fuzzing**: a threshold loose enough to
absorb real wording drift also accepts `Cancel` for `Confirm`, four edits apart
on seven characters, which is precisely the confusion the check exists to catch.

A label is required only when the resolved element has a non-empty accessible
name — terminal cell ranges and bare icons legitimately have none.

## Guardrail

*Implemented: `builtin:computer-destructive-actions` in `artist-rules`.*

`TtsrHook::on_tool_call` returns `ToolCallAction::Stop` **before dispatch**, so a
matched program aborts with no step having run. The built-in rule matches
destructive verbs in a `label` and fires `per-turn`, not `once` — a
once-per-session guardrail protects the cheap first mistake and goes dormant for
the expensive later one. Patterns anchor on the verb rather than requiring a
closing quote, because the delta hook fires mid-JSON.

Because the abort discards the whole program, the tool description tells the
model to isolate an irreversible step into its own single-step call.

## Context economy

Observations are the largest thing a computer-use session puts in context, and
most are worthless within a few turns.

**Decay** (`artist-session/src/decay.rs`) replaces stale observations with stubs
that preserve the surface, epoch and image digest, so the frame stays
recoverable. It is deliberately *not* part of compaction: the planner selects a
single positional cut point and has no per-message concept, and teaching it one
would change its contract. Decay instead holds one invariant —

> Only the content blocks of a `UserContent::ToolResult` change. No message or
> tool result is added, removed or reordered, and no `id` or `call_id` is altered.

— which keeps provider tool-call pairing valid. A result is recognized by **both**
an id→name map and a sentinel, belt and braces: the id map alone is fragile
across compaction snapshots, and the sentinel alone would let a `read` of a file
containing it trigger an elision of its own output.

It writes through `SessionMemory::revise`, not `replace`. `replace` records
`display_from: 0`, which display projections read as "clear the scrollback" —
and since decay runs before every turn it would blank the user's transcript each
time a screenshot aged out. It is **skipped entirely when the provider sidecar is
authoritative** (ChatGPT, OpenAI Responses), where remote compaction resets local
memory anyway and rewriting it saves nothing.

**Images** are externalized at the memory boundary
(`artist-session/src/convert.rs`). rig commits tool results verbatim, so an
inline base64 screenshot would land in `events.jsonl`, be re-read on every load,
be copied whole on fork, and be re-serialized by every subsequent reset snapshot.
Externalizing keeps the log proportional to the number of *distinct* images.

## Status

> **Corrected 2026-07-31** after two adversarial reviews and the fixes that
> followed. The audit's three CRITICALs and sixteen HIGHs are fixed, and the
> paths previously listed as unwired are wired. Known remaining gaps are at the
> bottom of this section; the audit's MEDIUM/LOW backlog is in
> [the audit](computer-use-audit.md), and what is worth adding next is in
> [the steal list](computer-use-steal-list.md).

**Built and verified**

- Anchors, deltas, render budgeting, program semantics, label cross-check.
  Containment requires the shorter side to carry at least three characters, so a
  one-glyph element no longer accepts any label; a name that normalization
  empties (`…`) is still treated as a name.
- Observation economy: deltas are budgeted like full renders, removals are
  truncated, and a surface that turns over past 60% renders full with
  `(surface replaced — full view)` rather than emitting two screens to describe
  one.
- PTY surface (rung 1): alternate-screen row anchors, primary-screen append,
  quiet settle — verified driving a real curses program end to end. Scrollback is
  bounded and escape sequences are rendered out before the model sees them.
- Stage: private D-Bus + a11y bring-up (async and timeout-bounded); headless GLES
  compositor verified with a purpose-built `wayland-client` fixture — client
  connects, window appears with real title/app_id/pid, keyboard and pointer reach
  the client, frame callbacks fire *even when a render fails*, capture contains
  what the client painted, per-window capture crops to that window, and the
  user's session is provably untouched.
- Damage noise-filter and quiet tracker.
- **CDP surface (rungs 0/1)** — verified against real headless Chromium.
  Navigation, history, per-element scrolling, clearing `type`, and an in-flight
  *set* that survives redirect chains.
- **AT-SPI surface (rung 2)** — breadth-first walk, element state read from the
  state set, tree-digest settle, `EditableText` for typing, and named-action
  `invoke`. Attached by pid on the stage's private bus.
- **Rung-0 adapters** — discovered from the user's config root only, and reachable
  from the shipped agent.
- Destructive-action guardrail: four pattern classes (English verbs unbounded in
  position, non-English verbs, whole-label confirmations, delete keys), plus an
  optional focus label on `key` so a destructive Enter is visible to a rule and
  cross-checked against the focused element.
- Observation decay, image externalization, screenshots, and `computer.*` event
  recording — so `artist computer log`, `frame` and `distill` have input.
- Tool UI.

**Known gaps** — real, and stated rather than implied:

- **Prompt injection is not addressed.** Rendered observations are
  attacker-controlled text entering context unmarked, and the stage shares
  `$HOME`. The guardrail inspects the model's output, not its input. See steal
  list items 4 and 11.
- **A bare `{"key":"Enter"}` on a focused destructive button is outside every
  guardrail pattern**, because no regex over the arguments can recover a name the
  arguments do not contain. The optional focus label covers the case where the
  model knows what it is aiming at; nothing covers the case where it does not.
- **Rung 3 is observation-only.** A surface with no tree reports "no actionable
  surface". Games, canvases and broken accessibility support are out of reach
  until a localizer exists (steal list item 15).
- **`Surface::children` has no implementors**, so a `target=_blank` tab is
  unreachable, and `GetFullAxTreeParams::default()` is main-frame only — an
  iframe (a payment form, an embedded login) is not observable.
- **No benchmark number.** The design argument is strong and the evidence is
  absent.
- **One compositor, one platform.** Linux/Wayland, our own stage. No macOS, no
  Windows, no Android.

## Concurrency

**Two agents on one machine** each get their own stage. The state directory is
`$XDG_RUNTIME_DIR/artist-stage-<pid>` — process-scoped, `0700`, on tmpfs, and
short enough that the Wayland and D-Bus sockets underneath it fit in `sun_path`.
Within a process each stage gets a further counter-suffixed directory, because
`short_id` is readable but only 1024 combinations wide and a stage owns sockets
whose paths must not collide. Nothing is shared: separate compositor, session
bus, accessibility registry and browser profile.

**Programs against one stage are serialized** by a stage-wide input lease held
for the duration of a program. A stage has one seat and one keyboard focus, and
delivering a keystroke is "focus this window, then send" — two programs
interleaving between those steps would land one's keystroke in the other's
window. The per-surface anchor lock does not help, because the surfaces differ;
the contended resource is the stage. The lease is taken before the anchor lock,
always, so two programs cannot deadlock by acquiring them in opposite orders.

**Subagents get their own stage**, budded off the parent's rather than sharing
it — one stage is one seat, and siblings sharing it would serialize behind the
input lease into uselessness rather than being made safe by it. A child
registry is cheap because stages are lazy: a delegate that never touches a GUI
never starts a compositor, and dropping the registry when the delegate finishes
tears down its display, session bus and browser profile with it.

Whether a subagent may drive a GUI at all is **a profile decision like any
other**, not a special rule about who is asking. The read-only built-ins
(`explorer`, `planner`, `reviewer`) deny it through their allow list, and a
project profile can grant it deliberately. `"computer"` counts as write access
in the delegate `read_only` classification, since a subagent that can drive a
GUI can do anything a person at that keyboard could.

**Design decisions that look like gaps**

- **One output per stage, deliberately.** Multi-monitor is a human affordance —
  somewhere to put things while looking at something else — and an agent has no
  use for it. Every extra output is another surface to search and another
  coordinate space to keep straight, for no capability gained. The *size* is
  configurable (`[computer] screen = "1280x800"`), because viewport size
  genuinely changes what an application shows; the number of outputs is not.
- **Rung 3 cannot act.** See above — this is the point, not a limitation.
- **No OCR.** A surface that reaches rung 3 with no tree reports "no actionable
  surface" rather than inviting the model to guess at pixels.

## Related

- `crates/artist-computer/` — the implementation
- `crates/artist-computer/tests/stage_client.rs` — the compositor fixture
- `crates/hashline-tools/src/mnemonic_anchors.rs` — the shared anchor allocator
