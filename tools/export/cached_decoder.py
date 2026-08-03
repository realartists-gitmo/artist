"""A single KV-cached decoder step, as an ONNX-exportable graph.

The uncached `decoder.onnx` recomputes the whole prefix every step: ~200 ms per
pass, so ~13 s to emit a 64-token s-expression. That is tolerable for a
fire-and-forget write but ruinous for the labelling pass, which has to run over
thousands of transcripts. Caching turns each step into one query token
attending over `past_len + enc_len` keys.

**Why a shim instead of `EncoderDecoderCache`.** `T5Gemma2MergedAttention`
keeps two caches with different lifetimes:

    self_attention_cache    grows by one position per step
    cross_attention_cache   computed once from the encoder, then constant

and it decides which to do via `past_key_values.is_updated[layer_idx]` — a
plain Python dict, mutated during the forward pass. Neither exporter can record
that; it is control flow over host state, not tensor math.

Rather than fork the model, this supplies an object with the same surface whose
state is *tensors passed in and out of the graph*. `is_updated` is preset to
`True` for every layer, so the cross branch always reads from the cache and
never recomputes the encoder projections — which is exactly the behaviour we
want at step N>0 anyway.

Shapes. `num_key_value_heads` is **1** (multi-query attention), so the cache is
far smaller than the layer count suggests:

    past_self_k/v   [layers, batch, 1, past_len, head_dim]
    cross_k/v       [layers, batch, 1, enc_len,  head_dim]

At 18 layers, head_dim 256, enc_len 128 that is ~2.4 MB of cross cache per
sequence in fp32, and the self cache grows by 18*2*256 floats per token.

Stacking along a leading `layers` axis keeps the graph at a handful of I/O
tensors instead of 144 separate ones.
"""

from __future__ import annotations

import torch


class _LayerView:
    """Stands in for a cache layer's `.keys` / `.values` attributes."""

    def __init__(self, keys: torch.Tensor, values: torch.Tensor) -> None:
        self.keys = keys
        self.values = values


class _CrossCache:
    """Read-only view over the precomputed cross-attention K/V."""

    def __init__(self, keys: torch.Tensor, values: torch.Tensor) -> None:
        # keys/values: [layers, batch, kv_heads, enc_len, head_dim]
        self.layers = [_LayerView(keys[i], values[i]) for i in range(keys.shape[0])]

    def update(self, key, value, layer_idx, cache_kwargs=None):
        # Never reached while `is_updated` is True, but kept honest.
        layer = self.layers[layer_idx]
        return layer.keys, layer.values


class _SelfCache:
    """Self-attention cache backed by input tensors; records what it produced.

    `update` is called once per layer per step, in layer order. It concatenates
    the new position onto the past and stashes the result so the wrapper can
    return it as `present_*` graph outputs.
    """

    def __init__(self, keys: torch.Tensor, values: torch.Tensor) -> None:
        self.past_keys = keys
        self.past_values = values
        self.present_keys: list[torch.Tensor] = []
        self.present_values: list[torch.Tensor] = []

    def update(self, key, value, layer_idx, cache_kwargs=None):
        new_key = torch.cat([self.past_keys[layer_idx], key], dim=2)
        new_value = torch.cat([self.past_values[layer_idx], value], dim=2)
        self.present_keys.append(new_key)
        self.present_values.append(new_value)
        return new_key, new_value

    def get_seq_length(self, layer_idx: int = 0) -> int:
        return int(self.past_keys.shape[3])


class _EncoderDecoderCacheShim:
    """The surface `T5Gemma2MergedAttention` actually touches."""

    def __init__(self, self_cache: _SelfCache, cross_cache: _CrossCache, layers: int) -> None:
        self.self_attention_cache = self_cache
        self.cross_attention_cache = cross_cache
        # Preset: the cross projections were computed once, upstream. This is
        # the branch that makes caching worthwhile, and presetting it is what
        # removes the untraceable Python mutation.
        self.is_updated = {i: True for i in range(layers)}

    def get_seq_length(self, layer_idx: int = 0) -> int:
        return self.self_attention_cache.get_seq_length(layer_idx)


