# Anchor v1 overhaul — work summary & remaining blast surface

Branch: `Gortnite` · Repo: `/home/adam/Projects/artist` · Lab: `/home/adam/Downloads/artist-anchor-address-lab`
Focus: four correctness issues in the deterministic semantic-anchor v1 system.

---

## 1. Issue 1 — token table corruption (SOLVED, but superseded by a bigger alphabet change)

### What was wrong
- Both production (`crates/hashline-tools/src/anchor_tokens_68399.txt`) and lab
  (`generated/anchor_tokens_68399.txt`) copies were corrupted by a TSV-export bug:
  123 rows contain `payload<TAB>#payload` (human anchor column leaked into the
  `.txt`; e.g. line 648 is `:\t#:` instead of `":`). 103 lines contain tabs.
- Old bad SHA-256: `55eff0fe01db950733379055c52d631219cd3b10e275cd60542fe35ed3bd7774`.

### What I did
- Reverse-engineered the exact selection rule from the authoritative strict table
  and proved the **selection** (the 68,399 token ids) was correct — the corruption
  was purely export/serialization:
  - iterate o200k_base mergeable ranks in id order
  - token bytes decode to valid UTF-8 with `s.encode() == b` round-trip
  - `encode(s) == [i]` and `encode("#"+s) == [2, i]`
  - no `#`, no `‖`, no whitespace, no Unicode category C*
  - first char category not in (Mn, Mc, Me, Cf); no U+FFFD/U+FFFC
  - take first 68,399
- Independent regeneration gate **PASSED**: `/tmp/regenerated_strict.txt` is
  byte-for-byte equal to the authoritative `/tmp/anchor_tokens_68399.strict.txt`
  (SHA-256 `193120140cee1f75d8ccde9c5b1d554c5d8e466a23dfbd869089c0b005757bb1`).
- Diff artifacts: `/tmp/table_diff.txt`, `/tmp/strict_lines.txt`, `/tmp/bad_lines.txt`.

## 2. Alphabet content audit (user-directed pivot — DONE)

User asked to inspect the table's actual contents ("the 60,000 number is a little
large and obscene"). Findings:

- **Hard-constraint violations in the old table:**
  - `/` in 243 rows, `\` in 98 rows → 333 unique rows of code/path fragments
    (`//`, `</`, `../`, `/*`, `*/`, `\\"`, …). The old rule never excluded them.
  - linebreak/newline/tab/space/`#` were already 0. zero-width/bidi: 0.
- **Profanity:** 15 real tokens (`sex`, `Sex`, `SEX`, `rape`, `shit`, `cock`,
  `Cock`, `porn`, `Porn`, `porno`, `fuck`, `Fuck`, `Dick`, `pedo`, `coon`).
  The ~48 other substring hits were innocent (`class`, `document`, `Advertisement`,
  `cuntegn` = Welsh "with").
- Composition: 42,009 ASCII/code rows + CJK/Cyrillic/Arabic/Latin-ext/Hangul/Hebrew/
  Georgian/Thai/Greek/… words. It was a mechanical filter, not curated.

### Decision (user): add `/`, `\`, and profanity exclusions; recompute the lab
artifact for whatever the new target prime is.

## 3. New alphabet, new prime, new scheme (DONE)

- New selection rule = old rule **plus** no `/`, no `\`, no profanity (exact-match
  blocklist, list below).
- Eligible pool: **68,068** tokens (349 dropped: 333 slash/backslash + 15 profanity + overlap).
- New field prime forced to **68,059** (largest prime ≤ pool; 68,068 is not prime;
  next prime above, 68,071, exceeds the pool).
- **Recomputed the scheme artifact in the lab** for prime 68,059:
  - 10 exact CTM root-loss landscapes, seeds `0x51A7E001..0x51A7E00A`
    → `reports/root_perm{1..10}_v2.npy` (v2 naming to avoid clobbering v1)
  - final fit: optimized points `[18107, 49268, 48374, 5905]`,
    tail `(offset 30766, step 8407)`, byte codec unchanged `(17, 17)`
    (codec is data-independent, exhaustively re-verified = `(17,17,(0,0.234375))`)
  - fit improvement 6.69x over uniform; held-out (32-relabel) 2.04x.
- **New table:** 68,059 rows, all unique payloads, all unique token ids (max id
  199,966), zero `/`, `\`, `#`, whitespace, `‖`.
  - SHA-256: `b5b5781a0496f912981bbc5607a413e101bfe35f50eb0fff2d30f59319d87216`
