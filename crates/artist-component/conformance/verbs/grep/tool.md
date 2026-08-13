---
name: artist-tool-grep
description: "Search resource contents. Patterns: bare text or lit:TEXT are literal; re:REGEX is regex; fz:TEXT is fuzzy."
version: 0.1.0
contract: artist:tool:grep@1
capabilities:
  - resource.grep
---

Executes the grep universal verb through the kernel capability bridge.
