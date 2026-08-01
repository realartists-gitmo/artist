# The artist expression language

The representation every durable fact is stored in, and the target every
extractor emits. Normative: where this document and the code disagree, the code
(`crates/artist-logic`) wins and this document is a bug.

Every claim below that can be executed is executed. `crates/artist-logic/tests/`
holds `roundtrip.rs`, `identity.rs`, `graph_eval.rs` — and `conformance.rs`,
which **is §5.2**: one test per row of the transfer-function table, so a claim
here cannot drift from the implementation silently. That discipline is not
decoration. An earlier draft of §2 asserted, from a summary rather than a test,
that the reader could not serialise an `Apply` whose first operand is a list of
lists. A reviewer reasoned correctly from it and reached a wrong conclusion; the
claim was false as stated and the underlying bug was real but different. A prose
spec is worth exactly the tests under it.

## 0. What it is for

One representation for expressions: facts, queries, rules, residuals and
continuations are all the same kind of object, so nothing needs translating at
the boundary between remembering and reasoning.

**A stored expression is not a belief.** The same graph serves as a queried
proposition, a quoted one inside `(believes sarah (quote P))`, a residual left
by a budget-exhausted evaluation, and something actually asserted. What is
believed is recorded separately — see §6.

The design constraint the whole thing is built to satisfy: **every structural
limit we impose is either a theorem or a dial.** Where something cannot be
expressed, that must follow from a proof, never from a closed enum someone
forgot to extend.

## 1. The kernel

Every expression is a node in a graph. Identity is a 128-bit `ObjectId`;
structure is one of exactly **six** shapes.

```
Atom     { name: Option<String> }
Literal  ( Int | Decimal | Text | Bool | Bytes )
Apply    { operator: ObjectId, operands: Vec<ObjectId> }
Bind     { binder: ObjectId, vars: Vec<Binder>, bodies: Vec<ObjectId> }
External { namespace, locator, version?, digest? }
Opaque   { tag: String, payload: Vec<u8> }
```

Only three are structural. The properties that matter follow from what is
*absent*:

**There is no construct list.** `Apply`'s operator is an ordinary `ObjectId`, so
a new connective, quantifier or modality is a new *atom* — not a new variant, not
a migration. Nothing distinguishes `and` from a predicate invented five minutes
ago except that the evaluator knows what to do with one.

**Arity is unbounded everywhere.** `operands`, `vars` and `bodies` are vectors.
`bodies` is plural because binders genuinely differ: `forall` takes one body,
`letrec` takes a definition *and* a scope.

**Predicates are first-class objects.** A relation can be quantified over,
asserted about, and passed as an argument — a consequence of `operator` being an
id rather than a tag.

Stated precisely, because "higher-order" is three claims and only two of them
hold here. *Representation* is higher-order: a predicate is an ordinary object
and can appear anywhere an object can. *Quantification and application* are
higher-order: `(forall [(R (Rel D))] …)` binds a predicate variable, and a bound
relation variable genuinely applies — `second_order` walks the candidate
relations and refuses to *support* a universal unless the space it walked was
the whole powerset. A counterexample still refutes one from a partial space,
which is the asymmetry §5.2 states for every quantifier; the paraphrase here
dropped it for a round. Note also that a relation variable in *argument*
position is not an entity the store has ever seen, so it is never resolved —
asking invited an authoritative answer to a question nobody had asked. **Derivation is first-order.** `match_goal` is structural: same
operator, same arity, arguments that are rule variables or constants. There is
no higher-order unification, so a *rule* cannot range over predicates even
though a *query* can. Saying "the language is higher-order" without that third
sentence overclaims the rule engine.

**Graphs, not trees.** A node reached twice is shared, not copied. Cycles are
legal — `alloc()` hands out an id before the body exists.

**`External` names things outside the language.** A file, a process, a person, a
dataset, an oracle. An unresolved external is still a well-formed expression.

**`Opaque` survives the future.** A node from a producer this build does not
understand round-trips byte-exact rather than being dropped.

### Identity

The 128-bit space is **partitioned by its top two bits**, so the three ways an
id comes into existence cannot collide:

```
00…  well-known   fixed, hand-assigned, below FIRST_FREE = 4096
01…  content      blake3 over canonical structure
10…  nominal      random, for things with no structure to identify them
11…  reserved
```

- `intern(node)` is **content-addressed**. Structurally identical expressions
  get the same id, so sharing and dedup are automatic.
- `alloc()` hands out a **collision-resistant random** id — one entropy draw per
  graph, expanded by a PRF. Used when the body is not yet known (cycles) or when
  the thing is genuinely nominal.

"Collision-resistant", not "unique": randomness gives a probability, not a
distributed guarantee. That is the same assumption content addressing already
rests on.

`fresh()` is `alloc()` plus an unnamed `Atom` — the **skolem**: an entity that
exists and can be referred to but has no name. Its identity to any other entity
is an ordinary proposition (§7), not a lexical accident.