class CrossProjector(torch.nn.Module):
    """encoder_hidden_states -> stacked cross-attention K/V for every layer.

    Run once per input, not per token. Uses each layer's own `k_proj`/`v_proj`,
    the same weights `MergedAttention` would have used, so the values are
    identical to what the uncached path computes.
    """

    def __init__(self, model) -> None:
        super().__init__()
        self.decoder = model.model.decoder
        cfg = self.decoder.config
        self.head_dim = int(getattr(cfg, "head_dim", cfg.hidden_size // cfg.num_attention_heads))

    def forward(self, encoder_hidden_states: torch.Tensor):
        keys, values = [], []
        shape = (*encoder_hidden_states.shape[:-1], -1, self.head_dim)
        for layer in self.decoder.layers:
            attn = layer.self_attn
            key = attn.k_proj(encoder_hidden_states).view(shape).transpose(1, 2)
            # `k_norm` applies to the cross keys too — `MergedAttention` does
            # `cross_key_states = self.k_norm(cross_key_states)` right after the
            # projection. Omitting it produced logits that disagreed with the
            # uncached path from the very first step (rel ~0.9, argmax 0/8).
            #
            # Rotary embedding is deliberately NOT applied here, matching the
            # reference: cross keys index encoder positions, which the decoder's
            # own position grid says nothing about.
            keys.append(attn.k_norm(key))
            values.append(attn.v_proj(encoder_hidden_states).view(shape).transpose(1, 2))
        return torch.stack(keys), torch.stack(values)


class CachedDecoderStep(torch.nn.Module):
    """One decode step: a single token in, logits plus the grown cache out."""

    def __init__(self, model, enc_len: int) -> None:
        super().__init__()
        self.model = model
        self.decoder = model.model.decoder
        self.lm_head = model.lm_head
        self.enc_len = enc_len
        cfg = self.decoder.config
        self.num_layers = int(cfg.num_hidden_layers)
        self.sliding_window = int(getattr(cfg, "sliding_window", 512))

    def forward(
        self,
        decoder_input_ids,        # [B, 1]
        past_self_k,              # [L, B, H, P, D]
        past_self_v,
        cross_k,                  # [L, B, H, E, D]
        cross_v,
        encoder_attention_mask,   # [B, E]
    ):
        neg = torch.finfo(torch.float32).min
        batch = decoder_input_ids.shape[0]
        past_len = past_self_k.shape[3]
        total_self = past_len + 1

        self_cache = _SelfCache(past_self_k, past_self_v)
        cross_cache = _CrossCache(cross_k, cross_v)
        cache = _EncoderDecoderCacheShim(self_cache, cross_cache, self.num_layers)

        # One query at position `past_len`. Causally it may attend to every past
        # position and itself, so the full-attention self mask is all-allow.
        self_keep = torch.ones(
            batch, 1, 1, total_self, dtype=torch.bool, device=decoder_input_ids.device
        )
        # The sliding variant additionally drops anything older than the window.
        idx = torch.arange(total_self, device=decoder_input_ids.device)
        within = (past_len - idx) < self.sliding_window
        slide_keep = self_keep & within[None, None, None, :]

        def to_float(keep):
            return torch.zeros_like(keep, dtype=torch.float32).masked_fill(~keep, neg)

        self_masks = {
            "full_attention": to_float(self_keep),
            "sliding_attention": to_float(slide_keep),
        }
        enc_keep = encoder_attention_mask.to(torch.bool)[:, None, None, :]
        cross_masks = {"full_attention": to_float(enc_keep)}

        position_ids = torch.full(
            (1, 1), past_len, dtype=torch.long, device=decoder_input_ids.device
        )

        out = self.decoder(
            input_ids=decoder_input_ids,
            attention_mask=self_masks,
            position_ids=position_ids,
            past_key_values=cache,
            encoder_hidden_states=torch.zeros(
                batch, self.enc_len, self.decoder.config.hidden_size,
                device=decoder_input_ids.device,
            ),
            encoder_attention_mask=cross_masks,
            use_cache=True,
        )
        logits = self.lm_head(out.last_hidden_state)
        return (
            logits,
            torch.stack(self_cache.present_keys),
            torch.stack(self_cache.present_values),
        )
