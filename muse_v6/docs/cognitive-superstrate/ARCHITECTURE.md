# Architecture

## 1. Exact harness write boundary

The harness submits `InterpretedGraph` rather than an untyped graph plus a semantic guesser:

```text
ObjectGraph
Ontology
ObjectId -> ObjectInterpretation
```

The interpretation map is total. Every object has an exact ontology type and one exact denotation mode:

- `Type`: an ontology type expression;
- `Symbol`: a declared noun, verb, relation, predicate, or function;
- `Application`: application of the graph operator in an explicit role order;
- `Quoted`: the exact graph object as data of universal type `QuotedObject`.

`InterpretedGraph::validate` checks total coverage and direct declaration identity. `typing_diagnostics` separately reports malformed applications without deleting or semantically downgrading the represented content.

## 2. Deterministic ontology-to-DTT compilation

`OntologyCompiler` mechanically extends a checked base theory with stable declarations derived from ontology and submission content identities:

- ontology type -> DTT type constant;
- distinguished proposition type -> transparent realization of `Type 0`;
- noun -> constant of its exact type;
- verb/function -> curried Pi type;
- universal `QuotedObject` -> DTT type constant;
- graph object -> submission-addressed quote constant.

The compiler performs no inference, overload resolution, or reinterpretation. Generated ontology names depend only on the ontology hash. Quote names additionally depend on the complete interpreted-submission hash. Existing generated names may be reused only when their complete checked declarations are identical.

`CompiledOntologySubmission` records the exact base theory, submission hash, graph hash, ontology hash, generated names, complete compiled theory, and compiler version. Verification recompiles it exactly.

## 3. Object elaboration

Ordinary semantic elaboration recursively constructs the exact DTT term in declared role order. Partial and higher-order application are supported because an application operator may itself be any exactly typed graph object.

A `Quoted` object elaborates as its exact quote constant and has DTT type `QuotedObject`. Any object can also be requested explicitly in the `QuotedObject` role, including malformed applications and cyclic graph structure.

Semantic cycles are not unfolded into cyclic kernel terms. They remain exact finite graph data and may participate in ordinary typed predicates through `QuotedObject`.

An `ElaborationRecord` pins the complete submission, graph object, ontology, compiled theory, requested role, generated term and type, transitive graph-object dependencies, and compiler version. Verification recomputes the record exactly.

## 4. Domain theory extensions

Facts, definitions, theorems, axioms, and oracles are added through `TheoryBuilder`. A proof theory used for a submission must be an exact append-only extension of that submission's compiled theory. `Theory::is_extension_of` checks this relation.

Ontology meaning does not change in an extension. Stable ontology-derived names allow domain declarations to be generated consistently for every submission using the same ontology. Submission-specific quote constants remain explicitly isolated by the submission hash.

## 5. Trusted kernel

The trusted kernel checks one fixed dependent type theory. It performs no proof search, ontology guessing, Bayesian inference, arbitrary rewriting, or universal-machine execution.

A successful certificate check establishes only:

```text
T ; Γ ⊢ proof : proposition
```

for the exact recorded theory, ordered context, proposition, and proof.

## 6. Universal proof and computation traces

`artist-theories` contains a deterministic reflected register/stack machine suitable for finite proof-checker and computation traces. A `UniversalRun` records the canonical initial state and every successor; the checker recomputes every transition.

Universal acceptance is untrusted proof-production evidence. It never replaces the DTT certificate required for deductive acceptance.

## 7. Queries and continuations

`CognitiveQuery` embeds the complete interpreted submission and supports proof, explicit refutation, witness search, function synthesis, enumeration, counterexamples, comparison, bounds, assumption-relative proof, and empirical posterior requests.

Every query has two identities:

- caller-selected `QueryId`;
- canonical `QueryHash` over the entire submission, request, theory selection, and policy.

Every answer and continuation carries both. Reusing a caller ID after changing any query content invalidates the old result.

Successful witness, synthesis, counterexample, comparison, and bound results are `CertifiedQueryWitness` values. Each tuple contains exact bindings, a kernel certificate for the fully instantiated success predicate, exact dependency disclosure, and optional untrusted production provenance.

Kernel checker state and discovery state are separate continuation variants. Both pin the complete query and interpreted submission. Kernel continuations also embed the exact checked theory and serialized kernel session.

## 8. Empirical layer

Observations are immutable source events with explicit valid time, recording time, clock identity, acquisition, provenance, derivation, and supersession.

Bayesian records retain exact interpreted-submission references for the model, dataset, proposition, procedure, and approximation classification. A raw graph hash is insufficient because ontology identity is part of meaning.

Optional empirical certification contains the deterministic DTT elaboration of the exact claim, a kernel certificate for that proposition under an append-only theory extension, and its recomputed dependency closure. An unrelated valid certificate cannot certify an empirical result, and posterior probability is never converted into unconditional deductive truth.

## 9. Persistence boundary

Mnestic integration is external. This workspace exposes serializable, content-identified values and canonical artifact envelopes. Storage, indexing, transport, and supersession propagation are host responsibilities.