**Three explicit exceptions to "identical structure, one id".** A node built
through `alloc` before its body exists has a nominal id that no later hashing
retroactively replaces, so **cyclic structures do not deduplicate**; two
independently constructed liars are two objects. Any expression containing a
free variable or skolem carries that nominal id, so it agrees with itself only
within one graph. And `transplant` remaps every non-well-known id through the
destination's allocator, converting content ids to nominal ones — so copying the
same closed expression into a graph twice yields two objects. Closed structural
expressions *built in place* are the fragment where the dedup claim holds
without qualification.

**The partition is an invariant of `intern` and `alloc`, not of the reader.**
`parse` will mint whatever id a `#…` token names, in whatever partition that
lands in: `#5` is `wk::EQ` and `#1a2f` — the skolem spelling used in §9 — is in
the well-known range, while a skolem from `fresh()` is nominal. Reading
untrusted text into a graph is therefore not partition-safe on its own.

## 2. Binding

Stored binders are in **de Bruijn form**. A bound occurrence is `(bvar k)`, a
flat index counting *slots* outward from the occurrence. A `Binder` carries a
domain and nothing else — no variable id, no name.

Flat indices rather than "k binders up": `vars` is a vector, so a binder depth
alone cannot say *which* of that binder's variables is meant. Entering a binder
with `n` slots shifts outer references by `n`. Within one binder, the
last-declared slot is index 0, so a later slot shadows an earlier one.

**Scope.**
- Domains **telescope**: slot `j`'s domain sees slots `0..j`, so
  `(forall [(n Nat) (xs (vec-of n))] …)` is expressible.
- **Every body sees every slot.** This is what makes `letrec` recursive.
- A variable free in the whole expression is bound by nothing, keeps its nominal
  identity, and is not abstracted.

**Names are not stored, and cannot be.** A display name inside the node but
excluded from the hash would give two alpha-equivalent expressions one id and
two different byte encodings; `intern` is first-writer-wins, so which body
survives would depend on insertion order, and a caller who built with `#A` could
read back a node mentioning `#B` with its own references dangling. The canonical
printer therefore *generates* names — `v0`, `v1`, … by absolute depth.

**Alpha-equivalence is the equality that holds.** `(forall [(f Path)] (p f))`
and `(forall [(x Path)] (p x))` are one object. Capture is impossible by
construction: `open_binder` instantiates fresh variables on every traversal, so
nothing downstream does index arithmetic.

## 3. Surface syntax

S-expressions. `print` emits this and `parse` reads it.

```
atom            prefers            bare symbol
                |has spaces|       bars when it contains whitespace, ()"#| or parses as a number
literal         42                 integer, arbitrary precision
                -12345d3           decimal: mantissa × 10^-scale
                "text"             \\ and \" escaped
                true / false       Literal::Bool
                #true / #false     the well-known truth ATOMS wk::TOP/BOT
                                   — different objects, different behaviour
                0xdeadbeef         bytes
                .                  the empty byte string, in external/opaque fields
anonymous       #1a2f              hex id — an unnamed atom (skolem, free variable)
application     (op a b c)         any arity
binding         (binder [(v0 D) (v1)] body ...)
                                   SQUARE brackets, always; domain optional
external        (#external ns hexlocator hexversion|- hexdigest|-)
opaque          (#opaque tag hexpayload)
sharing         #1=(...)  #1#      datum label at definition, reference after
```

Three delimiters carry meaning and none of them is guessed:

- **Binding lists use `[…]`.** They used to be detected by "the second element
  is a list of lists", which misread `(f ((g x)) y)` as a binder. That went
  unnoticed because the misparse re-printed identically; canonical variable
  names exposed it immediately.
- **`#external` / `#opaque`** cannot collide with a raw id or a datum label,
  both of which require a digit straight after the `#`. The atoms `external`
  and `opaque` are therefore ordinary operators.
- **`.` is the empty byte string**, distinct from `-` meaning absent, because
  `Some(vec![])` and `None` are different facts.

The correct round-trip property is `print(parse(print(g))) == print(g)`. Exact
graph equality does not hold and is not wanted — but the reason is **cycles**,
not skolems. An unnamed atom prints as its raw hex id and reparses to that same
id, exactly as §1's parser caveat says; what does not survive is a datum-labelled
cycle, whose node the reader must allocate afresh. This paragraph blamed skolems
for a round, while §1 said the opposite two pages earlier.

**Text equality alone is too weak, though.** `wk::TOP` used to print as `true`
and reparse as a `Literal::Bool` — satisfying that property exactly while
turning `Supported/Exact` into `Open/Unsupported`. The truth atoms now print as
`#true`/`#false`, and the round-trip test compares *evaluated results* as well
as text.

## 4. The well-known vocabulary

101 operators, ids fixed below 4096. **This is the closed part of the language**;
every domain predicate and entity is open. The evaluator privileges these; the
representation does not.

