---
name: artist-tool-find
description: "Search resource names under roots. Patterns: bare text or lit:TEXT are literal; re:REGEX is regex; fz:TEXT is fuzzy."
version: 0.1.0
contract: artist:tool:find@1
capabilities:
  - resource.find
---

Executes the find universal verb through the kernel capability bridge.
