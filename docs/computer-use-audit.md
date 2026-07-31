# Computer use — audit findings

Two independent adversarial reviews of the `artist-computer` subsystem, run
2026-07-31 against the state described in `docs/computer-use.md`. One reviewed
for defects and security; one reviewed for whether the thing is actually good to
use. Neither saw the other's output.

Findings are merged below and ranked on one scale. **Where both reviewers found
the same defect independently, it is marked ⚑** — that is the strongest signal
in the document, because they were looking through different lenses and still
converged.

Every finding was verified against code by its reporter, with `file:line`. Where
a reviewer flagged something as unconfirmed, that is preserved.

---

## The headline

**Several capabilities documented as "Built and verified" are wired to nothing.**
Not broken — *unreachable*. The code exists, has unit tests, and is never called
from any production path. Both reviewers found this independently and it is the
single most important correction to make, because the doc is currently telling
its own author things that are not true.

| Claimed | Reality |
|---|---|
| `artist computer log` / `distill` | No code anywhere records `computer.*` events. Both commands always print "nothing found". |
| Screenshots, set-of-mark overlay | `Surface::pixels()` has no override and no caller; `tool.rs` passes `image: None` at all three sites. No observation can ever carry an image. |
| Rung-0 adapters | `SurfaceRegistry::for_project` — the only path calling `AdapterSet::discover` — has no caller. `select()` can never return `Programmatic` in the shipped agent. |
| `[computer] screen` | Resolved into `ComputerConfig` and never read; `ensure_stage` calls `start`, not `start_sized`. |
| AT-SPI surface (rung 2) | `AtspiSurface` is never constructed outside tests. `Host::launch` returns `Err` unconditionally for any non-Chromium GUI app. |
| Macro replay | `distill` emits `text: None, key: None`, so every `key` step replays as `Step::Key("")` → "unknown key" and every `type` step types nothing. |

**Action:** either wire these or amend §Status. Leaving both as-is is the one
unacceptable option.

---

## CRITICAL

### C1 ⚑ — Key modifiers are silently dropped; the wrong action executes and reports `ok`

*Found independently by both reviewers.*

`stage/wayland.rs:1037` (`keycode_for`) and `surface/cdp.rs:638` (`press_key`)
both do `key.rsplit('+').next()` and discard the modifier. Neither ever sets a
modifier state.

On a page, `ctrl+a` falls through to the single-character arm
(`cdp.rs:649`) and calls `type_one('a')` — it **inserts the literal letter "a"**
into the focused field. `ctrl+s` types `s`. `ctrl+z` types `z`. Every one
reports `"ok"`.

This is the exact failure class the whole design exists to eliminate: a silent
wrong action that the model believes succeeded. It also punches through the
guardrail, whose rule includes a pattern for `"key":"(ctrl\+|shift\+)*delete"` —
implying chords are meaningful while the executor throws them away.

The Wayland unit test at `wayland.rs:1157` **asserts the broken behaviour**
(`keycode_for("ctrl+c") == Some(54)`; 54 is plain `c`).

`surface/pty.rs:258` (`key_bytes`) gets this right. Two of three backends are wrong.

**Fix:** parse the chord once (reuse the PTY split); pass modifiers through —
`DispatchKeyEventParams::modifiers` bitmask for CDP, a real `ModifiersState` plus
modifier press/release for Wayland. If that is deferred, *reject* any stroke
containing `+` as unsupported. Silently dropping is the only unacceptable option.
Delete the test that pins the bug.

### C2 — Stage sockets and the browser profile land world-traversable when `XDG_RUNTIME_DIR` is unset

`tool.rs:484-490` falls back to `std::env::temp_dir()`; `host.rs:121` then
`create_dir_all`s at default umask (0755). There is **no `set_permissions`
anywhere in the crate**.

Under that fallback, readable and traversable by any local user:
- the Wayland socket (`wayland.rs:720`) — and `wl_data_device_manager` is bound
  (`wayland.rs:300`), i.e. the agent's clipboard;
- the private D-Bus session socket (`bus.rs:66`) — the stage bus and a11y registry;
- `chrome-profile` (`host.rs:187`) — the profile of a browser that **by design
  shares the user's `$HOME` and credentials**: cookies, session tokens, logins.

