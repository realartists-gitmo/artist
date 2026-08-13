# FFF Integration Plan for Artist `find` and `grep`

## Goal

Replace Artist's current filesystem `find` and `grep` implementations with FFF-backed search without allowing FFF's API, query language, result types, fallback behavior, or lifecycle to leak into Artist's public abstraction.

The invariant is:

```text
Artist contract
    ↓
Artist semantics
    ↓
FFF implementation machinery
    ↓
Artist result
```

Never:

```text
Artist tool
    ↓
FFF API exposed directly
```

Artist owns:

```text
URI semantics
Pattern semantics
AnchoredText
anchors
errors
WIT contracts
tool composition
capabilities
WASM verb surface
```

FFF owns:

```text
filesystem indexing
filesystem watching
fast path matching
fast content matching
fuzzy scoring
search acceleration
```

---

# 1. Vendor FFF

Vendor a pinned upstream FFF revision into:

```text
vendor/fff/
```

Do not depend on a moving Git branch.

Record the exact upstream revision in something like:

```text
vendor/fff/UPSTREAM
```

containing:

```text
repository: dmtrKovalenko/fff
commit: <exact commit>
```

Preserve the upstream license and notices.

FFF is MIT-licensed. 

Vendor the upstream workspace rather than copying random implementation files out of context.

Current FFF packages relevant to us are centered around:

```text
fff-search
fff-grep
fff-query-parser
```

with `fff-search` currently living at `crates/fff-core`.  

Artist should path-depend on the vendored crates.

Do not make the FFF workspace part of the Artist workspace unless Cargo requires it.

Prefer keeping:

```text
Artist workspace
vendor/fff nested upstream workspace
```

separate.

Verify with:

```text
cargo metadata
cargo check --workspace
cargo test --workspace
```

Use FFF's default pure-Rust filesystem backend initially.

Do not introduce the optional Zig/zlob build requirement merely for performance. The vendored `fff-search` crate already provides a default pure-Rust backend; zlob is optional. 

---

# 2. Keep FFF Out of the Public Contract

Do not add FFF types to Artist WIT.

Do not expose:

```text
GrepMode
GrepMatch
FileItem
FFFQuery
FuzzySearchOptions
GrepSearchOptions
frecency scores
FFF pagination cursors
FFF constraints
```

through Artist tools.

The existing Artist contracts remain conceptually:

```text
find:
    Pattern × roots -> UriList

grep:
    Pattern × Resources|AnchoredText -> AnchoredText*
```

The FFF adapter is below this boundary.

---

# 3. FFF Must Be Host-Resident

Do not put the resident FFF index inside invocation-scoped `find` or `grep` WASM components.

FFF's useful architecture is a long-lived `FilePicker` with background scanning, shared state, filesystem watching, and reusable indexes. 

Therefore:

```text
find WASM component
grep WASM component
        ↓ typed host imports
Artist search substrate
        ↓
resident FFF indexes
```

The WASM components remain the verb implementations and semantic boundary.

The native host owns the persistent filesystem-search machinery.

This does not violate the WASM-component architecture.

It follows the existing split:

```text
WASM verb = operation semantics

host resource substrate = privileged persistent machinery
```

Implement a native Artist search service, approximately:

```rust
struct SearchService {
    indexes: ...,
}
```

It owns reusable FFF indexes keyed by canonical filesystem root.

For example:

```text
file:///repo-a/
    -> resident FFF picker A

file:///repo-b/
    -> resident FFF picker B
```

Repeated calls reuse the same index.

Do not rebuild the FFF index per `find` or `grep`.

---

# 4. Authority Still Belongs to Artist

FFF must never decide what filesystem tree it is authorized to search.

Artist resolves:

```text
requested URI
    ↓
capability check
    ↓
authorized file:// root
    ↓
FFF
```

FFF must only receive filesystem roots already approved by Artist.

Do not permit FFF convenience behavior such as scanning `$HOME`, `/`, or another broader root to bypass Artist capabilities.

The search substrate must not acquire authority independently.

---

# 5. Add One Shared `Pattern` Grammar

`find` and `grep` use one Artist-defined pattern language.

At the WIT level this can remain a string alias:

```wit
type pattern = string;
```

Then:

