# Semantics

> What a sentence of this language **means**, independently of any evaluator.
>
> Revision 5. Each earlier revision is named below where it was wrong, because a
> spec that quietly improves is one you cannot trust the second time. The
> corrections so far: monotonicity asserted from FOUR while the operator ranged
> over FIVE, then asserted for a lifting that has none (§4.1); unconditional
> soundness claimed for a kernel with a trust point (§7.1); an approximation
> level identified with a determinacy state (§5); Boolean valuations enumerated
> under a Belnap semantics (§3, §10); a rule given a lemma resting on a section
> that did not exist (§10); a single time index for two clocks (§8.1); and a
> defeasibility operator claimed monotone when it is antitone in one argument
> and not `⊑`-monotone in the other (§11.3).
>
> Most were found by adversarial review or by the property test in
> `crates/artist-logic/tests/soundness.rs`, which is the only part of this
> document that can refute it.

## 0. The commitment this must not break

**Representation is total; evaluation is what is allowed to be partial.**

Every syntactic form **in the fragment this document covers** denotes. The liar
denotes. A sentence about a predicate nobody has defined denotes. A quotation
denotes something other than the term. A claim graded by no probability denotes,
and denotes something distinct from a claim graded `[0,1]`.

**The fragment is stated, because the alternative is a false totality claim.**
Covered: the connectives, quantifiers over sorts, testimony and provenance,
modality over `W`/`A`, determinacy over `P`, credence over `C`, quotation and the
truth predicate. **Not covered, and therefore not part of the semantic language:**
`if-counterfactually`, and the defeasible operators `usually`/`unless`/`prefer`.
§11 gives `M` a rule base `R` and preference order `≺`, and then fails to
interpret them: no fixpoint theorem is available for the combined operator
(§11.3), so there is no valuation to point at. `M` contains no similarity ordering over worlds, so it has
no denotation here — and `Unsupported` is an *evaluation result*, not one. It
remains in the vocabulary and remains evaluable; what it lacks is a
model-theoretic meaning, which is why no kernel rule mentions it (§13).

This forbids defining `⟦·⟧` by recursion on syntax, because a language containing
its own truth predicate has no such recursion. The construction is a **fixpoint**
(Kripke 1975).

## 1. Sentence versus evaluation

| | property of | example |
|---|---|---|
| **semantic** | the sentence, relative to a structure | evidential value, grounding, determinacy, credence |
| **operational** | one attempt to compute it | compute status, traversal completeness |

`ComputeStatus` is **not** semantic. Conflating "I looked everywhere and found
nothing" with "I stopped" is what let reflective operators read semantic axes off
a truncated process; §7 keeps them apart by construction.

## 2. Structures

A **structure** `M` is a tuple `⟨D, T, S, W, A, P, Δ, C, N⟩`.

- **`D`** — the domain. Objects are `ObjectId`s and are their own names: the graph
  is content-addressed, so the term *is* the object. Quotation costs nothing.
- **`T`** — **testimony**, the primitive. Records
  `⟨agent, r, d⃗, polarity, valid, recorded⟩`, where `valid` is an interval of
  world time and `recorded` an instant of transaction time (§8).
- **`S`** — for each sort `σ`, a triple `S(σ) = ⟨ext, enum, complete⟩`:
  - `ext ⊆ D` — the **actual extension**. This is what quantifiers range over.
  - `enum ⊆ ext` — the part any evaluator can enumerate.
  - `complete : 𝔹` — asserts `enum = ext`.

  Revision 1 had only `(members, complete)` and so could not say what
  `∀x ∈ σ. P(x)` *means* when `complete` is false: it quantified over what the
  store happened to list. The quantifier ranges over `ext` always; `enum` is an
  access path, not a meaning. This is what makes the one-sided rules of §10
  sound and the two-sided ones unavailable.
- **`W, A`** — worlds and accessibility. Ordinary Kripke semantics for the modals.
- **`P`** — precisifications (§5).
- **`Δ`** — declarations of totality and their basis.
- **`C`** — a **partial** map from propositions to credal sets (§6).
- **`N`** — the deontic component.
- **`R`, `≺`** — the rule base and its preference order (§11). Absent from earlier
  revisions, which is why defeasibility had no denotation.

Revision 1's `RuleApp` cited a section defining a rule base, defeat relation and
priority order that was never written, and §10 accordingly derives rule
application from ordinary implication, needing none of them. `R` and `≺` are now *present* in `M` and **not interpreted**: §11.3 records that
two fixpoint constructions for the combined operator were proposed and both
refuted, so there is no valuation to point at. No kernel rule depends on them and
none may until there is.

## 3. The evidential axis is Belnap's FOUR

`Open | Supported | Refuted | Conflicted` is Belnap's four-valued logic (Belnap
1977), the bilattice FOUR (Ginsberg 1988; Fitting 1991):

```
Open = N (neither told)   Supported = T   Refuted = F   Conflicted = B (told both)
```

- **truth order `≤_t`**: `F <_t N <_t T`, `F <_t B <_t T`. `∧` is meet, `∨` is
  join, `¬` swaps `T`/`F` and fixes `N`/`B`.
- **knowledge order `≤_k`**: `N <_k T <_k B`, `N <_k F <_k B`.

Both orders are complete lattices. De Morgan, double negation and distributivity
are theorems of the bilattice, not test cases.

