# OpenCog Classic, Hyperon, and OmegaClaw — audit

What three neuro-symbolic architectures have that `artist-logic` + `artist-memory`
do not, what we should take, and where we already win. Companion in spirit to
[the computer-use steal list](computer-use-steal-list.md): grouped by what an item
buys, each with a **fit** verdict, and with outright rejections stated rather than
smoothed over.

**Why these three.** They are the only serious prior art for the thing this branch
is actually building — a durable symbolic store that an LLM writes into and reasons
over. OmegaClaw in particular is the closest existing system to Artist's shape:
an LLM agent whose memory and control loop are both symbolic expressions.

**Verification standard.** Claims below are marked as *read* (from docs) or
*verified* (from source). Where a project's marketing and its source disagree, the
source is quoted.

---

## 0. What each one is

**OpenCog Classic** (`opencog/atomspace`, ~2008–2020, now largely in maintenance).
A typed metagraph database — "immutable, globally unique, typed s-expressions"
called Atoms — with a pattern matcher, an inference layer (PLN), a rule engine
(URE), an attention economy (ECAN), and a program-evolution component (MOSES).
The wiki marks ECAN "Obsolete" and URE a "fossil"; the AtomSpace repo lists
TruthValues, BindLink and GetLink as being phased out. The database survived; the
cognitive layer did not.

**OpenCog Hyperon** (`trueagi-io/hyperon-experimental`). The rewrite. Atomese is
replaced by **MeTTa** — four atom kinds (Symbol, Expression, Variable, Grounded),
equality-based rewriting `(= <pattern> <body>)`, gradual typing with `%Undefined%`
and `Atom` as top, and *nondeterminism as the core evaluation mode* (`superpose` /
`collapse-bind`), which is what makes the interpreter double as an inference
engine. Storage is either the in-process Space, **DAS** (MongoDB + Redis +
OpenFaaS, network-served), or **MORK**, a zipper-trie kernel claiming
"thousands to millions of times" speedup. The MeTTa spec states **no soundness or
termination properties**; the only stated failure mode is `StackOverflow`.

**OmegaClaw** (`asi-alliance/OmegaClaw-Core`). SingularityNET's agent framework on
Hyperon. A ~200-line MeTTa core (`src/loop.metta`) driving a continuous,
non-turn-based loop; memory in `src/memory.metta` (ChromaDB embeddings + a
`history.metta` rolling buffer); `lib_pln.metta` and `lib_nal.metta` for
inference; Python plugins and chat channels (Telegram, IRC, Slack, Mattermost).
Marketed as "auditable inference," "auditable proof trails," "autonomous
self-improvement," and a "three-tier memory architecture."

---

## 1. Where we clearly win

Not a victory lap — each of these is a place where a design decision we already
made and paid for is one they made differently and worse, so the item is closed
and should not be reopened.

### 1.1 Derivations are objects; theirs are not

**Verified.** `lib_pln.metta`'s deduction rule is:

```
(= (Truth__Deduction (stv $Ps $Pc) (stv $Qs $Qc) (stv $Rs $Rc)
                     (stv $PQs $PQc) (stv $QRs $QRc))
   (if (and (conditional-probability-consistency $Ps $Qs $PQs) …)
       (stv … …)
       (stv 1 0)))
```

The conclusion is a bare `(stv strength confidence)`. It records nothing about
which premises produced it. The file contains no occurrence of *proof*, *trail*,
*derivation*, *justification*, or *premise*. Conclusions are then run through
`(unique-atom (collapse (superpose …)))`, which deduplicates the result set —
discarding, by construction, the distinction between "one derivation" and "five
independent ones."

We emit `Step::Rule { node, rule, premises, defeasible }` into a `Certificate`
(`graph_eval.rs:2440`), and `certificate.rs` re-checks it against the graph without
knowing anything about resolvers, budgets, rules or scans. That is the de Bruijn
criterion. OmegaClaw's "auditable proof trails" is, as far as the source shows, a
claim about the LLM's chain-of-thought text, not about a checkable object.

**Consequence:** stop treating "auditable inference" as a differentiator we still
need to build. It is built, and it is the strongest single thing we have.

### 1.2 Five axes versus two numbers

PLN and NAL both compress everything into a pair — `(strength, confidence)` for
PLN, `(frequency, confidence)` for NAL. `docs/formalism.md` §5.1 documents, with
the bugs that forced each split, why that pair is not enough: evidential state,
`ComputeStatus`, `Grounding`, `Derivation` and `Determinacy` are five independent
axes, and collapsing any two of them produced a specific wrong answer we shipped
and then fixed. PLN's `confidence` is doing the work of at least four of ours at
once — "weakly evidenced," "out of budget," "holds by default," and "not a sharp
predicate" are all just a low number.

