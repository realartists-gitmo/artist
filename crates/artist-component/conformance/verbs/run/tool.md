---
name: artist-tool-run
description: WASM run verb component
version: 0.1.0
contract: artist:tool:run@1
capabilities:
  - resource.run
---

Executes the run universal verb through the kernel capability bridge. `args`
are launch-time arguments/configuration analogous to argv; they are not stdin
and are not a shell-command submission channel. The owning namespace returns
the authoritative execution URI, which may equal or differ from the requested
URI.
