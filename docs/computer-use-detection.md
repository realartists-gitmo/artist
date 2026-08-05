# Computer use — non-human detectability, and the fix for each

An audit of everything `artist-computer` does — or fails to do — that lets a
site or an OS-level observer classify the agent as a bot rather than a person.
Run 2026-08-03 against the state described in `docs/computer-use.md`. Every
finding is grounded in code with `file:line`; where something is a design
stance rather than an oversight, that is stated and the stance itself is then
reviewed.

## Threat model — read this first

"Being flagged as a bot" is not one detector. It is four layers, and the
harness is strong on some and absent on others:

1. **The browser-automation fingerprint.** Does the page see `navigator.webdriver`,
   automation launch flags, a DevTools connection, event sequences no human
   produces? This is what Cloudflare "non-interactive challenge" and most
   commodity anti-bot (DataDome, PerimeterX/`perimeterx.js`, Arkose) score.
2. **Behavioral biometrics.** Mouse-path, keystroke-dynamics and click-latency
   models trained on real human trajectories (the Balabit Mouse Challenge class
   of detector). This is what trips long sessions on score-and-sink platforms
   even when layer 1 is clean.
3. **Environment coherence.** Does the *whole environment* agree it is a real
   machine — GPU, audio, camera, display config, browser profile continuity,
   fonts, timezone, locale — or does one invariant scream VM/container/test?
4. **Attribution.** Is there a stable identity (cookies, IP, fingerprint) that
   accumulates a history, or a fresh clean room every session?

The harness's own ladder is the strongest possible answer to layer 1 for native
apps (a real compositor, a real seat, real evdev, `isTrusted=true` — there is
no headless mode and no injected device). Its weaknesses are concentrated in
**layer 1 for the browser**, which it drives over CDP with no masking, and in
**layer 2 everywhere**, because every input synthesis path is deterministic.

Severity scale: **CRITICAL** — any decent detector trips on it immediately.
**HIGH** — common anti-bot flags it, or it accumulates into a flag over a
session. **MEDIUM** — only sophisticated behavioral/fingerprint systems, but
cumulative. **LOW** — edge cases, or required only against a hard target.

---

## A. Browser automation fingerprint (CDP rungs 0/1)

### A1 — CRITICAL — `navigator.webdriver` is unmasked and truthful

The stage browser is launched with `--remote-debugging-port=0 --user-data-dir=…
--force-renderer-accessibility --ozone-platform=wayland --no-first-run
--no-default-browser-check` (`host.rs:427-448`). None of these, and nothing
else in the connect path, disables the automation features. Chromium sets
`navigator.webdriver === true` whenever it is started with a debugging port or
driven over the DevTools protocol, and `--enable-automation` (implied by
`--remote-debugging-port`) additionally removes the Chrome WebDriver
branding-should-be-kept banners and flips internal flags a page can read. The
first page any anti-bot runs is `navigator.webdriver`; this harness fails it
before a single behavioral signal is scored.

**Fix:** launch with `--disable-blink-features=AutomationControlled`,
`--exclude-switches=enable-automation` (via `--disable-features` where the
flag set allows), and inject a pre-navigation script
(`Page.addScriptToEvaluateOnNewDocument`) that deletes `navigator.webdriver`
and normalises `window.chrome`, `navigator.plugins`/`mimeTypes`, and the
`Permissions` API so the object graph matches a headful install. Do this in
`CdpPage::build` (`cdp.rs:345`), before anything can navigate.

### A2 — CRITICAL — typing on the web is `Input.insertText`, which emits no key events at all

`Step::Type` on the CDP rung calls `InsertTextParams` with the whole string
(`cdp.rs:1465-1472`). `Input.insertText` injects text with **no keydown,
keyup or beforeinput-keystroke sequence** — the page receives a single input
mutation. A human produces a keydown/keyup pair (and for unshifted ASCII, a
`keypress`) per character. Sites check for the absence of key events on input;
keystroke-dynamics classifiers read the per-key timing that `insertText`
never generates. `key_event` (`cdp.rs:2084-2139`) has the same split: a bare
printable character goes through `InsertTextParams` (`cdp.rs:2102-2117`), only
chords get real `DispatchKeyEvent` events — and even those carry no `text`
field, so the `input` that a real key sequence produces never happens.