```wit
record find-request {
    roots: list<uri>,
    query: pattern,
}

record grep-request {
    pattern: pattern,
    source: grep-source,
}
```

Both fields use exactly the same grammar.

Internally parse into:

```rust
enum Pattern {
    Literal(String),
    Regex(String),
    Fuzzy(String),
}
```

No model-facing `mode` parameter exists.

No programmatic `mode` parameter exists.

---

# 6. Prefix Grammar

The grammar is:

```text
X
    => literal X

lit:X
    => literal X

re:X
    => regex X

fz:X
    => fuzzy X
```

Examples:

```text
foo
    literal "foo"

src/foo.rs
    literal "src/foo.rs"

repo://project/pr/12
    literal "repo://project/pr/12"

lit:re:foo
    literal "re:foo"

re:foo.*bar
    regex "foo.*bar"

fz:usrcontrler
    fuzzy "usrcontrler"
```

Prefix recognition occurs only at byte zero.

Only these exact prefixes are special:

```text
lit:
re:
fz:
```

Everything else is bare literal text.

Therefore:

```text
http://foo
repo://x/y
src/foo/bar.rs
foo:bar
whatever:baz
```

remain literals.

There is no slash-based regex syntax.

Paths and URIs therefore require no special escaping.

---

# 7. Prefix Escaping

`lit:` is the escape mechanism.

To search literally for:

```text
re:foo
```

use:

```text
lit:re:foo
```

To search literally for:

```text
fz:foo
```

use:

```text
lit:fz:foo
```

To search literally for:

```text
lit:foo
```

use:

```text
lit:lit:foo
```

The prefix parser does not implement another backslash-escaping language.

After the prefix is removed, the remaining payload is passed unchanged to the selected matcher.

JSON, Bash, Rust, Python, etc. retain only their normal transport-level escaping rules.

---

# 8. Invalid Patterns

Add:

```text
InvalidPattern
```

to the shared Artist error language.

These are invalid:

```text
""
"lit:"
"re:"
"fz:"
```

An invalid regex is also:

```text
InvalidPattern
```

Example:

```text
re:foo(
```

must fail.

It must never become:

```text
literal "foo("
```

Likewise:

```text
foo
```

returning zero literal matches must return zero matches.

It must never retry as fuzzy.

And:

```text
fz:foo
```

returning zero fuzzy matches must remain zero matches.

The fundamental rule is:

> A pattern's interpretation depends only on the pattern string, never on the corpus or previous search result.

---

# 9. Disable FFF Fallback Semantics

Artist must not inherit FFF fallback behavior.

In particular, do not allow:

```text
invalid regex
    -> literal fallback
```

Do not allow:

```text
zero literal matches
    -> fuzzy fallback
```

Do not allow:

```text
FFF query constraints produced zero matches
    -> reinterpret whole query as literal
```

Artist parses the pattern before FFF sees it.

FFF receives an already resolved semantic mode:

```text
Literal
Regex
Fuzzy
```

If current upstream APIs make deterministic invocation impossible, patch the vendored copy.

That is an acceptable and expected reason to vendor.

---

# 10. Do Not Use FFF's Query Language as Artist's Query Language

FFF has its own query parser and constraint syntax.

Artist does not inherit it.

Therefore bare patterns such as:

```text
git:modified
!foo
*.rs
test/
```

must not spontaneously acquire FFF-specific meanings.

Under Artist:

```text
git:modified
```

means literal `git:modified`.

```text
*.rs
```

means literal `*.rs`.

If regex behavior is desired:

```text
re:.*\.rs
```

If fuzzy behavior is desired:

```text
fz:mainrs
```

Do not feed Artist's raw pattern through an inference-heavy FFF query parser.

Use or add lower-level FFF APIs which accept the already parsed matcher type and payload.

---

# 11. Determinism Over FFF Personalization

Do not allow frecency or query-history state to change the canonical results of programmatic Artist `find`.

Artist search should initially be deterministic from:

```text
pattern
+
current resource corpus
+
pinned search implementation
```

not:

```text
what files happened to be opened recently
```

FFF's indexing and fuzzy algorithms are useful.

Its optional personalized ranking is not part of the Artist contract.

For `find`, define deterministic ordering.

Recommended:

