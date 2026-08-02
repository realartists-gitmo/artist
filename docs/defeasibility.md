# Defeasibility: two candidate constructions, and the cases that decide

> **Design document. No implementation.** Nothing here may reach the kernel until
> one construction survives review, and the review standard is the battery in §5
> — not a proof sketch, which is what the last two attempts were.
>
> Baseline: `strict-kernel-v1`. The strict kernel is frozen and is **layer 0** in
> everything below. It is not modified while this is open, unless a concrete
> counterexample breaks it.
>
> **A counterexample has broken it.** Adversarial review refuted `Instance` and
> `Instantiate` and showed `Connective` needs a side condition it does not have —
> see `semantics.md` §7.1a, reproduced live. Layer 0 is therefore *not* green, and
> the repair chosen in §7.1b **changes what layer 0 denotes**: `⟦·⟧` becomes a
> pair of independently partial evidence bits rather than one complete FOUR value
> or none.
>
> This does not invalidate §8 — every case there is about the *attack* structure,
> which is unaffected — but it does mean Design B's layer 0 is a moving target
> until §7.1b lands. **Sequence: repair layer 0 first, then defeasibility.**
>
> It is also a point in Design A's favour that should be recorded honestly: the
> §7.1b repair *is* an approximation bilattice, which is what AFT works over. The
> strict repair and the defeasible construction have converged on one structure,
> and §7's recommendation should be revisited once layer 0 is repaired rather
> than treated as settled.

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

### 8.1 `B` at a leaf — REFUTED, and reopened

The claim was that an argument whose leaf is `Conflicted` **undermines itself**,
so no policy is needed. **That does not follow.**

`⟦p⟧₀ = B` means `p` is designated *and* anti-designated, and negation fixes `B`,
so `p` and `¬p` are both supported. But "both are supported" is not "this
argument attacks itself": an attack is a relation between *arguments*, and the
claim never constructed the attacking argument, named the contrary relation, or
said which sub-argument it targets. It asserted a conclusion in the vocabulary of
the attack relation without building one.

It also contradicts §8.8. If a conflicted layer-0 leaf generates an undermining
attack automatically, then undermining is *not* vacuous when no defeasible
element exists, and the reduction fails. Both cannot hold.

**The shape a repair must have**, and it must be stated and proved rather than
observed:

> Strict conclusions remain independently accepted at layer 0. An attack may
> target them **only as premises inside an argument that contains a defeasible
> step.**

That preserves layer 0 under reduction while still preventing `B` from founding a
default. It is not yet written, and the interaction with `⟨1,1⟩` under the §7.1b
per-bit repair — where "designated and anti-designated" is a pair of settled bits
rather than one value — has not been examined at all.

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

### 8.5 Preference cycles — REFUTED, and backwards in the dangerous direction

The claim was that a preference cycle degrades to "no preference wins", leaving
both arguments mutually defeated and neither accepted. **The arithmetic runs the
other way.** With

```
B defeats A  ⟺  B attacks A  ∧  ¬(B ≺ A)
```

and both `a ≺ b` and `b ≺ a` on record, `¬(a ≺ b)` is *false* — so `a` does **not**
defeat `b`, and symmetrically. Both defeats are **suppressed**, not preserved.
Both arguments are then undefeated, and two contrary conclusions are *both
accepted*. That is the opposite of sceptical, and worse than the behaviour the
section claimed to rule out.

A three-cycle has no uniform answer at all: the result depends on the directions
of the attacks relative to the preference edges, so there is no "cycles degrade
gracefully" story to tell.

Three further omissions, each of which must be settled before this section can be
rewritten:

- **`≺` is over rules; arguments are what get compared.** ASPIC+ requires an
  explicit **lifting** — last-link, weakest-link, or another — and the choice
  changes which rationality postulates hold. None was named.
- **Preference is FOUR-valued testimony, so `¬(B ≺ A)` is not Boolean absence.**
  A *conflicted* or *unknown* preference should presumably not suppress an
  attack, which means suppression requires the preference to be **exactly `T`** —
  another exact-value judgment, and the same judgment-type problem as
  `semantics.md` §7.1b. The two are not independent.
