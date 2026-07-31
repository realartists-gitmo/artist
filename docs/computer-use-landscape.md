# Computer use — competitive landscape

A survey of existing computer-use / GUI-agent harnesses, run 2026-07-31, to
locate `artist-computer` against them. Companion to
[the audit](computer-use-audit.md).

## Sourcing caveat — read this first

**The session's WebSearch budget was exhausted before the survey began.** All
research was done by direct HTTP: the GitHub REST API,
`raw.githubusercontent.com`, and vendor documentation, plus primary sources
downloaded during an earlier interrupted run.

The consequence is specific and worth stating: **things could be looked up by
name, but not discovered without one.** Every "NOT FOUND" below means "no repo or
doc under that name via GitHub search plus direct fetch" — not "does not exist".
READMEs, docs and protocol schemas were read; competitor *source* was not, except
where noted. Star counts are from the GitHub API on 2026-07-31.

Every project carries a confidence label: **VERIFIED** (primary sources read),
**PARTIAL** (some references, key details unconfirmed), **NOT FOUND**, or
**KNOWN FROM TRAINING, UNVERIFIED**.

## Existence verdicts for the named projects

| Name | Verdict | What it is |
|---|---|---|
| **OpenClaw** | VERIFIED | `openclaw/openclaw`, TS, 384,722★. Three distinct computer-use surfaces. |
| **ZeroClaw** | VERIFIED | `zeroclaw-labs/zeroclaw`, Rust, 32,463★. Browser-only + VNC. |
| **Iron Claw** | VERIFIED, name ambiguous | No single project. The substantial one, `nearai/ironclaw` (12,584★), **has no GUI/computer-use capability at all.** Three others share the name. |
| **Codex** | VERIFIED, impl closed | `openai/codex` (102,906★) contains the computer-use *protocol*, not the implementation. |
| **Hermes** | VERIFIED | `NousResearch/hermes-agent`, 223,356★. No native computer use — delegates. |
| **oh-my-pi** | VERIFIED | `can1357/oh-my-pi`, 20,893★. Browser tool + CDP attach. |
| **AutoGLM** | NOT FOUND at `THUDM/AutoGLM` | Successor `zai-org/Open-AutoGLM` (25,926★) exists. PARTIAL — metadata only. |
| **Project Mariner** | PARTIAL | Could not verify a single capability from a live source. Not enumerated from memory, deliberately. |

Two projects not on the original list turned out to matter more than most that
were: **`agent-sh/computer-use-linux`** (our closest direct competitor) and
**`trycua/cua`** (quietly the infrastructure under several others).

## Where we genuinely lead

Three mechanisms with **no equivalent found anywhere** in the sweep:

1. **The ladder, with rung 0 winning over every other rung.** Everyone else is
   pixels, or DOM/a11y, or both. Nobody else says "before clicking play, check
   whether MPRIS answers." OpenClaw comes closest with three browser access
   modes — but that is one rung with three doors.
2. **Compositor damage as the settle signal, with learned noise-rect
   classification and a gap detector keyed to the last sighting.** Everyone else
   has no settle model, inherits Playwright's auto-waiting, or screenshots and
   hopes. The blinking-caret analysis appears genuinely novel.
3. **Identity-only anchor resolution with a three-way error taxonomy and bounded
   tombstones.** OpenClaw's frame-token binding is the nearest neighbour and is a
   *coordinate freshness* check — which their own docs admit is not a freshness
   guarantee. `computer-use-linux` selectors and Playwright refs both resolve by
   *matching*, which is exactly the fallback we deliberately refuse.

## Two claims to retire

- **The `label` echo is not novel.** Playwright MCP ships `element` (a
  human-readable description) alongside `target` (the ref) on *every* action.
  The defensible narrower claim is the **use**: a hard pre-dispatch
  equality-or-containment check with no edit-distance fuzzing, and as the regex
  surface for a *streaming* pre-dispatch guardrail.
- **"The agent steals your mouse unless it has its own display server" is too
  strong.** `trycua/cua` drives native apps on macOS and Windows **without taking
  cursor or focus**. Our stage is the right architecture on Wayland and buys
  damage regions, socket-credential window identity, and an unpolluted a11y bus
  that background-input drivers don't get — but it is not the only way to reach
  the goal.

