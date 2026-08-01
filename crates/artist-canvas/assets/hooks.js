// React bindings over the canvas runtime.
//
// The runtime is framework-agnostic on purpose; this is the layer that makes it
// idiomatic. Everything here is built on useSyncExternalStore so a canvas gets
// correct behaviour under concurrent rendering without thinking about it.

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { artist } from "@artist/canvas";

export { artist };

/// Event types that carry only more of the same text. `PromptEvent` is tagged
/// `type` in snake_case, so these are the wire names.
const DELTAS = new Set(["text_delta", "reasoning_summary_delta"]);

/**
 * Shared, durable state — the same key seen by every open tab, by the model,
 * and by tomorrow's session.
 *
 * Reads like useState. The difference is that setting it is a write everyone
 * sees, so use it for what the canvas is *about* (the selection, the filter,
 * the answer) and plain useState for what only this tab cares about.
 */
export function useCanvasState(key, initial) {
  const value = useSyncExternalStore(
    artist.state.subscribe,
    () => artist.state.get(key),
    () => undefined,
  );

  // Seed once, and only if nobody else already has. Two tabs opening together
  // must not race each other back to the initial value.
  const seeded = useRef(false);
  useEffect(() => {
    if (seeded.current || initial === undefined) return;
    seeded.current = true;
    if (artist.state.get(key) === undefined) {
      artist.state.set(key, initial);
    }
  }, [key, initial]);

  const set = useCallback(
    (next, options) => {
      const resolved = typeof next === "function" ? next(artist.state.get(key)) : next;
      return artist.state.set(key, resolved, options);
    },
    [key],
  );

  return [value === undefined ? initial : value, set];
}

/** Everything the canvas knows, as one object. */
export function useCanvasStateAll() {
  return useSyncExternalStore(artist.state.subscribe, artist.state.all, () => ({}));
}

/**
 * Another canvas's shared state, live.
 *
 * For the case the lobby made possible: a form writes a decision and a
 * dashboard shows it, without either knowing the other exists beyond a line in
 * `canvas.toml`. Read-only — the canvas that owns a key is the one that writes
 * it, which is what keeps two surfaces from fighting over one store.
 *
 * The mirror is memoised per slug so a component re-rendering does not start a
 * second subscription, and `useSyncExternalStore` gets the stable references it
 * requires either way.
 */
export function useCanvasStateOf(slug, key) {
  const mirror = useMemo(() => artist.state.of(slug), [slug]);
  const entries = useSyncExternalStore(mirror.subscribe, mirror.get, () => ({}));
  return key === undefined ? entries : entries[key];
}

/**
 * The running agent: what it is doing, what it has done, and how to talk to it.
 */
export function useAgent({ history = 200 } = {}) {
  const [context, setContext] = useState(null);
  const [events, setEvents] = useState([]);

  useEffect(() => {
    let live = true;
    const refresh = () =>
      artist.context().then((value) => live && setContext(value)).catch(() => {});
    refresh();
    const stop = artist.events.subscribe((event) => {
      // Bounded: a long session would otherwise grow this array without limit
      // and take the tab down with it.
      setEvents((previous) => [...previous, event].slice(-history));
      // Not on every event. The stream carries one delta per token while the
      // model writes, and refreshing on each of those meant an RPC round trip
      // per token — hundreds per response, for a value that cannot have
      // changed. Deltas are the model still saying the same thing; everything
      // else can move `busy`, the model, or the profile.
      if (!DELTAS.has(event?.type)) refresh();
    });
    return () => {
      live = false;
      stop();
    };
  }, [history]);

  return {
    context,
    events,
    busy: context?.busy ?? false,
    send: artist.send,
    call: artist.call,
  };
}

/** Just the agent event stream, for a canvas that only wants to watch. */
export function useAgentEvents({ history = 200, filter } = {}) {
  const [events, setEvents] = useState([]);
  // Read through a ref so the subscription does not depend on it. `filter` is
  // almost always written inline — `useAgentEvents({filter: e => …})` — which
  // is a new function every render, and depending on it tore the subscription
  // down and rebuilt it on each one, dropping whatever arrived in between.
  const current = useRef(filter);
  current.current = filter;

  useEffect(
    () =>
      artist.events.subscribe((event) => {
        const keep = current.current;
        if (keep && !keep(event)) return;
        setEvents((previous) => [...previous, event].slice(-history));
      }),
    [history],
  );
  return events;
}

/**
 * Questions awaiting an answer.
 *
 * The same questions the TUI is showing — answering here retires them there
 * too, and whoever gets there first wins.
 */
export function useAsk() {
  const questions = useSyncExternalStore(
    artist.ask.subscribe,
    artist.ask.pending,
    () => [],
  );
  return { questions, answer: artist.ask.answer };
}

/**
 * Call a tool, tracking the request the way a component wants it.
 *
 * Stale responses are dropped: if the arguments change while a call is in
 * flight, the older result must not overwrite the newer one.
 */
export function useTool(name) {
  const [state, setState] = useState({ loading: false, output: null, error: null });
  const generation = useRef(0);

  const run = useCallback(
    async (args = {}) => {
      const mine = ++generation.current;
      setState({ loading: true, output: null, error: null });
      try {
        const output = await artist.call(name, args);
        if (mine === generation.current) setState({ loading: false, output, error: null });
        return output;
      } catch (error) {
        if (mine === generation.current) {
          setState({ loading: false, output: null, error: error.message ?? String(error) });
        }
        throw error;
      }
    },
    [name],
  );

  return { ...state, run };
}

/** Prefers-dark, tracked live, for a canvas that themes itself. */
export function useTheme() {
  const query = "(prefers-color-scheme: dark)";
  return useSyncExternalStore(
    (notify) => {
      const media = matchMedia(query);
      media.addEventListener("change", notify);
      return () => media.removeEventListener("change", notify);
    },
    () => (matchMedia(query).matches ? "dark" : "light"),
    () => "light",
  );
}
