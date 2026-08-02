# Computer use — the exhaustive steal list

Everything worth taking from the competitive sweep, in one place. Companion to
[the landscape survey](computer-use-landscape.md) (where each item was found and
under what confidence) and [the audit](computer-use-audit.md) (what is wrong with
what we already have).

**How to read this.** Items are grouped by what they buy, not by who has them.
Each carries a **cost** (rough implementation size), a **fit** verdict, and — the
part that matters most — *what it changes about a task that currently fails*. An
item nobody can name a failing task for is not on this list.

**Fit** is a real filter, not a formality. Three of our design commitments are
load-bearing and several attractive ideas violate them:

- the model names elements, never coordinates;
- an anchor resolves by identity or not at all — no matching, no fallback;
- `expect` is mandatory, so a step that lands somewhere unexpected fails loudly.

Where an item conflicts, the conflict is stated rather than smoothed over. Two
items are **rejected outright** at the end, with reasons, because a steal list
that only ever says yes is a wish list.

Status as of 2026-07-31: the audit's three CRITICALs and sixteen HIGHs are fixed;
the previously-unwired paths (`computer.*` events, AT-SPI attach, adapters,
`[computer] screen`, screenshots) are wired. Nothing below is a defect fix — this
is all new capability.

---

## Tier 1 — Do these first

Cheap, and each one removes a category of failure we currently have no answer
for.

### 1. `computer doctor` — one structured readiness report

**From:** `agent-sh/computer-use-linux` · **Scope:** one probe per dependency, each with a fix string · **Fit:** perfect

One command that reports, per check: platform, DRM render node, `libgbm`/`libEGL`/
`Xwayland` presence, `dbus-daemon`, `at-spi-bus-launcher`, `at-spi2-registryd`,
`$XDG_RUNTIME_DIR` and its mode, chromium, and — for a running stage — whether the
a11y bus answers and how many nodes the tree has. Each check carries a **blocker**
and a **recommended next step**, not just a boolean.

*What it fixes:* the worst debugging experience in the subsystem. Our own module
docs name the trap — a toolkit that was not told accessibility is on produces an
empty tree, and an empty tree looks exactly like a compositor fault. We documented
it; they productized the fix. Every one of these checks already exists as a
scattered runtime error; this collects them into something a user can run *before*
a task fails.

