#!/usr/bin/env python3
"""Dump PyTorch reference activations so the Rust side can be checked against them.

This is the step the original rten spike got right and that made it
trustworthy: the embedder was validated to 7e-6 against a reference
implementation rather than assumed correct because it loaded. The same
discipline applies here, and more urgently — the encoder graph was produced by
the **legacy TorchScript tracer**, which records ops for one concrete shape and
bakes in every control-flow decision made at trace time.

So the thing under test is not really "does rten implement these 30 ops". It is
"did tracing at length 128 freeze something that should have varied". That is
why the cases below vary the *real* token count inside a fixed padded width: if
a branch froze, a short input is where it shows up.

Writes raw little-endian files, which any language can read without a format
library:

    case_<i>_input_ids.i32     [batch, seq]
    case_<i>_attn_mask.i32     [batch, seq]
    case_<i>_hidden.f32        [batch, seq, hidden]
    manifest.json              shapes and metadata
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
import torch

from export_t5gemma import DecoderWrapper, EncoderWrapper, load_text_only, MODEL_ID

# Real memories from this project, plus the degenerate lengths that would catch
# a frozen branch.
CASES: list[str] = [
    "Adam prefers tabs over spaces in Rust",
    "never use python heredocs for source edits",
    "the compaction planner never cuts at a tool result boundary",
    "we stage all work on the Gortnite branch because side branches fragment review",
    "x",  # single token: shortest possible real content
    "café — naïve → résumé",  # non-ASCII, the case that breaks syntax::parse
    " ".join(["token"] * 120),  # nearly fills the padded width
]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--model-id", default=MODEL_ID)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--token", default=None)
    ap.add_argument("--seq-len", type=int, default=128)
    ap.add_argument("--tokenizer", type=Path, required=True, help="tokenizer.json")
    ap.add_argument("--dec-len", type=int, default=64)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    from tokenizers import Tokenizer

    tok = Tokenizer.from_file(str(args.tokenizer))
    model, _ = load_text_only(args.model_id, args.token)
    wrapper = EncoderWrapper(model, args.seq_len).eval()

    manifest: dict = {"seq_len": args.seq_len, "cases": []}
    for i, text in enumerate(CASES):
        ids = tok.encode(text).ids[: args.seq_len]
        real = len(ids)
        pad_id = 0
        ids = ids + [pad_id] * (args.seq_len - real)
        mask = [1] * real + [0] * (args.seq_len - real)

        input_ids = torch.tensor([ids], dtype=torch.int64)
        attn = torch.tensor([mask], dtype=torch.int64)
        with torch.no_grad():
            hidden = wrapper(input_ids, attn)

        np.asarray(ids, dtype=np.int32).tofile(args.out / f"case_{i}_input_ids.i32")
        np.asarray(mask, dtype=np.int32).tofile(args.out / f"case_{i}_attn_mask.i32")
        h = hidden.numpy().astype(np.float32)
        h.tofile(args.out / f"case_{i}_hidden.f32")

        manifest["cases"].append(
            {
                "index": i,
                "text": text,
                "real_tokens": real,
                "hidden_shape": list(h.shape),
                "hidden_absmax": float(np.abs(h).max()),
                "hidden_mean": float(h.mean()),
            }
        )
        print(
            f"[ref] case {i}: {real:>3} real tokens, hidden {list(h.shape)}, "
            f"absmax {np.abs(h).max():.4f}  {text[:44]!r}"
        )

    # Decoder: run the real encoder output through it and dump logits. The
    # decoder input is arbitrary — the model is not fine-tuned yet, so this
    # tests numerical agreement of the graph, not the quality of any output.
    dec_wrapper = DecoderWrapper(model, args.dec_len, args.seq_len).eval()
    manifest["dec_len"] = args.dec_len
    manifest["decoder_cases"] = []
    for i, text in enumerate(CASES[:4]):
        ids = tok.encode(text).ids[: args.seq_len]
        real = len(ids)
        ids = ids + [0] * (args.seq_len - real)
        mask = [1] * real + [0] * (args.seq_len - real)
        input_ids = torch.tensor([ids], dtype=torch.int64)
        attn = torch.tensor([mask], dtype=torch.int64)
        # A deterministic decoder prefix; token 2 is bos per config.
        dec_ids = [2] + [(7 * (j + 1)) % 200000 for j in range(args.dec_len - 1)]
        dec_t = torch.tensor([dec_ids], dtype=torch.int64)
        with torch.no_grad():
            enc = wrapper(input_ids, attn)
            logits = dec_wrapper(dec_t, enc, attn)
        np.asarray(dec_ids, dtype=np.int32).tofile(args.out / f"dec_{i}_input_ids.i32")
        lg = logits.numpy().astype(np.float32)
        lg.tofile(args.out / f"dec_{i}_logits.f32")
        manifest["decoder_cases"].append(
            {"index": i, "real_tokens": real, "logits_shape": list(lg.shape),
             "logits_absmax": float(np.abs(lg).max())}
        )
        print(f"[ref] dec {i}: logits {list(lg.shape)}, absmax {np.abs(lg).max():.4f}")

    (args.out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"[ref] wrote {len(CASES)} cases to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
