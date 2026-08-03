#!/usr/bin/env python3
"""Does the cached decode path agree with the uncached one, in PyTorch?

Checked here before exporting anything. A cache bug produces plausible logits
that drift, which is exactly the failure an ONNX round-trip would obscure — so
the invariant is established against the reference implementation first, and
only then carried through the exporter.

The invariant: feeding tokens one at a time through `CachedDecoderStep`, with
the cache threaded between steps, must give the same logits as one full-prefix
`DecoderWrapper` pass over the same tokens.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import torch

from cached_decoder import CachedDecoderStep, CrossProjector
from export_t5gemma import DecoderWrapper, EncoderWrapper, load_text_only, MODEL_ID


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--model-id", default=MODEL_ID)
    ap.add_argument("--token", default=None)
    ap.add_argument("--seq-len", type=int, default=128)
    ap.add_argument("--steps", type=int, default=16)
    args = ap.parse_args()

    model, _ = load_text_only(args.model_id, args.token)
    enc_w = EncoderWrapper(model, args.seq_len).eval()
    dec_w = DecoderWrapper(model, args.steps, args.seq_len).eval()
    step = CachedDecoderStep(model, args.seq_len).eval()
    proj = CrossProjector(model).eval()

    cfg = model.model.decoder.config
    layers = int(cfg.num_hidden_layers)
    kv_heads = int(getattr(cfg, "num_key_value_heads", 1))
    head_dim = int(getattr(cfg, "head_dim", cfg.hidden_size // cfg.num_attention_heads))
    print(f"[cache] layers={layers} kv_heads={kv_heads} head_dim={head_dim}")

    ids = torch.arange(1, args.seq_len + 1, dtype=torch.int64).unsqueeze(0) % 1000 + 5
    mask = torch.ones(1, args.seq_len, dtype=torch.int64)
    dec_ids = torch.tensor([[2] + [(7 * (j + 1)) % 200000 for j in range(args.steps - 1)]])

    with torch.no_grad():
        enc = enc_w(ids, mask)
        full_logits = dec_w(dec_ids, enc, mask)

        cross_k, cross_v = proj(enc)
        print(f"[cache] cross_k {list(cross_k.shape)}  "
              f"({cross_k.numel() * 4 / 1e6:.2f} MB fp32)")

        past_k = torch.zeros(layers, 1, kv_heads, 0, head_dim)
        past_v = torch.zeros(layers, 1, kv_heads, 0, head_dim)

        worst = 0.0
        worst_at = -1
        argmax_ok = 0
        for t in range(args.steps):
            tok = dec_ids[:, t : t + 1]
            logits, past_k, past_v = step(tok, past_k, past_v, cross_k, cross_v, mask)
            want = full_logits[:, t, :]
            got = logits[:, 0, :]
            d = (got - want).abs().max().item()
            rel = d / want.abs().max().item()
            if got.argmax(-1).item() == want.argmax(-1).item():
                argmax_ok += 1
            if d > worst:
                worst, worst_at = d, t
            print(f"[cache] step {t:>2}: past_len {past_k.shape[3]:>3}  "
                  f"max|Δ| {d:.3e}  rel {rel:.2e}  "
                  f"argmax {'ok' if got.argmax(-1).item() == want.argmax(-1).item() else 'MISMATCH'}")

    print(f"\n[cache] worst max|Δ| {worst:.3e} at step {worst_at}; "
          f"argmax agreement {argmax_ok}/{args.steps}")
    ok = argmax_ok == args.steps
    print("[cache] CACHED PATH MATCHES UNCACHED" if ok else "[cache] MISMATCH")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