`create_dir_all` is not an exclusive create and the pid is enumerable, so the
path can be pre-created or symlinked by an attacker.

The `default_state_dir` doc comment and `docs/computer-use.md` §Concurrency both
assert "0700" and "not world-traversable". Both are false on this path — and the
fallback is precisely the case the comment worries about.

**Fix:** refuse to start a stage when `XDG_RUNTIME_DIR` is unset. If a fallback is
kept, use `create_dir` (not `_all`) with `DirBuilderExt::mode(0o700)` and fail on
`AlreadyExists`.

### C3 — Decay destroys the user's steering messages

`steering.rs:109-118` merges `<user_steering>…</user_steering>` into a tool
result's trailing text block. `decay.rs:120-130` later rewrites *every* block of
a stale observation — block 0 becomes the stub, the rest become `"[elided]"`.

So a steering message the user typed while a `computer` call was in flight is
**permanently removed from model context** three observations later, with nothing
in the stub indicating anything but an observation was dropped. The transcript
still shows it, so nobody notices the model stopped honouring it.

**Fix:** have `SteeringHook` always push steering as its own block, and have
`stub_for` preserve any block that is not part of the observation body.

---

## HIGH

### H1 ⚑ — `expect` is both under-checked and, as specified, unwritable

*Both reviewers, from different angles.*

**Under-checked:** `surface/mod.rs:153-155` is
`book.resolve(&program.expect.anchor).is_ok()`. `expect.label` is read nowhere,
so `expect: met` fires if the anchor resolves to *anything* — including an
element now named "Send failed".

**Unwritable:** anchors are minted only by observation (`anchors.rs:113-147`), so
an element that appears *as a result of the program* has an anchor the model
cannot know. The description tells it to do exactly that (`tool.rs:216,228`:
"give the anchor you believe will exist once the program finishes"). A model
following the description emits an invented token → `NotIssued` → `expect: NOT
met` **on a run that worked**. The crate's own tests can only write a tautology
(expect = the anchor just clicked) or garbage.

This is the design's flagship safety property and it is currently a false-alarm
generator.

**Fix:** make `expect` a predicate in durable terms, checked against the
post-program snapshot with `check_label`'s normalize-plus-containment:
`{"appears":"Message sent"}` / `{"gone":"Compose"}` /
`{"anchor":…,"label":…}`. Reject `appears` values that already exist
pre-program — that is a no-op hypothesis. Then the description's example becomes
truthful.

### H2 ⚑ — The destructive-action guardrail is far narrower than it reads

*Both reviewers. One confirmed the bypasses empirically by compiling probes
against `RuleSet::compile`.*

Not caught: `"Yes"`, `"OK"`, `"Continue"`, `"Proceed"`, `"Apply"`; any
non-English label (`Supprimer`, `Löschen`, `删除`); `"Move to Trash"`,
`"Empty Bin"`, `"Submit order"`, `"Place order"`; any label where the verb sits
past 60 characters (the `{0,60}` bound); `{"scroll":…}`; and **`{"key":"Enter"}`**.

The `key` gap is structural, not a missing word. `Step::Key` has no `Target`
(`program.rs:73`), so it carries no label, gets no cross-check
(`surface/mod.rs:169-171`), and offers a rule nothing to match. A program of
`[click "Delete"]` is stopped; `[click "More options", key Enter]` on a focused
destructive default button is not.

Given the stage shares `$HOME`, presenting this as *the* safety layer without
qualification is the dishonest part.

**Fix:** add the confirmation-affirmative class anchored to a whole short label
(`^(yes|ok|okay|continue|proceed|confirm|accept|agree)$`); raise or drop the
`{0,60}` cap; give `Step::Key` an optional focused-element label resolved at
dispatch — or state the keyboard-activation gap plainly in the docs.

### H3 — `type` on the Wayland stage lowercases text and rejects most punctuation

`wayland.rs:878-887` sends `Text` one character at a time through `keycode_for`
(`:1037`), which `to_ascii_lowercase`s and has only letters and digits.

- `type "Hello World"` → client receives `hello world`, reported `ok`. Silent
  data corruption.
- `type "adam@example.com"` → `unknown key "@"`. That is the tool description's
  own example (`tool.rs:225`) and the doc's example.