**`∧`, `∨` and `¬` are `≤_k`-monotone; `⊃` is not**, and revisions 4–5 claimed it
of every connective. With consequent `F`: `N ≤_k T`, yet `N ⊃ F = T` and
`T ⊃ F = F`, and `T ≰_k F`. Learning more about the antecedent can therefore
*lower* an implication in the knowledge order, so "evaluation only moves upward"
is not available for `⊃`. Nothing below depends on it: §4.2's fixpoint is
monotone in the **valuation** order `⊑` by an argument about total extensions,
which never appeals to connective monotonicity.

**Validity is *always designated*** — value in `{T, B}` — not always `T`. In a
logic where `B` means *told both*, a formula coming out `B` has been established;
demanding `T` everywhere is what empties the tautology set.

`→` is **Arieli–Avron's strong implication**, not material: `a ⊃ b = b` when `a`
is designated, and `T` otherwise. Read materially as `¬a ∨ b`, `⟦P → P⟧` at `N` is
`N` and the logic has essentially no valid formulas — fatal for a language meant
to carry rules. The strong reading gives `P ⊃ P` designated everywhere and a
deduction theorem, while **excluded middle stays invalid** (`N ∨ N = N`), which is
the property the vague-predicate cases depend on. Material implication remains
expressible as `(or (not a) b)`, so nothing is lost.

Note what this does *not* restore: `¬` is the FOUR involution, so `¬¬P = P` and
double-negation elimination is valid — while excluded middle is not. A De Morgan
lattice separates them, and that separation is correct rather than a leak.

## 4. FIVE, and why ⊥ is not N

Let `FIVE = {⊥} ∪ FOUR`, with `⊥` strictly below all four in `≤_k`.

`⊥` means **the construction has not reached this node**; `N` means the structure
was consulted and said neither. An atom nobody has mentioned is `N` and
**grounded**. The liar is `⊥`.

### 4.1 There is no lifting of the connectives to FIVE

Revisions 1 and 2 both tried to make `FIVE` a lattice and take fixpoints in it.
Revision 1 asserted `⊥ ∨ T = T` without defining the lifting at all. Revision 2
defined it as *exact agreement over completions* — `f(x⃗) = v` when every way of
replacing `⊥` by a FOUR value gives `v`, else `⊥` — and claimed `≤_k`-monotonicity.

**That lemma is false, and two independent counterexamples say so:**

```
or(⊥, T) = T          or(T, ⊥) = T
or(⊥, B) = ⊥          or(B, ⊥) = ⊥      completions give {T,B}
```

In both, `T ≤_k B` while the results go `T` and `⊥`, so monotonicity would demand
`T ≤_k ⊥`. Moving an argument *within* FOUR while another stays `⊥` changes the
completion set without shrinking it, and `B` is what makes the completions
disagree. **Knaster–Tarski over FIVE is not available, and no repair of the
lifting is attempted** — §4.2 does not need one.

### 4.2 The fixpoint is over partial valuations

The repair is Kripke's own construction, which never lifts the truth values at
all. `⊥` is not a fifth value; it is the **absence of a value**.

A valuation is a **partial** function `v : Nodes ⇀ FOUR`, and the order is
**information extension**: `v ⊑ v'` iff `v'` agrees with `v` wherever `v` is
defined and may be defined in more places. Partial functions under `⊑` form a
cpo with least element `∅`.

`Φ_M(v)(n)` is defined, with value `c`, iff **every total extension of `v` gives
`n` the value `c`** under §3's FOUR operations; otherwise `Φ_M(v)(n)` is
undefined. Write `⟦n⟧_M = ⊥` for "undefined in the least fixpoint".

**Lemma (monotonicity).** `v ⊑ v'` implies `Φ_M(v) ⊑ Φ_M(v')`. *Proof.* Suppose
`Φ_M(v)(n) = c`. Every total extension of `v'` is a total extension of `v`, since
`v'` only fixes values `v` left open and agrees elsewhere. So every total
extension of `v'` gives `c`, hence `Φ_M(v')(n) = c`. ∎

The earlier counterexample cannot arise here: `or(⊥,T)` versus `or(⊥,B)` compares
two valuations that **disagree** on an assigned value, which is not a `⊑`-increase
at all. The order is on *how much is known*, not on *which of FOUR is held*, and
that distinction is exactly what revision 2 collapsed.

`Φ_M` monotone on a cpo with bottom has a least fixpoint by iteration from `∅`
through the ordinals (Knaster–Tarski/Kripke); continuity is not needed. Worked
values are unchanged where they were right — `⊥ ∨ T = T` because every total
extension gives `T`, and `⊥ ∨ F` stays undefined because they differ.

### 4.3 Grounding

`Φ_M` reads `v` at recursive occurrences and `T`/`S`/`W`/`A` at the leaves. By
§4.2 it is monotone, so it has a least fixpoint `lfp(Φ_M)` by ordinal iteration
from `∅`. Write `⟦n⟧_M = lfp(Φ_M)(n)`, undefined written `⊥`, and `Fix(M)` for the
**set** of its fixpoints.

`Fix(M)` is *not* a complete lattice, and revision 4 said it was. The truth-teller
has fixpoints assigning `T` and assigning `F`; under `⊑` those are incompatible
and have no common upper bound. A least fixpoint exists because `⊑` is a cpo with
bottom; a lattice of fixpoints does not, and nothing below needs one — the
definitions quantify over `Fix(M)` as a set.