*Note:* our errors are already unusually good at naming the fix (see
`ensure_stage`'s `$XDG_RUNTIME_DIR` message). `doctor` makes that available
without first provoking a failure.

### 2. `find` over the observation collection

**From:** Playwright MCP · **Scope:** ~150 lines over the collection we already hold · **Fit:** perfect

`{"mode":"find","surface":…,"query":"Delete"}` → matching nodes with anchors and
tree-path context, without rendering the surface.

*What it fixes:* a large page costs a full render to locate one button. The
collection is **already complete** — a delta is a rendering choice, not a partial
read (`AnchorBook::observe` collects everything and `entries.retain` only filters
what is *shown*). So this is a pure read over data we already hold, with no new
backend work at any rung. Highest value per line on the list.

*Design note:* it must return anchors from the *current* epoch, not mint new ones,
or it becomes a second observation path with its own staleness semantics.

### 3. `needs_human` — a defined handoff error — **declined**

**From:** OpenClaw; Skyvern; Operator · **Scope:** one error variant plus a rule

A `StepError::NeedsHuman { kind }` over `login | 2fa | captcha | permission |
payment`, which halts the program and surfaces to the user rather than being
retried.

*What it fixes:* this is where real tasks die, and the agent currently has no
vocabulary for it — a 2FA prompt is just an unexpected screen, so the model
improvises, and improvising at a login page is the worst possible place to
improvise. OpenClaw's contract is explicit: *report* login/2FA/captcha as manual
action rather than guess.

*Fit note:* pairs with our existing guardrail machinery — the same `fire: per-turn`
shape, but firing on the *observation* rather than the tool arguments.

**Decision (Adam): NO.** *"An agent can do all of these except two-factor, and
even two-factor is possible with phone access."* The agent holds the user's own
credentials by design (18), so a login is work it should do, not a wall it should
stop at. This was built once anyway, in error, and reverted whole — the note
stays so it is not built a third time. The part Adam does want is (8): the user
plugs into the session because they choose to, *not* because the agent was
blocked.

### 4. Mark observations as untrusted content — **out of scope, by decision**

**From:** Anthropic; OpenClaw · **Scope:** a delimiter in `render.rs` plus tool-description guidance · **Fit:** perfect

Wrap rendered observations in a delimiter that says the enclosed text is data from
a third party, and add tool-description guidance that instructions found inside a
surface are not instructions.

*What it fixes:* **the whole prompt-injection surface, which we currently do not
address at all.** Our destructive-action guardrail inspects the model's *output*.
Nothing inspects the *input*, and a rendered page is attacker-controlled text
going straight into context. The stage shares `$HOME`. This is the largest
unaddressed risk in the subsystem and among the cheapest to reduce.

*Honest limit:* delimiters are mitigation, not prevention.

**Decision (Adam, standing): permanently out of scope**, together with (11). artist is a YOLO harness by design and prompt-injection defence is not a thing we are building. Recorded here rather than dropped so that the risk stays legible: the stage shares `$HOME`, a rendered surface is third-party text going straight into context, and nothing inspects it. That is an accepted risk, not an oversight, and it should not be re-proposed.

### 5. `observe: none | delta | full`

**From:** chrome-devtools-mcp · **Scope:** one field

Let a step return no observation at all.

*What it fixes:* a program whose outcome the model already knows still pays for an
observation. chrome-devtools-mcp defaults `includeSnapshot` to **false** on every
input tool; we default to a delta.

*Why we can do this more safely than they can:* `expect` is mandatory here, so
silence still carries a verified assertion that the program landed where the model
said. They default to silence *without* that guarantee. Keep `delta` as our
default — it is the right one — and make `none` available.

### 6. Payload budgeting with returned scale metadata — **declined**

**From:** `computer-use-linux`; Anthropic · **Scope:** a downscale step and the scale factor returned alongside it

Hard caps on screenshots (max width, max bytes) applied *before* the image enters
context, with the response reporting `coordinate_width`/`height`/`scale`.

*What it fixes:* our newly-wired `screenshot` mode returns a full-resolution PNG of
a 1920×1080 stage. Decay reclaims that context afterwards; a byte budget stops
paying for it in the first place. Prevention beats cure.

*Fit note:* the scale metadata matters less for us than for them, since we never
hand the model coordinates — but it is exactly what a set-of-mark overlay needs to
place its marks correctly on a downscaled frame.

**Decision (Adam): NO** — *"How do we know fidelity isn't lost in terms of agent
perception?"* The right answer, and more right than the reason given: rung 3 reads
text off pixels, and downscaling is precisely what destroys small text. If a cap
is ever wanted it should be **by bytes with a floor on the short edge**, and
justified by measurement — run OCR at each size and count how many labels
survive. That is an objective fidelity metric. Until someone runs it, no cap.

---

## Tier 2 — Substantial, clearly worth it

### 7. `zoom` — a region at full resolution

**From:** Anthropic reference implementation · **Scope:** crop and pad an existing frame; no new capture path

Capture a named element's bounds, or a named region, at native resolution.

*What it fixes:* "the screenshot is too small to read", for which we have nothing.
We already hold `Node::bounds` harness-side, and `capture(Some(window))` now crops
— so this is mostly plumbing an existing crop to an anchor instead of a window.

*Fit note:* the model names an *element*, not a rectangle, which keeps it inside
our contract. `{"mode":"zoom","anchor":…,"label":…}`.

### 8. A live view of the stage — **built**

**From:** ZeroClaw; oh-my-pi's `/collab` · **Scope:** a Wayland client, plus a dmabuf-exportable render target

The stage is deliberately invisible, which is right for isolation and wrong for
trust: *"it clicked something and said it worked"* is not a thing anyone should
have to take on faith.

**Decided: the seat is shared.** A person driving does not pause the agent.
Pausing would make the human's only way in a way of stopping the agent working,
which turns "let me help" into "let me interrupt" — and Adam's original note ruled
that out: the takeover is the user choosing to, never the agent being blocked.

**Decided: a native window, not a browser page.** A first attempt served PNGs
over HTTP on loopback and was thrown away, because it discards the one advantage
this design has:

* The stage's frame is **already a GPU buffer** on the same render node the
  user's compositor uses. Exporting it as a dmabuf and attaching it as a
  `wl_buffer` is zero-copy. The HTTP version instead did `copy_framebuffer` →
  `map_texture` → PNG encode → HTTP → decode → canvas blit: four conversions and
  a CPU round trip to move a buffer between two processes on one graphics card.
* **Damage passes straight through.** Our rects become `wl_surface.damage_buffer`,
  so the user's compositor does a partial update natively and an idle stage costs
  nothing. Over HTTP they would have to be re-encoded as cropped PNGs and
  recomposited in JavaScript.
* **Input gets better.** A real `wl_pointer`/`wl_keyboard` carries modifiers, key
  repeat and scroll with correct coordinates, instead of a hand-written JS keymap
  and `naturalWidth` scaling — which was the most likely bug in the whole page.

**The honest cost:** a native window shows nothing over SSH, and Adam works over
mosh. That is the only thing the web version was good for, so remote viewing is a
separate, explicitly-degraded fallback if it is ever wanted — never the primary.

**Built, in this order:**

1. **A dmabuf-exportable render target — done.** The stage currently draws into a
   `GlesRenderbuffer` from `Offscreen::create_buffer`, which cannot be exported.
   It needs a GBM buffer object bound as the render target instead, which means
   `make_renderer` keeps its `GbmDevice` rather than dropping it after the
   `EGLDisplay` is made. Everything downstream — `draw`, `capture`,
   `copy_framebuffer` — binds the same way, so this is one allocation site and
   one field, not a rewrite. It is still surgery on a render loop covered by 19
   integration tests, and should be done with the fake client's colour
   assertions run after every step.
2. **A viewer client — done.** `wayland-client` promoted from dev-dependency to
   dependency; a thread connected to the user's real `WAYLAND_DISPLAY`, binding
   `wl_compositor`, `xdg_wm_base`, `zwp_linux_dmabuf_v1` and `wl_seat`. It
   imports the exported fd through `zwp_linux_buffer_params_v1`, attaches, and
   damages on the stage's existing `DamageSubscription`.
3. **Input back into the seat — done.** Pointer and keyboard events forwarded to
   `Stage::pointer`/`key`/`text` — the same calls the agent makes, because input
   was already a function call rather than a device.
4. **Telling the agent it is not alone — done.** A human acting mid-program means the
   next observation contains something no step caused, and `expect` may fail for
   a reason the model cannot see. Human actions are counted, and the program
   report says a person acted during it. Without this the shared seat quietly
   makes the agent look broken; this is the piece that makes the decision above
   survivable rather than merely nice.


**How it ended up.** `computer watch` opens the window. The stage's target is a
GBM buffer object exported as a `Dmabuf`; the viewer imports those same file
descriptors through `zwp_linux_dmabuf_v1`, so the thing drawn into and the thing
shown are one allocation. A test asserts the export carries real planes and real
descriptors *and* that `capture` still reads the same buffer — because "exports
something" and "exports the screen" are different claims.

Input goes back through `Stage::pointer`/`key`, scaled from window coordinates to
stage coordinates, since the window is resizable and the stage is not. Keys are
passed as **names**, not characters: `keys::name_for_evdev` turns the compositor's
evdev code into the stage's own vocabulary and the stage applies its keymap once.
Translating to a character in the viewer would apply a layout twice, which is how
a viewer types the wrong character on a non-US keyboard.

The activity count is taken *inside* the input lease, so it counts only what
happened during that program — outside it, a click from before the lease would be
blamed on a program it preceded. When an `expect` fails and a person acted, the
report says so and tells the model to observe before concluding its own step
misfired.

**Driven by damage, not a clock.** The stage already computes which rectangles
changed for its own settle predicates; the viewer commits exactly those and
nothing at all when the screen is still, so an idle stage costs zero. A task
bridges the async `DamageSubscription` into the client thread over a sync
channel; a burst is coalesced into one commit, and a `Lagged` receiver repaints
everything once rather than replaying a queue of stale rectangles. The 50 ms
service tick is not a frame rate — nothing is drawn on it — it exists so
configures, input and close requests are still read on a perfectly still stage,
and a test pins that distinction so it is not later mistaken for one.

### 9. Self-repair for the environment — **built**

**From:** `computer-use-linux` · **Scope:** a repair action per `doctor` check, and a policy for when to run one · **Fit:** good, with a caveat

Where `doctor` reports a fixable blocker, offer to fix it: flip the toolkit
accessibility key, install what is missing.

*Caveat, and it is the whole design:* they modify the **user's live session**
(GNOME keys, a Shell extension). We must not. Our version repairs only the
*stage's* environment — which is strictly easier, because we own that environment
and set it at bring-up. Most of what they repair after the fact, we can simply get
right when starting the bus. The remainder (a missing `at-spi2-core`) is a
package-manager instruction, not an action.


**What it turned out to be.** `artist computer doctor --fix`, with a line drawn
that matters more than the feature: a repair may only touch **our own state** —
fetching model weights into our model directory. Anything needing a package
manager and root is reported with the command, never run. A tool that silently
installed system packages because a check failed would be doing something nobody
asked for and cannot easily undo. There is a test asserting no offered repair
mentions `apt`, `pacman`, `dnf`, `install` or `sudo`, and another asserting that
*rendering* a report never changes anything.

### 10. Heal-on-mismatch in macro replay, opt-in — **built**

**From:** Stagehand · **Scope:** a fallback path in replay plus the opt-in flag that gates it

When a distilled macro hits a label that no longer resolves, fall back to the model
*at that step*, then patch the macro.

*What it fixes:* macros are currently brittle by design — `replay` stops at the
step that broke. That is the right *default* (a macro that silently retargets is
the failure mode this whole subsystem exists to prevent), but it means a macro is
worthless the day the UI moves.

*Fit note:* keep the hard stop as the default and make healing an explicit opt-in.
Turning "works until the UI changes" into a durable asset is what makes
distillation worth having at all.


**Built, and the opt-in is the design.** The tolerance already existed —
`find_anchor` matched exactly, then fell back to the same `check_label`
containment a live step gets, requiring the match to be *unique*. Two things were
missing, and both are the reason this is acceptable where selector-with-index was
not:

* **It is off by default.** A macro is trusted because it is the path that was
  worked out once; quietly running against a merely similar element turns a
  script back into a guess, and the guess runs unattended. `Healing::Never`
  stops on a rename exactly as it stops on a deletion.
* **Every substitution is reported.** `Replayed::healed` names the recorded label
  and the one actually used, per step, and `note()` renders it with "re-record
  this macro; it is running on tolerance." A healing nobody is told about is the
  silent retarget this design rejects, wearing a different name.

Ambiguity is still a stop: two plausible candidates fail rather than picking one.

**A gap this exposed, now closed.** A distilled macro recorded *what was done*
but not what to do it to — the surface id it carries belonged to a session that
is gone. There is now a `computer.launched` event, `distill` carries the last
graphical launch into the macro, and `artist computer replay <file> [--heal]`
starts the application itself. `--launch` remains as an override, and a macro
distilled before the event existed says so by name rather than failing with a
missing argument.

### 11. Prompt-injection classification on observations — **out of scope, by decision**

**From:** Anthropic · **Scope:** a classifier pass over rendered observations — needs a model to do the classifying

Run rendered observations past a classifier before they enter context.

*What it fixes:* the residue of (4). Delimiters tell the model what is data;
a classifier catches the case where the model is convinced anyway.

*Fit note:* rung 0 and rung 1 observations are structured and comparatively safe;
this matters most for rung 2/3 where arbitrary page text arrives.

**Decision (Adam, standing): permanently out of scope**, with (4). Not revisited.

### 12. `extract` with a JSON schema — **built, with no model at all**

**From:** OpenClaw · **Built**, and without the side-model call it was ranked for

"Pull the order total and delivery date from this page" → a bounded side-model call
against the surface, returning only the answer.

*What it fixes:* reading data currently means rendering the whole surface into the
main context and reasoning there. This keeps the page out of the main context
entirely — the answer arrives, the page does not.

*Fit note:* the composition is good — `find` (2) narrows, `extract` reads. Both are
reads over the collection we already hold.

**What it turned out to be.** The side-model call was the wrong part of the
ranking. The structure a form or a spec table has is *already in the tree*: a
labelled field carries its value, and where it does not, the label and the value
are adjacent nodes and nothing but adjacency relates them. Three deterministic
strategies cover it — the labelled node's own value, an inline `Total: $42.00`
split, and the next node in document order — and `extract` reports which one
answered, because "the labelled element held this" and "this was the text beside
the label" are different confidence levels.

This works at **every rung, including rung 3**: `ScreenSurface` emits OCR boxes as
nodes, and both `detect.rs` and `incremental.rs` sort them by `(y, x)`, so
document order *is* reading order and the adjacency rule holds on a pixel-only
screen. The case that looked like it would most need a model needs none.

**Prose too, and still without a model.** "Summarise the refund policy" looked
like the case that finally needed a side-model reading the page. It does not.
The question is really *"give me the refund policy"* followed by a summary, and
the first half is structural: a heading names a section, and the section is the
nodes that follow it until the next heading. A field matching a heading returns
that section — a few hundred words instead of the whole page — and the model
already in the conversation does the summarising, on an input small enough not to
matter.

Better than the side-model call it replaces on every axis: no second model to
choose, host or pay for, no extra latency, and the reasoning done by the model
that knows what the task is. **Nothing here is waiting on the local-model
question any more.**

One trap, caught by a test: a heading with an empty section must answer with
*nothing*, never fall through to the adjacent-node rule — the node after an empty
heading is the next heading, so falling through answered "Refund policy" with the
words "Privacy policy". Confidently wrong, which is worse than missing.

### 13. Backend cascade with attributed failure — **built**

**From:** `computer-use-linux` · **Scope:** record why each rung declined, and render it · **Fit:** partial

They cascade GNOME extension → Introspect → COSMIC → KWin → hyprctl → i3 → EWMH,
reporting which won *and why each declined*.

*What is actually transferable:* not the cascade — we have exactly one compositor,
ours, deliberately. The transferable part is **attributed decline**: our `select()`
picks a rung and says what it picked, but not what it rejected or why. "Chose rung
2; rung 0 declined (no adapter matches `org.gnome.Nautilus`), rung 1 declined (not
a Chromium process)" is a far better error and a far better log line, and it is
the difference between "write an adapter" and "something went wrong".


**Built to Adam's framing, not the original.** Each decline carries a **remedy**
where one exists, because a decline that cannot be acted on is a shrug. "rung 0
declined" became "no adapter matches `zenity` — write one at
`$ARTIST_CONFIG_DIR/computer/adapters/zenity.toml` if this program has a D-Bus,
CLI or localhost-HTTP interface". Declines with no remedy carry none: a program
that is not Chromium-based will never speak CDP, and a to-do there would be noise
in every report. `surfaces` shows the rung by name and, beneath it, only the
*better* rungs that were passed over and could be opened:

```
tab:7f2   rung 1 (engine)   Example Domain   [click,type,key,scroll]
  ↑ could be cheaper: rung 0 — write an adapter at …/chromium.toml if …
```

### 14. CDP-attach to an already-running application — **built**

**From:** oh-my-pi · **Scope:** a second attach path alongside `Stage::spawn` · **Fit:** good, needs a decision

Point the agent at a *running, already-logged-in* Electron app or browser.

*What it fixes:* the credential problem, without a credential vault. The user is
already signed in; we attach rather than authenticate.

*The decision it forces:* this is explicitly **outside the stage**. It touches the
user's live session, which is the one thing the stage exists to avoid — the agent
would be driving a window on the user's screen. That may well be worth it, but it
is a policy choice for the user to make per attach, not a capability to add
quietly. Gate it behind an explicit opt-in that names what is being given up.

*Check first:* whether our existing `connect()` path already works against a
running browser we did not launch. It reads `DevToolsActivePort` from a profile
directory, so it plausibly does — which would make this a settings question rather
than an implementation.

---


**Built:** `computer attach` with a `ws://` devtools endpoint, plus a loopback-only
guard — a devtools endpoint is total control of a browser, and a remote one is
somebody else's.

**Why it is worth having:** credentials. The stage's browser starts with an empty
profile, so every task on a signed-in site begins at a login page. The user's own
browser is already signed into everything. That is the same reasoning as sharing
`$HOME` rather than sandboxing it.

**Said plainly to the model, because it is the one place isolation does not
hold** — and it is not a leak, it is what was asked for. Launching a browser with
a debugging port is not something that happens by accident. Focus and input
guarantees still hold, since a CDP page is driven by protocol message and never
through the seat. What does not hold is the private accessibility tree: the tabs
are the user's, and closing one closes theirs. The tool description says so, and
a test asserts it still does.

**The UX, resolved the honest way: it takes a port.** Nobody knows their devtools
websocket path — but everybody knows the number they typed after
`--remote-debugging-port`. So `endpoint` accepts `"9222"` and resolves the
websocket url itself through `/json/version`, read with a bounded request because
Chromium's DevTools endpoint holds a connection open regardless of
`Connection: close` — the same trap that made `connect` read
`DevToolsActivePort` off disk instead.

Rejected: **artist relaunching the user's browser** with the flag. It is the most
usable option and it closes every window they had open. The tool description
tells the model to ask rather than do it.

## Tier 3 — Real capability gaps, expensive

### 15. A localizer rung between 2 and 3

**From:** Agent-S3 + UI-TARS · **Scope:** a whole new rung, and a hosted 7B model to serve it — the largest item here

A self-hosted grounding model converting a *label* to a click point, sitting below
the anchor layer.

*What it fixes:* **the largest capability gap we have.** Rung 3 is observation-only
by design, so anything with no tree — a game, a canvas, an application whose
accessibility support is broken — is entirely out of reach. Today the honest answer
is "no actionable surface", which is a good failure but still a failure.

*Fit, and this is the important part:* it does **not** violate our contract. The
model still names elements; the localizer is an implementation detail *below* the
anchor layer, exactly as `Component.GetExtents` is at rung 2. Agent-S3's
planner/grounder split is the proof: GPT-5 plans, UI-TARS-1.5-7B grounds, and the
planner never sees a coordinate.

*Ranked here purely on cost.* On value alone it would be Tier 1.

### 16. A benchmark number

**From:** OSWorld; Cua-Bench · **Scope:** a harness, a task set, and a scoring rig — an ongoing commitment, not a one-off

OSWorld-Verified on Docker+KVM, plus a WebVoyager subset.

*What it fixes:* we have a strong design argument and **zero evidence**. Every
competitor in the survey has a number; we have none. OSWorld-Verified parallelizes
a full evaluation to under an hour, so the recurring cost is low once built.

*Two honest counterweights.* First, *"An Illusion of Progress?"* (COLM 2025) argues
reported web-agent progress is overstated — a number is not truth. Second and more
pointed: **no existing benchmark rewards rung 0.** A benchmark that scores "pause
the music" identically whether you clicked a button or called MPRIS will never
show our main advantage. We would be measuring ourselves on the axis where we are
most ordinary.

*Which is an argument for publishing our own rung-0 benchmark alongside*, not for
skipping the standard ones.

### 17. Android

**From:** Open-AutoGLM; trycua/cua; Agent-S3's AndroidWorld · **Scope:** a second `Stage` implementation; see [platforms](computer-use-platforms.md)

*What it fixes:* a category we do not touch at all. Three independent projects
treat it as table stakes.

*Fit note:* the ladder transfers cleanly and the argument for it is *stronger*
there — Android has an excellent rung 0 (intents, `content` providers, `am
broadcast`) and a first-class rung 2 (`AccessibilityNodeInfo`, `uiautomator`). What
does not transfer is the stage: an emulator or ADB replaces the compositor, and
with it we lose damage-region settle. A genuinely separate `Stage` implementation.

### 18. Credential handling as a feature

**From:** Skyvern · **Scope:** a vault, an injection path, and a policy — but see the note below · **Fit:** questionable

A credential vault with Bitwarden/1Password integration, so the agent can log in.

*What it fixes:* nobody else in the survey has this, and login is where tasks die.

*Why it is ranked last and marked questionable:* our whole local-execution premise
is that **the user's credentials are already there** — the stage shares `$HOME`,
which is precisely why we run locally instead of remotely. A vault is the answer to
a problem remote agents have. (14) attaching to an already-authenticated session,
and (3)+(8) handing 2FA to a human who can see the screen, address the same failures
without the agent ever holding a secret.

*Revisit if* we ever run somewhere that does not share the user's home directory.

---

## Smaller items worth doing when nearby

| Item | From | Note |
|---|---|---|
| **`--mobile`-style viewport presets** | Playwright MCP | **Built**: `desktop`/`laptop`/`tablet`/`mobile` on `[computer] screen`. The token saving is real but it is bought by *hiding* things behind menus and disclosures, and each of those is a round trip — which is the dominant cost of driving a UI. Worth it on a narrow path through a site with a real mobile layout; a loss on exploratory work or an app that squashes rather than reflows. |
| **`--output-mode file`** | Playwright MCP | Large observations to a file, with the path in context. Complements decay: the content stays reachable without staying resident. |
| **Trajectory recording as a first-class artifact** | Anthropic; Cua-Bench | **Built**: `artist computer export`. Deliberately not the macro format — a macro drops observations, timings and failures because carrying them makes replay brittle, while a harness scores and a fine-tune learns from exactly those. Frame digests rather than pixels, so it stays a readable document. |
| **Batched tool calls** | Anthropic | **Checked: it does not.** rig supports it — `concurrency > 1` on the runner runs a turn's tool calls in parallel — but artist never sets it, so the default of 1 applies and tool calls are sequential. Enabling it is a whole-harness decision, not a computer-use one. Worth knowing for computer use specifically: it would change nothing, because `SurfaceRegistry::input_lease` already serializes programs against a single stage, so two concurrent `computer do` calls would queue on the lease rather than run together. The gains are elsewhere (parallel reads and greps). |
| **`medium` thinking effort as the default for computer use** | Anthropic (measured) | Best accuracy-to-cost ratio; `low` uses *fewer* output tokens than no thinking, because fewer mistakes mean fewer retries. A settings default, not code. |
| **Prompt caching across a session** | Anthropic | **Already built**, before this list existed: `prompt_cache_key` is threaded through `thinking.rs`, `provider_retry.rs` and the Responses request builder, keyed on project + model + lineage. The computer-use angle that remains is not the mechanism but the *boundary* — whether a large observation sits inside the cached prefix or after it — which is a measurement, not a feature. |
| **Durable surface ownership** | OpenClaw | **Built, and it is two features rather than one.** The stage is killed on `Drop` — that teardown guarantee is worth more than surface persistence, so a browser *we launched* cannot survive and what is remembered is a recipe to start it again. A browser the user **attached** to is not ours, was never started by us, and is still there. `Restorable` names that difference explicitly, `surfaces` reports what a previous session had when nothing is open, and each entry says which of the two it is — because "reattach" to something that died with the stage is exactly the wrong move. A JSON file in the state dir, not SQLite: one list, written on attach and launch. |
| **SSRF policy on adapter HTTP calls** | OpenClaw | **Built**, as an allow-list of three literal loopback hosts rather than a block-list — the block-list version keeps losing to DNS names that resolve inward, redirects, and encoded addresses. Checked *after* substitution, because the template is ours but the value spliced into it comes from the model. |
| **Attributed rung decline in the observation header** | `computer-use-linux` | **Built** with (13): `surfaces` names the rung and lists only the *better* rungs that were passed over and carry a remedy. |

---

## Rejected, with reasons

**Coordinate-based clicking with a frame-token guardrail** (OpenClaw's model, and
Anthropic's reference implementation wholesale). OpenClaw's frame tokens are a
genuinely well-designed *coordinate freshness* check — actions must echo the
screenshot's `frameId`, and a node-issued display identity makes a reconnect fail
closed rather than silently retarget. It is the best version of this idea found
anywhere in the sweep.

We still decline it, and their own documentation says why: *"A token is not a
freshness guarantee."* A screen can change without the frame identity changing.
Identity-only anchor resolution has no such gap, and adopting coordinates as a
parallel path would reintroduce exactly the failure the anchor layer exists to
remove — with the added cost that two targeting systems means every backend
implements both.

**Selector-with-index targeting** (`computer-use-linux`; and the "try selector,
fall back to AI" middle tier of Skyvern's three-tier model). Resolving by
`role`/`name` plus an index is ambiguous by construction: two "Delete" buttons, or
a list that re-renders with row 3 now holding what row 4 held, and the selector
silently picks the wrong one. This is the precise failure mode anchors exist to
prevent, and it is the same fallback our own design explicitly refuses in
`AnchorBook::resolve`. Stagehand's healing (10) is the acceptable version of the
same instinct, because it re-derives the target *and says it did*.
