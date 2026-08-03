# Beyond Linux: a Stage on four other architectures

Four parallel research passes, 2026-07-31, asking one question per target: **what
is the best way to implement a Stage-like abstraction here?** Companion to
[computer use](computer-use.md) and the [steal list](computer-use-steal-list.md).

Claims carry the confidence the research assigned them: **VERIFIED** (primary
source read), **LIKELY**, **UNCERTAIN**. Where a claim was checkable against our
own code, it was checked, and that is noted.

---

## The finding that organizes everything

**On Linux, isolation and the change signal come from the same object.** We own a
compositor, so a private display and per-frame damage regions are the same
purchase. That coupling is a Linux accident, and it does not survive the port:

| | isolation | damage regions | both from one mechanism? |
|---|---|---|---|
| **Linux** | own compositor | own compositor | **yes** |
| **Android** | container on our compositor | ~~forwarded from SurfaceFlinger~~ — **measured absent**; frame differencing instead | **no** |
| **Windows** | `CreateDesktop` | — *removed by* `CreateDesktop` | **no, they oppose** |
| **macOS** | none of the three is free — see below | occlusion/AX events, or a VNC framebuffer | no |
| **iOS** | mutually exclusive with credentials | — | n/a |

Damage regions are worth the most — they drive settle detection and let us OCR
only what changed, **measured at 15×**. So the platforms sort by what happens to
that signal, and the ordering that produces is also, roughly, the effort ordering.

Two cross-cutting facts worth stating before the per-platform detail:

- **Our CDP rung is platform-independent and already works.** `connect()` reads
  `DevToolsActivePort` and opens a `ws://127.0.0.1:{port}` — no display
  dependency of any kind. For anything Chromium, every platform's hardest
  problems (capture, change signal, input) simply do not arise. A browser-only
  mode ports to all four targets today, for free.
- **Settle detection is unsolved ground in this field.** Both serious open
  Android agents ship a blind post-action sleep; one advertises a
  `wait_for_stable_ui` knob no code path reads, and its on-device window-changed
  handler is an empty block (VERIFIED). This is the second survey in a row to
  find our change-signal work has no equivalent anywhere.

---

## Android — the clear first target

**Architecture: Waydroid inside artist's own smithay compositor.**

Android-in-a-container renders through Wayland. Point its HWComposer at our stage
with `WAYLAND_DISPLAY` and `persist.waydroid.multi_windows=true`, and Android
windows arrive as ordinary `xdg_toplevel`s on a display we already own.

**Damage does not survive — MEASURED, and this claim was wrong.** The reasoning
was that Waydroid's `hwcomposer.cpp` reads `hwc_layer_1::surfaceDamage` —
SurfaceFlinger's own per-layer dirty region — and forwards every rect to
`wl_surface_damage`, so `damage.rs` would need no changes and the 15× OCR win
would carry over. Reading the source supported it. Running it does not.

Against a live container (Settings on a 1920×1080 stage, 25 s, clicks
throughout): **709 damage events, 709 of them carrying real client rectangles,
and every single rectangle covering the entire surface.** Nothing is
malfunctioning — these are genuine `wl_surface.damage` commits, not our
conservative fallback for a buffer committed without damage, which the gate
distinguishes explicitly. They simply carry no location information. Every event
also arrives with **no window attribution**, because the surface Waydroid commits
on is not the toplevel we track.

So the change-signal argument does not transfer, and this is the one property
that mattered most. Two consequences follow. The noise filter classifies by
rectangle, so on Android a blinking caret is indistinguishable from a dialog
opening and `settle: quiet` waits out its timeout on anything animating.
Incremental OCR re-reads the whole frame every look.

**The compensation is `stage/diff.rs`.** When reported damage covers
substantially the whole surface, the locality is recovered by comparing the
capture with the previous one, tile by tile. This is only affordable because
capture here is a buffer read rather than a screencast negotiation: a full-frame
comparison is a couple of milliseconds against an OCR pass costing hundreds.
Precise damage is still always believed — a client that reports it knew what it
drew before the pixels existed.

