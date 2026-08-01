// The `artist` global, for a canvas that has been exported to a file.
//
// An exported canvas has no server, no session and no agent — it is a copy that
// travels. This stands in for `client.js` so the kit and the model's own code
// keep importing `@artist/canvas` unchanged, and implements one rule
// throughout: **drop affordances, keep content.**
//
// Anything that was only ever a way to *reach the harness* goes quiet. Anything
// that carried *content* through the harness still returns the content, just
// without the enhancement the harness was adding — unhighlighted code rather
// than no code, plain text rather than no prose. Losing the colour is a
// downgrade; losing the words would be a lie about what the canvas said.
//
// `artist.static` is true here and absent in a live canvas, so model-written
// code can branch on it the same way the kit does.

// Replaced wholesale by the exporter when a project is flattened as one
// document: several canvases then share a realm, and a global would be one
// canvas's boot data visible to all of them. A single-canvas export leaves this
// as it stands, because there it is the only canvas there is.
const boot = globalThis.__ARTIST__ ?? {};

/** Minimal version of client.js's channel, without the reporting path. */
function channel() {
  const listeners = new Set();
  return {
    emit(value) {
      listeners.forEach((fn) => {
        try {
          fn(value);
        } catch (error) {
          console.error("canvas subscriber threw", error);
        }
      });
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

// Seeded from the state the canvas held when it was exported. Writes still work
// and still notify: sorting a table or switching a tab is local interaction
// that never needed a server, and it would be a poor copy that froze.
let entries = boot.state ?? {};
let rev = boot.rev ?? 0;
const stateChannel = channel();

/** Nothing can be pending: there is no agent to have asked. */
const NO_QUESTIONS = [];

const refused = (name) => () =>
  Promise.reject(
    new Error(
      `artist.${name} needs the agent, and this is an exported copy. ` +
        `Guard it with artist.static if a canvas has to work both ways.`,
    ),
  );

const escape = (value) =>
  String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");

export const artist = {
  /**
   * Where this canvas mounts.
   *
   * A single-canvas export owns the document and uses `#root`. A whole-project
   * export puts several canvases in one realm and gives each its own element,
   * named here rather than assumed by the entry.
   */
  root:
    (boot.mount && document.getElementById(boot.mount)) ||
    document.getElementById("root"),

  /**
   * True in an exported canvas, absent in a live one.
   *
   * The kit no longer reads this — an export substitutes a different `@artist/ui`
   * rather than asking components which world they are in. It stays for
   * model-written code, which is the one place the question is honest: a button
   * you wrote that calls a tool has to decide for itself what to be here.
   */
  static: true,

  /** No session to key, so anything that needs one renders its absent case. */
  key: null,

  send: refused("send"),
  call: refused("call"),

  state: {
    get: (key) => (key === undefined ? entries : entries[key]),
    all: () => entries,
    rev: () => rev,
    async set(key, value) {
      const written = typeof key === "object" && key !== null ? key : { [key]: value };
      entries = { ...entries, ...written };
      rev += 1;
      stateChannel.emit(entries);
    },
    subscribe: stateChannel.subscribe,

    // A sibling canvas's state, if it travelled too. A single-canvas export has
    // no siblings and returns an empty mirror rather than throwing: the reading
    // canvas should render its empty case, which is a real state it already
    // knows how to show, not an error page.
    of(slug) {
      const entries = (boot.siblings ?? {})[slug] ?? {};
      return { get: () => entries, subscribe: () => () => {} };
    },
  },

  ask: {
    pending: () => NO_QUESTIONS,
    answer: refused("ask.answer"),
    subscribe: () => () => {},
  },

  events: { subscribe: () => () => {} },

  context: () =>
    Promise.resolve({ slug: boot.slug, title: boot.title, exported: boot.exported ?? null }),

  // Content, not affordance: the harness was colouring this, not producing it.
  // Returning the source as a single unstyled line keeps every character the
  // canvas meant to show.
  highlight: (source, language = "txt") =>
    Promise.resolve({
      language,
      lines: String(source)
        .split("\n")
        .map((line) => [{ text: line }]),
    }),

  // Same reasoning. A real renderer would mean vendoring one for the sake of
  // exports; the words matter more than the emphasis, so the text survives with
  // its line structure and nothing is silently dropped.
  markdown: (source) =>
    Promise.resolve(
      `<p style="white-space:pre-wrap">${escape(source)}</p>`,
    ),

  // Opening an editor is meaningless on someone else's machine; `FileLink`
  // renders as plain text rather than calling this.
  edit: () => Promise.resolve(),

  log: (...args) => console.log(...args),
};