The `(stv 1 0)` fallback in the deduction rule above is the sharp end of this: a
consistency-check failure returns *maximum strength, zero confidence*, which is
indistinguishable from an honest "no information." We return `Stalled` and say why.

### 1.3 Refutation requires an authoritative resolver

AtomSpace's `AbsentLink` is negation-as-absence: the pattern matches when a subgraph
is not found, i.e. closed-world by fiat over whatever happens to be loaded. Our
resolver contract has four answers — `Holds`, `Fails`, `Denied`, `Unknown` — where
`Fails` is an explicit claim of completeness *for that query*, `Denied` is positive
evidence of a negative needing no completeness at all, and a partial index must
return `Unknown`. `is_closed(pred)` (verified, `graph_eval.rs:4195`) makes the
closed-world assumption per-predicate and opt-in.

### 1.4 Alpha-equivalence, and the intensional barrier

Stored binders are de Bruijn (`formalism.md` §2), so `(forall [(f Path)] (p f))`
and `(forall [(x Path)] (p x))` are **one object** with one content id. AtomSpace
uses named `VariableNode`s inside `BindLink`s, so two alpha-equivalent rules are two
distinct atoms that never dedup — in a store fed by an LLM that renames variables
freely, that is a permanent leak.

Separately, `wk::extensional_positions` declares *per argument position* whether
coreference may substitute — `believes(agent, proposition)` is extensional in the
agent and intensional in the proposition. Atomese has `QuoteLink`, which blocks a
whole subtree. Whole-subtree quoting is the choice between the Lois Lane bug and
losing coreference on agent names; we don't have to make it.

### 1.5 Anytime evaluation with a resumable residual

URE terminates on "number of steps applied" or "number of BIT expansions" and hands
back what it found. We hand back a **residual** — the narrowed expression still to
be decided — and, for domain scans, a **continuation that is itself an ordinary
expression**, so resuming is just evaluating it (`formalism.md` §5.5). The wiki's
own URE retrospective calls out "no sophisticated budget handling" and "no
probabilistic exploration algorithms" as the reason forward/backward chaining alone
was insufficient, and concludes the problems "suggest a major rewrite, if not a
fresh start of the design."

### 1.6 Local-first

DAS is MongoDB + Redis + OpenFaaS, deployed as Docker containers behind HTTP/gRPC.
OmegaClaw's memory is ChromaDB. Both have the degenerate case
[[memory-is-local-first]] rejects: no server, no memory. This is settled; the point
of recording it here is that the comparison **favours us on capability too**, not
just ideology — our embedder is 82–127 ms with no egress, and their embedding path
is `rag.openai_embed` by default.

### 1.7 Determinism

MeTTa's nondeterminism is its headline feature and the spec offers no soundness or
termination guarantee. We are total under a budget with monotone bounds, and
`tests/properties.rs` evaluates generated terms at rising budgets asserting neither
bound ever falls. For a memory system two machines must answer a query the same
way; that is the whole content of [[memory-is-local-first]]'s hardest open item.

---

## 2. The steal list

### Tier 1 — take these

#### 2.1 The Atoms/Values split — quarantine the mutable layer

**From:** AtomSpace Classic · **Fit:** near-perfect · **Cost:** design, then moderate

AtomSpace's central architectural decision: Atoms are immutable, globally unique,
content-indexed, and define the topology. **Values** are a separate layer — mutable,
fast-changing, *not indexed, accessible only by direct reference from an Atom*.
Each Atom is a small key-value store hosting Values. Their framing: Atoms are the
pipes, Values are the fluid.

This is the exact shape of the problem [[artist-logic-is-already-a-gset]] left open.
Our object graph is already a G-Set — write-once, content-addressed, `nodes.extend`
merges. What breaks merging is `live` and `superseded_by`, mutable columns sitting
*on the fact relation itself*, plus embeddings, retrieval counters and credence.
OpenCog independently converged on the answer: those are not properties of the
expression, they are Values hung off it, and they live in a physically separate,
non-indexed, non-merged layer.

**What to take:** the discipline and the vocabulary, not the code. Formalise a
`ValueLayer` keyed by `ObjectId`, holding exactly the things that are derived or
machine-local — `live`, `superseded_by`, embedding vector, retrieval stats,
`Credence` interval, index membership. Merging a peer replicates the graph and the
assertion log and **never** the value layer, which is rebuilt. That makes
"never merge the projection — merge the event log and rebuild" a type-level property
instead of a rule people have to remember.