| # | Property | Verdict |
|---|---|---|
| 1 | Isolation | **1:1** — Android windows land on a display we own |
| 2 | Damage regions | **lost — MEASURED.** Real rects, all full-surface, none attributed to a window. Recovered by frame differencing (`stage/diff.rs`) |
| 3 | Window identity | **easier than expected.** `client_pid()` is the HWComposer's for every window, but Waydroid names each toplevel `waydroid.<package>` (VERIFIED: `waydroid.com.android.settings`). Strip the prefix and the package is free; `dumpsys` is only the fallback |
| 4 | Capture is a buffer read | **1:1** — one surface tree per Android task |
| 5 | Unpolluted a11y tree | different mechanism, *stronger* — the container holds only agent-launched apps |
| 6 | Input as a function call | **1:1** — Waydroid writes `input_event`s to in-container uinput; multi-touch free |
| 7 | Shares credentials | **degraded — the real cost.** Files bind-mount; **apps are not signed in** |
| 8 | Lazy | degraded — container boot is seconds, not ~0 |
| 9 | Teardown | **1:1** |

**Prerequisite now met.** This section previously said we advertised only five
globals and that `zwp_linux_dmabuf` and `wl_output` were missing. They exist:
dmabuf at version 4 with a feedback tranche naming our render node, `wl_output`
with xdg-output, plus `xdg_decoration`, `wp_viewporter` and `wp_presentation`.
Android's gralloc buffers import through it and render — confirmed by capturing
Settings off the stage as a PNG.

**Three traps that are not in any Waydroid documentation**, each found by hitting
it:

* **`PULSE_RUNTIME_PATH`.** Waydroid derives the PulseAudio socket from
  `XDG_RUNTIME_DIR`, which on a stage is a private directory holding a Wayland
  socket and nothing else. LXC is then told to bind-mount a socket that does not
  exist, the mount fails, and the *container* fails to start — with no mention of
  audio anywhere in the error, which sends you looking at binder and images.
  `android::prepare_env` points it back at the user's real one.
* **The container freezes itself.** When Android suspends, Waydroid freezes the
  LXC container: it renders nothing and answers nothing, while `waydroid status`
  still reports the *session* as `RUNNING`. It happens within seconds of boot if
  no app is active. Every input verb thaws first, because a frozen container
  accepts events and discards them — a step that reports success against a screen
  that could not have changed.
* **`waydroid prop get` blocks rather than failing.** While Android's platform
  service is still coming up it retries forever, so using `sys.boot_completed` as
  a boot predicate hangs on the very thing it is waiting for. Every CLI call is
  bounded, and a timeout is read as "still booting".

**Container control needs no root.** `waydroid container unfreeze` shells out to
`lxc-unfreeze` and fails for an ordinary user, but the same operation over
`id.waydro.ContainerManager` on the system bus succeeds, because the service does
it on our behalf. `GetSession` also returns the session's `wayland_display` and
`xdg_runtime_dir`, which is how a session belonging to a *dead stage* is told
apart from ours and restarted rather than adopted.

**Rung 2 is built and proven.** A resident `AccessibilityService`
(`crates/artist-computer/android-service/`, built by
`scripts/build-android-service.sh` — aapt2, javac, d8, apksigner, no Gradle)
pushes node trees and content-changed events over a loopback socket reached by
`adb forward`. Verified against a live container: 79 nodes off the running
Settings app with bounds in compositor screen coordinates, `performAction`
clicks landing, and a stale node id refused with
`stale node 1:5 — the tree has changed, observe it again`. Node identity is
generational, so an id from a previous tree fails loudly rather than resolving
against whatever now sits at that index.

Two things had to be solved to get there, neither of them documented anywhere:

* **adb refuses the first connection.** `device unauthorized`, answered by a
  dialog on the device — a screen the agent cannot yet drive. Because the
  container's `/data` is a host bind mount, `adb::authorize` writes the public
  key straight into `misc/adb/adb_keys`, which is what tapping "always allow"
  would have written. Appended, never replaced, so a key the user relies on is
  not revoked.
