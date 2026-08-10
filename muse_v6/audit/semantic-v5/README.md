# Semantic-v5 adversarial audit

This directory is the pre-label adversarial corpus audit for `muse-semantic-label-5`.

- `draw.json` is the first structured/tool adversarial draw from `agentic-sessions-big-37(2).zip`.
- `draw2.json` is an independent second draw after the first-round formalism repairs.
- `ADVERSARIAL_DRAW.md` records the failures found, the repaired contract, and representative corrected targets.
- `scripts/verify_adversarial_draw.py` verifies the committed draw manifests and the source/static markers that prevent the discovered failure classes from regressing.

The external session archive is not bundled into the workspace. The manifests pin exact relative source paths, line numbers, byte counts, and SHA-256 digests so the draw can be revalidated against the original archive when available.
