---
name: artist-tool-grep
description: "Search resource contents. Patterns: bare text or fz:TEXT are fuzzy; lit:TEXT is literal; re:REGEX is regex."
version: 0.1.0
contract: artist:tool:grep@1
capabilities:
  - resource.grep
---

Executes the grep universal verb through the kernel capability bridge.