## The closest competitor: `agent-sh/computer-use-linux` (VERIFIED)

Rust MCP server extracted from `codex-desktop-linux`, referenced by Hermes as its
Linux desktop server. **This is our design minus the stage**: AT-SPI trees,
semantic `role`/`name`/`text` selectors explicitly "not pixel coordinates",
`perform_action`/`set_value`, compositor-aware window targeting, Wayland-first.

We do better: isolation (they drive the user's real seat via portal
`RemoteDesktop` + `ydotool`/uinput — the exact fight-over-the-keyboard problem);
referential stability (their selectors plus *indices* are ambiguous by
construction); the ladder (they have one rung); settle; observation economy;
programs.

**They do better, and this is the strongest such list in the survey:**
- **`doctor` — one JSON readiness report** with platform, portals, AT-SPI,
  windowing, input, and an explicit **blocker + recommended next step**. Our doc
  *names* the "toolkit accessibility off → empty tree looks like a compositor
  fault" trap; they productized the fix.
- **Self-repair**: `setup_accessibility` flips the GNOME toolkit key;
  `setup_window_targeting` installs a Shell extension when Introspect is locked.
- **Backend cascade with attributed failure** — GNOME extension → Introspect →
  COSMIC → KWin → hyprctl → i3 → EWMH, reporting which won *and why each declined*.
- **Screenshot payload budgeting** — hard 1920px/2 MiB caps with
  `max_width`/`max_bytes`/`scale`/`quality`, and the response carries
  `coordinate_width`/`height`/`scale` so a caller can map a downscaled preview
  back to desktop pixels.
- Works on the user's *actual* desktop, with their logins and window state.
- Seven compositors. We support exactly one — ours.
- crates.io + npm + prebuilt binaries. We are a subsystem of one harness.

## Selected findings from the rest

**OpenClaw** — frame-token binding is a well-designed coordinate guardrail
(actions must echo the screenshot's `frameId`, and a node-issued display identity
means a reconnect **fails closed instead of silently retargeting**). Also:
snapshot deltas with `[new]` markers; `extract` with a JSON schema (a bounded
side-model call returning only the answer); vision fallback for text-only models
with prompt-injection wrapping; three browser access modes with an explicit
decision rule, one of which drives the user's real signed-in session; a
**human-handoff contract** telling the agent to report login/2FA/captcha as
manual action rather than guess; durable tab ownership in SQLite; SSRF policy.
Their node computer tool is coordinates, and their docs concede the limit: *"A
token is not a freshness guarantee."*