**Fix:** type with `Input.dispatchKeyEvent`: for each character send
`keyDown` (with `key`, `text`, `code`, `windowsVirtualKeyCode`, `modifiers`),
then `keyUp`, with a jittered inter-key delay on the caller's task. Reserve
`insertText` for the one case it exists for — pasting into a
`contenteditable` that rejects synthetic key events — and even there, prefer a
real `ctrl+v` if the field honours it. The stage's own rule already exists:
`deliver_text` refuses clipboard fallback because paste is a convention
(`wayland.rs:3119-3125`); the CDP rung should hold the same standard for keys.

### A3 — HIGH — clicks teleport: one `MouseMoved` then `Pressed`/`Released` at the same point, no path, no dwell

`click_backend_node` (`cdp.rs:1827-1865`) sends a single `MouseMoved` straight
to the element centre, then `MousePressed`/`MouseReleased` at the same
coordinate with no intermediate motion, no dwell, and a `clickCount` that is
exactly correct on the first try. A page or behavioural model sees: instant
teleport to the exact centre of the target, zero micro-correction, zero
latency variance, first-try-perfect aiming. That is the signature of a
coordinate-following agent and the thing a mouse-dynamics classifier scores
most strongly.

**Fix:** derive a short path from the current pointer position to the target
with a bell-shaped velocity profile (see B1), end it 8–30 px short, then a
second small motion onto the centre (the two-segment approach humans make),
then press → jittered dwell (30–90 ms) → release. All of this is already
expressible through the existing `mouse_event` primitive; it needs a path
generator in front of it, exactly as `StageWayland`'s `run_drag` has one.

### A4 — HIGH — page scroll is `window.scrollBy` executed via `Runtime.evaluate`, producing zero wheel events

When no container is named, `Step::Scroll` runs
`window.scrollBy(dx, dy)` through `Runtime.evaluate` (`cdp.rs:1481-1497`).
That produces **no `wheel` events at all** — the document scrolls by fiat.
Real users generate `wheel` events with small deltas, and anti-bot models
count scroll events and read scroll velocity/inertia. An instant multi-
thousand-pixel jump with zero wheel events is a strong tell on any page that
watches.

**Fix:** dispatch `Input.dispatchMouseEvent` of type `mouseWheel` with small
jittered deltas over time (scrolled-deltaX/Y) — the same easing you would put
on a real wheel; keep `Runtime.evaluate` scrolling only for containers that
explicitly refuse wheel events. The per-element path (`scroll_backend_node`)
should also emit `mouseWheel` rather than a scripted scroll.

### A5 — HIGH — the debugging endpoint and connection are not hidden, and `--force-renderer-accessibility` is on for everything

The browser exposes a DevTools websocket on `127.0.0.1` (dynamic port — good)
and is told to force the accessibility tree at launch
(`host.rs:433`, `--force-renderer-accessibility`), and the surface reads it via
`Accessibility.getFullAXTree` per frame including every child frame
(`cdp.rs:1135-1151`). Three separate tells:

- A live debugging target on `127.0.0.1` is reachable by any local process and
  listed in `chrome://inspect`; its existence is correlated with automation. A
  public page cannot probe it directly (mixed-content and Private Network
  Access block loopback fetch from a web context), so this is a local-observer
  and correlation tell rather than a page-visible one.
- `--force-renderer-accessibility` flips Chromium into "assistive technology
  present" mode for **every** page, including pages that never asked for
  a11y. That changes layout, disables some optimisations, and is a
  fingerprint-able state; screen-reader modes also change behaviour that
  anti-bot correlates with.
- The AX tree is read in the main-world document context (`getFullAXTree`),
  so its presence is visible to page JS through accessibility APIs.

**Fix:** drop `--force-renderer-accessibility` from the global launch and
enable the `Accessibility` domain lazily, only for the surfaces that need it;
use `DOM.getDocument`/`DOM` domain (or `Runtime`) for observation when the
task does not need the tree; and keep DevTools bound to `127.0.0.1` (already
true) but do not leave a reachable debugging port on the user's own browser
longer than the attach needs (see E6).

### A6 — MEDIUM — `Runtime.evaluate`/`CallFunctionOn` run in the main world and can be observed

