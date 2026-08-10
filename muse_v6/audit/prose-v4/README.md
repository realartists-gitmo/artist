# Muse prose-v4 fixed adversarial regression

This directory is the mandatory semantic regression fixture for `muse-prose-label-4.1`.

The 36 windows are the same Claude/Grok/Codex windows selected before the v3 redesign. V4 re-applies the corrected ontology-first contract to those exact windows; replacing them with an easier sample does not satisfy the pre-label gate.

The audit projection enforces the v4 invariants that were absent from v3: every occurrence has a pinned ontology type, every represented referent has a type inventory entry, every bound variable has an ontology domain, the obsolete occurrence-kind duplicate is absent, semantic roles are permitted, and exact numeric cardinality is structural rather than an anonymous lexical quantifier. Exact source lexical anchors remain for open vocabulary.

Files:

- `audit_windows.jsonl` — exact source windows and provenance metadata.
- `v4_strict_labels.jsonl` — audit-only nested projection of the v4 semantic structures, including globally unique namespaceless ontology symbols.
- `V4_STRATIFIED_LABEL_AUDIT.md` — human-readable source/label review with ontology type, semantic-role, and variable-domain information.
- `scripts/build_strict_audit.py` — deterministic hand-label fixture builder.
- `scripts/verify_builder_scope.py` — prevents accidental cross-window Python state reuse.
- `scripts/verify_v4_audit.py` — exact-span, binder, ontology-type/ancestry, operator, role, grammar, cardinality, and structural checks.
- `scripts/verify_source_coverage.py` — backstop against silently dropping large source clauses.
- `scripts/render_v4_audit.py` — deterministic readable rendering.
- `scripts/verify_regression.py` — non-mutating aggregate gate; rebuilds labels in a temporary directory and compares bytes before the other checks.

Run:

```bash
python3 audit/prose-v4/scripts/verify_regression.py

# Optional manual rebuild/render:
python3 audit/prose-v4/scripts/build_strict_audit.py
python3 audit/prose-v4/scripts/verify_builder_scope.py
python3 audit/prose-v4/scripts/verify_v4_audit.py
python3 audit/prose-v4/scripts/verify_source_coverage.py
python3 audit/prose-v4/scripts/render_v4_audit.py
```

The nested JSONL is an audit projection, not a substitute serialization for `OccurrenceDocument`. Production labels are admitted only through `muse_training::conform_label`, which applies the actual occurrence schema, canonicalization, ontology conformance, typed superstrate lowering, and kernel-check contract.
