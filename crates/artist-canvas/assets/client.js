// The Artist canvas runtime.
//
// Two jobs, both of which exist so the model is not writing UI blind:
//   1. reload the page when the model edits a file
//   2. ship every error the page produces back to the harness, so the failure
//      shows up in `canvas status` instead of only in a devtools console the
//      model cannot see.

const boot = globalThis.__ARTIST__ ?? {};
const endpoint = (path) => `${path}?k=${encodeURIComponent(boot.key ?? "")}&slug=${encodeURIComponent(boot.slug ?? "")}`;

/** Fire-and-forget: reporting must never itself throw into user code. */
function post(method, params) {
  try {
    return fetch(endpoint("/_artist/rpc"), {
      method: "POST",
      headers: { "content-type": "application/json", "x-artist-key": boot.key ?? "" },
      body: JSON.stringify({ method, params }),
    }).catch(() => {});
  } catch {
    return Promise.resolve();
  }
}

function report(level, message, detail) {
  post("canvas.report", { level, message, detail: detail ?? null });
}

// ---------------------------------------------------------------- diagnostics

addEventListener("error", (event) => {
  report("error", String(event.message ?? event.error ?? "error"), {
    source: event.filename ?? null,
    line: event.lineno ?? null,
    column: event.colno ?? null,
    stack: event.error && event.error.stack ? String(event.error.stack) : null,
  });
});

addEventListener("unhandledrejection", (event) => {
  const reason = event.reason;
  report("error", `unhandled rejection: ${reason && reason.message ? reason.message : String(reason)}`, {
    stack: reason && reason.stack ? String(reason.stack) : null,
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

function stringify(value) {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack ? String(value.stack) : String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

// ---------------------------------------------------------------- hot reload

let events;
let backoff = 250;

function connect() {
  events = new EventSource(endpoint("/_artist/events"));

  events.addEventListener("open", () => {
    backoff = 250;
  });

  events.addEventListener("reload", () => {
    location.reload();
  });

  // A compile error means the module the browser is about to ask for does not
  // exist yet. Surface it on the page rather than reloading into a blank frame.
  events.addEventListener("build-error", (event) => {
    showOverlay(JSON.parse(event.data));
  });

  events.addEventListener("build-ok", () => {
    hideOverlay();
  });

  events.addEventListener("error", () => {
    // The harness exited or is restarting. Retry with a ceiling so a closed
    // session does not leave a tab spinning on the CPU forever.
    events.close();
    backoff = Math.min(backoff * 2, 5000);
    setTimeout(connect, backoff);
  });
}

connect();

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

globalThis.artist = {
  /** Surface a message in the TUI and in `canvas status`. */
  log: (...args) => report("log", args.map(stringify).join(" ")),
};