Every operator that denotes a **proposition or a term** is reachable in the
evaluator's dispatch. The type constructors are not, and that is not a gap:
`Universe`, `->`, `Rel`, `Prop`, `Expr`, `World`, `Context`, `Refine`,
`Product` and `Sum` classify expressions rather than evaluating to truth
values, and every one now has a rule in `typing.rs`, which is advisory by
design. `World` and `Context` did not when that sentence was first written —
they fell to its catch-all and were typed `Prop` — so the sentence covered the
last two unreferenced constants by assertion rather than by code.

Stating it that way because the previous wording — "every one of them is
reachable in the evaluator's dispatch" — was simply false, and an audit found
five constants referenced nowhere outside their own declaration. `type-of`,
`sort` and `instants` were among them and *were* gaps: choosing the reserved
spelling to record a type fact made it permanently unretrievable, and the
reserved `instants` domain enumerated nothing. All three now evaluate.

| group | members |
|---|---|
| connectives | `not` 1, `and` 2, `or` 3, `implies` 4, `=` 5, `<=` 6, `true` 7, `false` 8 |
| binders | `forall` 16, `exists` 17, `lambda` 18, `letrec` 19, `count` 20, `sum` 21 |
| bound occurrence | `bvar` 22 |
| arithmetic | `+` 32, `*` 33, `-` 34, `/` 35, `mod` 36 |
| text | `concat` 48, `len` 49, `substr` 50, `contains` 51, `starts-with` 52, `ends-with` 53 |
| sequences | `seq` 54, `nth` 55 |
| quotation | `quote` 64, `denotes` 65, `holds` 66, `asserted-by` 67, `believes` 68, `eval` 69, `provable` 70 |
| time & modality | `at` 80, `in-world` 81, `necessarily` 82, `possibly` 83, `if-counterfactually` 84, `before` 85, `during` 86, `always` 87, `eventually` 88, `since` 89 |
| types | `Universe` 96, `->` 97, `Rel` 98, `Prop` 99, `Expr` 100, `World` 101, `Context` 102, `Refine` 103, `Product` 104, `Sum` 105, `type-of` 106, `Int` 107, `Nat` 108 |
| domains | `sort` 112, `set` 113, `where` 114, `instants` 115, `set-partial` 116 |
| evidence | `supports` 128, `attacks` 129, `derived-from` 130, `source` 131, `same-as` 132 |
| quantities | `quantity` 144, `scale` 145, `interval` 146 |
| defeasibility | `usually` 147, `unless` 148, `prefer` 149 |
| norms | `obliged` 150, `permitted` 151, `forbidden` 152, `violated` 153 |
| admissibility | `indeterminate` 154, `total` 155 |

Universe levels are values: `(Universe 9999)` inhabits `(Universe 10000)`, with
no tower ceiling. Typing is **advisory** — an ill-typed expression is detected
and still stores, prints and reparses, because refusing to represent it would
reinstate the ceiling the graph exists to remove.

`at` is why a predicate should never carry a tense. A count that changed is one
relation at two instants, not two relations.

**A norm is not a modality.** `obliged`, `permitted` and `forbidden` are stored
claims about what ought to be, and they are deliberately *not* evaluated by
checking whether the thing holds. Reading `(obliged P)` as "P in every
accessible world" makes a rule **false exactly when it is broken**, which is the
one behaviour a norm must not have in a memory whose job is recording what
happened. "Run `cargo fmt` before committing" stays true on the day it is
skipped; `violated` is what starts holding. The only entailments are that an
obligation implies a permission and that forbidding is obliging a negation.

**`set` promises exhaustiveness; `set-partial` does not.** The distinction
exists because a suspended scan has to describe what it still is: rebuilding the
unexamined members as a `set` made a continuation a *stronger* question than the
one it suspended.

## 5. Evaluation

### 5.1 The lattice

Results are two independent monotone bounds, `support` and `refutation`, each
`None ⊑ Partial ⊑ Certain`. They project onto four values ordered by
**information**, not truth:

```
        Conflicted          both sides have evidence
        /        \
  Supported    Refuted
        \        /
          Open              neither side has evidence
```

This is the knowledge order of a Belnap/FDE bilattice. Evaluation only ever
moves *up* it.

**`must` grows and `may` shrinks.** Saying both are "monotone" is ambiguous and
was wrong to leave that way. Concretely: as evaluation proceeds the set of
things established true only ever gains members, and the set of things not yet
excluded only ever loses them, with `must ⊆ truth ⊆ may` throughout. Halting at
any instant is therefore sound — the pair is a true statement about a weaker
claim, never a guess.

This is the property `tests/properties.rs` checks by evaluating generated terms
at rising budgets and asserting that neither bound ever falls. It is not
checkable one table row at a time, which is why a suite of table rows was green
while the tail abstraction returned `Refuted/Exact` at one budget and
`Supported/Exact` at another for the same term.

Computational status is a **separate axis**: `Exact`, `BudgetExhausted`,
`Unsupported`, `Stalled`. "No evidence", "contradictory evidence", "out of
budget" and "no semantics for this operator" are four different answers.

**And so are grounding and derivation**, which used to be smuggled onto the
evidence axis.

