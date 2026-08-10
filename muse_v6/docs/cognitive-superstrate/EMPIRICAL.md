# Empirical cognition

## Observations

An observation records source-emitted content, acquisition method, valid time, recording time, epistemic status, derivation links, and supersession links. `validate_observation_ledger` enforces immutable unique identities and prior-event references.

`CertainAtObservation` and `Revisable` are source attestations, not kernel truth. Deductive use requires an explicit local assumption, axiom, oracle, or theorem.

`ClockRelation` converts timestamps through an explicit rational scale and offset and returns a target interval widened by declared synchronization uncertainty.

## Bayesian models

`BayesianModel` retains hypothesis space, prior, observation space, likelihood, dependence assumptions, latent variables, optional causal structure, parameter space, and observation encoder.

`PosteriorClaim` retains the exact model, dataset, target proposition, posterior object, procedure, and one of:

- exact;
- bounded approximation with explicit error bound;
- asymptotic with convergence claim and diagnostics;
- heuristic.

`ModelComparison` preserves both models, criterion, data, procedure, and approximation class. No model is silently preferred.

## Certified numerical claims

`InferenceArtifact` always references the complete interpreted submission, graph, and claim root. It may carry an `InferenceCertification` containing the deterministic claim elaboration, a kernel certificate for that exact proposition, and the exact dependency closure. Verification recompiles the ontology bridge, checks the elaboration, rejects certificates for unrelated propositions or theories, checks the certificate, and recomputes its dependencies.

Display classification is derived from the exact ontology constructor of the verified claim: exact value, enclosure, finite-sample coverage, convergence, another certified proposition, or heuristic when no certification is attached. Confidence metadata cannot promote a result into a deductive class.