- `.`, `-`, `/`, `_`, `:` all fail.

**Fix:** don't route text through the keycode table. Bind `zwp_text_input`, or at
minimum hold shift for uppercase and extend the table with the US punctuation row
and its shifted forms.

### H4 — Every failure path returns a delta, so "re-observe" hands the model nothing

`run_program` always renders `book.observe(&snapshot, false)`
(`surface/mod.rs:149`). On a stale anchor or label mismatch nothing was
dispatched, so the surface is unchanged and the observation is literally
`(no change)` (`render.rs:103`) — while the error says "Re-read the surface to
get fresh anchors". The model re-observes, gets `(no change)` again, and the
anchors it needs are several turns back where decay may already have stubbed them.

Simulated recovery: stale → flail; not-issued → flail; label mismatch →
recovers (the error names the actual name).

**Fix:** one line — `full = failure.is_some() || expect_met == Some(false)`.

### H5 — Element state is never populated, so form state is invisible

`cdp.rs:425` and `atspi.rs:99` both end `.with_state(NodeState::default())`.
`flags()` feeds `Node::digest` (`model.rs:249`), so checking a checkbox or
disabling a submit button changes **nothing the model can see**: no `[checked]`,
no `[disabled]`, and no `~` delta line. The model cannot tell whether its click
landed, cannot see a disabled control before clicking it, and clicking a disabled
control silently "succeeds".

**Fix:** CDP — read AX node `properties` (`checked`, `disabled`, `focused`,
`expanded`, `selected`). AT-SPI — read the state set. Both are one call already
in hand.

### H6 — Deltas are unbudgeted, and a navigation renders the page twice

`render_full` budgets static text (`render.rs:66-79`); `render_delta`
(`:87-105`) budgets nothing, and removal lines skip `truncate` entirely
(`:99`), so a 5 kB removed text node prints in full. On navigation every binding
changes, so `observe` emits all new nodes as `+` **and** all old ones as `-`
(`anchors.rs:128-163`) — for a mid-size page AX tree (~600 named nodes) roughly
**8–9k tokens versus ~4k for a full render**.

**Fix:** apply the budget to both delta arms; when added or removed exceeds ~60%
of the node set, render full instead and say `(surface replaced — full view)`.

### H7 — AT-SPI: DFS documented as BFS, capabilities that lie, settle that never waits

`atspi.rs:147-177` uses `vec` + `pop()` — **LIFO, depth-first** — directly
contradicting its own comment and the doc's "bounded breadth-first tree walk".
With `MAX_NODES = 2_000`, hitting the budget drops **entire later branches**,
losing a whole dialog: precisely the outcome the comment claims the design
prevents.

`caps()` (`:190-198`) reports `type_text: true, key: true`; `apply` (`:264`)
returns `Unsupported` for everything except `Click`.

`watch()` (`:208-216`) returns `Unsupported` for `Quiet` — the default — so an
AT-SPI program never settles and snapshots a pre-change tree.

**Fix:** `VecDeque::pop_front`; make `caps()` honest; implement the tree-digest
settle the comment already describes (same shape as `pty.rs:337-378`).

### H8 — A failed render silently skips frame callbacks; clients freeze, capture returns a stale frame as success

`wayland.rs:775-778` clears `dirty` **before** calling `render`, and `render`
(`:1087`) bails on `bind` failure before reaching `send_frames` (`:1113-1116`).
All GL errors are `let _ =`.

The module docs name this exact failure: a client that gets no frame callback
draws one frame and appears to hang, looking like a client bug. So a transient GL
error hangs every client on the stage permanently, and `capture` then returns the
last-good buffer as a successful capture.

**Fix:** send frame callbacks unconditionally; on render failure set `dirty` again
to retry; propagate the error into the next `Capture` reply.

### H9 — Label containment accepts a one-character name; both defence layers fail together

`program.rs:189-192` accepts `claimed.contains(actual)`. Verified by probe:
`check_label(claimed="Save", actual="a") -> true`. Any element whose accessible
name is a single character (terminal cells, bare icons, list bullets) passes
*any* label.