- **Rust/Python parity verified** for the new artifact (`rustc` standalone compile +
  assert-equal on sample vectors) → `parity_vectors_v2.json`.

### New lab artifacts (all written)
- `generated/anchor_tokens_68059.txt` (451,615 B) and `.tsv` (header `u\ttoken_id\tpayload_hex`)
- `generated/scheme_v2.json`, `generated/anchor_address_v2.rs`,
  `generated/parity_vectors_v2.json`
- `reports/root_perm{1..10}_v2.{json,npy}`, `reports/final_fit_v2.json`
- Scripts: `/tmp/regenerate_68059.py` (selection), `/tmp/final_fit_68059.py`,
  `/tmp/newpool.py`, `/tmp/sep_scan*.py`, `/tmp/finalize_sep.py`
- `/tmp/anchor_tokens_68059.txt` / `.tsv` / `.meta.json` (canonical copies)

### Profanity blocklist used (exact whole-token, case-insensitive)
`fuck, fucking, fucker, shit, bitch, asshole, dick, cock, pussy, cunt, whore,
slut, nigger, nigga, faggot, fag, motherfucker, motherfucking, bullshit, shitty,
retard, retarded, bitchass, dumbass, jackass, douchebag, wanker, bastard, twat,
piss, pissing, jerkoff, dildo, penis, vagina, boob, tits, titty, nipple, porn,
porno, pornhub, sex, kike, spic, chink, gook, wetback, coon, tranny, dyke,
faggots, rape, rapist, raped, pedophile, pedo, molest, molested, nazi, hitler,
holocaust, kkk, kill yourself, suicide, kys, fagot, fuk, fukc, shat, biches,
bitches, dicks, cocks, pussies, sluts, whores, cunts`

## 4. Issue 4 — component separator `‖` (DECISION MADE, code NOT yet updated)

### Investigation
- `‖` (U+2016) encodes as `[318, 244]` and is **token-boundary unsafe**:
  `‖`+`ე` merges into token `10252`; `#in‖ე‖er` re-tokenizes as
  `[2, 258, 318, 10252, 318, 244, 259]` — the `ე` component token id 794 is lost.
- 6 selected payloads violate the mid-boundary (`ე`-prefixed Georgian tokens).
- Exhaustive separator search: every "nice" single-token symbol (`→` 20216,
  `⇒` 113961, `≥` 87319, `◆` 49429, `¶` 69022, `±` 32438, …) is *itself a payload
  row*, so it cannot be a separator (assert at `semantic_anchors.rs:25` requires
  payloads not to contain the separator). Only structurally-safe options were
  obscure (combining marks, CJK `旁`, multi-char `###`).
- **Key finding / decision (user):** contiguous payloads DO fuse when re-tokenized
  (5.33% of 400k random adjacent pairs, e.g. `Hu`+`security` → `[H, security]`),
  BUT nothing in production re-tokenizes rendered anchors — they are only
  display/opaque-key strings (`AnchorTable::binding`, `AnchorBook::resolve` do
  exact string lookup; no parsing of `#X‖Y‖Z`). UI labels are being nuked later.
  - **Final decision: keep `‖` as a cosmetic joiner; drop the re-tokenization
    recovery requirement.** The separator already satisfies the only real
    production constraint (absent from payloads).

## 5. Issue 2 — descendant identity must survive declaration renames (NOT STARTED)

- Location: `crates/artist-ast/src/anchors.rs`.
- `structured_base_identities` embeds ancestor **semantic name** into every
  descendant's identity (via `StructuralFrame.semantic`). Renaming
  `fn find_users` → `fn find_users_2` changes the `function_item` ancestor frame,
  so the identity of a descendant line (e.g. `target()`) changes — anchors break.
- Required fix: generic structural machinery, not Rust-only; audit Python/JS/TS
  (all go through the same `structured_base_identities` path, so a structural fix
  covers them; SQL/markdown use separate paths — need audit too).
- Constraints: no prehash, no trim/lowercase/normalization, no fuzzy matching, no
  collision-augmented identity, no persisted mnemonic allocator, no
  line/offset/neighbor/read-history in identity.

## 6. Issue 3 — recursive equivalent-sibling ranking (NOT STARTED)

- Location: `crates/artist-ast/src/anchors.rs`, `finish_identities` (file-wide
  `HashMap` rank over identical base identities).
- Problem: rank is computed file-wide; the "equivalent-sibling rank" must be
  **recursive under the same parent occurrence**, never a file-wide rank. Adding/
  removing an equivalent sibling elsewhere in the file must not change another
  occurrence's identity.