- **Attack and defeat must be split consistently.** This document says attack is
  structural and preference converts attack into defeat; `semantics.md` §11.2
  already folds preference *into* the definition of rebuttal. One of them is
  wrong and they are currently both written down.

### 8.6 Two-cycle ✓, but the certificate form does not survive

The semantic half stands. `a` attacks `b`, `b` attacks `a`, the grounded
extension is empty, and every *local* "is each attacker attacked by an accepted
argument" test wrongly accepts `{a}`. Self-support does not arise, because
arguments are finite trees and cannot take their own conclusion as a premise.

**The certificate form proposed for it does not.** Three problems, in increasing
order of seriousness:

- **Off by one.** `F⁰(∅) = ∅`; `F¹(∅)` is the unattacked set. The text said `F⁰`
  was the unattacked arguments, so every rank in it is wrong by one and the
  bottoming-out argument does not land where it was said to.
- **Natural-number ranks are not general.** They certify the whole grounded
  extension only for a **finite, or suitably finitary**, framework. A general
  framework needs transfinite iteration, and an ordinal witness is not local.
- **"Derive every attacker from the graph and rule base" may not terminate.**
  Unbounded rule instantiation can generate infinitely many arguments, so
  completeness is not something a checker can simply recompute.

So the design must **choose**, and the choice is a scope decision rather than a
detail:

1. require the relevant framework to be finite and closed;
2. permit only **sound-but-incomplete** finite-rank certificates — some accepted
   arguments are simply not certifiable; or
3. define ordinal or richer witnesses, giving up locality.

**And attacker completeness must not go in `Γ`.** §6 proposed exactly that, and
it recreates the defect it was written to avoid: *"assuming I omitted no
attacker, this is accepted"* is `Exhaustive`'s `complete` flag in its third
disguise. The repair is to bind the certificate to a **snapshot** — a hash of the
rule base and graph plus a transaction instant — and have the checker derive
completeness *relative to that snapshot*. Then the assumption is not "I found
them all" but "this is the world I was reading", which is checkable and is
already how `snapshot()` works in the store.

### 8.7 Floating conclusions — a stated choice, and the right one here

Two arguments concluding `X` by different routes, attacking each other on their
intermediate steps. Sceptical grounded semantics accepts neither, so `X` is not
accepted even though it follows either way.

Stated deliberately rather than inherited silently. For a memory that must
explain itself it is also the better answer: accepting `X` would mean holding a
belief whose every supporting argument the system has rejected, and "I cannot
tell you why" is worse than "undecided" for something whose whole purpose is
being able to say why.

### 8.8 Reduction — the theorem has no defined subject yet

§4 says `R = ∅` makes layer 1 *empty* and the answer is layer 0's. §8.8 said
`R = ∅` leaves arguments, *all of which are accepted*. Those are different
claims, and "recovered on the nose" was asserted over the gap between them.

Four definitions are missing, and the theorem cannot be stated without them:

- Is a layer-0 conclusion a **zero-step argument**, or does it sit outside the
  framework entirely?
- Do **strict-only arguments** live in layer 1?
- Is the final result `L₀ ∪ {Conc(a) : a ∈ GE}`, or `L₀` together with only those
  `a` containing a defeasible step, or something else?
- Can defeat ever **remove** a layer-0 conclusion? §8.1's repair says no; §8.1 as
  originally written said yes.

Until those are fixed, this is a statement of intent, not a lemma.

### 8.9 The restriction, stated precisely

Design B forbids exactly one thing: **a defeasible conclusion may not be an input
to layer 0's fixpoint.** It may be used freely inside layer 1, including by
strict rules.

Working the cases showed this is narrower than §4 feared. "Usually the build is
fast" followed strictly by "if the build is fast, deploy" is fine — the strict
step happens in layer 1. What is excluded is a default affecting layer 0's own
truth computation, e.g. a default about the truth predicate feeding groundedness.
That is exotic, and if it is ever needed it is the trigger to switch to AFT.

