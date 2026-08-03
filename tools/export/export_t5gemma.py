#!/usr/bin/env python3
"""Export T5Gemma-2 to ONNX, text-only, for `rten` to run.

Why this exists at all: `rten` executes *graphs*, and a graph is what a
safetensors file does not contain. There is no Rust path from weights to a
runnable model short of reimplementing the forward pass, so the export is a
Python step. It costs nothing extra — fine-tuning is PyTorch anyway, and this
is the last line of that pipeline rather than a new dependency. Nothing here
ships; the `artist` binary loads the `.onnx` and contains no Python.

**Text-only.** `google/t5gemma-2-270m-270m` is 786M parameters, of which
416.9M (53%) is a SigLIP vision tower and 0.7M a multimodal projector. Both are
dead weight for prose-to-logic. Dropping them leaves 368M:

    167.8M  embed_tokens [262144, 640]  (shared, tied across both towers)
    100.3M  encoder.layers
    100.3M  decoder.layers

which is ~1.5 GB at fp32 rather than ~3.1 GB. That matters on a machine whose
/tmp is RAM-backed.

Three graphs come out, which is what `rten-generate` wants:

    encoder.onnx          prose  -> hidden states, run once
    decoder.onnx          first decode step, no cache
    decoder_with_past.onnx every subsequent step, reading and writing the cache

Splitting the first decode step from the rest is not an optimisation; a graph
whose KV-cache inputs are empty tensors is a different shape from one whose
inputs are populated, and ONNX wants those static.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

import torch

MODEL_ID = "google/t5gemma-2-270m-270m"
# The parts of the checkpoint that exist only for images.
VISION_PREFIXES = ("vision_tower", "multi_modal_projector")


def log(msg: str) -> None:
    print(f"[export] {msg}", flush=True)


def load_text_only(model_id: str, token: str | None):
    """Load the checkpoint and drop the vision towers.

    Deleting the submodules rather than filtering the state dict is deliberate:
    it keeps `config` and the module tree in agreement, so the tracer never
    reaches an image code path and cannot emit ops for one.
    """
    from transformers import AutoConfig, AutoModelForSeq2SeqLM

    log(f"loading {model_id}")
    config = AutoConfig.from_pretrained(model_id, token=token)
    model = AutoModelForSeq2SeqLM.from_pretrained(
        model_id,
        token=token,
        dtype=torch.float32,  # rten runs fp32; bf16 weights would need a cast anyway
        attn_implementation="eager",  # SDPA/flash trace into ops rten does not have
    )
    # `dtype=torch.float32` above is not sufficient: the checkpoint config
    # declares `dtype: bfloat16` and transformers honours it for submodules, so
    # activations come back BFloat16 and `.numpy()` refuses them. rten runs
    # fp32, so cast the whole module rather than patching at the boundary —
    # a bf16 graph would otherwise be traced and every op would carry the type.
    model = model.float()
    model.eval()

    total_before = sum(p.numel() for p in model.parameters())
    encoder = model.model.encoder
    dropped = 0
    for name in VISION_PREFIXES:
        module = getattr(encoder, name, None)
        if module is None:
            log(f"  no {name} on this checkpoint, nothing to drop")
            continue
        dropped += sum(p.numel() for p in module.parameters())
        setattr(encoder, name, None)
    total_after = sum(p.numel() for p in model.parameters() if p is not None)

    log(f"  {total_before/1e6:.1f}M params -> dropped {dropped/1e6:.1f}M vision "
        f"({100*dropped/max(total_before,1):.1f}%) -> {total_after/1e6:.1f}M text-only")
    return model, config


class EncoderWrapper(torch.nn.Module):
    """`input_ids`, `attention_mask` -> `encoder_hidden_states`.

    Wrapped for two reasons. The bare module returns a dataclass and the
    exporter wants tensors — and, more importantly, **it builds its own
    attention masks in a way neither ONNX exporter can trace.**

    `transformers.masking_utils` constructs masks from an index predicate
    (`inner_mask(b, h, q, kv) -> bool`) expanded over four dimensions. The
    dynamo exporter dies in symbolic shape propagation (`8*s72 is not tracked
    with proxy`); the TorchScript tracer reaches a `torch.vmap` over a custom
    autograd Function and dies with `unordered_map::at`.

    `T5Gemma2TextEncoder.forward` opens with

        if not isinstance(self_attn_mask_mapping := attention_mask, dict):

    so **passing a prebuilt dict skips mask construction entirely**. That is the
    seam this wrapper uses. The masks are rebuilt here with ordinary tensor ops,
    matching `eager_mask`'s contract: 4D float `(batch, 1, q_len, kv_len)`,
    `0` where attention is allowed and `dtype.min` where it is not.

    The window predicate is copied from `sliding_window_mask_function` with
    `is_causal=False`, which is what the encoder passes:

        left  = (sw + 1) // 2        right = sw // 2 + 1
        keep  = (0 <= dist < left) or (0 < -dist < right)

    Sequence length is **fixed, not dynamic**. Every input here is one
    sentence; padding to a fixed width costs nothing and removes the entire
    class of symbolic-shape export failures above. Batch stays dynamic, which
    is what the labelling pass needs.
    """

    def __init__(self, model, seq_len: int):
        super().__init__()
        self.encoder = model.model.encoder
        self.seq_len = seq_len
        cfg = self.encoder.config
        self.sliding_window = int(getattr(cfg, "sliding_window", 512))

    def _masks(self, attention_mask: torch.Tensor) -> dict[str, torch.Tensor]:
        length = self.seq_len
        device = attention_mask.device
        neg = torch.finfo(torch.float32).min

        # Padding: [B, 1, 1, L] -> broadcast over queries.
        pad = attention_mask.to(torch.bool)[:, None, None, :]

        idx = torch.arange(length, device=device)
        dist = idx[:, None] - idx[None, :]  # [L, L], q - kv
        left_w = (self.sliding_window + 1) // 2
        right_w = self.sliding_window // 2 + 1
        window = ((dist >= 0) & (dist < left_w)) | ((dist < 0) & (-dist < right_w))

        full_keep = pad.expand(-1, 1, length, length)
        slide_keep = full_keep & window[None, None, :, :]

        def to_float(keep: torch.Tensor) -> torch.Tensor:
            return torch.zeros_like(keep, dtype=torch.float32).masked_fill(~keep, neg)

        return {
            "full_attention": to_float(full_keep),
            "sliding_attention": to_float(slide_keep),
        }

    def forward(self, input_ids, attention_mask):
        return self.encoder(
            input_ids=input_ids, attention_mask=self._masks(attention_mask)
        ).last_hidden_state


def _exporter_kwargs(opset: int, dynamo: bool) -> dict:
    """Exporter-specific arguments.

    `opset_version` must NOT be passed on the dynamo path. The dynamo exporter
    emits a graph containing ONNX *functions*, and asking for any specific
    opset runs a version-conversion pass that refuses to touch functions:

        ValueError: The model contains functions. The version conversion pass
        does not support functions.

    Omitting it lets the graph keep its native opset and skips the pass
    entirely. `do_constant_folding` is likewise a TorchScript-only knob.
    """
    if dynamo:
        return {"dynamo": True}
    return {"opset_version": opset, "do_constant_folding": True, "dynamo": False}


class DecoderWrapper(torch.nn.Module):
    """`decoder_input_ids`, `encoder_hidden_states`, `encoder_attention_mask` -> `logits`.

    Same mask problem as the encoder, same seam: both `attention_mask` and
    `encoder_attention_mask` are used as prebuilt dicts when given as dicts.

    The decoder does **merged attention** — it concatenates the self-attention
    and cross-attention masks along the key axis and runs one attention over
    `[decoder_keys | encoder_keys]`. That is why the two masks must agree on
    query length, and why the cross mask is broadcast over queries rather than
    being a separate pass.

    Self-attention here is *causal*, unlike the encoder's bidirectional case:

        full_attention     keep when q >= kv
        sliding_attention  keep when 0 <= q - kv < sliding_window

    matching `sliding_window_mask_function(..., is_causal=True)`, which sets
    `left = sliding_window` and `right = 0`.

    **No KV cache.** This graph recomputes the whole prefix at every step. For
    this workload that is a defensible starting point — outputs are short
    s-expressions, tens of tokens — and it isolates "does the architecture
    export and match" from the separate plumbing problem of threading an
    `EncoderDecoderCache` through ONNX, which the legacy exporter is documented
    not to support.
    """

    def __init__(self, model, dec_len: int, enc_len: int):
        super().__init__()
        self.model = model
        self.decoder = model.model.decoder
        self.lm_head = model.lm_head
        self.dec_len = dec_len
        self.enc_len = enc_len
        self.sliding_window = int(getattr(self.decoder.config, "sliding_window", 512))

    def forward(self, decoder_input_ids, encoder_hidden_states, encoder_attention_mask):
        neg = torch.finfo(torch.float32).min
        q = self.dec_len

        idx = torch.arange(q, device=decoder_input_ids.device)
        dist = idx[:, None] - idx[None, :]
        causal = dist >= 0
        sliding = causal & (dist < self.sliding_window)

        batch = decoder_input_ids.shape[0]

        def to_float(keep: torch.Tensor) -> torch.Tensor:
            return torch.zeros_like(keep, dtype=torch.float32).masked_fill(~keep, neg)

        self_masks = {
            "full_attention": to_float(causal[None, None].expand(batch, 1, q, q)),
            "sliding_attention": to_float(sliding[None, None].expand(batch, 1, q, q)),
        }
        enc_keep = encoder_attention_mask.to(torch.bool)[:, None, None, :].expand(
            batch, 1, q, self.enc_len
        )
        cross_masks = {"full_attention": to_float(enc_keep)}

        out = self.decoder(
            input_ids=decoder_input_ids,
            attention_mask=self_masks,
            encoder_hidden_states=encoder_hidden_states,
            encoder_attention_mask=cross_masks,
            use_cache=False,
        )
        return self.lm_head(out.last_hidden_state)


def export_decoder(
    model, out_dir: Path, opset: int, dynamo: bool, dec_len: int, enc_len: int
) -> Path:
    wrapper = DecoderWrapper(model, dec_len, enc_len).eval()
    hidden = model.config.decoder.hidden_size
    dec_ids = torch.ones(1, dec_len, dtype=torch.int64)
    enc_hidden = torch.zeros(1, enc_len, hidden, dtype=torch.float32)
    enc_mask = torch.ones(1, enc_len, dtype=torch.int64)
    path = out_dir / "decoder.onnx"
    log(f"exporting decoder (dec_len={dec_len}, enc_len={enc_len}, dynamo={dynamo})")
    torch.onnx.export(
        wrapper,
        (dec_ids, enc_hidden, enc_mask),
        str(path),
        input_names=["decoder_input_ids", "encoder_hidden_states", "encoder_attention_mask"],
        output_names=["logits"],
        dynamic_axes={
            "decoder_input_ids": {0: "batch"},
            "encoder_hidden_states": {0: "batch"},
            "encoder_attention_mask": {0: "batch"},
            "logits": {0: "batch"},
        },
        **_exporter_kwargs(opset, dynamo),
    )
    log(f"  wrote {path} ({path.stat().st_size/1e6:.1f} MB)")
    return path


def export_encoder(model, out_dir: Path, opset: int, dynamo: bool, seq_len: int) -> Path:
    """Export the encoder.

    `dynamo` selects the exporter. torch 2.9 defaults to the dynamo path
    (`torch.export` + onnxscript), which on this model dies inside symbolic
    shape propagation:

        RuntimeError: 8*s72 is not tracked with proxy for _ModuleStackTracer

    That `8*s72` is a symbolic expression over the sequence length, and the
    sliding-window attention mask is the obvious source — `sliding_window` is
    512 and the layer pattern alternates sliding/full, so mask construction
    does arithmetic on a dynamic dim. The legacy TorchScript tracer does not
    reason symbolically at all: it records the ops for one concrete shape and
    marks the named axes dynamic afterwards, which sidesteps the failure.

    The tradeoff is real and worth stating: the legacy tracer bakes in
    control-flow decisions made at the traced length. Anything that branches on
    sequence length — such as whether a sliding window is even reached —
    freezes at trace time, so the graph must be validated against the reference
    at several lengths rather than trusted.
    """
    wrapper = EncoderWrapper(model, seq_len).eval()
    input_ids = torch.ones(2, seq_len, dtype=torch.int64)
    attention_mask = torch.ones(2, seq_len, dtype=torch.int64)
    path = out_dir / "encoder.onnx"
    log(f"exporting encoder (dynamo={dynamo}, opset={opset})")
    torch.onnx.export(
        wrapper,
        (input_ids, attention_mask),
        str(path),
        input_names=["input_ids", "attention_mask"],
        output_names=["encoder_hidden_states"],
        # Both axes dynamic: sequence length varies per fact, and batching is
        # how the training-data labelling pass will use this.
        # Batch only. Sequence length is fixed at `seq_len` and padded to;
        # see EncoderWrapper for why dynamic length is not worth its cost here.
        dynamic_axes={
            "input_ids": {0: "batch"},
            "attention_mask": {0: "batch"},
            "encoder_hidden_states": {0: "batch"},
        },
        **_exporter_kwargs(opset, dynamo),
    )
    log(f"  wrote {path} ({path.stat().st_size/1e6:.1f} MB)")
    return path


def export_cached_decoder(model, out_dir: Path, opset: int, dynamo: bool, enc_len: int):
    """Export the two graphs that make cached generation work.

    `cross_proj.onnx` runs once per input; `decoder_step.onnx` runs per token.
    Splitting them is the whole point: the cross-attention K/V depend only on
    the encoder output, so recomputing them every step is pure waste.

    `past_len` is dynamic here, unlike everywhere else in this file — it has to
    be, since it grows by one per step. That was unaffordable before because
    symbolic shapes met untraceable mask construction; with the masks prebuilt
    the dynamo exporter handles it.
    """
    from cached_decoder import CachedDecoderStep, CrossProjector

    cfg = model.model.decoder.config
    layers = int(cfg.num_hidden_layers)
    kv_heads = int(getattr(cfg, "num_key_value_heads", 1))
    head_dim = int(getattr(cfg, "head_dim", cfg.hidden_size // cfg.num_attention_heads))
    hidden = int(cfg.hidden_size)

    proj = CrossProjector(model).eval()
    enc_hidden = torch.zeros(1, enc_len, hidden)
    proj_path = out_dir / "cross_proj.onnx"
    log("exporting cross projector")
    torch.onnx.export(
        proj, (enc_hidden,), str(proj_path),
        input_names=["encoder_hidden_states"],
        output_names=["cross_k", "cross_v"],
        dynamic_axes={"encoder_hidden_states": {0: "batch"},
                      "cross_k": {1: "batch"}, "cross_v": {1: "batch"}},
        **_exporter_kwargs(opset, dynamo),
    )
    log(f"  wrote {proj_path} ({proj_path.stat().st_size/1e6:.1f} MB)")

    step = CachedDecoderStep(model, enc_len).eval()
    past = 3  # non-zero and non-one, so the trace cannot special-case it
    args = (
        torch.ones(1, 1, dtype=torch.int64),
        torch.zeros(layers, 1, kv_heads, past, head_dim),
        torch.zeros(layers, 1, kv_heads, past, head_dim),
        torch.zeros(layers, 1, kv_heads, enc_len, head_dim),
        torch.zeros(layers, 1, kv_heads, enc_len, head_dim),
        torch.ones(1, enc_len, dtype=torch.int64),
    )
    step_path = out_dir / "decoder_step.onnx"
    log(f"exporting cached decoder step (layers={layers}, kv_heads={kv_heads}, head_dim={head_dim})")
    torch.onnx.export(
        step, args, str(step_path),
        input_names=["decoder_input_ids", "past_self_k", "past_self_v",
                     "cross_k", "cross_v", "encoder_attention_mask"],
        output_names=["logits", "present_self_k", "present_self_v"],
        dynamic_axes={
            "decoder_input_ids": {0: "batch"},
            "past_self_k": {1: "batch", 3: "past_len"},
            "past_self_v": {1: "batch", 3: "past_len"},
            "cross_k": {1: "batch"}, "cross_v": {1: "batch"},
            "encoder_attention_mask": {0: "batch"},
            "logits": {0: "batch"},
            "present_self_k": {1: "batch", 3: "total_len"},
            "present_self_v": {1: "batch", 3: "total_len"},
        },
        **_exporter_kwargs(opset, dynamo),
    )
    log(f"  wrote {step_path} ({step_path.stat().st_size/1e6:.1f} MB)")
    return proj_path, step_path


def verify_onnx(path: Path) -> None:
    """Structural check plus an op inventory.

    The inventory is the point. `rten` implements ~130 ops, and T5Gemma-2 is a
    2025-10 architecture with per-layer-type RoPE and an alternating
    sliding/full attention pattern — exactly the kind of thing that decomposes
    into something unusual. Better to read the list than to discover it at
    `Model::load`.
    """
    import onnx

    model = onnx.load(str(path))
    onnx.checker.check_model(model)
    ops: dict[str, int] = {}
    for node in model.graph.node:
        ops[node.op_type] = ops.get(node.op_type, 0) + 1
    log(f"  {path.name}: valid, {len(model.graph.node)} nodes, {len(ops)} distinct ops")
    log(f"  ops: {', '.join(f'{k}x{v}' for k, v in sorted(ops.items()))}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--model-id", default=MODEL_ID)
    ap.add_argument("--out", type=Path, required=True, help="output directory")
    ap.add_argument("--token", default=None, help="HF token; the repo is gated")
    ap.add_argument("--opset", type=int, default=17)
    ap.add_argument("--seq-len", type=int, default=128, help="fixed encoder length")
    ap.add_argument("--dec-len", type=int, default=64, help="fixed decoder length")
    ap.add_argument(
        "--torchscript",
        action="store_true",
        help="use the legacy TorchScript exporter instead of dynamo (slower graph, no KV cache)",
    )
    ap.add_argument("--no-cache", action="store_true", help="skip the cached decoder graphs")
    ap.add_argument(
        "--encoder-only",
        action="store_true",
        help="stop after the encoder — the cheapest GO/NO-GO on rten op coverage",
    )
    args = ap.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    model, config = load_text_only(args.model_id, args.token)

    enc = export_encoder(model, args.out, args.opset, (not args.torchscript), args.seq_len)
    verify_onnx(enc)

    if args.encoder_only:
        log("encoder-only requested; stopping. Run the rten check next.")
        return 0

    dec = export_decoder(model, args.out, args.opset, (not args.torchscript), args.dec_len, args.seq_len)
    verify_onnx(dec)

    if not args.no_cache:
        proj_p, step_p = export_cached_decoder(
            model, args.out, args.opset, (not args.torchscript), args.seq_len
        )
        verify_onnx(proj_p)
        verify_onnx(step_p)
    return 0


if __name__ == "__main__":
    sys.exit(main())
