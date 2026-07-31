// React bindings over the canvas runtime.
//
// The runtime is framework-agnostic on purpose; this is the layer that makes it
// idiomatic. Everything here is built on useSyncExternalStore so a canvas gets
// correct behaviour under concurrent rendering without thinking about it.

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { artist } from "@artist/canvas";

export { artist };

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
    (next) => {
      const resolved = typeof next === "function" ? next(artist.state.get(key)) : next;
      return artist.state.set(key, resolved);
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
 * The running agent: what it is doing, what it has done, and how to talk to it.
 */
export function useAgent({ history = 200 } = {}) {
  const [context, setContext] = useState(null);
  const [events, setEvents] = useState([]);

  useEffect(() => {
    let live = true;
    artist.context().then((value) => live && setContext(value)).catch(() => {});
    const stop = artist.events.subscribe((event) => {
      // Bounded: a long session would otherwise grow this array without limit
      // and take the tab down with it.
      setEvents((previous) => [...previous, event].slice(-history));
      artist.context().then((value) => live && setContext(value)).catch(() => {});
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
  useEffect(
    () =>
      artist.events.subscribe((event) => {
        if (filter && !filter(event)) return;
        setEvents((previous) => [...previous, event].slice(-history));
      }),
    [history, filter],
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
