---
name: artist-tool-send
description: WASM send verb component
version: 0.1.0
contract: artist:tool:send@1
capabilities:
  - resource.send
---

Executes the send universal verb through the kernel capability bridge. `content`
is delivered exactly to the addressed resource's ongoing input stream: the
universal layer adds no newline, separator, or command interpretation. A
namespace may create an absent resource on first input, but missing targets
are not universally created; the handler decides.
