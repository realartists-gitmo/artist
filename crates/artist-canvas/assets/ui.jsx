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
          display: "flex", alignItems: "center", gap: sp(3),
          padding: `${sp(3)} ${sp(4)}`, borderBottom: "1px solid var(--a-border)",
          flex: "0 0 auto",
        }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600, fontSize: 15, lineHeight: 1.3 }}>{title}</div>
          {subtitle && (
            <div style={{ color: "var(--a-muted)", fontSize: 12 }}>{subtitle}</div>
          )}
        </div>
        <div style={{ marginLeft: "auto", display: "flex", gap: sp(2) }}>{actions}</div>
      </header>
      <div style={{ display: "flex", flex: "1 1 auto", minHeight: 0 }}>
        {sidebar && (
          <aside
            style={{
              width: 240, flex: "0 0 auto", borderRight: "1px solid var(--a-border)",
              overflow: "auto", padding: sp(3),
            }}
          >
            {sidebar}
          </aside>
        )}
        <main
          style={{
            flex: "1 1 auto", overflow: "auto", padding: sp(4), minWidth: 0,
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

export function Stack({ gap = 3, horizontal = false, children, style }) {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: horizontal ? "row" : "column",
        gap: sp(gap),
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

export function Card({ title, actions, children, style }) {
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
            display: "flex", alignItems: "center", gap: sp(2),
            padding: `${sp(2)} ${sp(3)}`, borderBottom: "1px solid var(--a-border)",
            background: "var(--a-subtle)",
          }}
        >
          <div style={{ fontWeight: 600, fontSize: 13 }}>{title}</div>
          <div style={{ marginLeft: "auto" }}>{actions}</div>
        </div>
      )}
      <div style={{ padding: sp(3) }}>{children}</div>
    </section>
  );
}

export function EmptyState({ title, hint, action }) {
  return (
    <div style={{ textAlign: "center", padding: sp(10), color: "var(--a-muted)" }}>
      <div style={{ fontWeight: 600, color: "var(--a-fg)", marginBottom: sp(1) }}>{title}</div>
      {hint && <div style={{ fontSize: 13 }}>{hint}</div>}
      {action && <div style={{ marginTop: sp(4) }}>{action}</div>}
    </div>
  );
}

export function Skeleton({ lines = 3 }) {
  return (
    <div aria-busy="true">
      {Array.from({ length: lines }, (_, index) => (
        <div
          key={index}
          style={{
            height: 12, marginBottom: sp(2), borderRadius: 4,
            background: "var(--a-subtle)", width: `${90 - index * 12}%`,
          }}
        />
      ))}
    </div>
  );
}

// -------------------------------------------------------------------- controls