*Grounding* — `Grounded | StableLoop | Oscillatory` — says whether a sentence
reaches a value at all. The liar was reported `Conflicted`, which made one value
carry two unrelated instructions: `Conflicted` tells a caller to go read the two
sources, and the liar has none. §5.3 said in bold that its treatment was not
ordinary two-sided evidence and then reused the value for ordinary two-sided
evidence anyway.

*Derivation* — `Observed | Derived | Default` — says how a conclusion was
reached and therefore what could overturn it, with the defeaters named
alongside. Defeasibility was encoded as *weakness*: `usually` produced
`Partial`, so "holds by default" and "some evidence, not conclusive" were the
same value. They are not. A default can be overwhelmingly well attested and
still defeasible; a claim can be weakly evidenced and not defeasible at all.
Collapsing them cost in both directions, and it left graded strength with no
representation — for a round `Bound::Partial` had no producer at all, which was
stated plainly in its own documentation rather than papered over.

It has one now, and it is the case the level was always for: `(determinate P)`
over a proposition whose condition is *underspecified* — sharp, real, and simply
unrecorded. Neither yes nor no, and collapsing it to either would be a claim
about the records wearing the clothes of a claim about the world. Graded
*strength* stays on `Credence`, where the arithmetic is right.

### Determinacy, and the two modes

*Determinacy* — `Total | Underspecified | Unknown | Indeterminate`, with a
`basis` of `Presumed | Declared(…) | Derived` — says whether
a proposition has a **stable, determinate condition under which reality
satisfies it**. That is the admissibility criterion the whole design rests on,
and sorites is the reason it cannot be assumed: most ordinary predicates are not
sharp, so totality is a *checked or declared* property of a proposition under an
interpretation, often only over a region of the argument space. `heap` is sharp
at ten thousand grains and has no answer at forty-seven hundred.

It is deliberately **not** part of grounding, though folding it there is the
obvious-looking move. The test for separate axes is whether all four
combinations are inhabited, and they are: the liar is ungrounded and
semantically precise; a borderline `(heap 4783)` is perfectly grounded and
semantically indeterminate; "this sentence is heapish" is both. Combining them
would recreate exactly the conflation that splitting `Conflicted` and `Partial`
was meant to remove.

Determinacy gates *classical operations*; it does not touch evidence. There is
real evidence that someone is tall, and the honest report is `Supported` +
`Indeterminate` — genuine grounds, no sharp fact to be right about.

**Which forces two modes.** Presuming totality is the only workable default for
answering questions; almost nothing is ever certified sharp, and requiring
certification first would halt the system. But that same presumption inside a
trusted kernel certifies `heap(x) ∨ ¬heap(x)` for a predicate nobody established
as bivalent — excluded middle is *precisely* the tautology that needs bivalence.
So `eval` presumes and records that it presumed — the basis is what makes the
same `Total` distinguishable between the two — and `certify` requires totality
that was not merely presumed. A validity recogniser without that gate is a
bivalence-laundering machine that looks like a feature.

### Validity

Truth that does not depend on the store is decided before the store is
consulted. `P → P` used to come back `Open/Stalled` — it went looking for facts
about `P`, found none, and reported an absence, which is honest and useless. A
system that must ask the world whether `P → P` holds is not doing deduction.

Only the **propositional skeleton** is decided: full higher-order validity is
undecidable, the boolean structure over opaque atoms is not, and at these sizes
it is free. Quantified validity keeps going to the budget.

**Constructive and classical validity are separated, and only the second is
gated.** `P → P`, `¬(P ∧ ¬P)`, `P ∧ ⊤ → P` and `P → ¬¬P` hold intuitionistically
— no bivalence, no sharpness, no classical logic — so refusing to certify them
over a borderline predicate would be a bug rather than caution. They are decided
by G4ip, Dyckhoff's contraction-free sequent calculus, which terminates without
loop checking, and they certify unconditionally. Only what genuinely needs
bivalence is gated on `Total`: excluded middle, double-negation elimination,
reductio, and anything else the truth table proves and G4ip cannot.

Implication is therefore kept as itself in the skeleton rather than desugared to
`¬A ∨ B` — that rewrite is classically sound and constructively wrong, and it
would erase the distinction this exists to draw.

### Conflicted is about evidence, not about truth

Worth stating plainly, because the design would otherwise read as a quiet
concession that non-contradiction is negotiable. `Conflicted` says **two sources
disagree and you should go read them**. It does not say `P` and `¬P` both hold.
`Oscillatory` says a sentence never settles — under the admissibility criterion,
that it is not a proposition at all — which is a claim about groundedness, not a
true contradiction. Neither is dialetheism, and the separation is what keeps
them from being mistaken for it.

All four axes propagate the way `ComputeStatus` always has: a composite is no
better grounded, no less defeasible, and no more determinate than its parts.

