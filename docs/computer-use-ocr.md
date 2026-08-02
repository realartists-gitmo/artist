# Rung 3: reading the screen

How the pixels rung stopped being observation-only, and what four rounds of
optimization actually bought. Companion to [computer use](computer-use.md).

Every number here was measured on this machine (16 cores, AVX2 + FMA, **no
AVX-512**) against PP-OCRv5 mobile. Every one is reproducible from a command in
this document.

---

## Why rung 3 can be acted on now

The original design made rung 3 observation-only, and the reasoning was sound as
far as it went: minting anchors from a pixel grid is coordinates wearing a hat.
The conclusion was wrong, and the mistake is worth naming precisely, because it
is the same mistake the two rejected items in the [steal
list](computer-use-steal-list.md) make.

**Coordinates are dangerous when the *model* supplies them.** A model that says
"click 430, 210" is asserting something it cannot verify and we cannot check. But
if the *harness* reads the screen, finds the text, and mints an anchor for it,
then the model says `click kv7` exactly as it does at every other rung. Nothing
about the contract changes: the model names things, ambiguity is a miss, and a
name that no longer resolves is an error rather than a click on whatever moved
into that spot.

So this rung answers one narrow question and deliberately not the general one:
**given a name the model already used, where is it on screen?** That is far
easier than transcribing a screen, because we are matching against a known
candidate rather than reading an unknown one.

## What it runs on

Two ONNX models — PP-OCRv5 mobile detection (DBNet) and recognition (SVTR) —
on **`rten`**, the pure-Rust CPU runtime `artist-memory` already ships for
embeddings.

