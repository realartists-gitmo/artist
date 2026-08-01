// @artist/ui — the kit a canvas is assembled from.
//
// Vendored so the model composes instead of re-deriving a button, a table, a
// modal, and a dark mode on every canvas. Two rules govern what earns a place
// here: either the model would otherwise rebuild it every time, or it can only
// exist because Artist is underneath.
//
// Everything is themed from CSS custom properties that follow the viewer's
// light/dark preference, so a canvas looks like it belongs to the harness
// without any canvas having to think about colour.

import { Component, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { artist } from "@artist/canvas";
import { useAsk } from "@artist/react";

// ------------------------------------------------------------------- theming
//
// Tokens and the base layer are injected by the server from
// `artist_canvas::palette`, generated from the same six colours the TUI uses.
// Nothing is defined here: two palettes would drift, and the utilities a model
// types would stop matching the components it composes.

const sp = (n) => `calc(var(--a-sp) * ${n})`;
const cx = (...parts) => parts.filter(Boolean).join(" ");

/**
 * Four spacing steps, named for what they separate.
 *
 * The kit was picking multiples ad hoc — 12 around a card, 16 around the page,
 * 8 between two things and also 12 between two other things — so no two panels
 * lined up and nothing was obviously wrong either. Naming the steps by their
 * job means the gap between sections is always larger than the gap inside one,
 * which is the only rule that makes a page read as structured.
 */
const SPACE = { inner: 2, card: 4, page: 6, section: 8 };
const space = (step) => sp(SPACE[step] ?? step);

/** The type scale, for the places JS has to state a size. */
const TEXT = {
  micro: "var(--a-text-micro)",
  body: "var(--a-text-body)",
  head: "var(--a-text-head)",
  display: "var(--a-text-display)",
};

// ------------------------------------------------------------------- guardrails

/**
 * One vocabulary for "which of these is it", across the whole kit.
 *
 * Button said `variant`, Badge said `tone`, and they overlapped on `danger` —
 * so the model had to remember which component wanted which word, and got no
 * feedback when it guessed wrong. Everything says `variant` now, `tone` keeps
 * working, and `primary` and `accent` are the same thing.
 */
function variantOf(props, fallback = "default") {
  const chosen = props.variant ?? props.tone ?? fallback;
  return chosen === "primary" ? "accent" : chosen;
}

const warned = new Set();

/**
 * Say something when a prop is dropped.
 *
 * React silently discards a prop a component does not read, so a typo produced
 * a card with no title and no error anywhere — the model reported success on a
 * broken canvas. The static check in `drift.rs` catches the literal case; this
 * catches what it cannot see, which is a spread: `<Card {...props} />`.
 *
 * The warning goes to the console, which the harness forwards, so it reaches
 * `canvas status` and therefore the model.
 */
function guard(component, rest, extra = "") {
  for (const prop of Object.keys(rest)) {
    // Passed through to the DOM by every component, so never a mistake.
    if (prop.startsWith("aria-") || prop.startsWith("data-") || prop === "id") continue;
    const key = `${component}.${prop}`;
    if (warned.has(key)) continue;
    warned.add(key);
    console.warn(`<${component} ${prop}=…> — the kit does not read this prop, so it was dropped.${extra}`);
  }
}

// -------------------------------------------------------------------- layout

/**
 * The page frame. Every canvas needs a title, somewhere for actions, and a
 * body that scrolls — writing that from scratch each time is exactly the waste
 * this kit exists to remove.
 */
export function AppShell({ title, subtitle, actions, sidebar, children }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100vh", overflow: "hidden" }}>
      <header
        style={{
          display: "flex", alignItems: "center", gap: space("card"),
          padding: `${space("inner")} ${space("page")}`,
          borderBottom: "1px solid var(--a-border)", flex: "0 0 auto",
          minHeight: 52,
        }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600, fontSize: TEXT.head, lineHeight: 1.3 }}>{title}</div>
          {subtitle && (
            <div style={{ color: "var(--a-muted)", fontSize: TEXT.micro }}>{subtitle}</div>
          )}
        </div>
        <div style={{ marginLeft: "auto", display: "flex", gap: space("inner") }}>{actions}</div>
      </header>
      <div style={{ display: "flex", flex: "1 1 auto", minHeight: 0 }}>
        {sidebar && (
          <aside
            style={{
              width: 240, flex: "0 0 auto", borderRight: "1px solid var(--a-border)",
              overflow: "auto", padding: space("card"),
            }}
          >
            {sidebar}
          </aside>
        )}
        <main
          style={{
            flex: "1 1 auto", overflow: "auto", padding: space("page"), minWidth: 0,
            // Reserve the scrollbar's space whether or not it is showing.
            // Without this, moving between a tall panel and a short one adds
            // and removes 15px of gutter, and the entire page jumps sideways
            // on every switch — which reads as a flash, not as a scrollbar.
            scrollbarGutter: "stable",
          }}
        >
          {children}
        </main>
      </div>
      <AskDock />
    </div>
  );
}

export function Toolbar({ children }) {
  return <div style={{ display: "flex", gap: sp(2), alignItems: "center" }}>{children}</div>;
}

/**
 * A row or a column with one gap between everything in it.
 *
 * `gap` takes a name from the scale — "inner", "card", "page", "section" — or a
 * raw multiple for the rare case that needs one. Names are what keep two
 * canvases, and two panels of the same canvas, agreeing on their rhythm.
 */
export function Stack({ gap = "card", horizontal = false, children, style, ...rest }) {
  guard("Stack", rest);
  return (
    <div
      style={{
        display: "flex",
        flexDirection: horizontal ? "row" : "column",
        gap: space(gap),
        ...(horizontal ? { alignItems: "center" } : {}),
        ...style,
      }}
    >
      {children}
    </div>
  );
}