**That is enforced structurally rather than site by site**, because site by site
is how it failed four rounds running. Every result returned by `check` folds into
an accumulator, and `dispatch` stamps every compound verdict from it above every
operator arm — so an arm that forgets to carry an axis is not a thing that can be
written. The cost is deliberate conservatism: a conjunction that short-circuits
after examining a default is still marked defeasible, because proving the default
did not contribute needs exactly the per-arm reasoning that kept going wrong.
Losing precision is sound; losing an axis is not.

### 5.2 Transfer functions

Written as `(support, refutation)`; `⊓` is meet, `⊔` is join.

| form | support | refutation |
|---|---|---|
| `(not P)` | `refutation(P)` | `support(P)` |
| `(and P…)` | `⊓ support(Pᵢ)` | `⊔ refutation(Pᵢ)` |
| `(or P…)` | `⊔ support(Pᵢ)` | `⊓ refutation(Pᵢ)` |
| `(implies P Q)` | as `(or (not P) Q)` | — |
| `(forall [(v D)] P)` | all members supported **and** `D` enumerable and complete | any member refuted |
| `(exists [(v D)] P)` | any member supported | all members refuted **and** `D` complete |
| `(count …)`, `(sum …)` | a `[lo, hi]` interval carrying its own exactness; `<=` reads the bounds, so a comparison decides before the scan ends | — |
| `(usually P)` | `Certain` when a default is on record, marked `Derivation::Default` | `P` refuted, or a counter-default on record |
| `(unless E P)` | `P`, undecided when `E` holds | likewise |
| `(quantity n u)` under `=`/`<=` | both sides normalised to a base unit via stored `scale` facts | — |
| `(at t P)` | `P` against the structure as of `t` | likewise |
| `(quote E)` | opaque — `E` is **not** evaluated | — |
| `(holds (quote P))` | descends into `P` | likewise |
| atom `p(a…)` | resolver says `Holds` | resolver says `Fails` |

Note the asymmetry in the quantifiers: one counterexample refutes a universal
immediately, but *supporting* one requires the domain to be both enumerable and
complete. Absence of a counterexample in a domain that might have more members
is not support.

**An atom with no direct answer tries the stored rules.** A rule is a
`(forall [(x …)] (implies A C))`; matching the goal against `C` binds the
rule's variables, and a supported `A` supports the goal. Bounded by a rule-depth
dial *and* by the ordinary path check, which refuses to re-enter a goal already
being derived — so a rule that leads back to its own conclusion yields `Open`
rather than spinning. A refuted antecedent means the rule does not apply, never
that the goal is false.

**Refutation of an atom requires an authoritative resolver.** `Knowledge::Fails`
is an assertion that the resolver was complete *for that query* — not merely
that it found nothing. A resolver that timed out, has a partial index, was
refused permission, or cannot answer that pattern of bound and free arguments
must return `Unknown`. This is the item with the worst failure mode in the whole
system: a fabricated `Fails` becomes `Refuted`, and a `Refuted` under a negation
becomes a confident `Supported` the store never earned, propagating upward
without trace.

**…which is why a resolver has five answers, not three.** Requiring completeness
for every refutation left a recorded negative — *"`-C target-cpu=native` did not
fix the segfault"*, the most common thing a coding agent learns in a day — with
nowhere to live but a closed-world declaration that also refutes the same claim
about every flag nobody tried. `Denied` is positive evidence of a negative and
needs no completeness at all. `Conflicted` is both directions on file: a store
ingesting claims from many sessions reaches it constantly, and until it existed
the only expression in the entire language that could produce
`Evidential::Conflicted` was the liar — so §5.1's argument for four-valued
evidence described a state evaluation could not reach.

A conflict is a signal, not a tie to be broken. Nothing downstream may flatten
it: `(and Conflicted #true)` is `Conflicted`, not the `Refuted` you get by
short-circuiting on whichever side the operator happens to consult first.

### 5.3 Self-reference, and what the liar actually gets

Evaluation tracks the path of nodes currently being evaluated together with the
parity of `not` above each. Re-entering a node already on the path *is*
ungroundedness. If the parity differs between the two visits the expression
oscillates; if it matches, it is stably ungrounded.

```
L = (not (holds (quote L)))    parity flips    →  Open, Exact, Oscillatory
P = (holds (quote P))          parity stable   →  Open,       Exact
grounded                                       →  Supported / Refuted
```

**This is not a monotone least fixed point, and it must not be described as
one.** Under the ordinary four-valued least fixpoint the liar stays `Open` —
that is Kripke's result and it is correct on its own terms. What this reports
comes from *detecting oscillation*, a revision-theoretic reading: the claim is
that `L` has no stable value, not that both sides have evidence in the ordinary
way — which is exactly why it is `Grounding::Oscillatory` and **not**
`Conflicted`. `Conflicted` tells a caller to go read two sources; the liar has
none. This section still reported `Conflicted` a round after §5.1 recorded the
split, and it is the section a reader goes to for the liar. It was chosen deliberately, because a memory system benefits from
telling the liar apart from the truth-teller, and both are `Exact` findings
rather than timeouts.

### 5.4 Defaults