**Anthropic reference impl** — pure coordinates, but ships **`zoom`** (view a
region at full resolution — the best answer to "the screenshot is too small to
read", and we have nothing for it), **prompt-injection classifiers on
screenshots**, and a documented production track: image sizing and pruning,
prompt caching, server-side compaction, batched tool calls, trajectory recording.
Plus measured economics: `medium` thinking is the best accuracy-to-cost ratio and
`low` uses *fewer* output tokens than no thinking because fewer mistakes mean
fewer retries. Their own guidance admits the structural weakness: *"Claude
sometimes assumes outcomes of its actions without explicitly checking"* — fixed
by **asking in the prompt**, where our `expect` is harness-enforced.

**Agent-S3** — first to surpass human performance on OSWorld: **72.60%**. The
architecture is a **separate planner and grounder** (GPT-5 planning +
UI-TARS-1.5-7B grounding), plus **Behavior Best-of-N** (+6.6 points, pure
inference-time scaling). Critically, the planner/grounder split is *compatible
with our contract* — the model still names things; the localizer is an
implementation detail below the anchor layer.

**Playwright MCP** — `find` over the accessibility snapshot returning matching
nodes with tree-path context, "cheaper than capturing the whole snapshot when you
only need to locate an element". Also `--snapshot-mode none`, `--caps vision` as
an opt-in escape hatch, `--mobile` emulation noted as *saving tokens*, and
`--output-mode file`.

**chrome-devtools-mcp** — `includeSnapshot` **defaults to false on every input
tool**. We default to a delta; they default to nothing.

**Stagehand** — auto-caching plus **self-healing**: cached actions run without
inference and "know when to involve AI whenever the website changes". Our
`distill`/`replay` stops when a label no longer resolves; theirs falls back to
the model *at that step* and patches the script.

**Skyvern** — **credential handling as a product feature** (`page.agent.login`
with its own vault, Bitwarden, 1Password). Nobody else in the survey has this.
Plus a three-tier action model: selector → natural language → "try selector, fall
back to AI".

**oh-my-pi** — CDP-attach to arbitrary **already-running, already-logged-in
Electron apps** ("point it at Slack and the agent reads your DMs"). Worth
checking whether our attach path covers a running Electron app or only ones we
launch. Also `/collab`: a relay-backed live session with a QR code, read-write
for pairing or read-only for watching, frames sealed client-side — the best
human-oversight design in the survey.

**trycua/cua** — one API across Linux container, Linux VM, macOS, Windows,
**Android**, and bring-your-own-image; Cua-Bench with trajectory export; Lume for
macOS VMs. The humbling line: *"Agents click, type, and verify without stealing
the cursor or focus."*

**Benchmarks** — OSWorld-Verified parallelizes a full eval to under an hour.
Published numbers: CUA 38.1% OSWorld / 58.1% WebArena / 87.0% WebVoyager;
Agent-S3 72.60% OSWorld. **We have no score on anything.** Note the honest
counterweight: *"An Illusion of Progress?"* (COLM 2025) argues reported web-agent
progress is overstated, and **no existing benchmark rewards rung 0** — one that
scores "pause the music" identically whether you clicked or called MPRIS will
never show our main advantage.

**Android is a category we do not touch.** Open-AutoGLM, cua sandboxes, and
Agent-S3's AndroidWorld numbers mean three independent projects treat it as table
stakes.

## The ten highest-value things to steal, by value/cost

| # | Steal | From | Why |
|---|---|---|---|
| 1 | **`computer doctor`** — one structured readiness report with per-check blocker and recommended fix | computer-use-linux | Our doc names the silent-empty-tree trap; they productized the fix. Days of work, removes the worst debugging experience in the subsystem. |
| 2 | **`find` over the observation collection** — text/regex search returning anchors with tree-path context | Playwright MCP | The collection is already complete (only *rendering* is a delta), so this is a pure read over data we hold. Highest value per line. |
| 3 | **Payload budgeting with returned scale metadata** — pixel/byte caps before anything enters context | computer-use-linux; Anthropic | Decay is a cure; a byte budget is prevention. |
| 4 | **`needs_human{login\|2fa\|captcha\|permission\|payment}`** — a defined handoff error that halts and surfaces | OpenClaw; Skyvern; Operator | This is where real tasks die, and today the agent has no vocabulary for it. One error variant plus a rule. |
| 5 | **Heal-on-mismatch in macro replay** (opt-in) | Stagehand | We have the hard, safe version; this adds a fallback branch. Turns macros from "works until the UI moves" into a durable asset. Keep the hard stop as default. |
| 6 | **`observe: none\|delta\|full`**, default delta, `none` available | chrome-devtools-mcp | Our mandatory `expect` already makes silence safe — they default to silence *without* that guarantee. One field. |
| 7 | **Mark observations as untrusted content** + tool-description guidance | Anthropic; OpenClaw | Our guardrail inspects the model's *output*. Nothing inspects the *input*. Prompt injection via a rendered page is the obvious attack. |
| 8 | **A live view of the stage** — attachable viewer or on-demand VNC of the render target | ZeroClaw; oh-my-pi | We already render offscreen and capture PNGs. Precondition for #4: a human completing 2FA must *see* the screen. |
| 9 | **A benchmark number** — OSWorld-Verified on Docker+KVM, plus a WebVoyager subset | OSWorld | We have a strong design argument and zero evidence. Costly, and the only thing that makes the position defensible. |
| 10 | **A localizer rung between 2 and 3** — self-hosted UI-TARS-1.5-7B converting a *label* to a click point | Agent-S3 + UI-TARS | Biggest capability gap: rung 3 is observation-only, so games/canvas/broken-a11y are out of reach. Does **not** violate our contract — the model still names things. Ranked 10th purely on cost. |

Honorable mentions: Anthropic's **`zoom`**; OpenClaw's **`extract` with a JSON
schema**; Playwright's **`--mobile`** (lighter pages, fewer tokens — free given
our configurable `screen`); the **thinking-effort finding**; and the **Android
gap**.