**What to take with it:** their Values can be *streams* — a Value that reads a live
sensor each time it's dereferenced. That is the right shape for `(at now …)` and for
externals whose current value is a `git rev-parse`, and it is cheaper than making
every such fact a stored assertion.

**Caveat, stated because it is the thing that will bite:** OpenCog gets no
distributed story from this either. The split quarantines the un-mergeable data; it
does not make it mergeable. That is still ours to solve.

#### 2.2 Inverted matching — index rules by antecedent, not just conclusion

**From:** AtomSpace `DualLink` · **Fit:** perfect · **Cost:** small

The pattern matcher runs both directions. `MeetLink` is ordinary "fill in the
blanks." **`DualLink` is the inverse: given a concrete piece of data, find every
stored pattern that matches it.** That is rule-triggering as an index lookup.

**Verified gap on our side.** `Knowledge::rules(pred)` returns "stored rules whose
*conclusion* is about `pred`" (`graph_eval.rs:313`), and `derive` is documented
"Backward chaining: try each stored rule that concludes `pred`"
(`graph_eval.rs:2383`). We have exactly one index, keyed by conclusion. There is no
way to ask "a fact about `pred` just arrived — which stored rules could now fire?"
without scanning every rule.

**What it buys.** A store that ingests facts continuously and never forward-chains
only learns something when asked the right question. With an antecedent index, a
written fact can cheaply *notify* the rules it might trigger — which is the
precondition for materialising consequences at write time, for firing a defeater
when counter-evidence arrives, and for telling the agent "what you just recorded
contradicts something on file." Today we would only find that out if someone
happened to query for it.

**Fit note.** This is an index, not a chaining strategy. Adding the index does not
commit us to eager forward chaining, which has its own cost; it makes the choice
available. Take the index now, decide the policy separately.

#### 2.3 The incoming set — a reverse index from id to mentions

**From:** AtomSpace `JoinLink` ("what contains this?") · **Fit:** perfect · **Cost:** small

Every AtomSpace Atom knows its **incoming set**: every Link that contains it. This
is what makes "what do I know about this entity" an O(degree) lookup.

**Verified gap.** Our `mentions(f, p, depth)` (`graph_eval.rs:3090`) is a recursive
structural walk of one expression checking whether it mentions `p`. To answer "which
stored facts mention this file," we would walk every stored fact. Given that
retrieval today is embedding + BM25, the *structural* retrieval channel — the one
[[memory-is-unscoped-and-fully-embedded]] argues makes cross-project recall safe,
because "the strictness of the HOL representation makes it extraordinarily unlikely
to retrieve something genuinely out of scope" — is the channel we have no index for.

The graph is append-only, so this index is a pure derived function of it: maintain
`ObjectId -> Vec<ObjectId>` at `intern` time. It is also exactly the index that
makes §1.4's coreference views cheap to apply and cheap to retract.

#### 2.4 Long-term importance as an index-tiering currency

**From:** ECAN, heavily modified · **Fit:** good, after amputation · **Cost:** moderate

ECAN ran two currencies: **STI** governing processor time, **LTI** governing
*memory retention*. The economic dynamics failed — the wiki's retrospective is that
ECAN "depended strongly on the Agent subsystem, which was itself flawed: it
attempted to reinvent basic operating system scheduling concepts, but in a naive
and unscalable fashion," and the page is marked Obsolete. Take none of that.

Take the **separation**: retention pressure is its own signal, independent of truth
and independent of relevance-to-this-query. [[exact-vector-scan-measurements]] puts
a hard RAM cliff at 3–4M vectors on this machine, and
[[memory-is-unscoped-and-fully-embedded]] fixes the vector:memory ratio at 1, so at
the stated target the corpus does not fit. The question "which vectors are resident"
needs an answer that is not "all of them" and not "shard by project."

An LTI-analogue — a per-memory retention score from recency, access frequency,
incoming-set degree (§2.3), and whether the memory participates in a stored rule —
tiers the index: hot vectors resident, cold vectors on NVMe behind a second-pass
scan. It is a **capacity** mechanism, not a forgetting mechanism, which is what
keeps it compatible with "Everything durable is a fact. Mundanity is not a filter.
Salience is a retrieval-time question" (`formalism.md` §9). Nothing is deleted;
some things are just slower.

**This is the only item here that addresses the capacity wall.** Every other idea in
these three projects either assumes a server (DAS) or hasn't hit the wall.

### Tier 2 — take with modification

#### 2.5 Learned rule ordering, made deterministic

**From:** URE's Thompson sampling · **Fit:** only after de-randomising · **Cost:** small

URE selects which rule to apply next by **Thompson sampling over rule truth
values**, and selects sources by a fitness function compounding the probabilities
of previously applied rules. The motivation is exactly ours: under a budget, the
order you try rules in decides whether you get an answer.