`usually` produces **`Certain`** support *when a default is on record*, marked
`Derivation::Default` and carrying the defeater. Certain that the default
applies; defeasible because a defeater would overturn it. `is_definite` is false
for it — not because the support is weak, but because a defeasible reading is
not a settled one. With nothing on record it adds nothing: `(usually P)` over an
empty store is `Open`, because wrapping a claim cannot manufacture evidence.

This paragraph said `Partial` for one round after the axes were split, while
§5.1 thirty lines earlier said `Partial` had no producer. Both cannot be true,
and the anti-drift artifact did not catch it: `conformance.rs`'s row test
asserted `Bound::Certain` — the *opposite* of the table row it is named for —
under a doc comment still reading "enters `may` without entering `must`". A test
that pins the code is not a test that pins the prose to the code.

**"Nothing downstream can promote it" is a claim about eight scan sites, and
seven of them broke it.** Every scan classified a case by whether each side was
*certain*, which is invisible to `Partial`, so a universal over defaults
concluded `Certain` and — worse — an existential *refuted* a proposition while
supporting its only instance. The accumulation lives in one place now, and it is
this section's table applied to scans:

```text
universal:   support = ⊓ᵢ support(Pᵢ)   (needs a complete enumeration)
             refutation = ⊔ᵢ refutation(Pᵢ)
existential: support = ⊔ᵢ support(Pᵢ)
             refutation = ⊓ᵢ refutation(Pᵢ)   (needs a complete enumeration)
```

One side needs the enumeration to have been exhaustive and the other does not: a
single counterexample refutes a universal however partial the index was, but
*supporting* one means knowing there were no others.

**Defeat priority does not reduce to confidence, and it was a mistake to think
it did.** The reference-class argument — a narrower statistic screens off a
broader one — establishes *which statistic to use*, not that the narrower one is
better evidenced. Confidence and precedence come apart cleanly: one can be
almost certain of a broad default and much less certain of a narrow one, and the
narrow one still wins on a case in its class. Specificity, authority, temporal
and legal precedence, and explicit exception all order rules without ordering
credences, so defeat stays an explicit relation in the justification structure.
Credence bears on it only where a declared policy says it does.