* **`sys.boot_completed` is not readiness.** With the property reading `1`,
  zygote running and 63 services registered, `cmd package` still answered
  "Can't find service: package". System services register progressively and
  `pm`, `settings` and `am` are late; `Adb::wait_for_services` waits for the
  package service itself.

**Signing in.** Apps in the container start signed out and there is no way round
it from here. The flow is: bring the stage up, open a viewer onto it
(`cargo run -p artist-computer --example watch`), and sign in by hand once. The
Android data directory persists at `~/.local/share/waydroid/data`, so it is once
per container rather than once per session. The viewer counts human actions, so
the agent is told a person intervened rather than inferring it from a screen that
changed under it.

**Rungs.** Rung 2 is the strongest rung on Android — the a11y tree is a
first-class TalkBack product surface, not a bolt-on; ~70–80% of mainstream apps
substantially driveable (UNCERTAIN). Compose apps hide `resource-id` unless they
opt into `testTagsAsResourceId`; WebViews should route to CDP; games and Flutter
fall to rung 3. Rung 0 (`am start` deep links, `content query`) navigates but
rarely completes — fast-travel, not a driver.

**Never use `uiautomator dump`:** it fails outright with `ERROR: could not get
idle state` on any animating screen (VERIFIED) — exactly the problem `damage.rs`
exists to solve. Use a resident `AccessibilityService` pushing updates outbound.

**Scope:** depends on the dmabuf/`wl_output` work. Most of the `Stage` trait is
already satisfied by the existing Wayland stage, so `AndroidStage` is a thin
wrapper over it plus three pieces: a session supervisor, a package→`app_id` map,
and a resident accessibility listener.

**Gate: answered, and the answer was the bad one.** `surfaceDamage` is
degenerate for real apps — see the measurement above. The change-signal argument
collapses, and `stage/diff.rs` is what replaces it. Re-run the measurement with
`cargo run -p artist-computer --example waydroid-gate`.

---

## Windows — second, and gated on a four-hour experiment

**Architecture: a private desktop via `CreateDesktop` on the user's own window
station**, with every app launched via `STARTUPINFOW.lpDesktop`, driven from a
dedicated desktop-bound thread. No admin, no elevation, no UIAccess — because
every window on the stage is one we launched at our own integrity level.

The structural problem: **`CreateDesktop` buys isolation by removing the
compositor.** Windows does not render GUI to a non-input desktop (LIKELY), so
Desktop Duplication — which *does* expose `GetFrameDirtyRects` (VERIFIED) — has no
output to duplicate. Capture falls back to `PrintWindow`, which is app-cooperative
rendering rather than a buffer read.