`macros.rs:174-187`'s `find_anchor` has the same unguarded containment, takes the
**first** match in entry order, and uses raw `to_ascii_lowercase` rather than
`program::normalize` despite a comment claiming it applies "the same tolerance".
So replay can select a one-letter node and `check_label` rubber-stamps it.

Also: `normalize`'s punctuation trim reduces `"..."` to `""`, and an empty actual
name skips the check entirely (`:178`) — so a `…` overflow button accepts any label.

**Fix:** require `actual.chars().count() >= 3` (or a word boundary) before
allowing containment either way; make `find_anchor` call `program::normalize`;
don't skip the check when normalization *emptied* a non-empty name.

### H10 — `Stage::capture(Some(window))` ignores the window

`wayland.rs:891` destructures `_window` and captures the whole screen. The trait
signature advertises per-window capture; the caller gets a full-screen frame with
no indication.

**Fix:** honour the key, or delete the parameter.

### H11 — `wait_for_window` falls back to an arbitrary window despite claiming pid is authoritative

`host.rs:272-278`: `.find(|w| w.pid == Some(pid)).or_else(|| windows.first())`.
The `or_else` defeats the comment directly above it. With two apps on one stage,
launching B returns **A's** `WindowInfo`, whose `app_id` then feeds
`Probe.app_id` — so adapter matching and rung selection are decided from the
wrong window. It also returns immediately rather than waiting for B.

Relatedly, `wayland.rs:858` hardcodes `mapped: true` for every toplevel, so
`Probe.mapped` is meaningless.

**Fix:** drop the `or_else`; track real map state from the first buffer commit.

### H12 — No navigation, so every URL costs a browser launch

`CdpChrome::apply` handles only `Click` → `ActivateTarget` (`cdp.rs:321-338`),
despite the module doc promising `Page.navigate` / `Target.createTarget` /
`closeTarget`, and despite tab nodes advertising `with_actions(["activate",
"close"])` that nothing can invoke — `Node::actions` is never rendered and never
dispatchable.

A second URL means a whole `launch gui:true`: new process, new profile, up to
15 s connect polling plus 20 s window wait. No back/forward either.

**Fix:** add `{"navigate":{"url":…}}`, `{"back":{}}`, and a generic
`{"invoke":{"anchor":…,"label":…,"action":…}}` dispatching `Node::actions` —
which also unlocks AT-SPI's non-default actions and adapter verbs.

### H13 — `type` appends instead of replacing, and the usual workaround is broken

`cdp.rs:529-542` focuses then `InsertText`, so a pre-filled field becomes
`oldnew`. The standard fix (`ctrl+a` first) is broken by C1.

**Fix:** `Step::Type { clear: bool }`, defaulting true.

### H14 — CDP in-flight counter drifts up on redirects, guaranteeing settle timeouts

`cdp.rs:191-196` increments on every `requestWillBeSent` and decrements only on
finish/fail. Chromium emits an **extra** `requestWillBeSent` per redirect hop
(carrying `redirectResponse`) with no matching finish. The counter is also never
reset across navigations.

After a few redirects — a login or OAuth flow — `inflight` sits permanently above
`IDLE_INFLIGHT = 2` and `quiet` can never be satisfied. Every settle burns the
full timeout: exactly the failure §settle claims to have solved.

*Reporter flagged this as confident on CDP semantics but not driven against a
live browser.* Confirming test: a page that 302s three times, then assert
`inflight() == 0`.

**Fix:** track a `HashSet<RequestId>` rather than a count; skip events carrying
`redirect_response`; clear on main-frame `frameNavigated`.

### H15 — `distill` drops `text` and `key`, so every replayed macro breaks

`main.rs:786-789` sets `text: None, key: None` because neither `ComputerStep`
(`event.rs:224-231`) nor `StepReport` (`surface/mod.rs:74-82`) carries them.
On replay a `key` step becomes `Step::Key("")` → "unknown key"; a `type` step
types nothing and reports `ok`.

**Fix:** add the fields to both structs and populate in `report_for`.

### H16 — The D-Bus bring-up can hang the agent forever, from an async fn