No C++ toolchain, no ONNX Runtime, no conversion step. `rten` loads ONNX
directly (`Model::load` sniffs the format), and its own README states the
property that made it the right choice: *"End-to-end Rust. This project and all
of its required dependencies are written in Rust."* Given the `CXXFLAGS =
"-include cstdint"` line already sitting in `.cargo/config.toml` from the last
C++ dependency, not adding a second one has value beyond tidiness.

Weights are fetched, not committed:

```
scripts/fetch-ocr-models.sh
```

## The one place edit distance is allowed

`check_label` accepts equality or containment and deliberately refuses
edit-distance fuzzing, because a threshold loose enough to absorb real drift also
accepts `Cancel` for `Confirm`.

Rung 3 is the exception, and it was a **test that forced it**. The first version
reused the ordinary check on the assumption that containment would absorb a
misread glyph. It does not — our own fixture reads `SEND` back as `SENO`, which
neither equals nor contains the label. Refusing that would make the rung useless
for the reason it exists.

The difference that makes the exception safe: everywhere else, a label is
compared against a name the toolkit *told* us, where the only drift is cosmetic.
Here it is compared against a name we *guessed* from pixels. The bound is one
character in four, which separates the two cases by a wide margin:

| pair | distance | verdict |
|---|---|---|
| `seno` / `send` | 0.25 | match — one substituted glyph |
| `delcte` / `delete` | 0.17 | match |
| `cancel` / `confirm` | 0.71 | **no** |
| `delete` / `cancel` | 0.83 | **no** |

Ambiguity is still a miss: two boxes matching equally well returns nothing. That
matters *more* here than elsewhere, because noisy reads collide more readily than
exact names do.

---

## The four optimizations

Ranked by expected return before any of them was built. Three of the four turned
out to need no code at all — which is the result, not an evasion of it.

### 1. Crop to the damage region — **15.5×, built**

```
cargo run -p artist-computer --example bench-ocr --release
```

```
frame          1920x1080, 24 text boxes
damage region  260x80, 1 text boxes
full read         249.9 ms
damaged read       16.1 ms
speedup            15.5x
area ratio         99.7x
```

Detection cost scales with area, and **we are told which rectangles changed**
because we own the compositor. Every other pixel-driven harness re-reads the
whole screen because it has no idea what moved.

The speedup (15.5×) is smaller than the area ratio (99.7×) because of fixed
costs: the round-up-to-32 stride turns a 260×80 crop into 288×96, and one
recognition still runs. It is still worth more than the other three items
combined, and nothing in items 2–4 comes close.

Implemented in `ocr::Incremental`, which mirrors the anchor book on purpose — it
is the same problem. First contact reads everything; after that each look is a
delta and untouched text is carried forward. Damage is grown by a margin before
reading (a caret blinking inside a word reports a two-pixel sliver, and reading
only that sliver would crop the word in half), coalesced when regions overlap,
and abandoned in favour of one honest full read past 45% coverage.

### 2. Fold BatchNorm at export — **unnecessary, measured**

```
cargo run -p artist-computer --example inspect-ocr-graph --release
```

| | Conv | BatchNorm | foldable |
|---|---|---|---|
| detector | 62 | 3 | 2 |
| recognizer | 38 | 6 | 6 |

**Paddle's exporter already folded ~95% of it.** The residue is 8 nodes in 1410.

The same scan surfaced something that looked much bigger — 300 and 342
`Constant` nodes and zero initializers, about 45% of each graph, alongside a pile
of `Add`/`Mul`/`Reshape` shape arithmetic. That looks like a large win until you
check what the runtime does with it: rten runs `propagate_constants` when the
model loads, so the arithmetic is resolved once at startup and never appears in
an inference. Models are loaded once per process and kept resident.

Simplifying at export would move that work, not remove it.

### 3. Tune the GEMM register blocking — **stock is optimal, measured**

The profile said where to look:

```
RTEN_TIMING="sort=time" cargo run -p artist-computer --example bench-ocr --release
```

```
Conv               397.51ms (70.75%)   [detector]
Conv                53.63ms (75.40%)   [recognizer]
```

Convolution dominates, and most convolutions here are pointwise — which *is* a
GEMM. So the blocking constants are the right thing to test. A/B against the real
workload, via `[patch.crates-io]` onto a vendored rten:

| blocking | full read | vs stock |
|---|---|---|
| **MR=6 NR=16** (stock) | **278.6 ms** | — |
| MR=4 NR=24 | 289.8 ms | +4% |
| MR=5 NR=16 | 307.8 ms | +10% |
| MR=6 NR=24 | 315.8 ms | +13% |
| MR=8 NR=8 | build failed | — |

Stock wins, exactly as the register budget predicts: AVX2 has 16 YMM registers,
and 6×16 puts 12 into accumulators leaving 4 for the A broadcast and B loads.
rten's own comment says it was chosen "to fit 2 AVX registers and take advantage
of the two FMA execution ports" — that reasoning is correct and the measurement
agrees.

**The fork was removed**, because carrying a zero-diff vendor patch costs build
time and upstream drift for nothing. The experiment is reproducible: restore a
`[patch.crates-io]` block pointing at a clone of rten and run
`~/.local/share/artist-session-scratch/gemm-blocking-ab.sh`.

*Caveat worth stating:* this machine has no AVX-512, so the `Avx512Kernel`
constants could not be tested. rten's own source flags them as
microarchitecture-dependent ("The optimal value of MR depends on how many AVX-512
FMA units the CPU has"), so a server-class CPU is worth re-testing on. The AVX2
finding does not transfer.

### 4. Add Winograd for 3×3 convolutions — **measured out, not built**

This one was ranked fourth on cost and turned out not to be worth its place at
all. **My stated premise was wrong.** I claimed DBNet's backbone is mostly dense
3×3 convolutions. It is not:

| | pointwise 1×1 | depthwise / grouped | **dense 3×3 stride-1** |
|---|---|---|---|
| detector | 42 | 14 | **5** |
| recognizer | 21 | 15 | **0** |

PP-OCR mobile uses a MobileNetV3-style backbone, and depthwise separable
convolutions exist *precisely to avoid* dense 3×3. Winograd F(2×2,3×3) would
apply to **5 convolutions out of 100**, in one of the two models, and rten
already has specialized paths for both dominant cases (`conv_2d_pointwise` and
`DepthwiseConvExecutor`).

Against that: three transform passes to write and get numerically right — input,
filter and output — carried as a permanent fork of rten, with real accuracy risk,
since Winograd buys multiplies by spending precision and a detector that starts
missing faint text is worse than a slow one.

Not built. The measurement that decided it took twenty minutes.

---

## What this says about the method

Three of four items needed no code. That is not four items' worth of avoided
work — it is one item built and measured at 15.5×, and three cheap measurements
that each prevented a larger and less useful piece of work.

The one that would have hurt is item 4. It was ranked on a confident-sounding
claim about model architecture that a twenty-minute script disproved. Had it been
built on the strength of the ranking, it would have made 5 convolutions out of
100 about 2.25× faster — in one of the two models — and been carried as a fork
forever.

## Reproducing everything

```bash
scripts/fetch-ocr-models.sh
cargo test  -p artist-computer --release --features ocr ocr::
cargo run   -p artist-computer --example bench-ocr --release
cargo run   -p artist-computer --example inspect-ocr-graph --release
RTEN_TIMING="sort=time" cargo run -p artist-computer --example bench-ocr --release
```
