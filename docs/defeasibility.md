# Defeasibility: two candidate constructions, and the cases that decide

> **Design document. No implementation.** Nothing here may reach the kernel until
> one construction survives review, and the review standard is the battery in §5
> — not a proof sketch, which is what the last two attempts were.
>
> Baseline: `strict-kernel-v1`. The strict kernel is frozen and is **layer 0** in
> everything below. It is not modified while this is open, unless a concrete
> counterexample breaks it.

## 1. What has already been refuted

Two constructions were proposed in `semantics.md` and both were wrong. They are
recorded here because each failure narrows the design space, and because the
*way* they were wrong is the same way a third would be.

**Attempt 1 — componentwise monotone product.** `Ψ(v,S) = ⟨Φ^S(v), F^v(S)⟩` on
partial valuations × argument sets, claimed monotone in both. `F^v` is not
monotone in `v`: an argument with no *established* attackers is accepted, and
extending `v` so an attacker becomes established rejects it. More information
shrinks acceptance.

**Attempt 2 — "it is antitone, so square it".** Relabelled the above and invoked
Van Gelder's alternating fixpoint. `Φ^S` maps a `T` to a `B` when `S` gains a
refuter, and **`T` and `B` are incomparable under `⊑`** — neither above nor
below. An operator whose outputs are incomparable is not order-reversing, so the
theorem does not apply. This was word substitution.

**The lesson both share.** `⊑` orders *how much is known* and has no room for
**withdrawal**. Defeat withdraws. Any construction that keeps a single valuation
ordered by information extension and expects defeat to act on it will fail the
same way. So the two live options are: change what is ordered (§3), or arrange
that nothing is ever withdrawn (§4).

## 2. Requirements

A construction ships only with all five.

1. **A denotation for `usually`, `unless` and `prefer`.** Currently `R` and `≺`
   are in `M` and no clause interprets them; §11.3 says so.
2. **A mathematically valid order/approximation construction**, or a cleanly
   layered argumentation semantics over the frozen strict base.
3. **A theorem** establishing the selected sceptical semantics — existence and
   uniqueness of the intended fixpoint, stated and proved, not gestured at.
4. **A certificate form that verifies attacker completeness** rather than
   trusting it. A step carrying its own attacker list is `Exhaustive`'s
   `complete` flag again.
5. **The §5 battery**, adversarially.

## 3. Design A — Approximation Fixpoint Theory

Denecker, Marek and Truszczyński. Built for precisely this: deriving
well-founded semantics for **nonmonotone** operators, via approximation
bilattices.

**The move.** Do not fix the nonmonotone operator `O`. Replace it with an
**approximator** `A : L² → L²` on pairs, where `(x,y)` denotes the interval
`[x,y]` and the **precision order** is `(x,y) ≤_p (x',y')` iff `x ≤ x'` and
`y' ≤ y` — more precise means a narrower interval. `A` must be `≤_p`-monotone
and exact on consistent pairs (`A(x,x) = (O(x), O(x))`).

The point: `≤_p`-monotonicity requires `A₁` monotone in its first argument and
**antitone in its second**. That is exactly the shape "more established
attackers reduce acceptance" has — the property that broke attempts 1 and 2 is
the property AFT's precision order is built to accommodate. The well-founded
fixpoint is then the least fixpoint of the stable revision operator, and it
exists by Knaster–Tarski on `(L², ≤_p)`.

**Why it fits here specifically.** Our truth values are already a bilattice, and
our current construction is *already* an approximation in disguise: a partial
valuation approximates the set of total valuations extending it. AFT makes that
explicit and gives the pair the order the partial function could not carry.

**What must be discharged.**

- Choose `L`. The natural candidate is total valuations `Nodes → FOUR` under the
  **truth** order, with `L²` supplying the approximation — but FOUR is itself a
  bilattice, so this is a bilattice over a bilattice and the two `≤_k`s must not
  be conflated. This is the main hazard and it is a notational trap as much as a
  mathematical one.
- Define `A` for every operator, including `unless` and the rule base, and
  **prove** `≤_p`-monotonicity. Not assert it. Both prior failures were at this
  exact step.
- Prove exactness on consistent pairs, or the well-founded fixpoint does not
  relate to the intended semantics.
- Relate the result to `strict-kernel-v1`: on a rule base with no defeasible
  rules the construction must reduce **exactly** to §4.2's fixpoint, or the
  frozen kernel is not layer 0 of anything.

**Cost.** The heaviest option. AFT is well-studied but the instantiation is
genuinely new work, and the bilattice-over-bilattice structure is where a
mistake would hide.

## 4. Design B — Layered ASPIC+ over the frozen base