Page scroll executes `window.scrollBy(dx, dy)` via `Runtime.evaluate`
(`EvaluateParams`, `cdp.rs:1487-1494`), and the `clear` step resolves the
element and runs a JS function (`CallFunctionOnParams`, `cdp.rs:1570-1594`)
that mutates `value` and dispatches `input`/`change`. Both execute in the
page's main world. Page scripts can observe main-world evaluation side effects
(suspended breakpoint state, an interleaved `console`/stack trace), a hook on
`window.scrollBy` sees the call, and the `clear` produces a single mute
mutation — no keystrokes at all (see A2), plus the `dispatchEvent(new
Event('input', …))` is distinguishable from a real `input` event the browser
creates. The harness is not using an isolated world.

**Fix:** run any required JS in an isolated world
(`Page.createIsolatedWorld` / `Runtime.evaluate` with `contextId` set), which
is invisible to page scripts, and prefer protocol-native mechanisms
(`Input.dispatchMouseEvent` wheel, `DOM`/`Accessibility`) over injected
expressions at all.

---

## B. Input synthesis — mouse and gestures (stage rungs 2/3)

### B1 — CRITICAL — all motion is linear `lerp` at a fixed 16 ms cadence

Every gesture the stage produces — drag (`run_drag`, `wayland.rs:1263-1324`,
16 steps), stylus stroke (`run_stroke`, `wayland.rs:1204-1252`, 24 steps),
swipe and pinch (`run_gesture`, `wayland.rs:1326-1418`, `GESTURE_STEP_MS=16`)
— is a constant-velocity straight line: `lerp(start, end, step, steps)`
(`wayland.rs:1428`). Constant velocity is *the* strongest statistical
signature of synthetic motion. Human trajectories have a bell-shaped velocity
profile (accelerate, cruise, decelerate), curvature, overshoot near the
target, micro-corrections and sub-pixel jitter; a machine draws a ruler. A
mouse-dynamics classifier separates these with near-perfect AUC.

