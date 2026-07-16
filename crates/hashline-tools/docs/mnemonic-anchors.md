# Mnemonic line anchors

`hashline-tools` exposes stable, file-scoped mnemonic handles for line-oriented edits while retaining full content-derived hashes internally. Handle uniqueness is enforced across the entire file; no narrower region or read-window scope is used.

Rendered file views use the compact form `anchor | line text`. The separator and line text are not part of the anchor: for `time | beta`, every edit operation must pass only `time` as the edit anchor. Results can include the constant `ANCHOR_USAGE` with this rule.

Unchanged lines keep their handles when surrounding code moves. One-word handles are allocated first from the OpenAI-tokenizer-screened vocabulary in `mnemonic_words.txt`. Files larger than the one-word capacity use two-word handles only for newly encountered overflow lines.

A live line's handle remains fixed while that line remains live. When a one-word handle is freed by deletion, it may be assigned to a subsequently created line, but an existing two-word handle is never shortened, promoted, or otherwise changed merely because one-word capacity becomes available. Handle stability takes precedence over minimizing the current pair count. Some pair handles can therefore remain after the file later shrinks; this tradeoff is intentional.

The stale-anchor confirmation path validates the hidden hash binding and presents current mnemonic context before accepting an exact retry of the same edit batch.