Keep `⟦·⟧` exactly as `strict-kernel-v1` computes it, and never let defeasible
conclusions re-enter it. Build arguments *above* it.

- **Layer 0** — the strict base. Testimony, connectives, quantifiers, strict
  rules. Frozen, monotone, already proved.
- **Layer 1** — ASPIC+ arguments: trees whose leaves are layer-0 conclusions and
  whose steps are defeasible rules from `R`. Attack (rebut, undercut, undermine)
  is **structural**, and contradiction is decided by layer 0.
- **Layer 2** — Dung's grounded extension over that framework. Defeasible
  conclusions are the conclusions of accepted arguments.

**Why the earlier failure does not recur.** Nothing is withdrawn, because
layer 0 never changes. Argument construction is monotone in `R`; the attack
relation is fixed once the framework is built; and Dung's characteristic
function `F` is monotone in `S` **for a fixed framework**, which is the standard
result. The grounded extension exists by Knaster–Tarski on `2^Args`. The mutual
dependency that killed attempts 1 and 2 is severed by construction rather than
survived.

**What must be discharged.**

- **The stratification condition, stated precisely.** ASPIC+ permits strict
  rules over defeasible conclusions, so "defeasible output never re-enters
  layer 0" needs care: it must forbid a defeasible conclusion from being an
  *input to layer 0's fixpoint*, while still allowing strict inference to be
  applied to it inside layer 1. Getting this wrong reintroduces the cycle.
- **A policy for `B` at the leaves** (§5.1). ASPIC+ assumes a consistent base;
  ours is Belnap and can be told both.
- **Preference into defeat.** `≺` turns attack into *defeat*, and ASPIC+ is
  well-known to require care here — the rationality postulates are not automatic
  for arbitrary preference relations.
- **Reduction check**: with `R` empty, layer 1 is empty and the answer is exactly
  `strict-kernel-v1`. This should be immediate, and is the reason to prefer this
  design if the battery does not separate them.

**Cost.** Much lighter, and provable with existing theorems. The price is
expressive: anything requiring genuine mutual dependency between a default and
the base is out of scope by construction — which must be *stated*, not
discovered later.

## 5. The battery

Both designs are judged on these. Each is a case where a plausible construction
gives the wrong answer, and each corresponds to a defect already made once.

### 5.1 `B` at a leaf

A defeasible rule whose antecedent is `Conflicted`. Does the argument exist?

The strict kernel says `B` is **designated**, so the antecedent holds — and it is
simultaneously refuted. Naively the argument is built *and* an undermining
attack on it succeeds. **Required:** a stated policy, not an accident of
evaluation order. Note this is the value `MapGraphStructure` could not express
until this session, and the value where material implication's unsoundness lived.

### 5.2 Rebuttal

`usually(bird → flies)`, `usually(penguin → ¬flies)`, and a penguin bird. Two
defaults, contradictory conclusions, no preference. **Required:** sceptically
undecided, not an arbitrary winner. Whichever rule is evaluated first must not
decide it.

### 5.3 Undercutting, defeasibly derived

`(unless E P)` where `E` is itself the conclusion of a default. The exception is
neither established nor refuted; it is *defeasibly* concluded. **Required:** the
attack succeeds only if `E`'s argument is itself accepted — which is the
recursion, and where the rank witness (§5.6) becomes necessary.

### 5.4 Undermining

An attack on a defeasible *premise* rather than a conclusion. **Required:** the
attacker set for an argument includes attacks on its sub-arguments. The retracted
`Step::Defeasible` read only the `unless` exception off the node and called that
complete — this case is why that was wrong.

### 5.5 Preference cycles

`a ≺ b`, `b ≺ c`, `c ≺ a`. `≺` is declared a strict partial order; the *store*
can hold a cycle anyway, since it is ordinary testimony. **Required:** the cycle
is visible as a conflict rather than silently closed over, matching how `prefer`
already refuses to bake in transitivity. A construction that assumes
well-foundedness of `≺` must say what it does when the store violates it.

### 5.6 Self-support and the two-cycle

`a` attacks `b`, `b` attacks `a`. Every local "is each attacker attacked by an
accepted argument" test accepts `{a}` — and Dung's grounded extension is
**empty**. This refuted the local certificate in §11.4 and any certificate form
must fail to certify it. The fix is a **rank witness**: each accepted argument
ranked, defenders strictly lower, bottoming out at unattacked arguments.

Also: an argument that supports itself through a defeasible rule must not thereby
become accepted, which is the same defect one level down and the reason
groundedness exists in the strict kernel.

### 5.7 Floating conclusions

Two mutually attacking arguments whose conclusions *agree* on some `X`.
Sceptical grounded semantics does **not** accept `X`. **Required:** this is a
deliberate choice to be stated, because it surprises people and the alternative
(accepting floating conclusions) is a different semantics with different
theorems.