`bus.rs:84-95` uses a **blocking** `read_line` with no timeout inside `async fn
start`; `READY_TIMEOUT` covers only the later a11y poll. A `dbus-daemon` that
starts but never prints blocks a tokio worker indefinitely. `Drop for StageBus`
also calls blocking `child.wait()` on a runtime thread, and
`StageWayland::drop` additionally `thread.join()`s.

**Fix:** `tokio::process::Command` + `timeout` around the address read; move Drop
teardown behind `spawn_blocking` or an explicit async `shutdown()`.

---

## MEDIUM

- **M1 — `scroll.anchor` is schema'd and ignored.** `Step::Scroll` holds a bare
  `Option<String>`, not a `Target` (`program.rs:49-54`), so it is never resolved
  or label-checked; both backends scroll the whole document
  (`cdp.rs:544`, `pty.rs:390`) and report `ok`. A virtualized inner list cannot
  be scrolled at all.
- **M2 — `Screen.text` grows unbounded and carries raw ANSI into observations.**
  `pty.rs:56-63` appends every primary-screen byte forever with no trim, and
  appends the **raw stream**, so a colored prompt puts `\x1b[0;32m…` into the node
  the model reads. Cap it with a ring buffer and parse before emitting.
- **M3 — Teardown leaks.** *(Partly fixed 2026-07-31: process-group kill, reaping,
  poison-safe Drop locks.)* Remaining: `Host::launch` spawns a non-Chromium GUI
  app then always returns `Err` (`host.rs:237`), leaving it running and
  unregistered; nothing ever removes a stage runtime dir; `--remote-debugging-port=0`
  binds loopback with no auth, so any local process can take CDP control of a
  browser holding the user's credentials — inherent to CDP, but a hole in what
  the doc frames as an isolation boundary.
- **M4 — Adapter discovery is a latent supply-chain hole.** Not live (nothing
  calls `for_project`), but `adapters.rs:200` puts `<project>/.artist/computer/adapters/`
  in the roots, and those TOMLs declare argv executed verbatim by
  `programmatic.rs:96-105` with the user's `$HOME`. Cloning a repo would be enough
  to plant an executable adapter. `artist-rules` is **not** a comparable
  precedent — project rules produce only inert reminder text, and its loader
  rejects symlinks and caps entries; `adapters.rs:139-149` does neither. Restrict
  to the global config root, or gate project adapters behind a trust prompt keyed
  to a content hash.
- **M5 — `[computer] enabled = false` still hands the model the tool.** Only
  `compaction::decay` reads it (`compaction.rs:65`); registration checks solely
  `profile.permits` (`lib.rs:811`).
- **M6 — `panic!` on the compositor thread.** `wayland.rs:181` panics on
  unrecognized client data. To the crate's credit the proxy *detects* the death
  (the channel drops, `ask` returns "compositor has stopped") — but `spawn`
  swallows it via `.ok().flatten()` (`:544`), so a dead compositor silently
  becomes "no X11" and the app is launched against a dead socket anyway.
- **M7 — `surface_damage` publishes subsurface-local coordinates as screen
  coordinates.** `commit` fires for every `wl_surface` including subsurfaces
  (`wayland.rs:184-210`); only toplevels get a `WindowKey`. `NoiseFilter` keys on
  the exact rect tuple, so a caret in a subsurface collides in signature with
  anything damaging the same screen coordinates. Damage is never clipped to screen.
- **M8 — Advertised concurrency guards that don't exist.** `opening: DashSet` is
  declared "guards against two concurrent opens" and is never inserted into
  (`tool.rs:42,122`); the module doc claims "tombstone-once reaping in `list`" and
  `list` does none. *The input lease itself is correct and tested.*
- **M9 — The model is never told what a surface can do.** `Caps` exists so "the
  tool can refuse rather than pretend" (`model.rs:257`) and is never rendered;
  `render_surfaces` prints only id/rung/title. The model learns `click` is
  impossible on a terminal by trying it.
- **M10 — Decay's recovery instruction returns nothing.** The stub says
  `mode="observe"`, which returns a delta — `(no change)` on an idle surface.
  Worse, PTY primary-screen `snapshot()` is **destructive** (`pty.rs:70-83`), so
  decayed shell output can never be re-read at any setting. `keep_recent` is also
  global rather than per surface.