```text
Literal:
    URI lexical order

Regex:
    URI lexical order

Fuzzy:
    fuzzy score descending
    then URI lexical order
```

Do not include frecency in that score.

If personalized ranking is ever wanted later, make that an explicit architectural decision rather than accidentally inheriting it from FFF.

---

# 12. `find` Implementation

Artist `find` searches resource names.

It does not search contents.

For:

```text
file://
```

use the resident FFF filesystem index as the candidate corpus.

Then apply the parsed Artist `Pattern`.

Conceptually:

```rust
match pattern {
    Literal(text) => find_literal(index, text),
    Regex(regex)  => find_regex(index, regex),
    Fuzzy(query)  => find_fuzzy(index, query),
}
```

FFF already has the indexed filesystem corpus and fuzzy machinery.

If its public API only exposes fuzzy/query-parser-oriented search, patch the vendored code to expose raw deterministic candidate/matcher operations.

Do not force Artist semantics through an inappropriate high-level FFF API.

The final result is always:

```rust
Vec<Uri>
```

Example:

```text
file:///repo/src/main.rs
file:///repo/src/parser.rs
```

Directory URIs must preserve Artist's trailing-slash rule:

```text
file:///repo/src/
```

No FFF path object escapes the adapter.

---

# 13. `find` Across Virtual Namespaces

Artist `find` is not filesystem-only.

This must continue working conceptually for:

```text
repo://
tools://
agents://
processes://
session://
future namespaces
```

FFF therefore cannot become the definition of `find`.

For non-filesystem namespaces:

```text
namespace
    ↓
enumerate candidate resource URIs
    ↓
Artist Pattern matcher
    ↓
Vec<Uri>
```

Use the same pattern semantics.

For example:

```text
find(
    roots = ["tools://"],
    query = "fz:grepp"
)
```

must use the same FFF-derived fuzzy matcher semantics as filesystem fuzzy path search.

If necessary, extract/vendor the relevant FFF fuzzy scorer behind a pure function:

```rust
score_fuzzy_path(
    pattern: &str,
    candidate: &str,
) -> Option<Score>
```

That scorer must not require a `FilePicker`.

This is another acceptable vendor patch.

---

# 14. `grep` Implementation

Artist `grep` has two source forms:

```rust
enum GrepSource {
    Resources(Vec<Uri>),
    Text(Vec<AnchoredText>),
}
```

Both must preserve the same pattern semantics.

---

# 15. `grep(Resources)` With `file://`

For filesystem resource inputs:

```text
Artist URIs
    ↓
authorized filesystem paths
    ↓
resident FFF grep
    ↓
FFF matches
    ↓
Artist anchor conversion
    ↓
AnchoredText
```

FFF must not determine the public result structure.

The final result remains:

```rust
Vec<AnchoredText>
```

grouped by source URI.

---

# 16. FFF Grep Results Are Not Sufficient as Artist Results

Do not directly convert:

```text
FFF line_number
FFF line_content
```

into Artist output and call it done.

Artist requires:

```text
exact line text
exact line ending
stable Artist anchor
correct resource URI
```

FFF currently exposes useful implementation coordinates such as:

```text
line number
byte offset
column
match byte offsets
```

but its `line_content` representation is allowed to be truncated. 

Therefore:

> `GrepMatch.line_content` is not authoritative Artist file content.

---

# 17. Search Snapshot Consistency

This race is forbidden:

```text
FFF searches file version A

FFF returns:
    line 81
    byte offset 5000

file changes to version B

Artist reopens current file

Artist anchors line 81 from version B
```

That can produce an anchor for a line FFF never matched.

The FFF integration must bind the match to the exact searched file snapshot.

Preferred implementation:

```text
FFF match
    file identity
    snapshot generation/hash
    byte offset
        ↓
exact searched bytes
        ↓
Artist line/anchor engine
```

If FFF does not expose the searched snapshot cleanly, patch the vendored implementation.

Acceptable forms include:

```rust
struct SearchSnapshot {
    bytes: Arc<[u8]>,
    generation: ...,
}
```

or an equivalent stable handle.

The important invariant is:

> The bytes used to create Artist anchors must be the same bytes against which the match was produced.

Never repair this by trusting line numbers after rereading a mutable file.

---

# 18. Full Lines and Line Endings