### 5.8 Reduction to the frozen base

`R = ∅` ⟹ the answer is exactly `strict-kernel-v1`'s, on every case in
`tests/cross_spec.rs` and `tests/soundness.rs`. A construction failing this has
changed the strict kernel, which is frozen.

## 6. Certificate form

Whatever wins, the certificate must **verify** rather than trust:

- The attacker set is **derived** from the rule base and the graph, never read
  off the step. This is `Exhaustive`'s `complete` flag, and it has been the same
  defect three times.
- Acceptance carries a **rank**, checked to descend strictly to unattacked
  arguments (§5.6).
- The conclusion's derivation axis is `Default`, and `Γ` is extended with the
  rule base **and** the completeness of the attacker set — a defeasible
  conclusion is conditional on nobody having a defeater you did not record, and
  that condition must be visible to a reader who wants to attack it.
- `⟦E⟧ ∈ {N,F}` must be **expressible**. Two bounds over `{T,B}` and `{F,B}`
  cannot distinguish *fails* from *conflicted*, and a default defeated by a
  conflicted exception is exactly the case that matters (§5.1). This is a
  judgment-type change and it is a precondition, not a detail.

## 7. Recommendation, and what would change it

**Start with Design B**, and treat Design A as the fallback if the battery
separates them.

The reasoning is not that B is better in the abstract — A is strictly more
general and is the tool built for this class of problem. It is that B's theorem
already exists and B's failure mode is *expressive* (a stated restriction),
while A's failure mode is *soundness* (an approximator that isn't one), and this
project's last four rounds were all soundness failures dressed as proofs.

**What would flip it:** if §5.3 or §5.7 cannot be given a clean answer under the
layering — that is, if genuine mutual dependency between a default and the base
turns out to be needed for ordinary agent-memory patterns rather than being an
edge case — then the restriction is not acceptable and AFT is the answer.

**Next step is not code.** It is working §5.1–5.8 through Design B on paper,
recording where it gives an answer and where it refuses, and only then deciding.
That is done — see §8, which finds no unresolved case and two resolutions better
than this section expected.


## 8. The battery worked through Design B

Done on paper, case by case. **Design B survives all eight**, and two of them
resolve better than §3–§4 anticipated — the resolution falls out of the attack
relation instead of needing a policy exception.

**Setup.** Layer 0 is `strict-kernel-v1`, unchanged: testimony, connectives,
quantifiers, strict rules, `⟦·⟧₀` into `FIVE`. Layer 1 arguments are finite
trees whose leaves are layer-0 conclusions and whose steps are rules from `R`;
strict rules may be applied *within* layer 1 to defeasible conclusions, which is
ordinary ASPIC+. Layer 2 is Dung's grounded extension of `(Args, Defeat)`.

### 8.1 `B` at a leaf — resolved, and with no special rule

An argument whose leaf is `Conflicted` **undermines itself**. `⟦p⟧₀ = B` is
designated *and* anti-designated, so the very same layer-0 conclusion that
licenses the leaf also supplies the contradiction that undermines it. A
self-attacking argument is never in the grounded extension, since `F(S)` requires
every attacker to be attacked by a member of `S` and an argument cannot defend
itself.

So a conflicted fact cannot silently found a default, and **nothing had to be
added** — no "leaves must be `T`, not merely designated" exception, which was the
obvious move and would have been a second notion of support living beside layer
0's. Falling out of the attack relation is the difference between a semantics and
a pile of rules.

### 8.2 Rebuttal — sceptically undecided ✓

`usually(bird → flies)`, `usually(penguin → ¬flies)`, Tweety both. The two
arguments rebut each other; with no preference neither defeat is filtered;
`F(∅)` contains neither, and iteration adds neither. Undecided.

Crucially **evaluation order cannot affect this**, because the framework is built
before it is evaluated. That is the structural reason Design B cannot reproduce
the "whichever rule fired first wins" defect.

### 8.3 Defeasibly-derived undercutting — works, and needs no mutual dependency

`(unless E P)` with `E` concluded by argument `C`. `C` undercuts `A`. Grounded
semantics resolves it by construction: if nothing attacks `C`, then `C ∈ F(∅)`
and `A` is out; if some unattacked `D` attacks `C`, then `C` is out and `A` is
in at the next stage.

The recursion terminates on the ordinal construction of the grounded extension.
**No layer-0 feedback is involved** — `E` is concluded in layer 1, and this is
precisely the case that killed attempts 1 and 2 when the valuation and the
acceptance were mutually defined.

### 8.4 Undermining a sub-argument ✓, and it names the retracted rule's bug