### 8.10 The battery was too small, and said so before ignoring it

§4 records that the ASPIC+ **rationality postulates are not automatic** for
arbitrary preference relations, and §8 then declared the design sound without
checking one of them. Four more cases, and none is optional — a rule-based system
failing any of them produces conclusion sets no reader can act on:

- **Sub-argument closure.** If `a ∈ GE` then every sub-argument of `a` is in
  `GE`. Violated, a system accepts a conclusion while rejecting a step it rests
  on.
- **Closure under strict rules.** The accepted conclusions must be closed under
  the strict rules — otherwise the system accepts `A` and `A → C` strictly and
  refuses `C`.
- **Consistency, adapted to FOUR.** The standard postulate says the accepted set
  is consistent. Here "consistent" needs restating, because `B` is *designated
  and* anti-designated by construction: a base that holds `p` both ways is not an
  inconsistency the argumentation layer created and must not be one it is blamed
  for. What the postulate should say instead is not yet written.
- **The preference lifting** chosen in §8.5, whichever it is, satisfying the
  above. Last-link and weakest-link differ precisely on which postulates survive,
  so the choice cannot be deferred past this point.

## 9. Where this leaves the decision

**Design B does not survive §5 as written.** Adversarial review refuted three of
the eight worked cases and showed the reduction has no defined subject:

| case | status |
|---|---|
| 8.1 `B` at a leaf | **refuted** — self-undermining was asserted, not constructed, and it contradicts 8.8 |
| 8.5 preference cycles | **refuted** — the defeat condition suppresses both attacks, so both contrary conclusions are *accepted* |
| 8.6 certificate form | **refuted** — off-by-one in the rank, natural ranks are not general, completeness is not recomputable, and it was placed in `Γ` |
| 8.8 reduction | **undefined** — "layer 1 is empty" and "all arguments accepted" are different claims |
| 8.2, 8.3, 8.4, 8.7 | stand |

The direction still looks right, and the grounded-extension theorem was never the
hard part. The actual work is **preference semantics** (lifting, and FOUR-valued
`≺` needing exactly `T` to suppress), **the `B`-premise treatment**, **finite
attacker completeness against a snapshot**, and **the exact layer boundary**.

**Still required before any code**, and none of it is done:

1. The denotation written out — `⟦usually P⟧`, `⟦unless E P⟧`, `⟦prefer a b⟧` in
   terms of the layer-2 extension, as clauses, not prose.
2. **Attack and defeat split consistently**, in one place: this document and
   `semantics.md` §11.2 currently disagree about whether preference is inside
   rebuttal or converts attack to defeat.
3. **A preference lifting** named (last-link, weakest-link, …) with its
   postulates checked, and suppression requiring `≺` to be exactly `T` — which
   ties this to `semantics.md` §7.1b rather than being independent of it.
4. **The layer boundary**, stated as §8.1's candidate restriction and proved:
   strict conclusions independently accepted at layer 0, attackable only as
   premises inside an argument containing a defeasible step.
5. **A finiteness or incompleteness decision** for certificates (§8.6), and
   completeness bound to a **snapshot** rather than assumed in `Γ`.
6. The theorem stated and proved, with §8.8's reduction as a lemma — which first
   needs §8.8's four definitions.
7. The judgment-type change of §6.
8. **Rationality postulates** (§8.10), including a `FOUR`-adapted consistency
   postulate that does not blame the argumentation layer for a base holding `p`
   both ways.
9. Review again. Three constructions have now passed my own reading and not
   survived someone else's.

## References

Denecker, Marek & Truszczyński, *Approximations, stable operators, well-founded
fixpoints and applications in nonmonotonic reasoning* (AFT) · Dung 1995 ·
Prakken, *An abstract framework for argumentation with structured arguments*
(ASPIC+) · Modgil & Prakken, on rationality postulates and preferences ·
Van Gelder, the alternating fixpoint · Caminada & Amgoud, on rationality
postulates for rule-based systems.