**A default's natural form is a rule**, and rules may conclude one. `∀x.
under-tests(x) → usually (needs-review x)` fires and yields `Default`; a rule
concluding a negation refutes. Requiring a rule's conclusion to be the bare goal
atom meant a default could only ever be a ground fact, which is not the form a
default comes in. Explicit counter-evidence refutes it outright;
`unless` names an exception that *defeats* it, which is not the same as
refuting it — a defeated default is `Open`, because the rule stopped applying
rather than becoming false.

This is what `Derivation::Default` is for. Until defeasibility existed it had no
producer anywhere, and the three-element lattice was a two-element one in
practice — which also hid a bug in `is_definite`, since `Supported` means
"support exceeds `None`" and so covered defaults too.

### 5.5 Termination, honestly

**Under a budget, always.** Every path exhausts steps and yields
`BudgetExhausted` with a *residual* — the narrowed expression still to be
decided. A scan over a domain additionally yields a *continuation*, itself an
ordinary expression, and resuming it is just evaluating it.

Two corrections to what this section used to say. Continuations are produced by
domain scans, not by "every path" — most sites hand back a residual and nothing
more. And a continuation must answer **the question that was asked**, not the
sub-question it was suspended inside: propagated upward unchanged, the
continuation of `(not (exists …))` was the existential's, so resuming it
returned `Supported` where the query's answer was `Refuted`. Operators now
rewrap a sub-continuation or drop it, and where the bindings cannot travel with
the term — a lambda's parameters, a `letrec`'s relation — dropping it is the
only honest option.

**Evaluation is total under a budget**, which requires the budget to be
consulted before any unbounded work: a decimal literal with an extreme scale
used to overflow and panic, so a parseable term could crash the process without
spending a step.

**Exact convergence is claimed only for this fragment:** connectives,
quantifiers over finite domains the structure can *completely* enumerate,
aggregates over the same, exact rational arithmetic and text on literals,
sequence projection over literal sequences, `at`, `always`, `eventually` and
`since` over a finite instant set, modal operators over a finite frame, quantity
comparison where the `scale` chain terminates, norm lookup, rule chains within
the depth dial, and atoms whose resolvers answer without external calls.

No termination claim is made — and none should be inferred — for quantification
over infinite or non-enumerable domains, external resolvers, `eval`, `provable`,
or arbitrary combinations of the modal operators. Those get a budget and a
residual. That is the design: cost is a dial, not a refusal.

## 5.6 Certificates

A derivation is an **object**, not a re-run. `Certificate` is a list of steps in
dependency order; `check` validates it against the graph and returns what it
establishes, together with every authority it leaned on.

This is the de Bruijn criterion, and the point is the asymmetry: **the checker
knows nothing about resolvers, budgets, rules, scans, or any of the six thousand
lines it validates.** It knows the shape of a step and the rule for each shape.
If checking were as complicated as evaluating there would be no reason to prefer
it.

Two differences from a proof assistant's kernel, both forced by what this system
is. A certificate proves an *evaluation result* rather than a proposition —
this bound, from these facts, by these steps — because evaluation is anytime and
two-sided and must be able to say where it stopped. And `Told` is explicit: the
one step that rests on testimony names its **sources**, so whether to believe it
is the reader's judgement rather than the checker's.

Sources, not the relation symbol. That field used to hold the predicate, so
`(prefers adam tabs)` was certified on the authority of `prefers` — which is not
an authority: it cannot be doubted, consulted, or retracted, and every fact in
the store named itself. The channel is `GraphStructure::attribution`, and against
the real store it reads `assertion.agent`, `stmt.source_session` and
`stmt.origin`, all of which were on disk and unread.

An empty source list is a legitimate answer and stays visible. A structure that
vouches for something and names nobody has told the reader exactly that, and the
kernel records it under `assumed` rather than inventing a name. `assumed` is kept
apart from `authorities` because the reader's move differs: an authority can be
consulted or distrusted by name, an assumption can only be accepted or rejected.
The bivalence a classical tautology needs — which the kernel has no store to
verify — is recorded there too, for the same reason.

Premises are backward indices, so a certificate is acyclic by construction and
needs no cycle detection. A certificate may prove *less* than the evaluator
claimed, which is honest; it may never prove more.

**A certificate is a term.** `to_term` encodes one as `(proof (step by-told …) …)`
and `from_term` decodes it, exactly — every field, including the ones the checker
rejects on, because an encoding that dropped what makes a step falsifiable would
make every stored certificate valid. Without this, §5.6's claim that a derivation
is "an object the memory can store, transmit, and re-check" held only inside one
process. Encoded, it persists through the object table and — the part that
pays — is queryable: *"which conclusions rest on the assertion I am about to
retract"* is a question about a graph, and a certificate outside the graph cannot
be asked it.

## 5.7 Graded belief

`Credence` is an **interval** in log-odds milli-units, not a point. Interval
because Dekel–Lipman–Rustichini proved no single-space two-valued representation
can host an agent unaware of some propositions, and every escape makes
expressibility an index separate from truth — which is what the two-sided bound
already does. Log-odds because independent evidence adds there, and the ledger
already stores `llr_milli` deduplicated by origin, so corroboration compounds
while hearing the same thing twice does not.

**`None` is not `[0,1]`.** An unknown credence over a statable proposition is a
wide interval; a proposition the vocabulary cannot state has no interval at all.
Heifetz–Meier–Schipper make this exact — belief with probability ≥ 0 coincides
with awareness — and Piermont shows they are behaviourally distinct: refusing to
commit to any plan cannot be produced by any amount of uncertainty under full
awareness.

**On vocabulary growth the evaluator does not renormalise; it re-derives.**
Mahtani's pair of formally isomorphic awareness-growth episodes demand opposite
invariants, so no constraint on prior credences can determine the posterior — the
missing information is provenance, not credence. This system stores provenance,
so the honest operation is recomputation against the same origin-indexed ledger
over the enlarged vocabulary. Being derivation-based rather than
credence-propagating is what makes that well-defined, and it is why no
belief-revision rule is needed.

**Composition takes only the bounds that need no independence assumption.** A
conjunction is no likelier than its least likely conjunct and a disjunction no
less likely than its likeliest disjunct; the other side of each is vacuous, and
it is reported as vacuous. Two individually near-certain claims can be jointly
impossible, so a floor for a conjunction would be an assumption nobody made.
Adding log-odds — which is the right operation for independent evidence bearing
on *one* proposition, and is what the ledger does — is exactly how a system ends
up confident that six 90%-likely things are all true at once. An ungraded part is
skipped rather than treated as vacuous, because the bounds hold over any subset
of the parts.

`(likely P)` reads the interval: supported when it lies wholly above even odds,
refuted when wholly below, `Open/Exact` when it straddles, and **stalled when
there is no interval at all** — the ungraded case, which must not look like the
straddling one.

## 5.8 The axes, as questions

The evaluator computes grounding, derivation, determinacy and the defeater set on
every query. Until they had terms, none of it could be written down or asked
about, which is a strange property for a memory: it worked out its own epistemic
status for its own benefit.

`(grounded P)`, `(defeasible P)`, `(determinate P)`, `(presumed P)`,
`(defeated-by P D)`, and `(defeaters P)` as a domain. The last is the one that
was most missing — *"what would change my mind about X"* was computed on every
query and unaskable — and making it a **domain** rather than only a binary check
is what lets it be counted, quantified over, and joined against what else is
believed.

Two rules keep these honest.

**A claim about a sentence is not the sentence.** `(grounded L)` for the liar is a
perfectly grounded `Refuted`. Every other compound inherits the axes of what it
contains — that is what makes axis loss unexpressible — so reflection is the one
place that must opt out, and it does so in one function rather than per operator.
`provable`, the only reflective operator that existed before, did not opt out.

**An axis read off an unfinished evaluation is a guess.** A defeasible step the
budget never reached would make `(defeasible P)` answer *no* about a conclusion
that is defeasible — the manufacture-a-verdict-from-absence defect, one level up
in the language — so these require an exact evaluation and otherwise propagate
its compute status. `(defeasible P)` additionally requires a verdict to classify:
`Observed` is the struct default, and reading it off an `Open` result would report
"not a default" about a question nobody answered. And `(determinate P)` separates
*declared not sharp* from *nobody has said*: the first refutes, the second stalls.

## 6. Assertion

An assertion is a **separate record with its own derived identity**, never keyed
by its proposition. The same proposition is routinely asserted by different
agents, in different worlds, over different intervals, with opposite polarity;
keying on the proposition would let one silently overwrite another.

```
Assertion { id, proposition, polarity, asserting_agent?, world?,
            valid_from?, valid_to?, recorded_at, modality?,
            interpretation?, scope? }