- Plain-text fallback may keep deterministic exact-line rank.
- Constraint: rank computed before address generation, never from address
  collisions.

## 7. Blast surface — production code touched by the new alphabet/prime (NOT DONE)

Files that reference the frozen constants and MUST be updated for prime 68,059:

- `crates/hashline-tools/src/anchor_address_v1.rs` — FIELD_PRIME 68399→68059,
  ADDRESS_ALPHABET_SIZE 68399→68059, OPTIMIZED_POINTS → `[18107, 49268, 48374,
  5905]`, TAIL_OFFSET 30920→30766, TAIL_STEP 8449→8407 (codec 17/17 unchanged).
  Full replacement: copy lab `generated/anchor_address_v2.rs`.
- `crates/hashline-tools/src/scheme_v1.json` — replace with lab
  `generated/scheme_v2.json` contents (or regenerate under v1 naming).
- `crates/hashline-tools/src/anchor_tokens_68399.txt` → new
  `anchor_tokens_68059.txt` (include path in `semantic_anchors.rs:15` must change).
- `crates/hashline-tools/src/semantic_anchors.rs`:
  - `TOKENS_TEXT` include path (line 15)
  - tests `frozen_v1_constants_are_exact` (68399→68059, points/tail)
  - test `token_rows_are_direct_and_separator_is_reserved` (`vocabulary[68398]`
    checks, duplicate-payload count `68384` → now 68,059 unique)
  - tests `duplicate_token_rows_extend_textual_prefixes` and the duplicate-payload
    comment block in `shortest_live_anchors` must be REMOVED (new table has no
    duplicates; the "duplicate payloads are legitimate" stance is now invalid).
- `crates/hashline-tools/src/lib.rs` — module include path (unchanged name).
- `crates/hashline-tools/FRANKENSTEIN.md` — lines 30-32, 89, 232, 255-256
  (filenames, F_68399 naming, "do not edit" checklist).
- `crates/hashline-tools/docs/semantic-anchors-v1.md` — line 9 (token file name,
  duplicate-payload wording).
- Any other repo references to 68399 / anchor_tokens / scheme constants
  (grep across whole repo still needed — only crates were scanned).

### Lab side (sync, NOT DONE)
- No token-table exporter exists yet in the lab — must write one that emits the
  production `.txt` directly from raw selected bytes + machine-safe
  `payload_hex` metadata (never parse human TSV back into payloads).
- Decide v1 vs v2 artifact naming and align `generated/` with production
  (`scheme_v1.json`, `anchor_address_v1.rs`, `parity_vectors_v1.json`,
  `anchor_tokens_68059.{txt,tsv}`).
- `STATUS_V1.md` frozen-constants block must be rewritten.
- The old bad `generated/anchor_tokens_68399.{txt,tsv}` copies should be removed
  or replaced.

## 8. Validation still required (NOT RUN)

- `cargo fmt --check`
- `cargo test -p artist-ast`
- `cargo test -p hashline-tools`
- `cargo test -p artist-tools`
- `cargo test -p artist-computer`
- combined test run
- `cargo check --workspace`
- `cargo clippy --workspace --all-targets`
- `git diff --check`
- lab: token-table regeneration gate (byte-for-byte + SHA-256 + uniqueness +
  `encode(payload)==[i]` + `encode("#"+payload)==[2,i]`), scheme/Rust parity,
  Python tests (`uv run pytest`), ruff.

## 9. Preserved constraints (do not regress)

- Do NOT change (already-verified-unchanged): byte codec `(17,17)`, Hasse
  continuation, scheme structure/`anchor_address_v1` algorithm.
- Prohibited in identity: line/byte offsets, filenames, neighbor hashes,
  duplicate counts, read history, prehash, mnemonic allocator state,
  collision-augmented identity; keep stateless/deterministic.
- Worktree has many unrelated user changes (artist-agent, artist-ast,
  artist-canvas, artist-cli, Cargo.lock, etc.) — preserve; don't "fix" unrelated
  dirty code for lint noise.

## 10. Open questions / decisions pending

- v1 vs v2 naming for the recomputed scheme artifact (production currently says
  `ANCHOR_ABI_VERSION = "v1"`).
- Whether the profanity blocklist needs to be maintained/extended, and whether it
  belongs only in the lab exporter.
- Confirm the plan for Issue 2 (rename-transparent descendant identity) and
  Issue 3 (recursive sibling rank) — exact intended semantics from the existing
  anchors.rs tests (some current tests assert the *opposite* of Issue 2, e.g.
  `structured_identity_ignores_path_and_formatting_but_tracks_semantic_governor`).
