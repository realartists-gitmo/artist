# Extension Contracts

Extensions are wasm components. The "class" of an extension is **which of the three
contract families its exported interfaces satisfy**. A single component can satisfy
multiple families (and most real ones will).

## 1. Noun contract — resources

The component provides addressable state. Filesystem-shaped: you can read it, list it,
inspect it, address within it.

- A namespace. Examples: `bash://` (a shell session is a noun — an addressable thing),
  `file://`, `repo://`, `issues://`.
- Nouns are what verbs operate *over*.
- `bash` is a **resource**, not a tool.

## 2. Verb contract — tools

The component provides typed operations over resources. File-shaped in, file-shaped out.

- The universal verbs: `read()`, `write()`, `edit()`, `poll()`, and so on.
- They are designed to operate on **file shapes**.
- **Not defined yet.** We are not locking the verb signatures now.
- The verbs are hosted in `tools://` — the verb namespace. `tools://` is also where we
  define how a component is structured: its source, its compilation to wasm, its wit.
- Tools must be **self-inspectable and self-modifiable** — which is why they are wasm
  extensions. You read the source through the namespace, edit it, it recompiles.

## 3. Event contract — services

The component is **long-lived and reactive**. This is the arbitrary-bullshit contract:
anything the agent loop needs to plug into that isn't a noun or a verb.

The agent loop (and every extension in the harness) publishes a **typed event stream** —
turn-started, tool-called, result-produced, file-changed, etc. The event contract is:

> Subscribe to typed events, react, and emit.

The range it covers:

- **Hooks** — subscribe to a named event and react.
- **Policies / vetoes** — react to an "about to do X" event and suppress it.
- **Formatters / transforms** — react to output events and emit modified ones.
- **Background tasks** — a service reacting to time/timer events.
- **Sub-agents** — a long-lived service that subscribes, and *also* exports verb
  interfaces; a participant that can be driven.

The single abstraction:

> An **addressable, long-lived service** with a lifecycle (start/stop/reload, status you
> can read) that subscribes to and emits a typed event stream.

## Composition

A component isn't pigeonholed into one family.

- A formatter that's also inspectable exports event **and** namespace interfaces.
- The component behind `bash://` is a noun *and* a long-lived service (it maintains
  sessions and reacts to session events) — it exports namespace **and** event interfaces.
- A sub-agent exports event **and** verb interfaces.

The extension class is just **which of the three families its exported interfaces satisfy**.

## Kernel namespaces

The kernel hosts exactly the namespaces that **can't be bootstrapped as wasm
extensions** — the ones that are load-bearing for loading extensions in the first
place. Four, and no more:

- **`files://`** — passthrough to the actual OS filesystem. Named `files://`, not
  `os://`: `os://` is a vague grab-bag; `files://` is precise. The kernel needs real
  filesystem access before *any* wasm exists — it must read disk to find and compile
  extension source.
- **`resources://`** — mounts namespace extensions. Must exist before any namespace can
  be a wasm extension.
- **`tools://`** — hosts the verb surface and the source→wasm→mount tool-loading
  machinery. If tool *loading* were itself a tool, you'd get infinite regress.
- **`events://`** — the typed event stream, the substrate every extension (and the agent
  loop) subscribes to. If it were an extension, extension-loading events would depend on
  the thing being loaded.

**Namespaces (nouns) always live inside `resources://` extensions.** Tools are verbs,
not nouns — `tools://` hosts verb implementations, nothing noun-shaped. Everything beyond
the kernel set (`bash://`, `repo://`, `process://`, …) is a resource extension mounted
under `resources://`.