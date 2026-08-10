# Query and continuation contract

## Query identity

`CognitiveQuery` contains the complete `InterpretedGraph`, exact request, optional target theory, and discovery policy.

`QueryId` is caller-selected. `QueryHash` is canonical content identity over the complete query. Every `CognitiveAnswer`, `KernelContinuation`, and `DiscoveryContinuation` carries both identities. A result for changed query content is rejected even when the caller reuses the same `QueryId`.

## Deductive requests

`Prove`, `Disprove`, and `UnderAssumptions` return `CertifiedGraphClaim` values. The claim's graph-to-DTT elaboration is recomputed, its theory extension is checked, its certificate is checked by the fixed kernel, and its dependency report is recomputed.

`Disprove` proves the exact counterclaim explicitly supplied by the harness. The harness is responsible for selecting a counterclaim whose ontology meaning is the intended refutation.

`UnderAssumptions` requires the certificate context to equal the exact ordered list of elaborated assumptions.

## Result-producing requests

Each result-producing request declares typed variables and an exact success predicate:

- `Find` / `Enumerate`: `T1 -> ... -> Tn -> Proposition`;
- `SynthesizeFunction`: `FunctionType -> Proposition`;
- `Counterexample`: `Proposition -> CounterexampleType -> Proposition`;
- `Compare`: `LeftType -> RightType -> ResultType -> Proposition`;
- `ComputeBound`: `TargetType -> BoundType -> Proposition`.

A successful tuple is a `CertifiedQueryWitness` containing:

- exactly one binding for every declared variable;
- graph or closed DTT witnesses of the declared types;
- a DTT certificate proving the fully instantiated success predicate;
- the exact dependency closure;
- optional untrusted production evidence.

Type-correct output alone is not an answer. The success predicate must be proved. Duplicate enumeration tuples and result-limit violations are rejected.

`SearchExhausted` is accepted only when the query supplies an exact exhaustion proposition and the answer contains a complete certified proof of that proposition.

## Empirical requests

`EmpiricalPosterior` returns only a `PosteriorClaim` matching the exact proposition graph root, model, and dataset. It remains explicitly conditional and is not treated as a DTT proof.

## Honest statuses

Statuses distinguish:

- `Proved`;
- `Disproved`;
- `Answered`;
- `RejectedCertificate`;
- `FormalizationUnavailable`;
- `SearchExhausted`;
- `Unknown`;
- `Paused`;
- `Cancelled`;
- `ResourceLimitReached`.

`FormalizationUnavailable` means the exact content is understood but cannot currently be produced in the requested DTT role. It does not mean the graph is uninterpreted or false.

## Continuations

A kernel continuation stores the complete serialized `CheckSession` and pins its exact theory. A discovery continuation stores an explicit procedure identity/version, the exact theory selected by the suspended procedure, opaque producer state, and completed logical steps.

Kernel continuations use format version 2. Discovery continuations use format version 3. Both pin the full query hash, complete interpreted-submission hash, graph hash, ontology hash, and exact selected theory identity.