Seven of nine properties are 1:1: isolation, pid-authoritative identity
(`GetWindowThreadProcessId`, better than Wayland's socket-credential work),
unpolluted UIA tree, shared credentials, lazy, teardown. Property 2 is largely
lost; property 6 is partial (UIA patterns are true function calls, but no
drag-and-drop and no true key events on a hidden desktop).

**Change signal, ranked:** CDP for anything Chromium (desktop-independent, and it
should be *primary* here rather than a fallback) → `SetWinEventHook` scoped to our
desktop → UIA event handlers → `PrintWindow` tile-diffing. The 15× win does not
survive; partial recovery by scoping OCR to the bounding box of whichever element
raised the event.

**UIA is a strong rung 2** — excellent for WPF/WinForms/UWP, good for Win32 via
proxy providers, weak for Qt, and unreliable for Electron (use CDP).

**Rejected:** a second user session would give real composition and real dirty
rects, but client SKUs permit one interactive session, enforced in `termsrv.dll`
(VERIFIED). Windows Sandbox / Hyper-V kills property 7.

**Gate, before anything else is written:** does `PrintWindow` with the
undocumented `PW_RENDERFULLCONTENT` (0x2) return real pixels for a Chromium window
on a `CreateDesktop` desktop? One experiment, and that single result decides
first-class port vs. Win32-and-browsers-only — so nothing else should start until
it is answered.

---

## macOS — the ladder loses its bottom rung

Four research threads, and the last one overturned the other three. **The honest
macOS answer is to run our existing Linux stage in a Linux VM**, and to treat
native macOS support as a separate, narrower capability.

### The finding that decides it

**On macOS 26, positioned background mouse clicks are impossible.** Pid-routed
event delivery requires the `windowID` field, and its presence makes macOS 26.x
**discard the event location** — every pid-routed click lands at the window's
top-left. This is an empirical result from a 12-strategy matrix (Peekaboo PR #238,
July 2026), whose verdict is verbatim: *"Positioned background mouse clicks via
`SLEventPostToPid`/`postToPid` cannot work."* They deleted the code path.
**Keyboard is unaffected; mouse is not.**

This is structural, not a quality problem. `pointer()` is one of seven methods on
the `Stage` trait, and it is **rung 3's entire delivery mechanism**. The point of
the ladder is that when the accessibility tree is hollow or lying, you fall back
to pixels and click a coordinate. On macOS 26 that fallback does not exist in the
background — you are pinned to rung 2, or you steal focus.

**And rungs 2 and 3 fail in a correlated way**, which is what makes it serious:

- **Qt Quick/QML** — `QQuickItemPrivate::isAccessible` defaults to **false**.
  Plain `Text`, `Rectangle`, `Image`, `MouseArea` are invisible. QTBUG-146871
  (open): QML `ItemDelegate` has no press action, so list rows — the thing you
  most want to click — are not pressable.
- **Flutter, GTK-on-macOS, Tk 8.6, wxWidgets custom controls** — hollow or absent.
- **JavaFX** — JDK-8262292, open since 2021: *"JavaFX deadlocks if another app is
  using macOS Accessibility API in the background."* Walking the tree can hang the
  target.
- **SwiftUI** — `List` and lazy stacks expose only *visible* rows.

The applications whose AX trees are hollow are precisely the ones where you would
reach for pixels, and pixels are the rung that stopped working.

### A. In place, public API — **capture yes, positioned clicking no**

Capture holds up: `SCContentFilter(desktopIndependentWindow:)` and
`screencapture -l <windowID>` work regardless of display and even when the window
is occluded (VERIFIED). Background *keyboard* works. Background *positioned
clicking* does not, per above; the replacement is
`AXUIElementCopyElementAtPosition` + `AXPress`, which is rung 2 by another name.

Property 7 survives — real session, real keychain, real signed-in apps. Property 1
is partial: windows are visible to the user, but focus and cursor are not stolen.

**One live lead worth a spike.** Warp reportedly has a **public-APIs-only**
background-input recipe: `CGEventTapCreateForPid` on both the outgoing and target
apps to swallow the focus-change messages, then a synthetic
`appKitDefined`/`ApplicationActivated` event, then a primer click. If it holds, it
restores background input without private APIs. Unverified; spike it alongside the
capture test.

### Corrections worth recording

- **`AXManualAccessibility` is an Electron attribute, not a Chromium one** — zero
  hits in `chromium/chromium`. Setting it on Chrome or Edge does nothing; cua's
  `enable_chromium_accessibility()` is misnamed. Chromium's real trigger is merely
  *reading* `kAXRoleAttribute`, which flips `kAXModeBasic` on process-wide and
  stickily. Electron's path is **refcounted with a 2-second debounce** — setting
  `false` more often than `true` disables accessibility for *other clients on the
  machine*.
- **AX notifications rank below a SkyLight WindowServer event tap**, not above.
  AltTab abandoned them as a source of truth precisely because the tap is *"immune
  to a busy or AX-lying app."*
- **The default AX messaging timeout is 6 seconds** over synchronous Mach IPC.
  `AXUIElementSetMessagingTimeout` is mandatory, not a nicety.
- **Other Spaces are closed to AX entirely** — `kAXWindows` does not return them.
  That kills any "park the agent's windows on a spare Space" design.
- **Playwright uses zero AX on macOS** — `grep AXUIElement microsoft/playwright`
  returns nothing. It is pure CDP. A direct endorsement of our rung-1 design.
- **Appium's mac2 driver cannot drive background apps at all** — XCUITest calls
  `[app activate]` on session bring-up.

### B. `CGVirtualDisplay` — a real separate display, at a price

Apple ships **no display or graphics DriverKit family at all** — probed and
confirmed: `displaydriverkit` and `graphicsdriverkit` 404, while the seven real
families return 200 (VERIFIED). `VideoDriverKit` is virtual *cameras*, a red
herring. So a virtual-display DEXT cannot be written, and kexts on Apple silicon
need 1TR physical presence plus per-update re-approval — undistributable.

That leaves the private `CGVirtualDisplay` ObjC classes in CoreGraphics, which is
what **everyone** uses, DisplayLink included (LIKELY). Notable properties:

- **Costs nothing to reach** — no SIP change, no entitlement, no approval, and it
  notarizes fine (notarization does not scan for private API). Mac App Store is
  out, permanently.
- **It is not invisible to the user.** A first-class display: own Space, cursor
  travels onto it, appears in System Settings. **The Dock migrates onto it** and
  fighting that is a documented losing race.
- Needs **≥1 physical display present at creation** — kernel error 0x5 otherwise,
  so truly headless is fragile. A closed-lid MacBook or an HDMI dummy plug is the
  safe configuration.
- Cap of 4 virtual displays system-wide; a two-VD ScreenCaptureKit bug means **use
  exactly one** (FB17797423, open).

### C. A macOS guest in `Virtualization.framework` — strongest isolation, three risks we do not control

Headless VM captured and driven through Apple's **private `_VZVNCServer`** over
RFB, with zero contact with the host screen, cursor or focus. Proven and
open-source — Tart and lume reach it identically.

1. **No public framebuffer capture API exists** — `VZGraphicsDisplay` has no pixel
   accessor (VERIFIED by absence). The entire ecosystem ships on a private API.
2. **A two-VM cap enforced in the XNU kernel** — `hv_apple_isa_vm_quota`; a third
   guest fails outright (VERIFIED). The SLA's permitted purposes are development,
   testing, macOS Server, and *personal non-commercial use*, which arguably
   excludes a commercial product (UNCERTAIN, legal).
3. **macOS 26 guests are currently unusable** — application windows do not
   composite into the framebuffer, affecting the VNC path too, so it is the guest
   compositor (open bug FB21748086). Target macOS 15.

Plus a ~17–20 GB mandatory local IPSW, no Setup Assistant pre-seed until macOS 27
(so a one-time OCR-keystroke or offline-disk-patch workaround), and an unflagged
conflict: **save/restore forces trackpad-only input, dropping the absolute-coordinate
USB pointer.** Snapshot/resume and absolute clicking are mutually exclusive.

**Property 7 is gone** — no home directory, no keychain, distinct iCloud identity
derived from the host Secure Enclave.

### Routes that do not work

- **Fast user switching** (`CGSession -switchToUserID`) steals the display by
  construction. FUS *is* changing which session owns the console.
- **SSH → GUI** is an unambiguous no, restated by Apple DTS in 2025. There is no
  Xvfb equivalent; no GUI login means no Aqua session means no GUI.
- **A second user + Screen Sharing** is the one Apple-supported multi-session
  shape — a live off-console Aqua session while the console user keeps their
  screen (LIKELY) — but it needs a real login and a sharing client attached.
  Whether an *unattached* background session still renders is **UNCERTAIN and is
  the single most load-bearing thing to measure**, because AppKit occlusion tells
  well-behaved apps to stop drawing.

### The reframing that matters

**Linux guests remove essentially every constraint above** — headless is trivial,
no Setup Assistant, no VM cap, and you can run Wayland *inside* the guest and
capture natively. We already have that stage. So:

- *"artist on a Mac, driving Linux apps"* is nearly free.
- *"artist on a Mac, driving macOS apps"* is a separate product with a separate
  price.

## iOS — not a Stage at all

**There is no honest iOS Stage**, and the binding constraint is not tooling:
**isolation and credentials are mutually exclusive.** The Simulator gives a
private display but cannot run App Store binaries (FairPlay-encrypted,
device-platform Mach-Os), so nothing is signed in — killing the property the whole
local-execution thesis rests on. A real device keeps credentials, but the agent
*is* the user's screen. No private-display API exists (LIKELY, argued from
absence). Four of nine properties are lost on either branch — and a *different*
four, which is why one abstraction cannot cover both.