We try `self.s.rules(pred)` in whatever order the store returns. Under
`MAX_RULE_DEPTH` and a step budget, a store with many rules concluding the same
predicate will spend its budget on whichever came back first.

**Take the objective, reject the mechanism.** Stochastic sampling makes two machines
answer differently, which §1.7 says we may not do. The deterministic form: keep a
per-rule success ledger (fired / led to a supported antecedent / cost in steps) as
a *Value* per §2.1, and order `rules(pred)` by it. Same benefit, reproducible given
the same log, and it degrades to insertion order on a fresh store.

#### 2.6 The continuous loop with a scheduled wake

**From:** OmegaClaw `src/loop.metta` · **Fit:** questionable · **Cost:** large

OmegaClaw's loop is genuinely not turn-based: `(omegaclaw $k)` recurses, `&loops`
counts down, `&nextWakeAt` schedules the next activation, `&prevmsg` detects
whether new user input arrived (resetting the counter to `maxNewInputLoops`, i.e.
fresh user requests preempt self-directed work), and `&lastresults` feeds the
previous cycle's tool output back into the next prompt. It is the one architectural
thing any of these three has that we structurally do not.

**Recorded, not recommended.** Artist is a coding harness; a goal-autonomous loop is
a product decision, not an architecture gap, and the surrounding machinery
(`&error` accumulation, up to 5 tool calls per response parsed out of raw LLM text
by `sread` with parenthesis-rebalancing helpers) is markedly worse than what we
already have. If continuous operation is ever wanted, the piece worth copying is
narrow: **new user input preempts and resets the self-directed budget**, rather than
queuing behind it.

#### 2.7 MORK — track, do not adopt

**From:** `trueagi-io/MORK` · **Fit:** watch · **Cost:** n/a

A zipper-trie, multi-threaded kernel for metagraph evaluation, claiming
"thousands to millions of times" speedup over baseline MeTTa, Rust (nightly).
Claims are unbenchmarked from our side and the baseline being beaten is a slow
interpreter, so the multiplier says little about absolute performance. But it is
the only *Rust* prior art for the exact data structure question we have — indexed
storage of content-addressed n-ary expressions — and it is worth reading before we
design anything in that space. Per [[track-upstream-not-backports]]: point at the
repo, do not vendor.

---

## 3. Rejected outright

Stated with reasons, because a steal list that only says yes is a wish list.

**MeTTa or Atomese as our representation.** MeTTa's nondeterminism-by-default and
absence of any termination or soundness statement are disqualifying for a store two
machines must agree about (§1.7). Atomese is being deprecated by its own authors.
Our six-shape kernel with 78 fixed operators is smaller, typed, and has the
conformance suite pinning it to the spec.

**DAS, and ChromaDB.** MongoDB + Redis + OpenFaaS, and a Python vector service.
Both violate [[memory-is-local-first]] on the settled argument, and neither buys a
capability we lack.

**PLN and NAL truth values.** §1.2. Adopting `(strength, confidence)` would undo
four documented axis splits, each of which was forced by a real bug.

**ECAN's economic dynamics.** §2.4 takes the separation and leaves the currencies.
Their own retrospective explains why.

**Attention-driven forgetting.** Directly contradicts `formalism.md` §9's
"Mundanity is not a filter. Salience is a retrieval-time question, not a write-time
one." §2.4 is deliberately scoped to tiering, not eviction.

**MOSES.** Program evolution over Atomese. Unmaintained, and the niche it occupied
is now occupied by the LLM.

**URE wholesale.** A "fossil" by its maintainers' own labelling, whose retrospective
recommends a fresh start. §2.5 takes the one idea worth having.

---

## 4. What this audit changes

Three items are new capability, all small, all in the same direction — **the
structural retrieval channel is the one we have no index for**:

1. **§2.3 incoming set** — reverse index at `intern` time. Unblocks structural
   retrieval and makes coreference views cheap.
2. **§2.2 antecedent index** — the other direction on `rules()`. Unblocks
   write-time rule triggering and contradiction detection.
3. **§2.1 value layer** — makes "never merge the projection" structural rather than
   remembered, and is a precondition for §2.4 and §2.5 having somewhere to live.

One item is the only external answer to a wall we have already measured:

4. **§2.4 LTI-as-tiering** — the residency question at 1M+ memories.

And one conclusion that is not an item: **the reasoning core is ahead of the prior
art, and the storage and indexing layer is behind it.** Every win in §1 is in
`artist-logic`; every steal in §2 is an index or a storage discipline. That is
where the next work is.
