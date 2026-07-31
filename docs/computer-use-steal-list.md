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

**From:** `agent-sh/computer-use-linux` · **Cost:** ~1–2 days · **Fit:** perfect

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

**From:** Playwright MCP · **Cost:** ~150 lines · **Fit:** perfect

`{"mode":"find","surface":…,"query":"Delete"}` → matching nodes with anchors and
tree-path context, without rendering the surface.

*What it fixes:* a large page costs a full render to locate one button. The
collection is **already complete** — a delta is a rendering choice, not a partial
read (`AnchorBook::observe` collects everything and `entries.retain` only filters
what is *shown*). So this is a pure read over data we already hold, with no new
backend work at any rung. Highest value per line on the list.

*Design note:* it must return anchors from the *current* epoch, not mint new ones,
or it becomes a second observation path with its own staleness semantics.

### 3. `needs_human` — a defined handoff error

**From:** OpenClaw; Skyvern; Operator · **Cost:** one error variant plus a rule

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

### 4. Mark observations as untrusted content

**From:** Anthropic; OpenClaw · **Cost:** ~1 day · **Fit:** perfect

Wrap rendered observations in a delimiter that says the enclosed text is data from
a third party, and add tool-description guidance that instructions found inside a
surface are not instructions.

*What it fixes:* **the whole prompt-injection surface, which we currently do not
address at all.** Our destructive-action guardrail inspects the model's *output*.
Nothing inspects the *input*, and a rendered page is attacker-controlled text
going straight into context. The stage shares `$HOME`. This is the largest
unaddressed risk in the subsystem and among the cheapest to reduce.

*Honest limit:* delimiters are mitigation, not prevention. Worth pairing with (11).

### 5. `observe: none | delta | full`

**From:** chrome-devtools-mcp · **Cost:** one field

Let a step return no observation at all.

*What it fixes:* a program whose outcome the model already knows still pays for an
observation. chrome-devtools-mcp defaults `includeSnapshot` to **false** on every
input tool; we default to a delta.

*Why we can do this more safely than they can:* `expect` is mandatory here, so
silence still carries a verified assertion that the program landed where the model
said. They default to silence *without* that guarantee. Keep `delta` as our
default — it is the right one — and make `none` available.

### 6. Payload budgeting with returned scale metadata

**From:** `computer-use-linux`; Anthropic · **Cost:** ~1 day

Hard caps on screenshots (max width, max bytes) applied *before* the image enters
context, with the response reporting `coordinate_width`/`height`/`scale`.

*What it fixes:* our newly-wired `screenshot` mode returns a full-resolution PNG of
a 1920×1080 stage. Decay reclaims that context afterwards; a byte budget stops
paying for it in the first place. Prevention beats cure.

*Fit note:* the scale metadata matters less for us than for them, since we never
hand the model coordinates — but it is exactly what a set-of-mark overlay needs to
place its marks correctly on a downscaled frame.

---

## Tier 2 — Substantial, clearly worth it

### 7. `zoom` — a region at full resolution

**From:** Anthropic reference implementation · **Cost:** ~half a day

Capture a named element's bounds, or a named region, at native resolution.

*What it fixes:* "the screenshot is too small to read", for which we have nothing.
We already hold `Node::bounds` harness-side, and `capture(Some(window))` now crops
— so this is mostly plumbing an existing crop to an anchor instead of a window.

*Fit note:* the model names an *element*, not a rectangle, which keeps it inside
our contract. `{"mode":"zoom","anchor":…,"label":…}`.

### 8. A live view of the stage

**From:** ZeroClaw; oh-my-pi's `/collab` · **Cost:** ~3–5 days

An attachable viewer onto the render target — read-only for watching, read-write
for a human taking over.

*What it fixes:* two things at once. It is the best human-oversight design in the
survey, and it is a **precondition for (3)**: telling a user "I need you to
complete 2FA" is useless if they cannot see the screen to do it. oh-my-pi seals
frames client-side and gates by QR code; that shape is worth copying.

*Why it is cheap for us specifically:* we already render offscreen into a buffer we
own and already encode PNGs. There is no screencast portal to negotiate and no
permission prompt — the frames are ours.

### 9. Self-repair for the environment

**From:** `computer-use-linux` · **Cost:** ~2 days · **Fit:** good, with a caveat

Where `doctor` reports a fixable blocker, offer to fix it: flip the toolkit
accessibility key, install what is missing.

*Caveat, and it is the whole design:* they modify the **user's live session**
(GNOME keys, a Shell extension). We must not. Our version repairs only the
*stage's* environment — which is strictly easier, because we own that environment
and set it at bring-up. Most of what they repair after the fact, we can simply get
right when starting the bus. The remainder (a missing `at-spi2-core`) is a
package-manager instruction, not an action.

### 10. Heal-on-mismatch in macro replay, opt-in