- `Grounded(n)` iff `⟦n⟧_M ≠ ⊥`.
- `StableLoop(n)` iff `⟦n⟧_M = ⊥` and **some** fixpoint assigns `n` a classical
  value. The truth-teller `τ ↔ τ` qualifies (Kripke's non-intrinsic fixpoints).
- `Oscillatory(n)` iff `⟦n⟧_M = ⊥` and **no** fixpoint does. The liar qualifies.

> **A cycle is not ungroundedness.** `P ↔ P ∨ ⊤` cycles and grounds to `T`: by
> §4.1, `⊥ ∨ T = T` in one step. Deciding this in general requires a **closed
> unfounded set** — a set of nodes every route out of which stays inside the set —
> and verifying closure, not walking a loop. §10's rule covers a decidable
> fragment and nothing else.

## 5. Determinacy is supervaluation

Let `⟦n⟧_p` be `n`'s value in precisification `p ∈ P`.

- **`Total`** — all `p` agree on a classical value.
- **`Indeterminate`** — the `p` disagree and `Δ` records the disagreement as
  constitutive. There is no fact of the matter.
- **`Underspecified`** — the `p` disagree and `Δ` records that a sharp condition
  exists but is unwritten.
- **`Unknown`** — `Δ` says nothing.

`DeterminacyBasis` records why: `Declared`, `Derived`, or `Presumed` by a query
that assumed totality to proceed.

**Correction.** Revision 1 called `Underspecified` "the denotation of
`Bound::Partial`". That is a type error: determinacy is a semantic state of the
sentence, `Bound` is an approximation level on one side of an evaluation. They are
unrelated axes and neither denotes the other. `Bound` is defined in §7.2.

## 6. Credence is a credal set

`C(n)`, where defined, is a closed convex set of finitely additive probability
measures (Walley 1991; Levi 1980); `Credence { lo, hi }` is its envelope in
log-odds.

- **`C(n)` undefined ≠ `C(n)` vacuous** (Heifetz–Meier–Schipper).
- **Compounds** take the Fréchet–Hoeffding envelope: no independence assumption,
  because none was made.
- **Pooling** independent evidence about one proposition is log-odds addition, a
  different operation, and must be associative, commutative and order-independent
  — the store is content-addressed and replication order is not fixed. Clamping
  inside the fold breaks associativity; the envelope is clamped once, at the end.

Credence is **not checked by the kernel** and may not appear inside a checked
judgment (§9).

## 7. Evaluation is approximation

### 7.1 Conditional soundness

Revision 1 claimed every checked result bounds `⟦n⟧_M` in **every** structure.
That is false for any kernel with a trust point: `Told` accepts testimony it
cannot verify, and recording an assumption does not make it true. A certificate
therefore proves a **conditional** judgment, exactly as LCF theorems are sequents.

> **Γ ⊨ n.** A derivation carries a hypothesis set `Γ` — the testimony leaves it
> used. Its bounds are claimed only over structures `M` **satisfying every
> hypothesis in Γ**.

> **Soundness.** For every `M ⊨ Γ`: `⟦n⟧_M ∈ allowed(support, refutation)`.

Stated as **membership in an abstract domain**, not as `must ≤_k value ≤_k may`.
The latter is ill-typed — `may` is a *set* like `{T,B}`, and `⊑`/`≤_k` do not
relate a value to a set. The abstract domain is the four sets §7.2 tabulates,
ordered by inclusion; `must` is recoverable as the `≤_k`-greatest lower bound of
the allowed set, and is a convenience, not the statement.

Unconditional judgments are the special case `Γ = ∅`: every rule in the
derivation consulted nothing. `Axiom` and `Ungrounded` introduce nothing, and
every other rule contributes only what its premises carry — so `Negation` over an
axiom, a `Connective` over axioms, an `Instantiate` over an unconditional
universal, and any composition of these are unconditional too.

**This said "`Axiom` alone" for two revisions**, in three places, and it was
false each time. Unconditionality is a property of `Γ`, not a privilege granted
to one rule, and the distinction matters because §8.5 reads an empty `Γ` as a
*claim* — that the judgment holds in every structure — rather than as a missing
attribution.

### 7.1a REFUTED: the bounds and the denotation disagree

> **`Instance` and `Instantiate` are unsound, `Connective` needs a side
> condition it does not have, and the strict kernel is therefore *not* green.**
> Adversarial review, with countermodels, reproduced live below.

The cause is one mismatch. `⟦·⟧` assigns **one complete FOUR value or none**: a
node is defined only when every total extension agrees on the *whole* value. The
bounds reason about the two evidence bits **independently**. So a support bit can
be fixed across every completion while the complete value varies between `T` and
`B` — the semantics then says `⊥`, and the certificate says `Certain`, which
excludes `⊥`.

Write `c⁺`, `c⁻` for the evidence bits: `N=(0,0)`, `T=(1,0)`, `F=(0,1)`,
`B=(1,1)`.

**Connective.** Let `⟦P⟧ = B` and `Q` be a truth-teller, `⟦Q⟧ = ⊥`.
`B ∨ T = T` and `B ∨ F = B`, so the completions disagree and `⟦P ∨ Q⟧ = ⊥` —
while `P` has certain support. Dually `B ∧ T = B`, `B ∧ F = F`, so
`⟦P ∧ Q⟧ = ⊥` while `P` has certain refutation. **A short-circuiting
`Connective` is unsound**, and the implementation short-circuits:

```
(or  B Q) : Supported  StableLoop      (and B Q) : Refuted  StableLoop
```

Note those results are internally contradictory by §7.2's own reading: `Certain`
asserts `⟦n⟧ ∈ {T,B}`, and `StableLoop` asserts `⟦n⟧ = ⊥`.

**Instance.** `ext(σ) = {a,b}`, `⟦P(a)⟧ = B`, `P(b)` a truth-teller. The
existential's completions give `T` and `B`; they disagree, so it is `⊥` — while
the witness has certain support. The universal's give `B` and `F`, so it too is
`⊥` — while the counterexample has certain refutation. Domain membership repairs
neither.

**Instantiate.** Let `P(a) = Q ∨ B₀` with `Q` undefined and `⟦B₀⟧ = B`. Every
completion of `P(a)` is designated (`T` or `B`) yet they disagree, so
`⟦P(a)⟧ = ⊥`. Put `P(b) = B₀`. Then `T ∧ B = B` and `B ∧ B = B`: all completions
agree, so `⟦∀x ∈ σ. P(x)⟧ = B`, grounded and designated — while the instance
`P(a)` is `⊥`. "A designated meet forces every instance designated" holds of
*bits*, not of complete values, and the lemma stated the latter.

### 7.1b The repair: make the semantics as partial as the judgment

Two repairs are available. **The second is chosen**, and the reason is that the
first patches the rules to fit a semantics that does not fit the machine, while
the second removes the mismatch.

1. *Keep the exact-value semantics; strengthen the rules.* Add an exact-value or
   conflict-freedom judgment; `Instance` may use only an exact `T` witness or
   exact `F` counterexample, `Instantiate` requires the universal exactly `T`,
   and `Connective` must verify completion-constancy. Sound, and it makes the
   common cases uncertifiable — every one-sided rule now needs two-sided
   information.
2. **Give `⟦·⟧` independently partial components.** A denotation is a pair
   `⟨t, f⟩` with each bit in `{0, 1, ⊥}`, each the least fixpoint of its own
   Kleene-monotone clauses:

   ```
   ¬⟨t,f⟩ = ⟨f,t⟩       ⋀ᵢ⟨tᵢ,fᵢ⟩ = ⟨⋀tᵢ, ⋁fᵢ⟩       ⋁ᵢ⟨tᵢ,fᵢ⟩ = ⟨⋁tᵢ, ⋀fᵢ⟩
   (a ⊃ b)⁺ = ¬a⁺ ∨ b⁺                                (a ⊃ b)⁻ = a⁺ ∧ b⁻
   ```

   "Support established while refutation is unresolved" becomes a **genuine
   semantic value** rather than collapsing to `⊥`. Every countermodel above
   dissolves: `(or B Q)` is `⟨1, ⊥⟩` — supported, refutation open — so the
   evaluator's answer is *correct* rather than merely reported. `Instance` and
   `Instantiate` recover as stated, because they were always claims about bits.

   `Grounded` becomes "both bits defined"; `support = Certain` asserts `t = 1`
   and says nothing about `f`, so the contradictory pair above is no longer
   expressible.

**What it costs.** `Bound` must gain a state. All four FOUR values are already
distinguishable as pairs — `T = (Certain, None)`, `F = (None, Certain)`,
`N = (None, None)`, `B = (Certain, Certain)` — but `Bound::None` conflates *the
bit is 0* with *the bit is unknown*. `F`'s support bit is an established zero;
a truth-teller's is undefined; both report `None`. Every clause reading `¬a⁺`
needs the difference, so this is a judgment-type change and not a re-derivation.

**What it buys, beyond the refutation.** Working it through, the repair pays for
itself three times over — which is the strongest evidence available that it is
the right branch rather than the convenient one:

- `Instance`, `Instantiate` and `Connective` recover **as stated**, because all
  three were always claims about bits and only the semantics disagreed.
- **The false-antecedent row returns.** `(a ⊃ b)⁺ = ¬a⁺ ∨ b⁺`, so a known-zero
  support bit supports the implication outright. That row was given up when the
  strong implication landed, precisely because `refutation: Certain` admits the
  designated `B` — the repair recovers it instead of trading it away for good.
- **It is the precondition `defeasibility.md` §6 named.** `⟦E⟧ ∈ {N,F}` — *E
  fails* — becomes expressible, and a default defeated by a *conflicted*
  exception is exactly the case that separates it from `⟦E⟧ ∈ {F,B}`.

**And it is the same move AFT makes** (`defeasibility.md` §3): a pair of partial
components under a precision order, which is the approximation bilattice. The
strict repair and the defeasible construction have converged on one structure.
That reopens §7 of that document — Design B was preferred because it kept layer 0
frozen and avoided AFT's cost, and layer 0 is now moving to AFT's structure
anyway. **Sequence: repair layer 0, re-decide, then defeasibility.**

**Not yet written**, and it is the next thing: the per-bit clauses for every
operator, the per-bit fixpoint and its monotonicity proof, what `Grounding`
becomes when one bit is settled and the other is not, and the nine lemmas
re-derived. None of that is code, and none of it should be until reviewed — the
last three rules that shipped on my own reading did not survive someone else's.


### 7.1c The construction, worked

Not code, and not yet reviewed. Written out so it can be attacked.

**Values.** Let `𝟛 = {0, 1, u}` be the flat domain: `u ⊑ 0`, `u ⊑ 1`, and `0`,
`1` incomparable. A denotation is a pair

```
⟦n⟧_M = ⟨t, f⟩ ∈ 𝟛 × 𝟛
```

— nine values, of which four are the old FOUR (`N=⟨0,0⟩`, `T=⟨1,0⟩`, `F=⟨0,1⟩`,
`B=⟨1,1⟩`) and five are genuinely partial. `⟨u,u⟩` is the old `⊥`. `⟨1,u⟩` —
*supported, refutation unresolved* — is the value that did not exist and whose
absence refuted three rules.

**Clauses.** With `∧`, `∨`, `¬` the Kleene operations on `𝟛` (`∧` is `0` if any
operand is `0`, `1` if all are `1`, else `u`; `∨` dually; `¬` swaps `0` and `1`
and fixes `u`):

```
¬⟨t,f⟩        = ⟨f, t⟩
⋀ᵢ ⟨tᵢ,fᵢ⟩    = ⟨ ⋀ᵢ tᵢ , ⋁ᵢ fᵢ ⟩          — and, ∀ over ext
⋁ᵢ ⟨tᵢ,fᵢ⟩    = ⟨ ⋁ᵢ tᵢ , ⋀ᵢ fᵢ ⟩          — or, ∃ over ext
a ⊃ b         = ⟨ ¬a⁺ ∨ b⁺ , a⁺ ∧ b⁻ ⟩
```

Leaves take `R_{v,τ}` (§8.2), which always yields a *defined* FOUR value — which
is why an atom nobody has mentioned is `N = ⟨0,0⟩` and **grounded**, the
distinction §4 needed a fifth value to make and now gets from the base case.

**Lemma (monotonicity).** Every clause is `⊑`-monotone, because each Kleene
operation is monotone on the flat domain: raising an argument from `u` can only
settle a result, never unsettle or change a settled one. Hence `Φ_M` is monotone
on the product cpo of valuations, and `lfp(Φ_M)` exists by ordinal iteration from
the all-`u` valuation. ∎

This is markedly better than §4.2's construction, which quantified over *total
extensions* and needed an argument about extension sets. Here monotonicity is
immediate and per-clause, which is also why it is harder to get wrong.

**Grounding, per bit.** `Grounded(n)` iff both bits are defined in `lfp`. When a
bit is undefined, classify it as §4.3 does — `StableLoop` when some fixpoint
settles it classically, `Oscillatory` when none does. The liar gives
`⟨t,f⟩ = ⟨f,t⟩`, so `t = f`: fixpoints exist at `⟨0,0⟩` and `⟨1,1⟩` but none at
`⟨1,0⟩` or `⟨0,1⟩`, so it is `Oscillatory`. The truth-teller gives `⟦Q⟧ = ⟦Q⟧`,
satisfied by every value, so `StableLoop`. Both classifications survive.

**`Bound` gains a state.** Each side of a judgment must say which of three things
it knows about its bit:

| `Bound` | asserts | |
|---|---|---|
| `Certain` | the bit is `1` | |
| `Excluded` | the bit is `0` | **new** |
| `None` | nothing | the bit is unknown |
| `Partial` | nothing | search state, as before |

`None` currently means both *known zero* and *unknown*, which is the whole
defect: `F`'s support bit is an established `0`, a truth-teller's is `u`, and
every clause reading `¬a⁺` needs to tell them apart.

**The nine lemmas, re-derived.** Each is now a statement about one bit, which is
what they always were:

| rule | obligation | holds because |
|---|---|---|
| `Axiom` | `⟦#true⟧ = ⟨1,0⟩` | by definition |
| `Told` | leaf bits defined by `R_{v,τ}` | §8.2, no recursion |
| `Negation` | bits swap | the clause |
| `Connective` | `∧`: all `t=1 ⟹ t=1`; any `f=1 ⟹ f=1`. `∨` dual | Kleene `⋀`/`⋁`; **short-circuit is sound**, since `⋁` is `1` as soon as one operand is |
| `Instance` | `∃`: one `t=1 ⟹ t=1`. `∀`: one `f=1 ⟹ f=1` | same, over `ext ⊇ enum` |
| `Exhaustive` | `∀`: all `t=1 ⟹ t=1`, needs `enum = ext` | `⋀` over the whole domain |
| `ModusPonens` | `(a⊃c)⁺=1 ∧ a⁺=1 ⟹ c⁺=1` | `¬1 ∨ c⁺ = 0 ∨ c⁺` |
| `Instantiate` | `∀`'s `t=1 ⟹ every instance `t=1` | `⋀` is `1` only if all are |
| `Ungrounded` | per-bit fixpoint classification | above |

`Instance` and `Instantiate` are sound again, and `Connective` needs no side
condition — the countermodels all turned on the *complete value* being undefined
while a bit was settled, and a settled bit is now a value the semantics can hold.

**What this does not do.** It does not make `⟦E⟧ ∈ {N,F}` free: that is
`E⁺ = 0`, and establishing it still requires the evaluator to *derive* a zero
rather than fail to derive a one. `Excluded` makes it expressible; the resolver
must still supply it, and for a closed predicate it does.

**Open, and the reason this is not code.** Whether `Determinacy` and `Credence`
compose per-bit or per-value is unexamined. `Conflicted` as an *evidential*
report is `⟨1,1⟩`, but `Evidential` is currently derived from two `Bound`s and
that derivation needs restating over four states rather than three. And the
`Excluded` state has to reach the resolvers, which is where the `Knowledge`
enum's `Fails` already lives — so the plumbing may be shorter than it looks, or
may not.

### 7.2 What a `(support, refutation)` pair asserts

`Bound = None | Partial | Certain` on each side, giving nine pairs. Only the
`Certain` cells constrain `M`:

**The domain is `FIVE`, not `FOUR`.** Revisions 4–5 tabulated allowed sets over
`FOUR` alone, so an `Ungrounded` certificate — which establishes `⟦n⟧ = ⊥` — had
no cell it could land in and the theorem could not state what that rule proves.
Undefinedness is a possible value of `⟦·⟧`, so it must be a possible member.

- `support = Certain` ⟹ `⟦n⟧ ∈ {T, B}`
- `refutation = Certain` ⟹ `⟦n⟧ ∈ {F, B}`
- `None` ⟹ no constraint — and *no constraint* includes `⊥`. An unestablished
  support is not a denial, and it is not a claim that the sentence is grounded
  either.
- `Partial` ⟹ **no constraint.** It reports the search, not the structure.
- A `grounding` of `StableLoop` or `Oscillatory` ⟹ `⟦n⟧ = ⊥`, and this is the
  *only* way to establish that, which is why grounding is a component of the
  judgment rather than a decoration on it.

Hence:

| support \ refutation | None / Partial | Certain |
|---|---|---|
| **None / Partial** | `allowed = FIVE` | `allowed = {F,B}` |
| **Certain** | `allowed = {T,B}` | `allowed = {B}` |

and independently, `grounding ≠ Grounded` ⟹ `allowed = {⊥}`. Both bounds
`Certain` together with a non-`Grounded` grounding is unsatisfiable, and a
checker producing it has proved a contradiction — which is a rejectable state,
not a value.

`Partial` asserting nothing is deliberate: it makes every `Partial` cell
trivially sound, so a rule that can only reach `Partial` is one that has told the
reader it established nothing about `M`. Widening is always permitted; narrowing
past the truth never is.

### 7.3 Operational axes

`ComputeStatus ∈ {Exact, BudgetExhausted, Unsupported, Stalled}` says why the
interval is as wide as it is. `traversal_complete : 𝔹` says whether the recursion
bottomed out or was cut by depth, width, an unenumerable domain, or a missing
rule. **Two fields, because they are two facts.** `Stalled ∧ traversal_complete`
means the structure was consulted everywhere and said nothing — semantic axes are
readable. `Stalled ∧ ¬traversal_complete` means the process stopped — they are
not.

### 7.4 Reflection

`(grounded P)`, `(determinate P)`, `(defeated-by P D)`, `(likely P)` and the rest
denote facts *about* `P`, so their own axes are those of the report. A report
about an ungrounded sentence is itself grounded. They are evaluated in the
metalanguage against `⟦·⟧_M`, which is why they must not inherit the accumulator.

## 8. Provenance

Provenance is a function of the same data, at the same instant, under the same
equivalence as the verdict.

### 8.1 Time, defined

Two clocks, and revision 1 used both without defining either.

- **Valid time** — the interval of world time a record claims about. A record's
  `valid` is `[from, to)`, with `to = ∞` for open-ended claims.
- **Transaction time** — `recorded`, when the store learned it.
- **Retraction** — a later record by the same agent for the same
  `(r, d⃗, polarity)` closing the earlier record's valid interval. A retraction is
  a claim about the *record*, not about the world, and so contributes no
  testimony of its own.
- **Live at `⟨v, τ⟩`** — record `x` is live iff `v ∈ x.valid` **and**
  `x.recorded ≤ τ` **and** no retraction of `x` has `recorded ≤ τ`.

**Two indices, not one.** Revision 2 wrote `R_t`, `att_t` and "live at `t`" with a
single parameter while defining two clocks, which cannot express "what did we
believe on Tuesday about Monday" — the query every audit trail exists to answer.
Everything below is indexed `⟨v, τ⟩`: `v` selects which slice of the world is
being asked about, `τ` which state of the store is doing the answering. Write
`R_{v,τ}`, `att_{v,τ}`, `~_{v,τ}`. Verdict and attribution read the *same pair*
and the *same liveness test*, which is what makes them agree by construction
rather than by matching guards.

### 8.2 The derived extension

```
aff_{v,τ}(r,d⃗) = { a : ⟨a,r,d⃗,+,·,·⟩ ∈ T live at ⟨v,τ⟩ }   den_{v,τ} = likewise with −
R_{v,τ}(r,d⃗) = T if aff≠∅ ∧ den=∅ · F if den≠∅ ∧ aff=∅ · B if both · N if neither
```

### 8.3 Coreference

A merge `⟨agent, is, (a,b), +, ·, ·⟩` is ordinary testimony. `~_{v,τ}` is the
equivalence on `D` generated by merges live at `⟨v,τ⟩`; propositions are evaluated at
their class, so aliases are the same proposition and must give the same verdict
*and* the same sources.

### 8.4 Attribution

```
att_{v,τ}(n) = aff if R_{v,τ}(n)=T · den if F · aff∪den if B · ∅ if N
             ∪ witnesses_{v,τ}(n)
```

over the whole class `[d⃗]_{v,τ}`, where `witnesses_{v,τ}(n)` is the agents whose
merges were used to reach it. Polarity-correctness, dating, retraction-awareness and
coreference-honesty all follow from the definition rather than from guards.

**Agents are objects, and naming must be injective**: two distinct provenance
names must not denote one object. Routing names through a graph with the reserved
vocabulary pre-interned violates this, and the violation is semantic — a reader
resolves an authority and gets a logical connective.

### 8.5 What a certificate must report

Revision 1 offered `authorities ⊆ att_t(n)` and `assumed ⊇ assumptions(n)`, which
Sol observed is satisfied by reporting nothing at all when a conclusion rests
entirely on verified testimony. The conditional reading of §7.1 supplies the
missing object: `Γ` is the set of testimony leaves the derivation actually used.

```
authorities = ⋃ { att_{v,τ}(h) : h ∈ Γ, att_{v,τ}(h) ≠ ∅ }   — equality, not ⊆
assumed     = { h ∈ Γ : att_{v,τ}(h) = ∅ }                   — equality, not ⊇
```

Empty `authorities` **and** empty `assumed` therefore means `Γ = ∅`: the judgment
is **unconditional**, holding in every structure. `Axiom` is the only *rule* that
introduces nothing into `Γ`, but it is not the only derivation that ends with `Γ`
empty — `Negation` over an axiom, a `Connective` over axioms, `Ungrounded`, and
any composition of these are unconditional too, which is as it should be: they
consult no testimony. What is rejectable by inspection is a derivation reporting
`Γ = ∅` while containing a `Told` step.

## 9. What the kernel may do

LCF (Milner 1972): judgments are **constructed** only through primitives that
preserve correctness. No field is believed — including the axis fields.

- `determinacy` starts at `Unknown`. Totality is `Δ`'s business and the kernel has
  no `Δ`. Revision 1 seeded `Total` in a helper, so every connective, instance and
  exhaustive step asserted a sharp condition it had never seen.
- **Information the kernel cannot check must not enter the judgment.** Credence
  and modal evaluation may travel *beside* a certificate as explicitly unchecked
  metadata, never inside it.

**Obligation.** Every rule needs a lemma: if each premise's judgment satisfies
§7.1 for its node under `Γᵢ`, the conclusion satisfies it for its node under
`⋃Γᵢ` plus whatever the rule itself assumes.

## 10. The rules

| rule | Γ contributed | rests on |
|---|---|---|
| `Axiom` | `∅` | `⟦#true⟧ = T`, `⟦#false⟧ = F` in every `M` |
| `Told` | `{the record}` | §8.2 — the trust point |
| `Negation` | premise's | §3, the FOUR involution |
| `Connective` | premises' | §3, meet and join **by position** |
| `Instance` | premise's | §4.2, over `ext`, one-sided only |
| `Exhaustive` | premises' | §4.2, needs `complete` |
| `ModusPonens` | premises' | §3, `→` is the strong implication |
| `Instantiate` | premise's | §4.3, a designated meet forces every instance |
| `Ungrounded` | `∅` | §4.3, reference fragment only |

Throughout, `⊥` is **undefinedness** — the absence of a value in a partial
valuation (§4.2) — and never the false truth atom, which is written `#false`.
Revision 5 used the one glyph for both in `Axiom`'s lemma.

**`Axiom` is restored and is not a special case of `Tautology`.** Revision 1
subsumed it, having enumerated *Boolean* valuations — but this semantics is
Belnap's, and a classical tautology need not be `T` in FOUR: `P ∨ ¬P` at `N` is
`N`, and `P → P` at `N` is `N`. Enumerating Boolean assignments certified formulas
that are not valid here. A `Tautology` rule for this logic must enumerate **FOUR**
valuations, and the set it then licenses is nearly empty — so the rule is
**withdrawn**, and `#true`/`#false` are handled by `Axiom`, whose lemma is immediate. The
`constructive` flag goes with it: it was distinguishing two classical notions
inside a semantics where neither applies.

**`RuleApp` is replaced by `ModusPonens`, and needs no rule base.** Revision 1
cited a section defining rule bases, defeat and priorities that does not exist. It
is not needed: a stored rule is an ordinary universally quantified implication, so
from a premise establishing `⟦∀x⃗. A → C⟧ ≥_t T`, an `Instance` of it, and a
premise establishing the antecedent, the consequent follows by §3 alone. The
rule's own truth is a *premise*, not a citation — which is also what puts the rule
into `Γ`, so a reader who rejects the rule rejects the conclusion.

**Defeasible rules are removed from the kernel entirely.** Limiting them to
`Partial` was not a fix: by §7.2 `Partial` asserts nothing, so a "defeasible
conclusion" carries no information at all, and pretending otherwise was
borrowing against the unproved theorem in §12. Until that theorem exists, a
defeasible conclusion is not certifiable.

**`Instance` is one-sided and `Exhaustive` is two-sided.** Over any domain, a
witness supports an existential and a counterexample refutes a universal, because
both quantify over `ext` and a witness in `enum ⊆ ext` is a witness in `ext`.
Exhaustive support for a universal or exhaustive refutation for an existential
requires `enum = ext`, i.e. `complete`. This is why §7.2 keeps two independent
bounds: collapsing them would lose the interval that makes a partial answer
honest.

**`Ungrounded` covers a decidable fragment.** Loops through the single-operand
reference operators — `not`, `holds`, `quote` — returning to their origin. There
the loop is exactly `v(n) = ¬ᵏ v(n)`: odd `k` admits no classical fixpoint
(`Oscillatory`), even `k` admits two (`StableLoop`), and parity is a proof. Off
that fragment it proves nothing (§4.2) and the rule refuses.

Two obligations from §0 rather than from soundness: the checker must **terminate
on cyclic graphs**, since self-reference is a feature, so it memoises rather than
recursing naively; and it must be **small enough to read**.

## 11. Defeasibility

The largest hole relative to what this system is *for*: an agent's memory is
mostly preferences and defaults. Earlier revisions left it out of `M` entirely
and recorded the combined fixpoint as undefined. It is definable, and the
combination is not novel — Dung 1995 proved the correspondence this section
rests on.

### 11.1 What the structure gains

`M` gains **`R`**, a rule base: a set of rules `∀x⃗. A₁ ∧ … ∧ Aₙ ⇒ C`, each marked
*strict* or *defeasible*, together with a strict partial order `≺` on the
defeasible ones (preference; `r ≺ r'` reads "r' takes precedence").

Strict rules need nothing new — they are the implications §10's `ModusPonens`
already handles. Everything below concerns the defeasible ones.

### 11.2 Arguments and attack

An **argument** is a derivation tree whose leaves are testimony and whose steps
are rules. Write `Def(a)` for the defeasible rules used in `a`, and `Conc(a)` for
its conclusion. Following ASPIC+, `b` **attacks** `a` when:

- **rebuts**: `Conc(b)` is `¬Conc(a')` for some sub-argument `a'` of `a` whose
  last step is defeasible, and not `b ≺ a'`;
- **undercuts**: `Conc(b)` says a defeasible rule used in `a` does not apply
  here — this is what `(unless E P)` expresses, and why `E` must be nameable;
- **undermines**: `Conc(b)` contradicts a defeasible leaf of `a`.

`Att ⊆ Args × Args` is the resulting relation. This is `defeated_by` given a
definition: the set the evaluator already computes is `{Conc(b) : b Att a}`.

### 11.3 There is no fixpoint theorem for the combined operator

Two revisions have now claimed one, and both were wrong. The claims and their
refutations, because the pattern is the point:

- **Revision 4** paired `Φ_M^S` with Dung's `F_M^v` and asserted componentwise
  monotonicity. `F_M^v` is not monotone in `v`: an argument with no *established*
  attackers is accepted, and extending `v` so an attacker becomes established
  rejects it. More information shrinks acceptance.
- **Revision 5** relabelled that as *antitone* and invoked "the square of an
  antitone operator is monotone". **That was word substitution, not
  mathematics.** `Φ_M^S` maps a `T` to a `B` when `S` grows a refuter, and `T`
  and `B` are **incomparable** under `⊑` — neither above nor below. An operator
  whose outputs are incomparable is not order-reversing, so the alternating
  fixpoint theorem does not apply to this `Ψ`.

Van Gelder's construction works because its operator is genuinely antitone on a
lattice where the relevant values are comparable. Ours is not, and no
reformulation attempted so far makes it so: the difficulty is that `⊑` orders
*how much is known* while defeat needs to **withdraw** what was known, and `⊑`
has no room for withdrawal by construction.

**Consequence, stated rather than worked around: `usually`, `unless` and `prefer`
have no denotation.** `R` and `≺` are in `M` and no clause interprets them. This
is the honest state, and §13 carries it as the open problem it is.

**No kernel rule may mention them.** Revision 5 shipped a `Step::Defeasible` for
`(unless E P)` on the strength of §11.3's invalid proof; it has been removed. It
had two further holes worth recording so a future attempt does not repeat them:

- **The named exception is not the attacker set.** §11.2 admits rebutters of `P`
  and underminers of `P`'s premises, so reading `E` off the node and calling the
  set complete was the `Exhaustive` `complete`-flag defect wearing a new hat.
- **"`E` is refuted" is not "`E` fails".** `refutation: Certain` asserts
  `⟦E⟧ ∈ {F,B}` (§7.2), and `B` is *designated* — a conflicted exception holds,
  and would have been treated as defeated. Certifying a default needs
  `⟦E⟧ ∈ {N,F}`, which the two-bound judgment cannot express at all. That is a
  judgment-type problem, not a missing check.

## 12. What this buys

| was a bug | becomes |
|---|---|
| `Stalled` conflating absence with truncation | two fields, §7.3 |
| `(grounded ·)` confident about an unvisited liar | violates §7.1, statable |
| an unmentioned atom looking ungrounded | `N` vs `⊥`, §4 |
| bilattice edge cases by hand | theorems of FOUR, §3 |
| `Exhaustive` trusting `complete` | no lemma exists for it, §10 |
| `credence_of` order-dependence | associativity is an obligation, §6 |
| a denier listed as an authority | `att_{v,τ}` selects by verdict, §8.4 |
| a retractor still vouching | one `⟨v,τ⟩`, one liveness test, §8.1 |
| coreference relocating provenance | evaluation at the class, §8.3 |
| empty `authorities` and empty `assumed` | means `Γ = ∅`, §8.5 |
| classical tautologies certified | FOUR validity, §10 |

§7.1's inequality is checkable over randomly generated structures, which is a
property test rather than another table of examples.

## 13. Open

- **General ungroundedness.** Needs a closed-unfounded-set witness and a closure
  checker. Tractable; not written.
- **Multi-slot instantiation** remains the binder gap: `Instantiate` and
  `Instance` substitute one variable, so `∃x,y ∈ σ. R(x,y)` is evaluable and not
  certifiable.
- **Counterfactuals.** No settled similarity ordering. Pick one and name it here,
  or return `Unsupported`.
- **Credence under quantifiers.** §6 defines the compound's credal set; the
  evaluator cannot reach instances under a binder, so the definition is ahead of
  the code.

## References

Belnap 1977; Ginsberg 1988; Fitting 1991 · Kripke 1975; Gupta & Belnap 1993 · van
Fraassen 1966; Fine 1975 · Dung 1995; ASPIC+ (Prakken) · Walley 1991; Levi
1980; Fréchet–Hoeffding · Milner 1972; de Bruijn · Snodgrass (valid vs
transaction time) · Wang, NARS.