/** Two panes with a draggable divider. */
export function Split({ children, initial = 50, min = 15, vertical = false }) {
  const [first, second] = Array.isArray(children) ? children : [children, null];
  const [pct, setPct] = useState(initial);
  const frame = useRef(null);

  const onDown = (event) => {
    event.preventDefault();
    const move = (e) => {
      const box = frame.current?.getBoundingClientRect();
      if (!box) return;
      const raw = vertical
        ? ((e.clientY - box.top) / box.height) * 100
        : ((e.clientX - box.left) / box.width) * 100;
      setPct(Math.min(100 - min, Math.max(min, raw)));
    };
    const up = () => {
      removeEventListener("pointermove", move);
      removeEventListener("pointerup", up);
    };
    addEventListener("pointermove", move);
    addEventListener("pointerup", up);
  };

  return (
    <div
      ref={frame}
      style={{ display: "flex", flexDirection: vertical ? "column" : "row", height: "100%", minHeight: 0 }}
    >
      <div style={{ flex: `0 0 ${pct}%`, minWidth: 0, minHeight: 0, overflow: "auto" }}>{first}</div>
      <div
        onPointerDown={onDown}
        style={{
          flex: "0 0 5px", cursor: vertical ? "row-resize" : "col-resize",
          background: "var(--a-border)",
        }}
      />
      <div style={{ flex: "1 1 auto", minWidth: 0, minHeight: 0, overflow: "auto" }}>{second}</div>
    </div>
  );
}

// -------------------------------------------------------------------- surfaces

export function Card({ title, actions, children, style, ...rest }) {
  guard("Card", rest, " Did you mean `title`?");
  return (
    <section
      style={{
        border: "1px solid var(--a-border)", borderRadius: "var(--a-radius)",
        background: "var(--a-bg)", overflow: "hidden", ...style,
      }}
    >
      {(title || actions) && (
        <div
          style={{
            display: "flex", alignItems: "center", gap: space("inner"),
            padding: `${space("inner")} ${space("card")}`,
            borderBottom: "1px solid var(--a-border)", background: "var(--a-subtle)",
          }}
        >
          <div style={{ fontWeight: 650, fontSize: TEXT.body }}>{title}</div>
          <div style={{ marginLeft: "auto" }}>{actions}</div>
        </div>
      )}
      <div style={{ padding: space("card") }}>{children}</div>
    </section>
  );
}

export function EmptyState({ title, hint, action, ...rest }) {
  guard("EmptyState", rest);
  return (
    <div style={{ textAlign: "center", padding: sp(10), color: "var(--a-muted)" }}>
      <div style={{ fontWeight: 600, color: "var(--a-fg)", marginBottom: sp(1) }}>{title}</div>
      {hint && <div style={{ fontSize: TEXT.body }}>{hint}</div>}
      {action && <div style={{ marginTop: sp(4) }}>{action}</div>}
    </div>
  );
}

/**
 * Waiting, with a reason.
 *
 * This replaced a skeleton loader, which was the wrong primitive twice over. A
 * skeleton promises the shape of what is coming, and a canvas almost never
 * knows that shape — so the grey bars settled into content that looked nothing
 * like them, which reads as the page breaking. And what a canvas waits on is
 * usually the agent, which can take a minute: grey bars for a minute look like
 * a hang, whereas "running the test suite…" looks like progress.
 */
export function Pending({ label = "Working…", children, ...rest }) {
  guard("Pending", rest);
  return (
    <div
      role="status"
      aria-busy="true"
      style={{
        display: "flex", alignItems: "center", gap: space("inner"),
        padding: space("card"), color: "var(--a-muted)", fontSize: TEXT.body,
      }}
    >
      <span className="a-spin" aria-hidden="true" />
      <span>{children ?? label}</span>
    </div>
  );
}

/** @deprecated Use `<Pending label="…">`; kept so existing canvases still run. */
export function Skeleton({ lines = 3 }) {
  return (
    <div aria-busy="true" role="status">
      {Array.from({ length: lines }, (_, index) => (
        <div
          key={index}
          style={{
            height: 12, marginBottom: space("inner"), borderRadius: 4,
            background: "var(--a-subtle)", width: `${90 - index * 12}%`,
          }}
        />
      ))}
    </div>
  );
}

// -------------------------------------------------------------------- controls

export function Button({ variant, tone, size = "md", children, style, ...rest }) {
  const palette = {
    default: { background: "var(--a-subtle)", color: "var(--a-fg)", border: "1px solid var(--a-border)" },
    accent: { background: "var(--a-accent)", color: "var(--a-accent-fg)", border: "1px solid transparent" },
    // Destructive actions are outlined, not filled. A filled red button is the
    // loudest thing on the page, so it pulls the eye and the cursor toward the
    // one action that cannot be taken back; the outline still reads as danger
    // without recruiting for it. `variant="danger-filled"` is there for the
    // rare case where destruction genuinely is the primary action.
    danger: {
      background: "transparent",
      color: "var(--a-danger)",
      border: "1px solid var(--a-danger)",
    },
    "danger-filled": {
      background: "var(--a-danger)",
      // Not a literal white: the danger colour is a dark red in light mode and
      // a light pastel in dark mode, so the text on it has to flip as well.
      color: "var(--a-accent-fg)",
      border: "1px solid transparent",
    },
    ghost: { background: "transparent", color: "var(--a-fg)", border: "1px solid transparent" },
  }[variantOf({ variant, tone })];
  const pad = size === "sm" ? `${sp(1)} ${sp(2)}` : `${sp(2)} ${sp(3)}`;

  return (
    <button
      className="a-focus"
      style={{
        ...palette, padding: pad, borderRadius: "var(--a-radius)",
        font: `500 ${size === "sm" ? "var(--a-text-micro)" : "var(--a-text-body)"} var(--a-font)`,
        cursor: rest.disabled ? "not-allowed" : "pointer",
        opacity: rest.disabled ? 0.5 : 1,
        ...style,
      }}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Input({ label, hint, style, ...rest }) {
  const id = useId();
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: sp(1) }}>
      {label && <label htmlFor={id} style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>{label}</label>}
      <input
        id={id}
        className="a-focus"
        style={{
          padding: `${sp(2)} ${sp(2)}`, borderRadius: "var(--a-radius)",
          border: "1px solid var(--a-border)", background: "var(--a-bg)",
          color: "var(--a-fg)", font: `${TEXT.body} var(--a-font)`, ...style,
        }}
        {...rest}
      />
      {hint && <div style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>{hint}</div>}
    </div>
  );
}

/** Input's taller sibling, for the fields a tool schema calls `content`. */
export function Textarea({ label, hint, style, rows = 5, ...rest }) {
  const id = useId();
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: sp(1) }}>
      {label && <label htmlFor={id} style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>{label}</label>}
      <textarea
        id={id}
        rows={rows}
        className="a-focus"
        style={{
          padding: space("inner"), borderRadius: "var(--a-radius)",
          border: "1px solid var(--a-border)", background: "var(--a-bg)",
          color: "var(--a-fg)", font: `${TEXT.body} var(--a-mono)`,
          resize: "vertical", ...style,
        }}
        {...rest}
      />
      {hint && <div style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>{hint}</div>}
    </div>
  );
}