- **M11 — Human observability is thin and partly wrong.** `computer_title` reads
  `arguments["program"]` but the parameter is `command` (`titles.rs:61`), so every
  launch renders an empty target; branches exist for modes that don't exist; a
  multi-step program shows only `Ran 3 steps on tab:7` — the watcher cannot see
  *what* is being clicked, which is exactly when they would intervene. The
  guardrail matches on labels the human never sees.

---

## LOW

- `wayland.rs:832` — comment says the client handle keeps XWayland alive,
  immediately above `drop(client)`.
- `main.rs:846` `short()` slices bytes; panics on a non-ASCII digest.
- `computer_frame` with an empty digest matches the first attachment of the first
  session; prefix collisions resolve first-wins with no ambiguity error.
- `program.rs:211` filters `_` anywhere in a name, so `delete_all` and `deleteall`
  normalize alike.
- `render.rs:72` counts nameless nodes into `text_hidden`, inflating "N more".
- `model.rs:241` `digest()` excludes `actions`, so a node becoming non-actionable
  produces no delta.
- Anchors: if the allocator reissues a freed handle in the same `reconcile` that
  retires it, the removal loop's guard (`anchors.rs:154`) skips reporting the
  removal — the model is never told its element vanished. Needs a full cursor wrap
  over ~2983 words to trigger.
- Dead mechanisms: `SettleKind::Anchor` (absent from the schema, `Unsupported` in
  both backends), `Node::actions`, `Surface::children` (no implementors — which is
  why a `target=_blank` tab is unreachable), `pixels::annotate`.
- `gui:true` splits `command` on whitespace (`tool.rs:316`), breaking quoted args
  and URLs; `cwd` is silently ignored for `gui`.

---

## Missing affordances a real task hits immediately

From the UX review, none currently supported: **iframe traversal**
(`GetFullAxTreeParams::default()` is main-frame only, so any payment or login
iframe is invisible); **file upload** (`DOM.setFileInputFiles`, and the native
chooser it opens cannot be attached); **download handling**; **attaching a new
tab** (`Surface::children` has no implementors); **`<select>`/combobox
selection**; **hover**; and **paging a long table** — `TEXT_BUDGET = 40` with an
overflow hint that suggests `full=true`, which re-renders the same first 40.

---

## What both reviewers agreed is genuinely sound

Worth recording, because it is what should *not* be touched while fixing the above:

- **`anchors.rs` is the strongest module.** Identity-only resolution, the
  tombstone/reclaim interaction, and `dedupe` are correct; the
  `NotIssued`/`Stale` distinction holds; geometry is correctly excluded from the
  digest. Both reviewers said so unprompted.
- **`check_label`'s normalize-plus-containment with no edit distance** is exactly
  right (given the H9 length guard).
- **Arm-before-dispatch settle ordering**, with a test that asserts the call
  order — a subtlety most implementations get wrong.
- **`stage/damage.rs`** — the gap-detector-keyed-to-last-sighting reasoning is
  right and the tests pin what the comments claim. No defects found.
- **`decay.rs`'s stated invariant** — arity, ids and `call_id`s are genuinely
  preserved; the id-map-plus-sentinel double check does what it claims;
  `revise` vs `replace` is correctly reasoned.
- **CDP `networkIdle` from the event stream**, the `DevToolsActivePort` trick, and
  the lease-before-lock ordering.
- **`render.rs` budget** — interactive nodes survive unconditionally, overflow is
  named, whitespace preserved inside values, and `{:?}` escaping means page
  content cannot inject a fake `<observation>` sentinel.
- **`pty.rs::key_bytes`** — the only correct modifier handling in the crate, and
  the model for fixing C1.

---

## Suggested order of work

1. **C1, C2, C3** — silent wrong actions, credential exposure, losing the user's
   instructions. Nothing else matters until these are done.
2. **The §Status correction** — one edit, removes the false confidence that makes
   everything else harder to reason about.
3. **H1, H2, H4** — the three safety/recovery properties that are advertised and
   don't hold. H4 is one line.
4. **H5, H6, H13, M9** — the things that make the model flail on ordinary tasks.
5. **Wire what exists** — event recording (unlocks log/distill/audit), AT-SPI
   attachment, adapter discovery (behind M4's trust decision), `screen` setting.
6. Everything else.
