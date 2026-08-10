# Trusted dependent-type kernel

## Judgment

A successful certificate check establishes exactly:

```text
T ; Γ ⊢ proof : proposition
```

`T` is a content-identified append-only theory and `Γ` is the exact ordered local context.

## Why dependent type theory

Dependent type theory is the fixed proof calculus; it is not selected per interpretation. It supplies propositions-as-types, proofs-as-terms, higher-order functions, dependent specifications, equality, induction, and certified programs.

Domain theories provide the specific constants, facts, definitions, and assumptions. DTT supplies the universal typing and proof rules.

## Core syntax

The calculus contains predicative cumulative universes, de Bruijn variables, constants, Pi, Sigma, lambda, application, pairs, projections, identity, reflexivity, `J`, native strictly positive inductive elimination, and `let`.

`Type n : Type (n+1)`. There is no `Type : Type`.

## Definitional equality

Trusted computation is limited to terminating rules:

- beta and zeta;
- append-order-safe transparent delta unfolding;
- Sigma projections;
- `J` on reflexivity;
- inductive iota reduction;
- congruence over core constructors.

Arbitrary user rewriting never enters definitional equality. It is checked by explicit finite traces above the kernel.

## Theories

Definitions and theorem bodies are checked before insertion. Axioms and oracles are explicit declaration kinds with provenance. Inductive families use field forms that make negative recursive occurrences unrepresentable.

Theory identity hashes namespace, version, declarations, bodies, kinds, transparency, provenance, and inductive metadata. Deserialized theories are rebuilt through the checked builder before use.

## Continuations

`CheckSession` stores exact theory, control state, frame stack, transition count, and terminal result. It is deterministic and serializable. Restored sessions revalidate their embedded theory.

A transition is a logical machine transition, not a guaranteed constant-time CPU microstep. Hard real-time preemption is not claimed.