**The correction that matters: a Mac is not required for runtime.**
`pymobiledevice3` reimplements lockdown/usbmux/RemoteXPC in Python and runs on
Linux (VERIFIED), exposing HID-layer touch and gestures, screen capture, an HEVC
video stream, **accessibility element enumeration with no WebDriverAgent at all**,
XCUITest launching without `xcodebuild`, and a CDP bridge into WKWebViews. A Mac is
needed only for a one-time *signing* event; pure rung-0 needs no Mac.

**Recommendation: build an `IosBridge`, not a `Stage`, and go rung-0-first.**
iOS's programmatic surface above the GUI is unusually strong — App Intents give a
typed, discoverable action vocabulary, Shortcuts is the runtime, URL schemes are
the invocation channel. Composed: agent → deep-link → Shortcut → App Intent →
typed result. No pixels, no coordinates, no tree. That is better than most
*desktop* applications offer, and it is exactly what our ladder prizes most.

**Gate, before the rung-0 bridge:** is `pymobiledevice3`'s `accessibility
list-items`, with no WDA, rich and stable enough to serve as rung 2? If yes, the
entire XCUITest/signing/Mac branch disappears — which makes this the cheapest
question in the document and the one that removes the most work.

---

## Recommended sequencing

**0. `zwp_linux_dmabuf` and `wl_output` on the existing compositor.** An Android
prerequisite that is also unfinished Linux work — the original plan called for
both, and dmabuf is what video, WebGL and games need. Do it for Linux; get Android
unblocked as a side effect.

**1. Android.** Highest property retention of any target, the only one that keeps
the damage signal, and most of the `Stage` trait is already satisfied by code that
exists and passes tests.

**2. Windows**, after the four-hour `PrintWindow` experiment. Seven of nine
properties, no admin, and UIA is a genuinely strong rung 2.

**3. macOS, as a Linux VM.** Reuses `wayland.rs` unchanged at roughly a quarter
the cost of a native port, and Mac users get the full Linux capability — browser,
terminal, and every Linux GUI app. This is the macOS answer.

**4. iOS as a bridge**, cheap and Linux-native, gated on the one-day accessibility
experiment.

**5. Native macOS last, and scoped honestly** — not "macOS support" but
specifically *"drive a macOS-only application with the user's own credentials,
accepting that positioned clicking needs foreground on macOS 26."* A real but
narrow capability. Scoping it that way up front stops the rung-3 gap being
discovered after the port is already built around assuming it away.

**Four experiments decide the entire roadmap.** Each is a single measurement
against a real device, and each has a binary answer that removes or adds a whole
branch of work:

| Target | Question | What the answer decides |
|---|---|---|
| Android | Is `surfaceDamage` tight or degenerate for real apps? | Degenerate means the change-signal argument collapses to polling |
| Windows | Does `PrintWindow` + `PW_RENDERFULLCONTENT` capture Chromium on a hidden desktop? | First-class port vs. Win32-and-browsers-only |
| macOS | Does Warp's public-API focus-swallowing recipe restore background *positioned* clicking on 26? | Whether macOS gets a stage at all, or only a foreground driver |
| iOS | Is `accessibility list-items` alone enough for rung 2? | Whether the entire XCUITest/signing/Mac branch exists |

Run all four before writing anything else. Every one of them can invalidate a
design that would otherwise be discovered wrong deep into a build.