**Fix:** replace `lerp` with a path generator (see the open techniques in
`docs/computer-use-landscape.md`'s discussion): Bézier curves with randomized
control points, or — strictly better — samples from a learned human-motion
model (the open "HumanCursor"/Pyclick approach trains a GAN on real human
trajectories; a lighter option is an Ornstein–Uhlenbeck process on velocity,
which gives smooth mean-reverting noisy motion). Whatever the curve, scale
duration by target distance *and* target size (Fitts's law), and jitter the
step count per gesture so no two paths share a fingerprint.

### B2 — CRITICAL — clicks have zero dwell and zero aiming latency

`deliver_click` (`wayland.rs:2594-2620`) does `deliver_motion` then
`deliver_button(true)`/`deliver_button(false)` back-to-back in one command-loop
iteration; `deliver_button` stamps both with the same `now_ms()` millisecond
(`wayland.rs:2563, 2661`). A click is therefore "arrive and be done in the
same millisecond" — no dwell, no press-release interval, and the pointer is
already exactly on the centre when it lands. Two signals in one gesture.

**Fix:** after the final approach motion, hold the press for a jittered
30–90 ms before releasing (paced on the caller's task, like `run_drag`
already sleeps between steps, `wayland.rs:1309`), and land 2–6 px from the
detected centre with a final sub-20 px corrective motion so the effective
aiming point is human-off-centre.

### B3 — HIGH — pointer hover has no pre-click dwell and no approach

`hover` is a single `PointerStep::Motion` (`wayland.rs:1553-1555`); there is
no hover-then-pause, and clicks never hover the vicinity first. Humans pause
on the target (or near it) before pressing; the harness never does.

**Fix:** give `hover` the same approach path as B1 and leave the pointer on the
target with a jittered 60–200 ms pause; make `click` on the pixels surface
(`screen.rs:495-499` for `type`, `stage.pointer` for click) implicitly hover
the vicinity before pressing.

### B4 — MEDIUM — `MIN_CONTACT_MS` and gesture durations are fixed constants

Long presses are a constant `LONG_PRESS_MS=600` (`screen.rs:43`), swipes a
constant `SWIPE_MS=250` (`screen.rs:48`), touch taps a constant
`MIN_CONTACT_MS=40` (`wayland.rs:1170`). A real finger varies contact time by
±20% and velocity by far more. Constant gesture timings are legible to a
session-level classifier.

**Fix:** draw each constant from a jittered distribution (log-normal with the
constant as the mean, 15–25% coefficient of variation) at call time.

---

## C. Input synthesis — keyboard and typing (all rungs)

### C1 — CRITICAL — `deliver_text` bursts every keystroke in one millisecond

`deliver_text` (`wayland.rs:3085-3134`) loops over `text.chars()` and calls
`tap` for each character synchronously inside the single command-loop
invocation. Every keystroke gets the same `now_ms()` timestamp. There is no
inter-key interval at all — a `type` of 40 characters is 40 key events in the
same millisecond. Even a machine-classifier that knows nothing about the
harness separates "no human ever types like this" at a glance.

**Fix:** pace text on the caller's task, one character per iteration with a
jittered log-normal inter-key interval (mean ≈ 60–120 ms, 20–35% CV, with a
slow-to-start first key and occasional 2× hold on double letters), reusing the
same pattern `run_gesture` uses to sleep off the compositor thread
(`wayland.rs:1179-1184`). `deliver_text` must not block the compositor thread,
so the pacing has to live in the proxy, exactly as `run_stroke`/`run_drag` do.

### C2 — HIGH — the keyboard is US-layout-only, and that is a fingerprint

`keys.rs` maps characters through a hardcoded US layout
(`evdev_for_char`, `keys.rs:351-418`); any character requiring a non-US
keymap is refused (`wayland.rs:3126-3131`). Two problems: (a) a real user
typing on a German/French/etc. machine never produces US keycodes, so the
event stream disagrees with the machine; (b) the refusal surfaces to the model
as a limitation, so the harness cannot type accented text at all.

**Fix:** load the host's actual keymap and derive `evdev_for_char` from the
active layout instead of a constant table — the stage already applies the
evdev→xkb keycode offset (+8) when sending held keys (`wayland.rs:2655-2673`),
so a layout-aware table plugs into the same path.

### C3 — HIGH — key events are unaccompanied by the `text`/`input` a real sequence produces (CDP)

Even when `key_event` dispatches real `DispatchKeyEvent` events for chords
(`cdp.rs:2119-2137`), the params carry no `text` field, so the page's
`input`/`beforeinput` pipeline never sees the composed text a real keypress
generates. Combined with A2, the CDP rung produces either zero key events
(`insertText`) or key events that never produce `input`.

**Fix:** set the `text` field on `keyDown` for printable characters and keep
`DispatchKeyEvent` as the single mechanism (drop `insertText`), so the page
observes a real keypress → `input` sequence.

### C4 — MEDIUM — terminals get the whole string in one write

`PtySurface::send` writes `text.as_bytes()` in one `write_all`
(`pty.rs:346-357`, `pty.rs:454`), and `Scroll` fires PageUp/PageDown instantly
(`pty.rs:473-481`). For a raw shell this is invisible (a terminal buffers),
but for a remote terminal, an SSH `Terminal`-watched app, or a web terminal,
the byte stream is machine-flat.

**Fix:** stream `type` through the PTY master at a human cadence (same pacing
as C1) — a terminal can absorb it and nothing else changes.

---

## D. Session-level behavior and pacing

### D1 — CRITICAL — there is no human latency between observe and act

The settle machinery (`watch`, damage/networkIdle quiet) returns the instant
the screen goes quiet, and the next program starts immediately. The model's
own LLM latency is the only gap between a result and the next action — and it
is not shaped like a person's. On the page, `expect`-verified results are
acted on with zero "reading" time. Anti-bot session models measure
observation→action latency distributions and flag the near-zero tail.

**Fix:** a harness-level pacing layer: after each settled observation, wait a
randomised 250–1200 ms (right-skewed) before the next program, and add a
further 40–150 ms floor inside programs between steps. The wait belongs in the
tool call, not the model's prompt, so it cannot be optimised away.

### D2 — HIGH — every interaction is first-try-perfect and error-free

Clicks land dead-centre on the first attempt; typing is never corrected;
nothing is ever hovered then abandoned. Real users misclick, hover the wrong
item, type an extra character and backspace it, and overshoot a field. A
session with zero errors and zero corrections is itself a distributional
anomaly to a behavioural model (and is also visibly "too clean" to the model's
transcript).

**Fix:** do not aim to add deliberate errors — aim to add *realistic aiming*:
off-centre landings (B2) and a nonzero probability of a small
hover-then-adjust or an errant keystroke that is immediately corrected, tuned
per task difficulty. This is a knob, not a constant: never introduce it into
destructive steps.

### D3 — HIGH — action cadence is model-shaped, not session-shaped

Programs run back-to-back with no variation in inter-action interval across
the whole session; there is no concept of a user "working" — pauses for
reading, occasional long gaps, faster bursts. Long-session classifiers score
this cadence.

**Fix:** covered by D1 if the pacing layer models bursts (cluster of actions,
then a longer read-pause) rather than uniform gaps; make the layer stateful
across a session, not per-call.

### D4 — MEDIUM — screenshots and observations are taken at machine cadence

First-contact full renders, then damage-driven deltas (`computer-use.md:213`).
The *content* is invisible to the site, but the capture cadence (many
observations in a few seconds after each action) is a pattern a site-side
network monitor could correlate with; more importantly it entrenches D1.
Nothing about this needs fixing as a fingerprint per se, but the pacing layer
(D1) should govern when observations happen, not just when actions do.

---

## E. Browser identity and environment coherence

### E1 — HIGH — a fresh, empty profile per stage unless the user passes one

`launch` uses a fresh `chrome-profile` in the stage's runtime dir, cloned from
the user's real profile **only when** `browser_profile` is given
(`host.rs:417-421`, `clone_profile` at `host.rs:86-116`). Cloned profiles
exclude `Singleton*` and `DevToolsActivePort` (correct) but otherwise copy
everything (correct when used). The default path — an empty profile with no
cookies, no history, no local state — is the classic clean-room fingerprint,
and it guarantees every task that touches a logged-in site begins at a login
wall, which is itself a flag-raising loop.

**Fix:** make the cloned real profile the default for web work (it already
shares `$HOME` by design, `computer-use.md:95-98`; the profile is the same
argument, extended), and only fall back to a fresh profile when isolation is
explicitly requested. Identity churn across sessions is worse than any single
fingerprint.

### E2 — MEDIUM — profile continuity across sessions is not preserved

`chrome-profile` lives inside `$XDG_RUNTIME_DIR/artist-stage-<pid>-<ordinal>`
(`host.rs:283-288`) — tmpfs, wiped on reboot, process-scoped. Even with a
cloned profile, a second session starts from a different copy and any cookies
gained during the first are gone. The site sees a returning-looking visitor
become a new one.

**Fix:** persist the working profile (a `~/.cache/artist/computers/<id>/`
profile that is cloned-from-user once then reused, with the live cookies
written back), so a returning identity is actually the same identity.

### E3 — MEDIUM — no fingerprint-coherence layer

Nothing constrains UA/locale/timezone/DNT/client-hints agreement. The stage is
a single fixed `1920×1080`, scale 1, 60 Hz output (`wayland.rs:123-132`) —
every session identical — while the real user's desktop is varied. A
fingerprint across sessions shows: same canvas (real GPU — good), same
viewport invariant, same refresh, same locale from the OS. One of those is
fine; all of them fixed is a signature.

**Fix:** derive the stage display from the host's real configuration where
possible (or at least jitter the reported refresh and vary the default size per
stage), and validate UA ↔ `Accept-Language` ↔ timezone ↔ `navigator.languages`
coherence in the pre-navigation script (A1).

### E4 — LOW — the stage advertises a gamepad that does not exist

`has_gamepad: true` (`wayland.rs:1109`) while the docs state no gamepad is
delivered because a uinput device is machine-global (`computer-use.md:506-513`).
An application or page that enumerates devices sees a seat claiming a gamepad
with no events — a small but real inconsistency, and the same class of
"advertised and unserviced" bug the compositor globals were audited for
(`computer-use.md:72-83`).

**Fix:** advertise only what is real (either deliver a stage-local gamepad
someday or report `false` here); never claim a device that cannot report.

### E5 — LOW — `--no-first-run`/`--no-default-browser-check` are automation-flavoured

These flags are also what every Selenium launch uses. They are harmless alone,
but combined with A1 they are part of the launch-flag fingerprint. Suppress the
first-run UI another way if possible (or accept them, since A1's masking
changes the calculus) — noted here for completeness rather than as a blocker.

### E6 — HIGH — attaching to the user's browser leaves it with a debugging port open and a11y forced

`attach_to_endpoint` (`cdp.rs:104-141`) drives the user's already-running
browser over loopback DevTools. The value is credentials — exactly right
(`cdp.rs:84-99`). But: (a) the user's browser is left running with
`--remote-debugging-port` for the whole attach, exposed to any local process,
and (b) the surface's `getFullAXTree` calls force accessibility on the user's
personal browser. Both are footprint and privacy leaks on a browser that is
not the harness's.

**Fix:** document/require a short-lived debugging session (`--remote-debugging-
port` with `--remote-allow-origins`), close the target/endpoint when the
surface drops, and prefer the DOM domain over `getFullAXTree` for attached
browsers so the user's accessibility state is untouched.

---

## F. Form and page interaction behavior

### F1 — HIGH — no form-fill pacing or field-level behaviour

Typing a form is: focus, `ctrl+a` (via `clear`), `insertText` whole-value
(A2), `expect` settled, next field — with nothing between fields resembling a
human's tab-key timing, and submission no different from any other step.
Form-fill time is a classic anti-bot feature (too-fast forms = bots; the
harness also types the whole value in one mutation, so per-field time is
~zero).

**Fix:** pace per-field (C1), advance fields with a real `Tab` key or a click
with dwell, and add a submission step that waits a randomised 200–800 ms after
the last keystroke before Enter/click-submit. Do not gate on `autocomplete`
heuristics — but keep the pacing so a honeypot timestamp field reads like a
person took time.

### F2 — MEDIUM — no honeypot handling

The harness never checks for hidden/`display:none`/`aria-hidden` honeypot
fields before filling, and `clear`-then-type into a hidden field is exactly the
behaviour honeypots score. The AX tree already carries `Hidden` state
(`cdp.rs:1227-1236`) — the data is there and unused.

**Fix:** in the observation layer, mark hidden nodes `offscreen` (already done)
and refuse or skip `type`/`click` steps targeting them unless the model
explicitly overrides; add a rule that flags interactions with `aria-hidden`
fields.

### F3 — LOW — no scroll-read or dwell-while-reading behaviour

Nothing emulates a user pausing to read, or scrolling a page in content-sized
chunks. Low value for most targets; subsumed by D1/F1 pacing. Listed so the
space is covered.

---

## G. Desktop/native environment realism (stage rungs 2/3)

### G1 — MEDIUM — the stage has no audio device, camera, or media stack

The docs state it plainly: *"No sound, and no video as a sequence"*
(`computer-use.md:496-501`). The stage's apps see no ALSA/PulseAudio sink, no
camera. A native app (or a site inside the stage browser) enumerating devices
finds none — a hallmark of a VM/container. This is the environment-coherence
layer's most obvious gap on the desktop plane.

**Fix:** expose a real (or null but present) PulseAudio/PipeWire sink on the
stage bus — smithay-side this is out of scope, but the stage's `bus.rs` can
run a `pipewire`/`pulseaudio` with a null sink, and `host.rs:301-311` already
merges environment before launch; a null sink + `PULSE_SERVER` env is the
cheap version. (Audio *capture* for the agent is a separate, acknowledged gap;
the point here is only that the device must exist.)

### G2 — LOW — a pristine runtime dir and forced toolkit backends are legible to a local observer

`GDK_BACKEND=wayland`, `QT_QPA_PLATFORM=wayland`, `SDL_VIDEODRIVER=wayland`,
`MOZ_ENABLE_WAYLAND=1` are forced (`wayland.rs:1087-1093`) — correct for
isolation, but a local audit tool (not a remote site) can enumerate the
stage's processes/env. This is the accepted cost of the architecture; noted so
it is a decision, not an accident.

---

## H. Android / Waydroid

### H1 — HIGH — the Waydroid build fingerprint is unmodified

Android is run inside Waydroid on the stage (`host.rs:329-363`,
`android/`). Waydroid containers carry unmistakable build props —
`ro.product.device`/`brand`/`manufacturer`, `ro.build.fingerprint`,
`ro.kernel.android`, `ro.dalvik.vm` — plus the `com.waydroid` package and
`Settings.Global` markers. Any Android anti-bot that checks build props (and
several do) flags the container outright; the agent's app traffic originates
from "a Waydroid device" no matter what it does.

**Fix:** mask the props at container start (`ro.product.*`,
`ro.build.fingerprint`, `ro.kernel.android`) to plausible vendor values, and
remove the `com.waydroid` presence, only when a task will run in the container
against a fingerprinting app; gate this behind the same explicit-opt-in as
the rest of Android use.

### H2 — MEDIUM — Android input is deterministic gesture constants

The Android surface shares the stage's `LONG_PRESS_MS=600`/`SWIPE_MS=250`
constants (`screen.rs:43-48`) and its step-cadence gestures; see B4. The
container also reports one full-surface damage rect per frame
(`screen.rs:206-212`), which is an observation-coverage cost, not a
detection signal, but the gesture timing is.

**Fix:** as B4 — jittered gesture timings per interaction.

---

## I. Network and transport

### I1 — MEDIUM — nothing is done about TLS/HTTP fingerprinting or IP reputation

For local tasks on the user's home IP this is a non-issue and should stay one —
driving a site from the user's real IP with their real cookies is the strongest
possible attribution. But the harness has **no story** for the case where it is
expected to work away from that environment (a user's remote box, a relay): the
TLS/JA4 and HTTP/2 fingerprints are whatever Chromium produces (fine — a real
browser), but IP reputation and geolocation will be whatever the relay is. The
gap is not code — it is that there is no stated policy about when a stage is
"the user's machine" vs "somewhere else".

**Fix:** a policy note: a stage is only trusted for credential-bearing work on
the user's own network; document that any relayed/remote stage should use the
user's residential exit and expect the fingerprint to change.

### I2 — LOW — no detection-of-detection feedback loop

There is no way for the harness to notice it is being fingerprinted (a
`navigator.webdriver` probe, a challenge page, a suspicious burst of
`/cdn-cgi/challenge-platform` beacons) and adapt — back off, switch rung, or
hand off. This is policy, not mechanics, and it is the missing top of the whole
pyramid: all of A–H only matters if the harness knows *when* it is being
scored.

**Fix:** watch the network/cookie/settle signals already collected (the CDP
surface already tracks `inflight` and dialogs, `cdp.rs:566-634`) for challenge
pages and probe patterns; on detection, pause and surface a
`needs_human{…}`-style decision (that vocabulary is already on the steal list,
`computer-use-landscape.md:188`) rather than continuing to press.

---

## J. What is already right — do not "fix" these

- **The stage is a real compositor, real seat, real GPU, real evdev.** No
  headless mode, no injected input device, `isTrusted=true`, real monotonic
  timestamps (`wayland.rs:438`). For native apps this is the strongest
  anti-detection position in the computer-use landscape; the failures are the
  *synthesis* artifacts in B and C, not the transport.
- **Shared `$HOME` and credential reuse.** The design stance that keeps
  real logins available (`computer-use.md:95-98`) is also the correct
  attribution play (E1/E2 fix direction).
- **Anchors, labels, and no coordinates from the model.** The model never
  emits a coordinate, so there is no coordinate-based aiming to leak.
- **No global input injection; the user's seat is untouched.** Isolation and
  anti-detection coincide here.
- **Dynamic DevTools port, loopback-only attach, private a11y bus.** These are
  right and should not regress while adding A1–A5.
- **`relax` releases held keys/buttons** so a program end never leaves a stuck
  input that a classifier (or a real user) would notice.

## Priority order

1. **A1, A2, A3, A4 (browser fingerprint + CDP input realism)** — these are the
   difference between "flagged on sight" and "not flagged", on the one surface
   (the web) that has mature anti-bot.
2. **B1, B2, C1 (path, dwell, keystroke timing on the seat)** — the
   difference between "survives a session" and "flagged mid-session" on
   behavioral models.
3. **D1 (pacing layer)** — cheap, harness-wide, and it subsumes D2–D4.
4. **E1/E2 (profile identity continuity)** — makes every other fix actually
   accumulate into a trusted identity instead of starting from zero each stage.
5. **C2, B4, F1, H1** — breadth fixes once the first four are in.
6. **E4, E6, G1, I1, I2** — environment realism and the policy/feedback loop.

Everything in A–I is mechanically achievable inside the existing architecture:
the stage already proves the pacing primitive off the compositor thread
(`run_drag`), the CDP surface already has all the protocol primitives, and the
profile/identity work only extends `clone_profile`. The hardest item is B1 —
the learned path generator — and the cheapest available improvement over the
current `lerp` is a Bézier + Fitts's-law duration + OU noise, which is a
weekend, not a research project.
