// The Artist canvas runtime.
//
// Everything a page can do to the harness goes through here, and everything the
// harness pushes arrives here. Two responsibilities beyond the bridge itself:
// reload when the model edits a file, and ship every error the page produces
// back to `canvas status` — a canvas the model cannot debug is a canvas the
// model cannot write.

const boot = globalThis.__ARTIST__ ?? {};
// The slug identifies which canvas is talking; the key authenticates it. Only
// the SSE stream carries the key in the URL, because EventSource cannot set a
// header — everything else sends it as `x-artist-key`, which stays out of `ps`,
// out of Referer, and out of logs.
const query = `slug=${encodeURIComponent(boot.slug ?? "")}`;
const streamQuery = `k=${encodeURIComponent(boot.key ?? "")}&${query}`;

/** Call the harness. Returns the parsed body; throws only on a refusal. */
async function rpc(method, params = {}) {
  const response = await fetch(`/_artist/rpc?${query}`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-artist-key": boot.key ?? "" },
    body: JSON.stringify({ method, params }),
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(body.error ?? `${method} failed (${response.status})`);
  }
  return body;
}

/** Reporting must never throw into user code, so it is fire-and-forget. */
function report(level, message, detail) {
  try {
    rpc("canvas.report", { level, message, detail: detail ?? null }).catch(() => {});
  } catch {
    /* ignore */
  }
}

