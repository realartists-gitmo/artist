// @artist/ui, for a canvas that has left the machine it was made on.
//
// The kit as it exists in an exported file. Everything is re-exported from the
// real kit unchanged; the handful of components below are redefined because
// they were affordances — ways of reaching a harness that is not there.
//
// This is a module, not a mode. An earlier version had the kit check
// `artist.static` at render time, which made every component potentially two
// components and asked model-written code to reason about which world it was
// in. The truth being expressed is "this is a different build of the kit", and
// a module boundary is exactly where that belongs: the export's import map
// points `@artist/ui` here, and nothing downstream has to know.
//
// The rule these implement is *drop affordances, keep content*. The question
// an Approve was asking, the path a FileLink named, the canvas a CanvasLink
// pointed at — all of it stays, as text. Only the acting goes.

export * from "@artist/ui-full";

import { Card } from "@artist/ui-full";

/**
 * The question and what it was asked about, without the buttons.
 *
 * A disabled Approve would be a puzzle with no answer anywhere on the page:
 * there is nothing here to explain why it cannot be pressed, and the file
 * deliberately carries no "this is a snapshot" banner. Absence reads correctly
 * on its own — this artifact shows information.
 */
export function Approve({ label = "Approve?", children }) {
  return <Card title={label}>{children}</Card>;
}

/** The path, as text. There is no editor to open on someone else's machine. */
export function FileLink({ path, line, children }) {
  return (
    <span style={{ font: "12px var(--a-mono)", color: "var(--a-muted)" }}>
      {children ?? `${path}${line ? `:${line}` : ""}`}
    </span>
  );
}

/**
 * Where the link pointed, as text.
 *
 * A single-canvas export has no sibling to reach. A whole-project export does,
 * and rewrites these into in-document navigation before this is ever reached —
 * so arriving here means the target genuinely did not travel.
 */
export function CanvasLink({ to, children }) {
  return <span style={{ color: "var(--a-muted)" }}>{children ?? to ?? "canvases"}</span>;
}

/**
 * Nothing. No agent has asked anything, and none can.
 *
 * Rendered as an empty fragment rather than omitted, so a canvas that mounts
 * the dock inside a layout does not lose the slot and reflow around it.
 */
export function AskDock() {
  return null;
}
