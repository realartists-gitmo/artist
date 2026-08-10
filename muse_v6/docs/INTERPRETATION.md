# Total interpretation

The interpreter consumes harness-provided units rather than attempting unrestricted natural
language parsing. Each unit already carries its surface span, lexical-resolution result, asserted
upper types, excluded types, and optional literal/quotation/formal-symbol hint.

Every structurally valid unit produces a semantic node. Unknown surfaces become unresolved nodes;
ambiguous senses become ambiguous denotations with common guaranteed ontology ancestors when
available. Contradictory type claims are represented and diagnosed rather than dropped.

This preserves the later superstrate boundary: interpretation is total, while elaboration and
provability may still fail.