function stringify(value) {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack ? String(value.stack) : String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

// ---------------------------------------------------------------- diagnostics

addEventListener("error", (event) => {
  report("error", String(event.message ?? event.error ?? "error"), {
    source: event.filename ?? null,
    line: event.lineno ?? null,
    column: event.colno ?? null,
    stack: event.error?.stack ? String(event.error.stack) : null,
  });
});

// Resource failures never reach the handler above. An element that fails to
// load fires `error` on itself, and element events only pass the window during
// the capture phase — so a stylesheet, an image, or a declared dependency that
// does not load was invisible, while being exactly the kind of failure that
// leaves the page half-built and the reason unguessable.
//
// The canvas server reports its own missing modules, which it can name
// precisely. This covers what it cannot see: anything loaded from somewhere
// else, which since deps link at the CDN means every declared package.
addEventListener(
  "error",
  (event) => {
    const element = event.target;
    if (!element || element === globalThis || !element.tagName) return;
    const url = element.src || element.href;
    if (!url) return;
    report("error", `failed to load ${url}`, { element: element.tagName.toLowerCase() });
  },
  true,
);

addEventListener("unhandledrejection", (event) => {
  const reason = event.reason;
  report("error", `unhandled rejection: ${reason?.message ?? String(reason)}`, {
    stack: reason?.stack ? String(reason.stack) : null,
  });
});

// Mirror rather than replace: the user still gets a working console.
for (const level of ["error", "warn"]) {
  const original = console[level].bind(console);
  console[level] = (...args) => {
    original(...args);
    report(level, args.map(stringify).join(" "));
  };
}

// --------------------------------------------------------------- subscriptions

/** Minimal fan-out. Returns an unsubscribe function, as hooks expect. */
function channel() {
  const listeners = new Set();
  return {
    emit: (value) => listeners.forEach((fn) => {
      try {
        fn(value);
      } catch (error) {
        report("error", `canvas subscriber threw: ${error?.message ?? error}`);
      }
    }),
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

const stateChannel = channel();
const askChannel = channel();
const agentChannel = channel();

// Local mirror of shared state. Revision-guarded: an update older than what we
// already applied is a straggler from a slow connection, not news.
let stateRev = 0;
let stateEntries = {};

// One mirror per sibling canvas this one declared, kept apart from its own so a
// key called `rows` in two canvases stays two different things.
const siblings = new Map();

function sibling(slug) {
  if (!siblings.has(slug)) {
    siblings.set(slug, { entries: {}, rev: -1, channel: channel() });
  }
  return siblings.get(slug);
}

function applyState(payload) {
  // A state signal for a canvas this one is only watching updates that mirror
  // and nothing else — in particular it must not move this canvas's own
  // revision, or the next write here would look stale and be dropped.
  if (payload.slug && payload.slug !== boot.slug) {
    const other = sibling(payload.slug);
    if (payload.rev < other.rev) return;
    other.rev = payload.rev;
    other.entries = payload.entries
      ? payload.entries
      : { ...other.entries, ...(payload.changed ?? {}) };
    other.channel.emit(other.entries);
    return;
  }
  if (payload.rev < stateRev) return;
  stateRev = payload.rev;
  // `entries` is a full seed (the initial fetch); `changed` is a delta. Merging
  // rather than replacing is what keeps a write to one key from re-rendering
  // every consumer of every other key.
  stateEntries = payload.entries
    ? payload.entries
    : { ...stateEntries, ...(payload.changed ?? {}) };
  stateChannel.emit(stateEntries);
}

let pendingQuestions = [];

// The latest registration wins. These live outside hot-swapped application
// modules, so a replacement registration atomically becomes active without
// creating a second harness channel.
let pollHandler = null;
let sendHandler = null;
let sendChain = Promise.resolve();

function publishHandlers() {
  rpc("canvas.handlers", { poll: typeof pollHandler === "function", send: typeof sendHandler === "function" })
    .catch((error) => report("error", `handler registration failed: ${error?.message ?? error}`));
}

// ---------------------------------------------------------------- hot reload

let events;
let backoff = 250;

function connect() {
  events = new EventSource(`/_artist/events?${streamQuery}`);

  events.addEventListener("open", () => {
    backoff = 250;
    hideOverlay();
    publishHandlers();
  });

  events.addEventListener("reload", () => location.reload());
  events.addEventListener("update", (event) => applyUpdate(JSON.parse(event.data)));
  events.addEventListener("state", (event) => applyState(JSON.parse(event.data)));
  events.addEventListener("ask", (event) => {
    pendingQuestions = JSON.parse(event.data).questions ?? [];
    askChannel.emit(pendingQuestions);
  });
  events.addEventListener("agent", (event) => agentChannel.emit(JSON.parse(event.data).event));
  // The harness asks; the page answers. A digest gathered on demand describes
  // what is on screen now rather than what was there when the page loaded.
  events.addEventListener("digest", () => {
    try {
      rpc("canvas.digest", digest()).catch(() => {});
    } catch (error) {
      rpc("canvas.digest", { error: String(error?.message ?? error) }).catch(() => {});
    }
  });
  events.addEventListener("poll", (event) => {
    const request = JSON.parse(event.data);
    const sequence = request.sequence;
    Promise.resolve()
      .then(async () => {
        if (typeof pollHandler !== "function") throw new Error("no artist.onPoll handler registered");
        const value = await pollHandler();
        const encoded = JSON.stringify(value);
        if (encoded === undefined) throw new Error("artist.onPoll returned a non-JSON-serializable value");
        const serializable = JSON.parse(encoded);
        await rpc("canvas.poll.result", { sequence, hasValue: true, value: serializable });
      })
      .catch((error) => rpc("canvas.poll.result", { sequence, error: String(error?.message ?? error) }).catch(() => {}));
  });
  events.addEventListener("input", (event) => {
    const request = JSON.parse(event.data);
    // Preserve universal send ordering even if one handler invocation is async.
    sendChain = sendChain.then(async () => {
      try {
        if (typeof sendHandler !== "function") throw new Error("no artist.onSend handler registered");
        await sendHandler(request.input);
        await rpc("canvas.input.result", { sequence: request.sequence });
      } catch (error) {
        await rpc("canvas.input.result", { sequence: request.sequence, error: String(error?.message ?? error) });
      }
    });
  });
  events.addEventListener("build-error", (event) => showOverlay(JSON.parse(event.data)));

  events.addEventListener("error", () => {
    // The harness exited or is restarting. Retry with a ceiling so a closed
    // session does not leave a tab spinning forever.
    events.close();
    backoff = Math.min(backoff * 2, 5000);
    setTimeout(connect, backoff);
  });
}

connect();

// Seed from the server so a page that loads after a write is not blank.
rpc("canvas.state.get").then(applyState).catch(() => {});
rpc("canvas.ask.pending")
  .then((body) => {
    pendingQuestions = body.questions ?? [];
    askChannel.emit(pendingQuestions);
  })
  .catch(() => {});

// ----------------------------------------------------------- fast refresh

// Bump per edit so the browser fetches the new module rather than its cached
// copy. A module URL is its identity, so this is also what makes the re-import
// a genuinely new evaluation.
let generation = 0;
// Which build of the canvas this page is actually showing. Starts at whatever
// the server was serving when the shell was sent and advances only when a swap
// succeeds — so a failed update leaves it behind, which is exactly the state
// the model needs to be told about.
let revision = boot.rev ?? 0;

/**
 * Swap one edited module without reloading the page.
 *
 * React Refresh keys components by a stable id, so re-evaluating the module
 * re-registers the same families with new implementations and every mounted
 * instance is updated in place — state, scroll and focus survive.
 *
 * Falls back to a full reload whenever that cannot be guaranteed: a module the
 * page never imported, an import that throws, or a runtime that reports the
 * update was not handled.
 */
async function applyUpdate({ path, rev }) {
  const runtime = globalThis.__ARTIST_REFRESH__;
  if (!runtime || !path) {
    location.reload();
    return;
  }
  try {
    generation += 1;
    // Resolve against the page, not against this module: the runtime is served
    // from /@artist/, so a relative specifier here would look for the canvas's
    // modules alongside the runtime and 404.
    const target = new URL(path, location.href);
    target.searchParams.set("t", String(generation));
    const updated = await import(target.href);

    // Only a module whose exports are all components can be swapped in place.
    // Anything else — a module exporting a constant, a store, a helper — has
    // live references held by its importers that Refresh cannot rewrite, so
    // the honest move is to rebuild the page.
    if (!isRefreshBoundary(runtime, updated)) {
      location.reload();
      return;
    }

    // Let the newly registered families settle before asking React to swap.
    await new Promise((resolve) => setTimeout(resolve, 0));
    runtime.performReactRefresh();
    // Only now: everything above can bail to a reload or throw, and claiming
    // the build before it is on screen is the whole failure this number exists
    // to expose.
    if (typeof rev === "number") revision = rev;
    hideOverlay();
  } catch (error) {
    // A compile error arrives as a throwing module; showing it beats a reload
    // loop against a file that does not parse.
    report("error", `hot update failed for ${path}: ${error?.message ?? error}`);
    showOverlay({ path, message: String(error?.message ?? error) });
  }
}

/** Are every one of this module's exports React components? */
function isRefreshBoundary(runtime, module) {
  const names = Object.keys(module ?? {}).filter((name) => name !== "__esModule");
  if (!names.length) return false;
  return names.every((name) => {
    try {
      return runtime.isLikelyComponentType(module[name]);
    } catch {
      return false;
    }
  });
}

// -------------------------------------------------------------------- digest

/**
 * A description of what is actually on screen.
 *
 * The model writes a UI it cannot see, and every other channel reports on
 * whether the code *ran*. This reports on what it produced — so a header
 * reading 603 next to a card reading 604, which no compiler will ever catch,
 * is at least visible.
 */
function digest() {
  const root = document.getElementById("root");
  if (!root) return { error: "no #root — the entry never mounted" };

  const seen = new Map();
  const text = [];
  const overflowing = [];
  const unreadable = [];

  const parse = (value) => {
    const match = /rgba?\(([^)]+)\)/.exec(value || "");
    if (!match) return null;
    const [r, g, b, a = "1"] = match[1].split(",").map((n) => parseFloat(n));
    return a === 0 ? null : [r, g, b];
  };
  const luminance = ([r, g, b]) =>
    [r, g, b]
      .map((c) => (c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)
      .reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i], 0);

  const backdrop = (node) => {
    for (let at = node; at && at !== document.documentElement; at = at.parentElement) {
      const colour = parse(getComputedStyle(at).backgroundColor);
      if (colour) return colour;
    }
    return parse(getComputedStyle(document.body).backgroundColor) ?? [255, 255, 255];
  };

  for (const node of root.querySelectorAll("*")) {
    const tag = node.tagName.toLowerCase();
    seen.set(tag, (seen.get(tag) ?? 0) + 1);

    // Content the user can actually read, in document order.
    const own = [...node.childNodes]
      .filter((child) => child.nodeType === 3)
      .map((child) => child.textContent.trim())
      .filter(Boolean)
      .join(" ");
    if (own && text.length < 120) text.push(own);

    if (node.scrollWidth > node.clientWidth + 2 && getComputedStyle(node).overflowX === "visible") {
      overflowing.push(`${tag}${node.id ? "#" + node.id : ""}`);
    }

    if (own) {
      const fg = parse(getComputedStyle(node).color);
      if (fg) {
        const [light, dark] = [luminance(fg), luminance(backdrop(node))].sort((a, b) => b - a);
        const ratio = (light + 0.05) / (dark + 0.05);
        if (ratio < 4.5 && unreadable.length < 8) {
          unreadable.push(`"${own.slice(0, 40)}" at ${ratio.toFixed(2)}:1`);
        }
      }
    }
  }

  return {
    mounted: true,
    // Which build this describes. Without it a digest taken before an edit
    // landed looks exactly like a digest proving the edit changed nothing.
    rev: revision,
    elements: [...seen.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12)
      .map(([tag, n]) => `${tag}×${n}`),
    text,
    overflowing: overflowing.slice(0, 8),
    unreadable,
    size: { width: innerWidth, height: innerHeight, scrollHeight: document.body.scrollHeight },
  };
}

// ------------------------------------------------------------------- overlay

const OVERLAY_ID = "artist-canvas-error";

function showOverlay(detail) {
  let node = document.getElementById(OVERLAY_ID);
  if (!node) {
    node = document.createElement("div");
    node.id = OVERLAY_ID;
    node.setAttribute(
      "style",
      "position:fixed;inset:0;z-index:2147483647;background:#1a1210;color:#ffb4a2;" +
        "font:13px/1.6 ui-monospace,SFMono-Regular,Menlo,monospace;padding:24px;overflow:auto;white-space:pre-wrap",
    );
    document.body.appendChild(node);
  }
  const where = detail.line ? `${detail.path}:${detail.line}:${detail.column}` : detail.path;
  node.textContent = `${where}\n\n${detail.message}`;
}

function hideOverlay() {
  document.getElementById(OVERLAY_ID)?.remove();
}

// ---------------------------------------------------------------------- api

export const artist = {
  /**
   * Where this canvas mounts.
   *
   * Asked for rather than looked up, because `#root` is only the answer when a
   * canvas is the whole document. A project exported as one file holds several
   * canvases in one realm, and each needs its own element — a hard-coded
   * `getElementById("root")` in every entry made that impossible, which is why
   * the set export used to have to isolate canvases in separate frames.
   */
  root: document.getElementById("root"),

  /** This session's key, so nothing has to build a canvas URL by hand. */
  key: boot.key,

  /**
   * Put text into the conversation. `mode` is required:
   *   "steer" — correct the turn that is running. Refused if none is.
   *   "queue" — start a turn after the current one, or now if idle.
   * Resolves to {outcome: "steered" | "queued" | "no_turn_running"} so a button
   * can tell the user what happened rather than firing into silence.
   */
  send: (text, { mode } = {}) => {
    if (mode !== "steer" && mode !== "queue") {
      return Promise.reject(new Error('artist.send needs mode: "steer" or "queue"'));
    }
    return rpc("canvas.send", { text, mode });
  },

  /** Register the current idempotent model-facing canvas snapshot hook. */
  onPoll(handler) {
    if (typeof handler !== "function") throw new TypeError("artist.onPoll needs a function");
    pollHandler = handler;
    publishHandlers();
  },

  /** Register the current model-to-canvas input handler. Latest registration wins. */
  onSend(handler) {
    if (typeof handler !== "function") throw new TypeError("artist.onSend needs a function");
    sendHandler = handler;
    publishHandlers();
  },

  /** Invoke a tool this canvas declared in [permissions] allow. */
  call: async (tool, args = {}) => (await rpc("canvas.call", { tool, arguments: args })).output,

  state: {
    // Both of these are snapshots for useSyncExternalStore, so they must return
    // the *same reference* until the data actually changes. Returning a fresh
    // copy each call is an infinite render loop, not a defensive copy — the
    // store replaces these wholesale on every write, so the reference is
    // already safe to hand out.
    get: (key) => (key === undefined ? stateEntries : stateEntries[key]),
    all: () => stateEntries,
    /**
     * Write shared state. `notify: true` also raises a badge in the terminal,
     * so a click the agent should act on is not invisible until it next thinks
     * to look.
     */
    rev: () => stateRev,
    async set(key, value, { notify = false } = {}) {
      const entries = typeof key === "object" && key !== null ? key : { [key]: value };
      // Apply locally first so the UI does not wait a round trip; the echo
      // from the server carries the authoritative revision.
      stateEntries = { ...stateEntries, ...entries };
      stateChannel.emit(stateEntries);
      return rpc("canvas.state.set", { entries, notify });
    },
    subscribe: stateChannel.subscribe,

    /**
     * Read another canvas's shared state, live.
     *
     * Only canvases named in this one's `[permissions] canvases` — the server
     * refuses the rest, and refuses them by name rather than returning an empty
     * object, so a missing declaration reads as a missing declaration.
     *
     * Returns `{get, subscribe}` rather than a promise: the server pushes
     * changes for declared canvases down the same stream, so this is a live
     * mirror and not a fetch someone has to remember to repeat.
     */
    of(slug) {
      const other = sibling(slug);
      if (other.rev < 0) {
        other.rev = 0;
        rpc("canvas.state.get", { from: slug })
          .then((body) => {
            other.entries = body.entries ?? {};
            other.rev = body.rev ?? 0;
            other.channel.emit(other.entries);
          })
          .catch((error) => report("error", `cannot read canvas ${slug}: ${error.message}`));
      }
      return { get: () => other.entries, subscribe: other.channel.subscribe };
    },
  },

  ask: {
    // Same contract as `state` above: a stable reference, replaced on change.
    pending: () => pendingQuestions,
    answer: (questionId, selected, notes) =>
      rpc("canvas.ask.answer", {
        question_id: questionId,
        selected: Array.isArray(selected) ? selected : [selected],
        notes: notes ?? null,
      }),
    subscribe: askChannel.subscribe,
  },

  events: { subscribe: agentChannel.subscribe },

  context: () => rpc("canvas.context"),

  /**
   * Syntax-highlight code with the harness's own highlighter.
   * Resolves to `{language, lines: [[{text, color, bold, italic}]]}`.
   */
  highlight: (source, language = "txt", dark = matchMedia("(prefers-color-scheme: dark)").matches) =>
    rpc("canvas.highlight", { source, language, dark }),

  /** Render markdown with the harness's renderer, code blocks highlighted. */
  markdown: (source) => rpc("canvas.markdown", { source }).then((body) => body.html),

  /** Open a file in the user's editor, at a line if given. */
  edit: (path, line) => rpc("canvas.edit", { path, line: line ?? null }),

  /** What is on screen right now, for `canvas status`. */
  digest,

  /** Surface a message in the TUI and in `canvas status`. */
  log: (...args) => report("log", args.map(stringify).join(" ")),
};

globalThis.artist = artist;
