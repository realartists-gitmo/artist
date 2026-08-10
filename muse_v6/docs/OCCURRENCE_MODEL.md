# Muse occurrence/proposition model

`muse-occurrence` is the shared semantic language for deterministic structured events and SLM prose outputs.

## No privileged narrator

Proposition content and presentation are separate.

- `I thought P` in a user message: user is the attitude holder.
- `I thought P` in an agent message: agent is the attitude holder.
- `maybe P`: possibility/hypothesis presentation, not an unconditional assertion of P.
- a tool result saying success: reported success for the invocation, not proof of every requested effect.

Muse records what each source presents and what deterministic/compositional derivations are licensed. It does not silently turn source-relative claims into metaphysical truth.

## Semantic object classes

The IR has explicit identities for source spans, referents, occurrences, propositions, statements, variables, and unresolved ambiguities.

Occurrences distinguish event, process, state, situation, and transition.

Propositions can express type/relation claims, occurrences, equality, negation, conjunction/disjunction, implication, quantification, modality, attitudes with explicit holders, speech acts with explicit speakers/addressees, temporal relations, causal relations, and quotation.

Statements attach proposition content to a presenter, presentation mode, source spans, evidence, and derivation basis.

## Derivation boundary

`StatementBasis::Expressed` means the source directly presents the statement.

`StatementBasis::CompositionalEntailment` names an explicit `CompositionalRule`. Protocol v2 permits only `ConjunctionProjection`: one member of one explicitly represented conjunction may be projected as a derived top-level statement. The validator checks that shape mechanically. It does not authorize unrestricted theorem-prover, lexical-entailment, presupposition, or commonsense closure during labeling.

The training target should include every expressed semantic statement and every explicitly licensed compositional entailment, while leaving broader logical consequence to downstream formal reasoning.

## Ambiguity

Unresolved interpretation uncertainty is represented as ambiguity metadata with targets and alternatives. Parser uncertainty is not encoded as though the source itself asserted a disjunction.

## Canonical training serialization

`OccurrenceDocument::canonical_training_projection` is the training/output normalization boundary. It canonicalizes producer-local source-span/referent/occurrence/proposition/statement/variable/ambiguity IDs, alpha-normalizes quantified variables, canonicalizes conjunction/disjunction members and equality operands by structural semantics, and retains source coordinates plus ontology-snapshot identity.

`canonical_training_bytes` and `canonical_training_digest` are therefore invariant to producer-local semantic IDs, bound-variable renaming, commutative member ordering, and equality orientation. They deliberately remain sensitive to semantic differences such as presenter/attitude-holder identity and source provenance. Unbound variables, multiply-bound variable IDs, proposition cycles, and recursively self-referential occurrence graphs are rejected rather than assigned traversal-dependent training encodings.

`OccurrenceDocument::canonical_digest` remains the stable digest of one concrete producer representation. Training labels and deterministic-vs-learned convergence checks use the training canonicalization methods instead.

## Prose-v4 open-world representation

`muse-semantic-label-6` uses a separate model-facing `LearnedSemanticTarget`. Every open predication is a singularly ontology-typed `LearnedOccurrence` with one `sort` and an exact lexical source anchor; open nominal identity is preserved the same way on `LearnedReferent`. The compiler then constructs the richer compatibility/provenance IR. Multi-type `Occurrence.types` is not part of the learned target.

Learned prose uses the narrowest participant role justified by source structure plus pinned ontology/lexicon semantics. Ontology relations and finite semantic roles such as Agent, Patient, Theme, Experiencer, Content, Source, Goal, Recipient, Instrument, Location, and Possessor are valid learned targets. Grammatical roles remain fallbacks when semantic narrowing is not justified.

Occurrences no longer carry a separate learned `kind` field. The narrowest ontology type is the semantic category; its ancestry deterministically supplies broad event/situation classification, while the former finer `Process`/`Transition` distinctions had no independent formal semantics and are not predicted. Tense/aspect/voice and structural operator grammar still require exact source evidence. Structural propositions carry exact `operator_spans`; source-anchored fallbacks remain available where the finite operator inventory genuinely cannot represent an overt construction.

Knowledge and Recollection are explicit attitudes without automatic factivity leakage. Capability is distinct from epistemic possibility/permission. Interrogatives bind answer variables. Phase is distinct from grammatical aspect. Purpose is distinct from Causes/Enables/Motivates.

The formal bridge lowers occurrence anchor, kind, grammar, participants, grounding, and deterministic metadata into typed structural formal propositions; causal and temporal relations lower their actual semantic endpoints. Canonical JSON remains an audit root rather than the only semantic payload reaching the fixed superstrate.