export function Select({ label, options = [], style, ...rest }) {
  const id = useId();
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: sp(1) }}>
      {label && <label htmlFor={id} style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>{label}</label>}
      <div style={{ position: "relative", display: "grid" }}>
        <select
          id={id}
          className="a-focus"
          style={{
            padding: sp(2), borderRadius: "var(--a-radius)", border: "1px solid var(--a-border)",
            background: "var(--a-bg)", color: "var(--a-fg)", font: `${TEXT.body} var(--a-font)`,
            width: "100%", ...style,
          }}
          {...rest}
        >
        {options.map((option) => {
          const value = typeof option === "string" ? option : option.value;
          const label = typeof option === "string" ? option : (option.label ?? option.value);
          return <option key={value} value={value}>{label}</option>;
          })}
        </select>
        {/* The native arrow goes with the native appearance, so draw one that
            follows the theme. Non-interactive, so clicks reach the select. */}
        <span
          aria-hidden="true"
          style={{
            position: "absolute", right: sp(3), top: "50%", transform: "translateY(-50%)",
            pointerEvents: "none", color: "var(--a-muted)", fontSize: 10,
          }}
        >
          ▼
        </span>
      </div>
    </div>
  );
}

export function Checkbox({ label, style, ...rest }) {
  const id = useId();
  return (
    <label htmlFor={id} style={{ display: "flex", gap: sp(2), alignItems: "center", fontSize: TEXT.body, ...style }}>
      <input id={id} type="checkbox" className="a-focus" {...rest} />
      {label}
    </label>
  );
}

export function Badge({ variant, tone, children, style, ...rest }) {
  guard("Badge", rest);
  const chosen = variantOf({ variant, tone });
  const color = {
    default: "var(--a-muted)", ok: "var(--a-ok)", warn: "var(--a-warn)",
    danger: "var(--a-danger)", accent: "var(--a-accent)",
  }[chosen] ?? "var(--a-muted)";
  return (
    <span
      // A badge is the state of the thing beside it, and colour alone does not
      // carry that — to a screen reader, or to anyone who cannot separate the
      // hues, an "ok" and a "danger" pill read identically.
      role={chosen === "default" ? undefined : "status"}
      style={{
        display: "inline-block", padding: `2px ${sp(2)}`, borderRadius: 999,
        border: `1px solid ${color}`, color, fontSize: TEXT.micro, fontWeight: 500,
        whiteSpace: "nowrap", ...style,
      }}
    >
      {children}
    </span>
  );
}

export function Tabs({ tabs = [], value, onChange, children }) {
  const [internal, setInternal] = useState(tabs[0]?.id);
  const active = value ?? internal;
  const select = onChange ?? setInternal;
  return (
    <div>
      <div role="tablist" style={{ display: "flex", gap: sp(1), borderBottom: "1px solid var(--a-border)" }}>
        {tabs.map((tab) => (
          <button
            key={tab.id}
            role="tab"
            aria-selected={tab.id === active}
            onClick={() => select(tab.id)}
            className="a-focus"
            style={{
              padding: `${sp(2)} ${sp(3)}`, background: "transparent", border: "none",
              // Square: the global button radius made the focus ring trace a
              // rounded box around a tab whose indicator is a flat underline.
              borderRadius: 0,
              borderBottom: `2px solid ${tab.id === active ? "var(--a-accent)" : "transparent"}`,
              color: tab.id === active ? "var(--a-fg)" : "var(--a-muted)",
              font: `${tab.id === active ? 600 : 500} 13px var(--a-font)`,
              cursor: "pointer",
            }}
          >
            {tab.label}
          </button>
        ))}
      </div>
      <div role="tabpanel" style={{ paddingTop: sp(3) }}>
        {typeof children === "function" ? children(active) : children}
      </div>
    </div>
  );
}

