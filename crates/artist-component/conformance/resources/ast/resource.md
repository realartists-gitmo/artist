---
name: artist-ast
description: AST projections over ordinary file resources
version: 0.1.0
contract: artist:resource:extension@1
routes:
  - schemes: [file]
exports:
  - read
capabilities:
  - resource.read
docs:
  - uri: file://<path>?symbols=
    summary: Symbol projection for a source file
    verbs: [read]
    query:
      - name: symbols
        summary: AST projection path
  - uri: file://<path>?symbols=<symbol>/callers
    summary: Callers of a symbol
    verbs: [read]
    query:
      - name: symbols
        summary: AST projection path
      - name: limit
        summary: Maximum caller rows
---

The AST resource exposes a typed symbol projection and obtains source text
through the nested artist:resource/read interface.