**From:** Stagehand · **Cost:** ~2 days

When a distilled macro hits a label that no longer resolves, fall back to the model
*at that step*, then patch the macro.

*What it fixes:* macros are currently brittle by design — `replay` stops at the
step that broke. That is the right *default* (a macro that silently retargets is
the failure mode this whole subsystem exists to prevent), but it means a macro is
worthless the day the UI moves.

*Fit note:* keep the hard stop as the default and make healing an explicit opt-in.
Turning "works until the UI changes" into a durable asset is what makes
distillation worth having at all.

### 11. Prompt-injection classification on observations

**From:** Anthropic · **Cost:** ~2 days, plus a model

Run rendered observations past a classifier before they enter context.

*What it fixes:* the residue of (4). Delimiters tell the model what is data;
a classifier catches the case where the model is convinced anyway.

*Fit note:* rung 0 and rung 1 observations are structured and comparatively safe;
this matters most for rung 2/3 where arbitrary page text arrives. Scope it there.

### 12. `extract` with a JSON schema

**From:** OpenClaw · **Cost:** ~2 days, plus a side-model call

"Pull the order total and delivery date from this page" → a bounded side-model call
against the surface, returning only the answer.

*What it fixes:* reading data currently means rendering the whole surface into the
main context and reasoning there. This keeps the page out of the main context
entirely — the answer arrives, the page does not.

*Fit note:* the composition is good — `find` (2) narrows, `extract` reads. Both are
reads over the collection we already hold.

### 13. Backend cascade with attributed failure

**From:** `computer-use-linux` · **Cost:** ~1 day · **Fit:** partial

They cascade GNOME extension → Introspect → COSMIC → KWin → hyprctl → i3 → EWMH,
reporting which won *and why each declined*.

*What is actually transferable:* not the cascade — we have exactly one compositor,
ours, deliberately. The transferable part is **attributed decline**: our `select()`
picks a rung and says what it picked, but not what it rejected or why. "Chose rung
2; rung 0 declined (no adapter matches `org.gnome.Nautilus`), rung 1 declined (not
a Chromium process)" is a far better error and a far better log line, and it is
the difference between "write an adapter" and "something went wrong".

### 14. CDP-attach to an already-running application

**From:** oh-my-pi · **Cost:** ~2 days · **Fit:** good, needs a decision

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

## Tier 3 — Real capability gaps, expensive

### 15. A localizer rung between 2 and 3

**From:** Agent-S3 + UI-TARS · **Cost:** weeks, plus a hosted 7B model

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

**From:** OSWorld; Cua-Bench · **Cost:** weeks

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

**From:** Open-AutoGLM; trycua/cua; Agent-S3's AndroidWorld · **Cost:** weeks

*What it fixes:* a category we do not touch at all. Three independent projects
treat it as table stakes.

*Fit note:* the ladder transfers cleanly and the argument for it is *stronger*
there — Android has an excellent rung 0 (intents, `content` providers, `am
broadcast`) and a first-class rung 2 (`AccessibilityNodeInfo`, `uiautomator`). What
does not transfer is the stage: an emulator or ADB replaces the compositor, and
with it we lose damage-region settle. A genuinely separate `Stage` implementation.

### 18. Credential handling as a feature

**From:** Skyvern · **Cost:** weeks · **Fit:** questionable

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
| **`--mobile`-style viewport presets** | Playwright MCP | Free given our configurable `screen`. They report it as a *token* saving, not just a testing feature — a narrow viewport renders fewer nodes. |
| **`--output-mode file`** | Playwright MCP | Large observations to a file, with the path in context. Complements decay: the content stays reachable without staying resident. |
| **Trajectory recording as a first-class artifact** | Anthropic; Cua-Bench | We record `computer.*` events already; exporting them in an interchange format costs little and is what a benchmark harness consumes. |
| **Batched tool calls** | Anthropic | Their production track. Our `Program` already batches steps — worth checking whether the *tool-call* layer batches too. |
| **`medium` thinking effort as the default for computer use** | Anthropic (measured) | Best accuracy-to-cost ratio; `low` uses *fewer* output tokens than no thinking, because fewer mistakes mean fewer retries. A settings default, not code. |
| **Prompt caching across a session** | Anthropic | Observations are large and mostly stable; the cache boundary matters more here than in ordinary use. |
| **Durable surface ownership in SQLite** | OpenClaw | They persist tab ownership across restarts. Our registry is per-process; a resumed session loses its surfaces. Low value while sessions are short. |
| **SSRF policy on adapter HTTP calls** | OpenClaw | Our `Call::Http` adapters hit localhost by design; a policy that says so explicitly, rather than by convention, is a few lines. |
| **Attributed rung decline in the observation header** | `computer-use-linux` | The user-facing half of (13): show which rung is driving each surface and what was passed over. |

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