```

`id` is derived from *all* fields — **including `recorded_at`** — so it is
idempotent only for a byte-identical record. That is narrower than it sounds and
the distinction matters: learning the same external claim again tomorrow is a
*new observation of an old proposition*, and it gets a new id, correctly.

What follows is that an assertion id is an **observation** identity and cannot
double as a deduplication key. Two sources reporting the same thing, or one
source re-read, are distinct records by design — that is how independent
corroboration stays visible — so suppressing a genuine duplicate needs a
separate source-event key (which log, which offset), never the assertion id. Valid time (when the claim holds)
and transaction time (when we learned it) are separate, which is what makes
"what did I used to believe" answerable without a liveness flag standing in for
history.

## 7. `=` versus `same-as`

- **`=`** is computational value equality — `(= (+ 2 2) 4)`. Decided, not
  asserted; nothing revises it.
- **`same-as`** is defeasible coreference: a claim that two names denote one
  thing. Asserted, can be wrong, can be retracted, can be contradicted.

Four rules follow:

**Never merge destructively.** Coreference is applied as a *view* over stored
ids. The originals survive, which is what makes retraction possible at all.

**Substitution stops at intensional contexts, and it stops per *position*.**
`quote`, `believes`, `asserted-by`, and the modal operators mention some of
their arguments rather than using them. Rewriting inside one is how "Lois
believes Superman flies" silently becomes "Lois believes Clark Kent flies".

A hand-written list of *operators* is not enough, because the barrier is not
uniform across an operator's arguments: `believes(agent, proposition)` is
extensional in the agent and intensional in the proposition, and a rule that
blocks both loses coreference on agent names for no reason while a rule that
blocks neither is the Lois Lane bug. So every privileged operator **declares
which of its positions are extensional** — `wk::extensional_positions` — and any
layer that applies coreference consults it.

This was enforced in the evaluator and undone one layer below it. The evaluator
passed intensional arguments through unsubstituted, and `RelationalView::known`
then canonicalised *every* argument position through the `is` union-find before
the lookup. The barrier was real in one file and absent in the next.

**Retraction removes what depended on it.** A conclusion drawn through an
identity is only as good as the identity.

**Conflicted identity yields conflicted conclusions**, not a merge. If the store
holds both `same-as(a,b)` and evidence against it, downstream results inherit
`Conflicted` rather than silently picking one entity.

## 8. What cannot be expressed

Two limits, both theorems.

**Cardinality.** Finite syntax can individually name or finitely define at most
countably many denotations, even when its semantic domains are uncountable. So
every uncountable domain contains elements — and functions, relations and
subsets — that no expression names. Note what this does *not* forbid:
quantifying over unnamed members of an uncountable domain is fine, since the
quantifier names the domain rather than its members.

**Computability.** Many representable propositions are undecidable, or decidable
only by unbounded search. Representation is total; evaluation is what is allowed
to be partial, and it says which of `Exact`, `BudgetExhausted`, `Unsupported`
and `Stalled` applies.

Everything else is a dial: depth, budget, arity, universe level, vocabulary and
domain size are parameters we chose, not ceilings the design imposes.

## 9. Writing facts in it

Closed above and open below: **the 101 operators are fixed, the predicates and
entities are yours.**

**A qualifier that varies is an argument, not part of the name.** Not
`file-line-count-after`; `(at t3 (file-line-count f 40))`. Not
`harness-forbids-interactive-commands`; `(forbids harness interactive-commands)`.
A name that composes is a name that cannot be reasoned over.

**Use the operators that exist.** Negation is `not`. Conjunction is `and`. Time
is `at`. Attribution is `asserted-by`. Identity is `same-as`. Quotation is
`quote`.

**Give a relation the arity the fact has.** Do not decompose a genuinely 4-place
relation into two binary ones, and do not inflate a binary one.

**An entity with no name is a skolem** — written `#1a2f`, minted by `fresh()` —
and its identity to anything else is stated with `same-as`, or left open.

**Everything durable is a fact.** Mundanity is not a filter. What a file
contains, what a command returned, what the agent did. Salience is a
retrieval-time question, not a write-time one.
