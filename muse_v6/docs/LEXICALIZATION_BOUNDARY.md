# Lexicalization boundary

Muse does not require a comprehensive English lexicon and must never acquire a finite-lexicon bottleneck.

The learned semantic labels language is split deliberately:

- a finite structural grammar represents scope, force, binding, attitudes, modality, interrogatives, phase, tense/aspect/voice, temporal/causal/purpose structure, quotation, ambiguity, and occurrence identity;
- unbounded domain vocabulary is preserved by exact source-anchored occurrence predicates.

`LexicalAnchor` is not a lemma database. It is a set of exact source spans containing the content-bearing predicate material. The annotator does not invent a normalized predicate string. An unfamiliar predicate is therefore legal without a dictionary entry.

For open prose arguments, Muse preserves grammar rather than guessing a lexical semantic frame: Subject, DirectObject, IndirectObject, PredicateComplement, Content, Possessor, or an exact source-anchored role. Rich semantic/ontology roles are reserved for deterministic/ontology-backed evidence.

A lexicon may still provide optional conveniences such as lemma/form metadata, aliases, high-confidence ontology mappings, attestations, or frame hints. Those are pinned semantic evidence when applicable. They need not enumerate every sentence, but a known source-licensed ontology mapping/frame must be used by the semantic-v6 target; unknown lexical identity remains exact-source-anchored and still receives foundational ontology typing.

Resolution/classification components likewise remain auxiliary. The canonical target is determined by source evidence plus the frozen structural rules, not by unrestricted lexical lookup.