FFF integration must provide Artist with the complete matched source line.

Do not use truncated display text.

Artist must reconstruct:

```rust
AnchoredLine {
    anchor,
    text,
    ending,
}
```

including:

```text
LF
CRLF
CR
NONE
```

Correctness tests must include:

```text
LF file
CRLF file
CR file
file without final newline
blank line
very long line
tab-indented line
```

`grep` must return exactly the same line representation that `read` would return for that line.

This invariant matters:

```text
grep(X) line
==
corresponding read(X) line
```

including URI, anchor, text, and line ending.

---

# 19. `grep(Text)` Must Not Reopen Resources

For:

```rust
GrepSource::Text(Vec<AnchoredText>)
```

search exactly the supplied text.

Do not:

```text
extract URI
reopen file
search new contents
```

The input itself is the corpus.

Example:

```text
read
  ↓ AnchoredText
grep
```

must preserve the original anchors.

If input is:

```text
#A    foo
#B    target
#C    bar
```

then:

```text
grep("target")
```

returns:

```text
#B    target
```

with the exact same `#B`.

This is what enables:

```text
read -> grep -> edit
poll -> grep -> edit
write -> grep -> edit
```

without identity translation.

---

# 20. Pure Matcher Layer

To make `grep(Text)` and virtual-resource search use the same behavior as filesystem FFF search, expose a pure matching layer.

Conceptually:

```rust
trait TextMatcher {
    fn matches_line(...) -> ...
}
```

with implementations:

```text
LiteralMatcher
RegexMatcher
FuzzyMatcher
```

Prefer reusing/extracting FFF's implementation rather than writing unrelated algorithms.

The vendored `fff-grep` crate is already a small grep-oriented crate, so inspect whether it can supply the lower-level machinery cleanly before copying code. 

If FFF's fuzzy matcher remains coupled to `FilePicker`, expose the necessary pure matcher through a small vendor patch.

Do not create two different fuzzy definitions:

```text
FFF fuzzy for file://
Artist fuzzy for AnchoredText
```

They must mean the same thing.

---

# 21. Regex Semantics

`re:` uses one documented regex syntax.

Use the same Rust regex semantics used by the FFF integration where possible.

Compile before searching.

Failure:

```text
re:(
```

returns:

```text
InvalidPattern
```

immediately.

Never pass malformed regex into an API which silently converts it into a literal search.

---

# 22. Fuzzy Semantics

`fz:` invokes FFF-derived fuzzy matching.

Fuzzy search is heuristic by definition, but must remain deterministic for:

```text
same Artist version
same pinned FFF version
same query
same corpus
```

Do not mix in:

```text
frecency
query history
randomness
caller identity
model identity
previous zero-match state
```

The match set/ranking may change when the vendored FFF implementation is deliberately upgraded.

That is an implementation-version change, not runtime modality.

Add golden tests for fuzzy behavior important to Artist.

---

# 23. No Silent Partial Results

Do not inherit FFF's UI-oriented pagination or time-budget behavior as invisible Artist semantics.

If Artist's non-streaming `find` contract returns:

```text
Vec<Uri>
```

then it must not silently mean:

```text
first FFF page of Uri
```

Likewise:

```text
grep -> Vec<AnchoredText>
```

must not silently stop after FFF's internal page limit.

The adapter must either:

```text
collect all internal FFF pages
```

or use Artist's typed streaming surface.

Any global safety limit imposed by Artist must be explicit in Artist semantics.

FFF pagination is an implementation mechanism.

It is not the public contract.

---

# 24. WASM Boundary

The public verb boundary remains:

```text
artist:tool/find
artist:tool/grep
```

The model calls:

```text
find(...)
grep(...)
```

The named tools execute their WASM components.

Those components use typed host capabilities.

The persistent native FFF service sits behind the host resource/search capability.

Do not create:

```text
fff://
```

Do not expose:

```text
fff_find
fff_grep
```

to the model.

Do not make FFF a parallel tool system.

---

# 25. No JSON in the Search Core

Do not integrate FFF through:

```rust
serde_json::Value
```

The path should be typed:

```text
model JSON
    ↓ adapter
typed Artist request
    ↓
find/grep WASM component
    ↓
typed host import
    ↓
Artist SearchService
    ↓
FFF
```