Attacks are on *sub-arguments*, so the attacker set of `A` is the union over
every sub-argument of `A`. The retracted `Step::Defeasible` read the exception
off the top `unless` node and called that complete — under this definition it was
missing every attack on every premise beneath it.

### 8.5 Preference cycles — degrade to sceptical, no acyclicity assumption

Defeat is attack *not filtered by* preference: `B` defeats `A'` iff `B` attacks
`A'` and **not** `B ≺ A'`. With a store holding `a ≺ b ≺ c ≺ a`, the relation is
not a strict partial order — and the literal reading still behaves. Where `a ≺ b`
and `b ≺ a` both hold, both attacks fail to be filtered, so both defeats stand,
so both arguments are mutually defeated and neither is accepted.

A cycle therefore reads as *no preference wins*, which is sceptical and safe, and
it matches `prefer`'s existing refusal to bake in transitivity. **No
well-foundedness assumption on `≺` is required**, which was the alternative and
would have made a store's ordinary testimony able to make the semantics
undefined.

### 8.6 Two-cycle and self-support ✓ — and this is where the rank comes from

`a` attacks `b`, `b` attacks `a`. `F(∅) = {unattacked arguments} = ∅`, and
iteration stays `∅`. The grounded extension is empty, as required — while every
*local* "is each attacker attacked by an accepted argument" test accepts `{a}`.

The certificate consequence is exact: carry **`rank(a)` = the least `n` with
`a ∈ Fⁿ(∅)`**, and have the kernel check that every attacker of `a` is defeated
by an argument of *strictly lower* rank. `F⁰(∅)` is the unattacked arguments, so
ranks bottom out and circular defence cannot be certified. This is the local
check the retracted §11.4 wanted and did not have.

Self-support does not arise: arguments are finite trees, so an argument cannot
take its own conclusion as a premise.

### 8.7 Floating conclusions — a stated choice, and the right one here

Two arguments concluding `X` by different routes, attacking each other on their
intermediate steps. Sceptical grounded semantics accepts neither, so `X` is not
accepted even though it follows either way.

Stated deliberately rather than inherited silently. For a memory that must
explain itself it is also the better answer: accepting `X` would mean holding a
belief whose every supporting argument the system has rejected, and "I cannot
tell you why" is worse than "undecided" for something whose whole purpose is
being able to say why.

### 8.8 Reduction to the frozen base ✓

`R = ∅` ⟹ no argument contains a defeasible step ⟹ rebut, undercut and undermine
are all vacuous (each requires a defeasible element) ⟹ no defeats ⟹ the grounded
extension is *all* arguments ⟹ conclusions are exactly layer 0's.

`strict-kernel-v1` is recovered on the nose, so `tests/cross_spec.rs` and
`tests/soundness.rs` continue to pin the same behaviour.

### 8.9 The restriction, stated precisely

Design B forbids exactly one thing: **a defeasible conclusion may not be an input
to layer 0's fixpoint.** It may be used freely inside layer 1, including by
strict rules.

Working the cases showed this is narrower than §4 feared. "Usually the build is
fast" followed strictly by "if the build is fast, deploy" is fine — the strict
step happens in layer 1. What is excluded is a default affecting layer 0's own
truth computation, e.g. a default about the truth predicate feeding groundedness.
That is exotic, and if it is ever needed it is the trigger to switch to AFT.

## 9. Where this leaves the decision

Design B survives §5 with no unresolved case, one restriction that is narrow and
stateable, and a certificate form (§8.6's rank) that is locally checkable — which
was the sticking point that killed the last attempt.

**Still required before any code**, per §2, and none of it is done:

1. The denotation written out — `⟦usually P⟧`, `⟦unless E P⟧`, `⟦prefer a b⟧` in
   terms of the layer-2 extension, as clauses, not prose.
2. The theorem stated and proved: existence and uniqueness of the grounded
   extension over `(Args, Defeat)` as constructed here, plus §8.8's reduction as
   a lemma rather than an observation.
3. The judgment-type change of §6 — `⟦E⟧ ∈ {N,F}` is not expressible today, and a
   default defeated by a conflicted exception is exactly the case that matters.
4. Review of this document, including §8, by someone trying to break it. Two
   prior constructions passed my own reading and did not survive that.

## References

Denecker, Marek & Truszczyński, *Approximations, stable operators, well-founded
fixpoints and applications in nonmonotonic reasoning* (AFT) · Dung 1995 ·
Prakken, *An abstract framework for argumentation with structured arguments*
(ASPIC+) · Modgil & Prakken, on rationality postulates and preferences ·
Van Gelder, the alternating fixpoint · Caminada & Amgoud, on rationality
postulates for rule-based systems.