export function Button({ variant = "default", size = "md", children, style, ...rest }) {
  const palette = {
    default: { background: "var(--a-subtle)", color: "var(--a-fg)", border: "1px solid var(--a-border)" },
    primary: { background: "var(--a-accent)", color: "var(--a-accent-fg)", border: "1px solid transparent" },
    danger: {
      background: "var(--a-danger)",
      // Not a literal white: the danger colour is a dark red in light mode and
      // a light pastel in dark mode, so the text on it has to flip as well.
      color: "var(--a-accent-fg)",
      border: "1px solid transparent",
    },
    ghost: { background: "transparent", color: "var(--a-fg)", border: "1px solid transparent" },
  }[variant];
  const pad = size === "sm" ? `${sp(1)} ${sp(2)}` : `${sp(2)} ${sp(3)}`;

  return (
    <button
      className="a-focus"
      style={{
        ...palette, padding: pad, borderRadius: "var(--a-radius)",
        font: `500 ${size === "sm" ? 12 : 13}px var(--a-font)`,
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
      {label && <label htmlFor={id} style={{ fontSize: 12, color: "var(--a-muted)" }}>{label}</label>}
      <input
        id={id}
        className="a-focus"
        style={{
          padding: `${sp(2)} ${sp(2)}`, borderRadius: "var(--a-radius)",
          border: "1px solid var(--a-border)", background: "var(--a-bg)",
          color: "var(--a-fg)", font: `13px var(--a-font)`, ...style,
        }}
        {...rest}
      />
      {hint && <div style={{ fontSize: 11, color: "var(--a-muted)" }}>{hint}</div>}
    </div>
  );
}

export function Select({ label, options = [], style, ...rest }) {
  const id = useId();
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: sp(1) }}>
      {label && <label htmlFor={id} style={{ fontSize: 12, color: "var(--a-muted)" }}>{label}</label>}
      <div style={{ position: "relative", display: "grid" }}>
        <select
          id={id}
          className="a-focus"
          style={{
            padding: sp(2), borderRadius: "var(--a-radius)", border: "1px solid var(--a-border)",
            background: "var(--a-bg)", color: "var(--a-fg)", font: "13px var(--a-font)",
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
    <label htmlFor={id} style={{ display: "flex", gap: sp(2), alignItems: "center", fontSize: 13, ...style }}>
      <input id={id} type="checkbox" className="a-focus" {...rest} />
      {label}
    </label>
  );
}

export function Badge({ tone = "default", children }) {
  const color = {
    default: "var(--a-muted)", ok: "var(--a-ok)", warn: "var(--a-warn)",
    danger: "var(--a-danger)", accent: "var(--a-accent)",
  }[tone];
  return (
    <span
      style={{
        display: "inline-block", padding: `2px ${sp(2)}`, borderRadius: 999,
        border: `1px solid ${color}`, color, fontSize: 11, fontWeight: 500,
        whiteSpace: "nowrap",
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
          {title && <h2 style={{ margin: `0 0 ${sp(3)}`, fontSize: 15 }}>{title}</h2>}
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
            borderRadius: "var(--a-radius)", padding: `${sp(2)} ${sp(3)}`, fontSize: 13,
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
          <strong style={{ fontSize: 14 }}>{question.question}</strong>
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
              <div style={{ fontWeight: 600, fontSize: 13 }}>{option.label}</div>
              {option.description && (
                <div style={{ fontSize: 12, color: "var(--a-muted)", marginTop: 2 }}>
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

/**
 * A sortable, filterable table over TanStack Table.
 *
 * `columns` accepts plain strings for the common case — a canvas showing rows
 * of an object should not have to write column definitions.
 */
export function DataTable({ rows = [], columns, onRowClick, empty = "Nothing to show", dense }) {
  const [sort, setSort] = useState(null);
  const [filter, setFilter] = useState("");

  const resolved = useMemo(() => {
    if (columns?.length) {
      return columns.map((c) => (typeof c === "string" ? { key: c, label: c } : c));
    }
    const keys = new Set();
    rows.forEach((row) => Object.keys(row ?? {}).forEach((k) => keys.add(k)));
    return [...keys].map((key) => ({ key, label: key }));
  }, [columns, rows]);

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

  return (
    <Stack gap={2}>
      <Input
        placeholder={`Filter ${rows.length} row${rows.length === 1 ? "" : "s"}…`}
        value={filter}
        onChange={(event) => setFilter(event.target.value)}
      />
      <div style={{ overflow: "auto", border: "1px solid var(--a-border)", borderRadius: "var(--a-radius)" }}>
        <table style={{ borderCollapse: "collapse", width: "100%", fontSize: dense ? 12 : 13 }}>
          <thead>
            <tr>
              {resolved.map((column) => {
                const active = sort?.key === column.key;
                return (
                  <th
                    key={column.key}
                    onClick={() =>
                      setSort((prev) =>
                        prev?.key === column.key && prev.dir === "asc"
                          ? { key: column.key, dir: "desc" }
                          : { key: column.key, dir: "asc" },
                      )
                    }
                    style={{
                      textAlign: "left", padding: pad, cursor: "pointer",
                      borderBottom: "1px solid var(--a-border)", background: "var(--a-subtle)",
                      position: "sticky", top: 0, whiteSpace: "nowrap",
                      color: active ? "var(--a-fg)" : "var(--a-muted)", fontWeight: 600,
                    }}
                  >
                    {column.label}
                    {active ? (sort.dir === "asc" ? " ↑" : " ↓") : ""}
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {shown.map((row, index) => (
              <tr
                key={row?.id ?? index}
                onClick={() => onRowClick?.(row)}
                style={{ cursor: onRowClick ? "pointer" : "default" }}
              >
                {resolved.map((column) => (
                  <td key={column.key} style={{ padding: pad, borderBottom: "1px solid var(--a-border)" }}>
                    {column.render ? column.render(row?.[column.key], row) : String(row?.[column.key] ?? "")}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {!shown.length && <EmptyState title={empty} />}
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
function stroke(index) {
  // Read the tokens the server injects, not Tailwind's `--color-*`: those are
  // produced by the browser JIT for use inside utilities and are not exposed
  // on :root, so asking for them silently yields "" and every line goes grey.
  const token = `--a-chart-${(index % 8) + 1}`;
  return getComputedStyle(document.documentElement).getPropertyValue(token).trim() || "#888";
}

export function Plot({ data, series, height = 240, title, scales, ...rest }) {
  const host = useRef(null);
  const chart = useRef(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    if (!host.current) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(host.current);
    return () => observer.disconnect();
  }, []);

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
      // Trim rather than crash: a mismatch is a mistake in the canvas, and an
      // undersized chart reads better than a blank error boundary.
      const resolved = declared.slice(0, data.length).map((entry, index) =>
        index === 0 ? entry : { stroke: stroke(index - 1), width: 2, ...entry },
      );

      chart.current = new uPlot(
        {
          width, height, title, series: resolved,
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
  }, [data, series, width, height, title, scales, rest]);

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
export function SchemaForm({ schema = {}, value = {}, onChange, onSubmit, submitLabel = "Run" }) {
  const [draft, setDraft] = useState(value);
  const properties = schema.properties ?? {};
  const required = new Set(schema.required ?? []);

  const update = (key, next) => {
    const merged = { ...draft, [key]: next };
    setDraft(merged);
    onChange?.(merged);
  };

  const missing = [...required].filter((key) => {
    const v = draft[key];
    return v === undefined || v === "" || v === null;
  });

  return (
    <Stack gap={3}>
      {Object.entries(properties).map(([key, spec]) => {
        const label = required.has(key) ? `${key} *` : key;
        if (spec.enum) {
          return (
            <Select
              key={key}
              label={label}
              options={["", ...spec.enum]}
              value={draft[key] ?? ""}
              onChange={(event) => update(key, event.target.value)}
            />
          );
        }
        if (spec.type === "boolean") {
          return (
            <Checkbox
              key={key}
              label={`${label}${spec.description ? ` — ${spec.description}` : ""}`}
              checked={Boolean(draft[key])}
              onChange={(event) => update(key, event.target.checked)}
            />
          );
        }
        const numeric = spec.type === "number" || spec.type === "integer";
        return (
          <Input
            key={key}
            label={label}
            hint={spec.description}
            type={numeric ? "number" : "text"}
            value={draft[key] ?? ""}
            onChange={(event) =>
              update(key, numeric ? Number(event.target.value) : event.target.value)
            }
          />
        );
      })}
      {onSubmit && (
        <Stack gap={2} horizontal>
          <Button variant="primary" disabled={missing.length > 0} onClick={() => onSubmit(draft)}>
            {submitLabel}
          </Button>
          {missing.length > 0 && (
            <span style={{ fontSize: 12, color: "var(--a-muted)" }}>
              needs {missing.join(", ")}
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
          <span style={{ fontSize: 12, color: "var(--a-muted)", fontFamily: "var(--a-mono)" }}>
            {call.duration_ms != null ? `${call.duration_ms}ms` : ""}
          </span>
        </Stack>
      ))}
    </Stack>
  );
}