and back:

```text
FFF
    ↓
typed Artist adapter
    ↓
UriList / AnchoredText
    ↓
typed component result
    ↓
model adapter
```

FFF integration must participate in the convergence away from the old generic JSON kernel path, not reinforce it.

---

# 26. Recommended Internal Structure

Something approximately like:

```text
crates/
    artist-search/
        src/
            lib.rs
            pattern.rs
            fff.rs
            find.rs
            grep.rs
            snapshot.rs

vendor/
    fff/
```

`artist-search` is native host machinery.

Responsibilities:

```text
pattern.rs
    parse Artist prefix grammar

fff.rs
    own resident FFF indexes
    translate authorized file roots
    wrap/pin vendored APIs

find.rs
    implement Artist find semantics

grep.rs
    implement Artist grep semantics
    convert matches into AnchoredText

snapshot.rs
    guarantee grep-match / anchor snapshot consistency
```

The precise crate name is not important.

The separation is.

---

# 27. Required Vendor Patches

Only patch FFF where the public upstream API cannot satisfy Artist semantics cleanly.

Likely patches include:

### A. Deterministic raw grep entrypoint

Expose something equivalent to:

```rust
grep_raw(
    pattern,
    mode,
    files,
    ...
)
```

with:

```text
no query inference
no regex fallback
no literal fallback
no fuzzy fallback
```

### B. Pure fuzzy matcher

Expose FFF fuzzy matching independently of filesystem picker state so virtual URIs and `AnchoredText` can use identical semantics.

### C. Snapshot-aware grep result

Expose enough state to map a match back to the exact searched bytes.

For example:

```text
snapshot handle/generation
byte offset
full line boundaries
```

### D. Deterministic file ranking

Expose path fuzzy score without frecency/query-history contamination.

Keep patches minimal and isolated.

Document them in:

```text
vendor/fff/ARTIST_PATCHES.md
```

Each patch should explain:

```text
upstream behavior
why it violates Artist semantics
our changed behavior/API
whether it could reasonably be upstreamed
```

---

# 28. Tests: Pattern Grammar

Required:

```text
"foo"
    => Literal("foo")

"src/foo.rs"
    => Literal("src/foo.rs")

"repo://foo/pr/1"
    => Literal("repo://foo/pr/1")

"lit:re:foo"
    => Literal("re:foo")

"lit:fz:foo"
    => Literal("fz:foo")

"lit:lit:foo"
    => Literal("lit:foo")

"re:foo.*bar"
    => Regex("foo.*bar")

"fz:usrcontrler"
    => Fuzzy("usrcontrler")
```

Failures:

```text
""
"lit:"
"re:"
"fz:"
"re:("
```

must return:

```text
InvalidPattern
```

---

# 29. Tests: No Fallback

Explicitly prove:

```text
Literal with zero results
    != fuzzy retry

invalid Regex
    != literal retry

Fuzzy with zero results
    != literal retry

bare "*.rs"
    != implicit glob

bare "!foo"
    != implicit exclusion

bare "git:modified"
    != implicit FFF constraint
```

These tests are mandatory.

---

# 30. Tests: Composability

Prove the actual compositions:

```text
find -> read

find -> grep(Resources)

read -> grep(Text)

write -> grep(Text)

poll -> grep(Text)

grep -> edit
```

For:

```text
read -> grep
```

the grep result must preserve anchors from `read`.

For:

```text
poll -> grep
```

the grep result must preserve anchors from `poll`.

For:

```text
grep -> edit
```

the returned match anchor must successfully address the intended line without rereading.

---

# 31. Tests: Filesystem Correctness

Test:

```text
file created after index startup
file changed after index startup
file deleted after index startup
directory created after index startup
directory deleted after index startup
```

FFF's resident watcher/index must reflect them correctly.

Also test:

```text
two repository roots
root switching
same root reused across searches
```

Searches must never leak across authorized roots.

---

# 32. Tests: Snapshot Race

Construct a test where:

```text
1. version A contains target at line N
2. FFF produces a match against A
3. file becomes version B before Artist conversion
4. Artist converts result
```

The result must never claim that an unrelated line from B is the match.

Allowed strategies:

```text
return result anchored to snapshot A
```

or:

```text
detect stale generation and retry against B
```

Not allowed:

```text
silently anchor B's current line N
```

---

# 33. Tests: Line Fidelity

For every supported line ending:

```text
LF
CRLF
CR
NONE
```

prove:

```text
grep match
==
same AnchoredLine returned by read
```

Also test:

```text
very long matched line
blank line
tabs
Unicode
final line without newline
```

No display truncation is allowed in Artist `AnchoredText`.

---

# 34. Tests: Cross-Namespace Semantics

Create the same textual/name corpus in:

```text
file://
session://
repo://
```

or equivalent test namespaces.

Verify:

```text
literal pattern
regex pattern
fuzzy pattern
```

have equivalent matching semantics.

FFF acceleration may differ.

Artist meaning must not.

---

# 35. Tool Descriptions

Model-facing descriptions should explain the prefix language succinctly.

For `find`:

```text
Search resource names under one or more roots.

Patterns:
- bare text or lit:TEXT — literal
- re:REGEX — regex
- fz:TEXT — fuzzy
```

For `grep`:

```text
Search textual resource contents.

Patterns:
- bare text or lit:TEXT — literal
- re:REGEX — regex
- fz:TEXT — fuzzy
```

Do not expose a mode enum.

Do not explain FFF to the model.

FFF is an implementation detail.

---

# 36. Implementation Order

Do this in order.

## Phase 1 — Vendor

```text
vendor pinned FFF
preserve license
record upstream revision
verify Cargo builds
```

## Phase 2 — Pattern

Implement the shared Artist `Pattern` parser.

Add:

```text
InvalidPattern
```

Add exhaustive parser tests.

## Phase 3 — Native Search Service

Create the long-lived Artist search substrate.

Initialize/reuse authorized FFF indexes.

Disable personalization/frecency effects on canonical ordering.

## Phase 4 — Vendor APIs

Patch FFF only as necessary to expose:

```text
raw deterministic matching
pure fuzzy scoring
snapshot-consistent grep data
```

## Phase 5 — `find`

Replace current filesystem `find`.

Return only canonical Artist URIs.

Then support generic virtual-namespace candidate matching using the same pattern engine.

## Phase 6 — `grep`

Replace current filesystem grep.

Convert FFF matches into exact `AnchoredText`.

Implement `GrepSource::Text` using the same matcher semantics.

## Phase 7 — WASM Wiring

Wire the typed:

```text
host-find
host-grep
```

imports to the native search substrate.

Keep:

```text
artist:tool/find
artist:tool/grep
```

as the public WASM verb components.

Remove any old JSON search path that bypasses these contracts.

## Phase 8 — Conformance

Run all:

```text
pattern
fallback
composition
snapshot
line-ending
watcher
cross-namespace
WASM contract
```

tests.

Only after those pass should Bash/Python adapters depend on `find` and `grep`.

---

# 37. Final Invariants

The completed implementation must satisfy all of these:

```text
FFF is vendored implementation machinery.

FFF does not define Artist's public types.

FFF does not define Artist's query grammar.

FFF does not define Artist's errors.

FFF does not leak filesystem paths instead of URIs.

FFF does not leak line numbers instead of anchors.

FFF does not silently change matching strategy.

FFF does not silently truncate programmatic results.

FFF does not acquire filesystem authority.

FFF does not become another model-facing tool subsystem.

find always returns Artist URIs.

grep always returns Artist AnchoredText.

grep(Text) preserves input anchors.

file grep anchors the exact searched snapshot.

bare patterns are literal.

lit: explicitly means literal.

re: explicitly means regex.

fz: explicitly means fuzzy.

the same Pattern means the same thing from JSON, Rust, Python, Bash, and WASM.

the public find and grep verbs remain WASM components.
```

The desired final architecture is:

```text
                  ┌─ file:// ── resident FFF index/search
                  │
find WASM ────────┤
                  └─ virtual namespace candidates ── same Artist matcher
        ↓
      UriList


                  ┌─ file:// ── resident FFF grep ── snapshot → anchors
                  │
grep WASM ────────┼─ virtual resources ── AnchoredText matcher
                  │
                  └─ supplied AnchoredText ── same matcher
        ↓
   AnchoredText
```

FFF makes Artist search fast.

Artist continues to define what search means.