export function Dialog({ open, title, onClose, children, actions }) {
  useEffect(() => {
    if (!open) return;
    const onKey = (event) => event.key === "Escape" && onClose?.();
    addEventListener("keydown", onKey);
    return () => removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div
      onClick={onClose}
      style={{
        position: "fixed", inset: 0, background: "rgba(0,0,0,0.5)",
        display: "grid", placeItems: "center", zIndex: 1000, padding: sp(4),
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        onClick={(event) => event.stopPropagation()}
        style={{
          background: "var(--a-bg)", border: "1px solid var(--a-border)",
          borderRadius: "var(--a-radius)", maxWidth: 560, width: "100%",
          maxHeight: "85vh", overflow: "auto",
        }}
      >
        <div style={{ padding: sp(4) }}>
          {title && <h2 style={{ margin: `0 0 ${space("inner")}`, fontSize: TEXT.head }}>{title}</h2>}
          {children}
          {actions && (
            <div style={{ display: "flex", gap: sp(2), justifyContent: "flex-end", marginTop: sp(4) }}>
              {actions}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** Transient messages. `toast(...)` is callable from anywhere. */
const toastChannel = { listeners: new Set() };
export function toast(message, tone = "default") {
  toastChannel.listeners.forEach((fn) => fn({ id: Math.random(), message, tone }));
}

export function Toaster() {
  const [items, setItems] = useState([]);
  useEffect(() => {
    const add = (item) => {
      setItems((prev) => [...prev, item]);
      setTimeout(() => setItems((prev) => prev.filter((i) => i.id !== item.id)), 4000);
    };
    toastChannel.listeners.add(add);
    return () => toastChannel.listeners.delete(add);
  }, []);

  return (
    <div style={{ position: "fixed", bottom: sp(4), right: sp(4), display: "grid", gap: sp(2), zIndex: 1100 }}>
      {items.map((item) => (
        <div
          key={item.id}
          style={{
            background: "var(--a-subtle)", border: "1px solid var(--a-border)",
            borderLeft: `3px solid ${item.tone === "danger" ? "var(--a-danger)" : "var(--a-accent)"}`,
            borderRadius: "var(--a-radius)", padding: `${space("inner")} ${space("card")}`, fontSize: TEXT.body,
            maxWidth: 380,
          }}
        >
          {item.message}
        </div>
      ))}
    </div>
  );
}

// ------------------------------------------------------------------ resilience

/**
 * Catches a render failure and reports it, so a broken component shows up in
 * `canvas status` instead of leaving a blank page and no explanation.
 */
export class ErrorBoundary extends Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error) {
    return { error };
  }

  componentDidCatch(error, info) {
    console.error(`canvas render failed: ${error?.message ?? error}`, info?.componentStack ?? "");
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div
        style={{
          margin: sp(4), padding: sp(4), border: "1px solid var(--a-danger)",
          borderRadius: "var(--a-radius)", color: "var(--a-danger)",
          font: "13px/1.6 var(--a-mono)", whiteSpace: "pre-wrap",
        }}
      >
        {String(this.state.error?.stack ?? this.state.error)}
      </div>
    );
  }
}

// ----------------------------------------------------------------------- ask

/**
 * Questions the agent is waiting on, rendered wherever the canvas is.
 *
 * Mounted by AppShell, so any canvas built on the shell can answer a question
 * without doing anything. The same question is showing in the TUI; whoever
 * answers first wins.
 */
export function AskDock() {
  const { questions, answer } = useAsk();
  const [choice, setChoice] = useState({});
  const [notes, setNotes] = useState({});

  if (!questions.length) return null;
  const question = questions[0];
  const picked = choice[question.id] ?? [];

  const toggle = (label) =>
    setChoice((prev) => {
      const current = prev[question.id] ?? [];
      if (!question.multi_select) return { ...prev, [question.id]: [label] };
      return {
        ...prev,
        [question.id]: current.includes(label)
          ? current.filter((l) => l !== label)
          : [...current, label],
      };
    });

  return (
    <div
      style={{
        borderTop: "2px solid var(--a-accent)", background: "var(--a-subtle)",
        padding: sp(4), flex: "0 0 auto", maxHeight: "50vh", overflow: "auto",
      }}
    >
      <Stack gap={3}>
        <Stack gap={2} horizontal>
          {question.header && <Badge tone="accent">{question.header}</Badge>}
          <strong style={{ fontSize: TEXT.body }}>{question.question}</strong>
        </Stack>
        <div style={{ display: "grid", gap: sp(2) }}>
          {question.options.map((option) => (
            <button
              key={option.label}
              onClick={() => toggle(option.label)}
              className="a-focus"
              style={{
                textAlign: "left", padding: sp(3), borderRadius: "var(--a-radius)",
                cursor: "pointer", background: "var(--a-bg)",
                border: `1px solid ${picked.includes(option.label) ? "var(--a-accent)" : "var(--a-border)"}`,
                boxShadow: picked.includes(option.label) ? "0 0 0 1px var(--a-accent)" : "none",
                color: "var(--a-fg)",
              }}
            >
              <div style={{ fontWeight: 600, fontSize: TEXT.body }}>{option.label}</div>
              {option.description && (
                <div style={{ fontSize: TEXT.micro, color: "var(--a-muted)", marginTop: 2 }}>
                  {option.description}
                </div>
              )}
              {option.preview && (
                <pre
                  style={{
                    font: "11px/1.5 var(--a-mono)", background: "var(--a-subtle)",
                    padding: sp(2), borderRadius: 4, marginTop: sp(2), overflow: "auto",
                  }}
                >
                  {option.preview}
                </pre>
              )}
            </button>
          ))}
        </div>
        <Input
          placeholder="Anything to add?"
          value={notes[question.id] ?? ""}
          onChange={(event) => setNotes((prev) => ({ ...prev, [question.id]: event.target.value }))}
        />
        <Stack gap={2} horizontal>
          <Button
            variant="primary"
            disabled={!picked.length && !(notes[question.id] ?? "").trim()}
            onClick={() => answer(question.id, picked, notes[question.id])}
          >
            Answer
          </Button>
          <Button variant="ghost" onClick={() => answer(question.id, [], null)}>
            Skip
          </Button>
        </Stack>
      </Stack>
    </div>
  );
}

/**
 * A button that does something in the harness.
 *
 * Use this rather than an onClick calling `artist.call` or `artist.send`
 * yourself. Not style — it is the only way to write a canvas that is good both
 * live and exported. An export substitutes this whole module for one where
 * `Action` renders its label without the button, so a canvas built from it
 * degrades correctly with nothing in it checking which world it is in. A raw
 * `artist.call` in your own handler cannot be substituted, so it becomes a
 * button that rejects, and you are left branching on `artist.static`.
 *
 * Give it either `tool` (with `args`) or `send` (with `mode`).
 */
export function Action({ tool, args, send, mode = "queue", children, ...rest }) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      if (tool) await artist.call(tool, args ?? {});
      else if (send) await artist.send(send, { mode });
    } catch (error) {
      toast(String(error?.message ?? error), { variant: "danger" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button onClick={run} disabled={busy} {...rest}>
      {busy ? <Pending /> : children}
    </Button>
  );
}

/** Approve or reject one thing, answered straight into the agent's turn. */
export function Approve({ questionId, label = "Approve?", children }) {
  const { answer } = useAsk();
  return (
    <Card title={label}>
      {children}
      <Stack gap={2} horizontal style={{ marginTop: sp(3) }}>
        <Button variant="primary" onClick={() => answer(questionId, ["approve"])}>Approve</Button>
        <Button variant="danger" onClick={() => answer(questionId, ["reject"])}>Reject</Button>
      </Stack>
    </Card>
  );
}

export { useAsk };

// ------------------------------------------------------------------ data

/** Above this many rows a filter box earns its space; below it, it is clutter. */
const FILTER_THRESHOLD = 12;

/** Above this many rows the table windows instead of mounting every one. */
const VIRTUAL_THRESHOLD = 200;

/** Row height in pixels, by density — the windowing needs a known one. */
const ROW_HEIGHT = { dense: 26, normal: 34 };

/**
 * A sortable, filterable table.
 *
 * `columns` accepts plain strings for the common case — a canvas showing rows
 * of an object should not have to write column definitions.
 *
 * Long tables window: past a few hundred rows only the visible slice is
 * mounted. A canvas listing every test in a suite or every symbol in a crate is
 * a completely ordinary thing for this harness to be asked for, and mounting
 * ten thousand `<tr>`s is how that becomes a frozen tab.
 */
export function DataTable({
  rows = [], columns, onRowClick, empty = "Nothing to show", dense,
  height, filterable, ...rest
}) {
  guard("DataTable", rest);
  const [sort, setSort] = useState(null);
  const [filter, setFilter] = useState("");
  const viewport = useRef(null);
  const [scroll, setScroll] = useState(0);

  const resolved = useMemo(() => {
    if (columns?.length) {
      return columns.map((c) => (typeof c === "string" ? { key: c, label: c } : c));
    }
    const keys = new Set();
    rows.forEach((row) => Object.keys(row ?? {}).forEach((k) => keys.add(k)));
    return [...keys].map((key) => ({ key, label: key }));
  }, [columns, rows]);

  /**
   * Which columns hold numbers, decided from the data rather than declared.
   *
   * Numbers belong right-aligned with tabular figures: that is what makes a
   * column of counts scannable, and it is the detail a hand-rolled table never
   * has. Sampling beats asking, because the model writing the canvas will not
   * think to say so.
   */
  const numeric = useMemo(() => {
    const sample = rows.slice(0, 32);
    return new Set(
      resolved
        .filter((column) => {
          if (column.align) return column.align === "right";
          const values = sample.map((row) => row?.[column.key]).filter((v) => v != null);
          return values.length > 0 && values.every((v) => typeof v === "number");
        })
        .map((column) => column.key),
    );
  }, [resolved, rows]);

  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    let out = needle
      ? rows.filter((row) =>
          resolved.some((c) => String(row?.[c.key] ?? "").toLowerCase().includes(needle)),
        )
      : rows.slice();
    if (sort) {
      const { key, dir } = sort;
      out.sort((a, b) => {
        const x = a?.[key], y = b?.[key];
        // Numbers must not sort as strings — "10" before "9" is the single
        // most common wrong-looking table.
        const cmp = typeof x === "number" && typeof y === "number"
          ? x - y
          : String(x ?? "").localeCompare(String(y ?? ""), undefined, { numeric: true });
        return dir === "asc" ? cmp : -cmp;
      });
    }
    return out;
  }, [rows, resolved, filter, sort]);

  const pad = dense ? `${sp(1)} ${sp(2)}` : `${sp(2)} ${sp(3)}`;
  const rowHeight = dense ? ROW_HEIGHT.dense : ROW_HEIGHT.normal;

  // A filter box over eight rows is noise: the user can already see all eight,
  // and the control costs a line of vertical space to say so.
  const showFilter = filterable ?? rows.length > FILTER_THRESHOLD;

  // Windowing needs a fixed viewport to measure against, so a table long
  // enough to need it gets one whether or not the caller asked.
  const virtual = shown.length > VIRTUAL_THRESHOLD;
  const viewportHeight = height ?? (virtual ? 420 : undefined);

  // A generous overscan: scrolling a windowed table renders on the scroll
  // event, and rows arriving a frame late read as tearing.
  const overscan = 12;
  const first = virtual ? Math.max(0, Math.floor(scroll / rowHeight) - overscan) : 0;
  const visibleCount = virtual
    ? Math.ceil((viewportHeight ?? 420) / rowHeight) + overscan * 2
    : shown.length;
  const slice = virtual ? shown.slice(first, first + visibleCount) : shown;

  return (
    <Stack gap={2}>
      {showFilter && (
        <Input
          type="search"
          aria-label="Filter rows"
          placeholder={`Filter ${rows.length} row${rows.length === 1 ? "" : "s"}…`}
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
      )}
      <div
        ref={viewport}
        onScroll={virtual ? (event) => setScroll(event.currentTarget.scrollTop) : undefined}
        style={{
          overflow: "auto", border: "1px solid var(--a-border)",
          borderRadius: "var(--a-radius)", maxHeight: viewportHeight,
        }}
      >
        <table style={{ borderCollapse: "collapse", width: "100%", fontSize: dense ? 12 : 13 }}>
          <thead>
            <tr>
              {resolved.map((column) => {
                const active = sort?.key === column.key;
                return (
                  <th
                    key={column.key}
                    // A sortable header is a control, and screen readers are
                    // told which way it currently sorts rather than being left
                    // to infer it from an arrow glyph.
                    aria-sort={active ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
                    style={{
                      padding: 0, borderBottom: "1px solid var(--a-border)",
                      background: "var(--a-subtle)", position: "sticky", top: 0,
                      textAlign: numeric.has(column.key) ? "right" : "left",
                      whiteSpace: "nowrap", zIndex: 1,
                    }}
                  >
                    <button
                      type="button"
                      className="a-focus"
                      onClick={() =>
                        setSort((prev) =>
                          prev?.key === column.key && prev.dir === "asc"
                            ? { key: column.key, dir: "desc" }
                            : { key: column.key, dir: "asc" },
                        )
                      }
                      style={{
                        width: "100%", padding: pad, background: "none", border: "none",
                        font: "inherit", fontWeight: 600, cursor: "pointer",
                        textAlign: "inherit", borderRadius: 0,
                        color: active ? "var(--a-fg)" : "var(--a-muted)",
                      }}
                    >
                      {column.label}
                      {active ? (sort.dir === "asc" ? " ↑" : " ↓") : ""}
                    </button>
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {/* The rows above the window, as one spacer, so the scrollbar
                reflects the whole table rather than the mounted slice. */}
            {first > 0 && <tr style={{ height: first * rowHeight }} aria-hidden="true" />}
            {slice.map((row, index) => (
              <tr
                key={row?.id ?? first + index}
                onClick={onRowClick ? () => onRowClick(row) : undefined}
                // A clickable row is a control. Without this it is reachable
                // only by mouse, which quietly excludes anyone not using one.
                tabIndex={onRowClick ? 0 : undefined}
                role={onRowClick ? "button" : undefined}
                onKeyDown={
                  onRowClick
                    ? (event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          onRowClick(row);
                        }
                      }
                    : undefined
                }
                style={{ cursor: onRowClick ? "pointer" : "default", height: rowHeight }}
              >
                {resolved.map((column) => (
                  <td
                    key={column.key}
                    style={{
                      padding: pad, borderBottom: "1px solid var(--a-border)",
                      textAlign: numeric.has(column.key) ? "right" : "left",
                      fontVariantNumeric: numeric.has(column.key) ? "tabular-nums" : undefined,
                    }}
                  >
                    {column.render ? column.render(row?.[column.key], row) : String(row?.[column.key] ?? "")}
                  </td>
                ))}
              </tr>
            ))}
            {first + slice.length < shown.length && (
              <tr
                style={{ height: (shown.length - first - slice.length) * rowHeight }}
                aria-hidden="true"
              />
            )}
          </tbody>
        </table>
        {!shown.length && (
          <EmptyState title={filter ? `Nothing matches “${filter}”` : empty} />
        )}
      </div>
    </Stack>
  );
}

/**
 * A chart.
 *
 * uPlot is fast and tiny but its API is imperative, which is not how the model
 * writes React. This wrapper is the whole point: pass data, get a chart.
 */
/** Series colours, taken from the palette so a chart matches everything else. */
function token(name, fallback) {
  // Read the tokens the server injects, not Tailwind's `--color-*`: those are
  // produced by the browser JIT for use inside utilities and are not exposed
  // on :root, so asking for them silently yields "" and every line goes grey.
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

function stroke(index) {
  return token(`--a-chart-${(index % 8) + 1}`, "#888");
}

/** A stroke at low opacity, for the fill under an area series. */
function translucent(colour) {
  const hex = colour.trim();
  return /^#[0-9a-f]{6}$/i.test(hex) ? `${hex}26` : "transparent";
}

/**
 * A chart.
 *
 * `kind` picks the shape: `"line"`, `"area"`, or `"bars"`. Anything uPlot
 * accepts still passes straight through, so the wrapper is a starting point
 * rather than a ceiling.
 */
export function Plot({ data, series, height = 240, title, scales, kind = "line", ...rest }) {
  const host = useRef(null);
  const chart = useRef(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    if (!host.current) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(host.current);
    return () => observer.disconnect();
  }, []);

  // `series`, `scales` and the passthrough options are almost always written
  // inline, so they are a fresh object on every render. Depending on them by
  // identity tore the chart down and rebuilt it on every parent render, which
  // on a live dashboard means a chart that never finishes appearing. Their
  // *value* is what matters here, so that is what the effect depends on.
  const shape = JSON.stringify([series, scales, rest, kind]);

  useEffect(() => {
    let cancelled = false;
    import("uplot").then(({ default: uPlot }) => {
      if (cancelled || !host.current || !width) return;
      chart.current?.destroy();
      // uPlot's first series is the x axis, and so is the first entry the
      // caller passes — prepending another one silently offsets everything and
      // makes uPlot read a series that has no data behind it.
      const declared = series ?? [
        {},
        ...data.slice(1).map((_, index) => ({ label: `series ${index + 1}` })),
      ];
      // Bars need one path builder shared by every series, or uPlot lays each
      // one out as though it were alone and they overprint.
      const count = Math.max(1, data.length - 1);
      const bars = kind === "bars"
        ? uPlot.paths.bars({ size: [0.62 / count, 40], align: 0 })
        : undefined;

      // Trim rather than crash: a mismatch is a mistake in the canvas, and an
      // undersized chart reads better than a blank error boundary.
      const resolved = declared.slice(0, data.length).map((entry, index) => {
        if (index === 0) return entry;
        const colour = stroke(index - 1);
        return {
          stroke: colour,
          width: 2,
          ...(kind === "bars" ? { paths: bars, fill: colour, width: 0 } : {}),
          // A single area series reads as a magnitude; several stacked
          // translucent fills read as mud, so only the first one is filled.
          ...(kind === "area" && index === 1 ? { fill: translucent(colour) } : {}),
          ...entry,
        };
      });

      // uPlot draws its axes in black by default, which on the dark ground a
      // canvas defaults to is an invisible chart with a visible line in it.
      const ink = token("--a-muted", "#888");
      const grid = { stroke: token("--a-border", "#8884"), width: 1 };
      const axis = {
        stroke: ink,
        grid,
        ticks: { ...grid, size: 4 },
        // uPlot draws to a canvas, so this has to be a resolved font string
        // rather than a var() reference the CSS engine would expand.
        font: `${token("--a-text-micro", "11px")} ${token("--a-font", "sans-serif")}`,
      };

      chart.current = new uPlot(
        {
          width, height, title, series: resolved,
          axes: [{ ...axis }, { ...axis }],
          legend: { show: data.length > 2 },
          ...rest,
          // uPlot reads x as a unix timestamp unless told otherwise, so a plain
          // index axis renders as dates in 1969. Merged after `rest` so a
          // caller that does want a time axis can still say so.
          scales: { x: { time: false }, ...scales },
        },
        data,
        host.current,
      );
    });
    return () => {
      cancelled = true;
      chart.current?.destroy();
      chart.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `shape` stands in
    // for series/scales/rest/kind by value; see above.
  }, [data, width, height, title, shape]);

  return <div ref={host} style={{ width: "100%", minHeight: height }} />;
}

// ------------------------------------------------------- harness-backed code

/**
 * Syntax-highlighted code, coloured by the same Rust that colours the
 * transcript. No grammar files reach the browser.
 */
// Highlighting survives unmount. Switching tabs used to re-request it and
// re-render unstyled text for a frame, which showed as a flash on every swap.
const highlighted = new Map();

export function Code({ children, language = "txt", showLines, wrap, style }) {
  const source = typeof children === "string" ? children : String(children ?? "");
  const dark = typeof matchMedia === "function"
    ? matchMedia("(prefers-color-scheme: dark)").matches
    : true;
  const key = `${language}\u0000${dark}\u0000${source}`;

  // Seeded from the cache, so a block that has been rendered before comes back
  // already styled rather than passing through a plain-text frame.
  const [lines, setLines] = useState(() => highlighted.get(key) ?? null);

  useEffect(() => {
    const cached = highlighted.get(key);
    if (cached) {
      setLines(cached);
      return;
    }
    let live = true;
    artist
      .highlight(source, language, dark)
      .then((result) => {
        highlighted.set(key, result.lines);
        if (live) setLines(result.lines);
      })
      // Falling back to unstyled code is right: the content is what matters,
      // and a highlighting failure must not blank the block.
      .catch(() => live && setLines(null));
    return () => {
      live = false;
    };
  }, [key, source, language, dark]);

  const width = String(source.split("\n").length).length;

  return (
    <pre
      style={{
        margin: 0, padding: sp(3), overflow: "auto", borderRadius: "var(--a-radius)",
        border: "1px solid var(--a-border)", background: "var(--a-subtle)",
        font: "12px/1.6 var(--a-mono)", whiteSpace: wrap ? "pre-wrap" : "pre", ...style,
      }}
    >
      {(lines ?? source.split("\n").map((text) => [{ text, color: "inherit" }])).map(
        (spans, index) => (
          <div key={index}>
            {/* Rendered in both branches: a gutter that appears only once
                highlighting lands would shift every line sideways. */}
            {showLines && (
              <span style={{ color: "var(--a-muted)", userSelect: "none", marginRight: sp(3) }}>
                {String(index + 1).padStart(width, " ")}
              </span>
            )}
            {spans.map((span, i) => (
              <span
                key={i}
                style={{
                  color: span.color,
                  fontWeight: span.bold ? 600 : undefined,
                  fontStyle: span.italic ? "italic" : undefined,
                }}
              >
                {span.text}
              </span>
            ))}
          </div>
        ),
      )}
    </pre>
  );
}

/**
 * A unified diff, rendered.
 *
 * Takes the patch text the model already has from an `edit` result rather than
 * computing one, which is both cheaper and what actually gets shown.
 */
export function Diff({ patch = "", language = "txt" }) {
  const lines = useMemo(() => patch.split("\n"), [patch]);
  const tone = (line) => {
    if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("@@")) return "meta";
    if (line.startsWith("+")) return "add";
    if (line.startsWith("-")) return "del";
    return "ctx";
  };
  const background = {
    add: "color-mix(in srgb, var(--a-ok) 14%, transparent)",
    del: "color-mix(in srgb, var(--a-danger) 14%, transparent)",
    meta: "var(--a-subtle)",
    ctx: "transparent",
  };
  const color = { add: "var(--a-ok)", del: "var(--a-danger)", meta: "var(--a-muted)", ctx: "var(--a-fg)" };

  return (
    <div
      style={{
        border: "1px solid var(--a-border)", borderRadius: "var(--a-radius)",
        overflow: "auto", font: "12px/1.6 var(--a-mono)",
      }}
    >
      {lines.map((line, index) => {
        const kind = tone(line);
        return (
          <div
            key={index}
            style={{
              background: background[kind], color: color[kind],
              padding: `0 ${sp(3)}`, whiteSpace: "pre",
            }}
          >
            {line || " "}
          </div>
        );
      })}
    </div>
  );
}

/**
 * A form derived from a JSON Schema.
 *
 * Every Artist tool publishes one, so this puts a real interface in front of
 * any tool — including MCP and extension tools — for free.
 */
/** `output_dir` and `outputDir` both become "Output dir". */
function humanise(key) {
  const words = key
    .replace(/[_-]+/g, " ")
    .replace(/([a-z\d])([A-Z])/g, "$1 $2")
    .trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function SchemaForm({
  schema = {}, value = {}, onChange, onSubmit, submitLabel = "Run", ...rest
}) {
  guard("SchemaForm", rest);
  const [draft, setDraft] = useState(value);
  const properties = schema.properties ?? {};
  const required = new Set(schema.required ?? []);

  const update = (key, next) => {
    const merged = { ...draft, [key]: next };
    setDraft(merged);
    onChange?.(merged);
  };

  const empty = (key) => {
    const held = draft[key];
    return held === undefined || held === "" || held === null;
  };
  const missing = [...required].filter(empty);

  // Required first. A tool schema lists properties in whatever order it was
  // declared, which puts the two fields you must fill below eleven you need
  // not — and the form reads as far longer than the job actually is.
  const ordered = Object.entries(properties).sort(
    ([a], [b]) => Number(required.has(b)) - Number(required.has(a)),
  );
  const optionalStarts = ordered.findIndex(([key]) => !required.has(key));
  const worthSplitting = optionalStarts > 0 && optionalStarts < ordered.length;

  const field = ([key, spec], index) => {
    const label = spec.title ?? humanise(key);
    const marked = required.has(key) ? `${label} *` : label;
    const divider = worthSplitting && index === optionalStarts && (
      <div
        key="optional"
        style={{
          fontSize: TEXT.micro, color: "var(--a-muted)", textTransform: "uppercase",
          letterSpacing: ".06em", marginTop: space("inner"),
        }}
      >
        Optional
      </div>
    );

    let control;
    if (spec.enum) {
      control = (
        <Select
          key={key}
          label={marked}
          options={["", ...spec.enum]}
          value={draft[key] ?? ""}
          onChange={(event) => update(key, event.target.value)}
        />
      );
    } else if (spec.type === "boolean") {
      // The description belongs under the control, not glued to the label with
      // an em dash — a checkbox whose label is a paragraph has no hit target.
      control = (
        <div key={key} style={{ display: "flex", flexDirection: "column", gap: sp(1) }}>
          <Checkbox
            label={label}
            checked={Boolean(draft[key])}
            onChange={(event) => update(key, event.target.checked)}
          />
          {spec.description && (
            <div style={{ fontSize: TEXT.micro, color: "var(--a-muted)", paddingLeft: sp(6) }}>
              {spec.description}
            </div>
          )}
        </div>
      );
    } else {
      const numeric = spec.type === "number" || spec.type === "integer";
      const long = spec.format === "textarea" || spec.format === "multiline" || spec.maxLength > 200;
      control = long ? (
        <Textarea
          key={key}
          label={marked}
          hint={spec.description}
          value={draft[key] ?? ""}
          onChange={(event) => update(key, event.target.value)}
        />
      ) : (
        <Input
          key={key}
          label={marked}
          hint={spec.description}
          type={numeric ? "number" : "text"}
          placeholder={spec.default !== undefined ? String(spec.default) : undefined}
          value={draft[key] ?? ""}
          onChange={(event) =>
            update(key, numeric ? Number(event.target.value) : event.target.value)
          }
        />
      );
    }
    return divider ? [divider, control] : control;
  };

  return (
    <Stack gap="card">
      {ordered.flatMap(field)}
      {onSubmit && (
        <Stack gap="inner" horizontal style={{ marginTop: space("inner") }}>
          <Button variant="accent" disabled={missing.length > 0} onClick={() => onSubmit(draft)}>
            {submitLabel}
          </Button>
          {missing.length > 0 && (
            <span style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>
              needs {missing.map((key) => properties[key]?.title ?? humanise(key)).join(", ")}
            </span>
          )}
        </Stack>
      )}
    </Stack>
  );
}

// ------------------------------------------------------------- live agent

/** What the agent is saying, as it says it. */
export function Transcript({ events = [], height = 320 }) {
  const bottom = useRef(null);
  useEffect(() => bottom.current?.scrollIntoView({ block: "end" }), [events.length]);

  const text = events
    .filter((event) => event?.type === "text_delta" || event?.type === "TextDelta")
    .map((event) => event.text ?? event.delta ?? "")
    .join("");

  return (
    <div
      style={{
        height, overflow: "auto", padding: sp(3), whiteSpace: "pre-wrap",
        border: "1px solid var(--a-border)", borderRadius: "var(--a-radius)",
        font: "13px/1.6 var(--a-font)",
      }}
    >
      {text || <span style={{ color: "var(--a-muted)" }}>Nothing yet.</span>}
      <div ref={bottom} />
    </div>
  );
}

/** Tool calls as they happen. */
export function ToolLog({ events = [], limit = 50 }) {
  const calls = events
    .filter((event) => String(event?.type ?? "").toLowerCase().includes("tool"))
    .slice(-limit);

  if (!calls.length) return <EmptyState title="No tool calls yet" />;
  return (
    <Stack gap={1}>
      {calls.map((call, index) => (
        <Stack key={index} gap={2} horizontal>
          <Badge tone={call.outcome === "error" ? "danger" : "accent"}>{call.name ?? call.type}</Badge>
          <span style={{ fontSize: TEXT.micro, color: "var(--a-muted)", fontFamily: "var(--a-mono)" }}>
            {call.duration_ms != null ? `${call.duration_ms}ms` : ""}
          </span>
        </Stack>
      ))}
    </Stack>
  );
}

// ------------------------------------------------------- missing primitives

/**
 * One headline number.
 *
 * The single most-reached-for dashboard primitive, and it was hand-rolled
 * independently in a shipped template and in the demo — which is the clearest
 * evidence a kit is missing something.
 */
export function Metric({ label, value, variant, tone, hint, trend, style, ...rest }) {
  guard("Metric", rest);
  const chosen = variant ?? tone;
  return (
    <Card style={{ flex: "1 1 150px", minWidth: 150, ...style }}>
      <div style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>{label}</div>
      <div
        style={{
          fontSize: TEXT.display, fontWeight: 650, lineHeight: 1.15, margin: "2px 0 4px",
          // Figures that change in place must not shift width as they do, or a
          // live dashboard jitters on every tick.
          fontVariantNumeric: "tabular-nums",
        }}
      >
        {value}
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: sp(2) }}>
        {chosen ? <Badge variant={chosen}>{hint}</Badge>
                : hint && <span style={{ fontSize: TEXT.micro, color: "var(--a-muted)" }}>{hint}</span>}
        {trend?.length > 1 && <Sparkline values={trend} width={64} height={18} />}
      </div>
    </Card>
  );
}

/**
 * A callout.
 *
 * Also hand-rolled in the demo, as a `bg-red-100` div — which is exactly what
 * produced a blinding near-white block on the dark ground. Getting it from the
 * kit makes it theme-correct without anyone having to think about it.
 */
export function Alert({ variant, tone, title, children, ...rest }) {
  guard("Alert", rest);
  const chosen = variantOf({ variant, tone }, "accent");
  const accent = {
    default: "var(--a-accent)", accent: "var(--a-accent)", info: "var(--a-accent)",
    ok: "var(--a-ok)", warn: "var(--a-warn)", danger: "var(--a-danger)",
  }[chosen] ?? "var(--a-accent)";

  return (
    <div
      role={chosen === "danger" || chosen === "warn" ? "alert" : "status"}
      style={{
        borderRadius: "var(--a-radius)", border: "1px solid var(--a-border)",
        borderLeft: `3px solid ${accent}`, background: "var(--a-subtle)",
        padding: `${space("inner")} ${space("card")}`, fontSize: TEXT.body,
      }}
    >
      {title && <div style={{ fontWeight: 600, marginBottom: sp(1), color: accent }}>{title}</div>}
      <div style={{ color: "var(--a-fg)" }}>{children}</div>
    </div>
  );
}

/**
 * Markdown, rendered by the harness.
 *
 * Models write markdown by default. The renderer runs in Rust, so no library
 * reaches the browser, fenced code is highlighted by the same syntect that
 * colours `<Code>`, and raw HTML in model prose is dropped rather than run.
 */
export function Markdown({ children, style }) {
  const source = typeof children === "string" ? children : String(children ?? "");
  const [html, setHtml] = useState(null);

  useEffect(() => {
    let live = true;
    artist.markdown(source).then((result) => live && setHtml(result)).catch(() => live && setHtml(null));
    return () => { live = false; };
  }, [source]);

  // Until it arrives, show the source. It is prose either way, so this reads
  // as slightly-wrong text rather than as a blank space.
  if (html === null) {
    return <div style={{ whiteSpace: "pre-wrap", ...style }}>{source}</div>;
  }
  return (
    <div
      className="a-markdown"
      style={{ ...style }}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

/**
 * A link to another canvas in this project.
 *
 * Canvases used to be islands: each one a URL the model handed over separately,
 * with no way to get from a row in a dashboard to the review tool for that row.
 * The session key lives in the path, so building the URL by hand is both fiddly
 * and a good way to leak it into somewhere it should not be — this is the only
 * thing that should be constructing canvas URLs.
 *
 * `to` omitted links to the lobby, which is the way back to everything else.
 */
export function CanvasLink({ to, params, children, ...rest }) {
  const key = artist.key;
  if (!key) {
    return <span style={{ color: "var(--a-muted)" }}>{children ?? to ?? "canvases"}</span>;
  }
  const query = params ? `?${new URLSearchParams(params)}` : "";
  return (
    <a
      className="a-focus"
      href={`/c/${key}/${to ? `${to}/` : ""}${query}`}
      style={{ color: "var(--a-accent)", textDecoration: "underline", textUnderlineOffset: 2 }}
      {...rest}
    >
      {children ?? to ?? "All canvases"}
    </a>
  );
}

/**
 * A path that opens where the user actually works.
 *
 * The kit's bar for inclusion is "it can only exist because Artist is
 * underneath", and this is the purest example: a canvas showing
 * `palette.rs:131` should be one click from the editor.
 */
export function FileLink({ path, line, children }) {
  return (
    <button
      className="a-focus"
      onClick={() => artist.edit(path, line)}
      title={`Open ${path}${line ? `:${line}` : ""}`}
      style={{
        background: "none", border: "none", padding: 0, cursor: "pointer",
        color: "var(--a-accent)", font: "12px var(--a-mono)",
        textDecoration: "underline", textUnderlineOffset: 2,
      }}
    >
      {children ?? `${path}${line ? `:${line}` : ""}`}
    </button>
  );
}

/** A bare trend line, sized to sit beside a number. */
export function Sparkline({ values = [], width = 96, height = 24, tone }) {
  if (values.length < 2) return null;
  const low = Math.min(...values);
  const high = Math.max(...values);
  const span = high - low || 1;
  const points = values
    .map((value, index) => {
      const x = (index / (values.length - 1)) * (width - 2) + 1;
      const y = height - 1 - ((value - low) / span) * (height - 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");

  return (
    <svg width={width} height={height} role="img" aria-label={`trend, ${values.length} points`}>
      <polyline
        points={points}
        fill="none"
        stroke={tone ?? "var(--a-chart-1)"}
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
    </svg>
  );
}
