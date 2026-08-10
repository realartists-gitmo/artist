# Resolution

`RegistryResolver` is deterministic and deliberately conservative. It considers exact lemma/form
matches, case policy, domain hints, context concepts, evidence, deprecation, and quotation
requirements. Scores are integer basis points and ties inside the configured margin remain
ambiguous.

Resolution has exactly three outcomes:

- `Resolved`: one sense is sufficiently better supported.
- `Ambiguous`: multiple viable senses remain.
- `Unknown`: no registered sense matches.

Mention, quotation, hypothetical, and ordinary-use modes are preserved independently of sense.
