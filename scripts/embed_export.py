"""Turn a sentence-transformers embedding model into an artist model directory.

Driven by `export-embedding-model.sh`; not usually run directly.

Produces, in the output directory:

    model.onnx          transformer + pooling, output node `sentence_embedding`
    tokenizer.json      copied from the source model
    embed.json          prefixes, token ceiling, projection filename
    dense_collapsed.bin optional [out][in] f32 projection, row-major LE

Two things here are less obvious than they look.

**Pooling is appended as graph surgery, not exported.** Wrapping the model in an
nn.Module that pools and calling `torch.onnx.export` on it dies inside
functorch (`RuntimeError: unordered_map::at`). Exporting the bare transformer
works, so we export that and append the pooling as plain ONNX nodes. That also
keeps the verified transformer bytes untouched.

**Pooling stays in the graph on purpose.** Masked mean pooling has a silent
failure mode — let padding contribute and every vector is quietly wrong, with no
error anywhere. In the graph it cannot be got wrong by a caller.

**Sequential Dense layers are collapsed.** sentence-transformers heads are often
`Linear(a->b)` then `Linear(b->a)` with no bias and no activation, which is a
single `a x a` matrix. Collapsing keeps it out of the graph so that Matryoshka
truncation becomes a narrower matmul rather than a slice.
"""

import argparse
import json
import os
import shutil

import numpy as np
import onnx
import torch
from onnx import TensorProto, helper, numpy_helper
from safetensors.torch import load_file


def append_pooling(src: str, dst: str, mode: str) -> None:
    m = onnx.load(src, load_external_data=False)
    g = m.graph
    hidden, mask_in = "last_hidden_state", "attention_mask"

    if mode == "cls":
        nodes = [
            helper.make_node("Constant", [], ["_zero"], value=numpy_helper.from_array(
                np.array([0], dtype=np.int64), "_zero_v")),
            helper.make_node("Constant", [], ["_one"], value=numpy_helper.from_array(
                np.array([1], dtype=np.int64), "_one_v")),
            helper.make_node("Constant", [], ["_ax1"], value=numpy_helper.from_array(
                np.array([1], dtype=np.int64), "_ax1_v")),
            helper.make_node("Slice", [hidden, "_zero", "_one", "_ax1"], ["_cls"]),
            helper.make_node("Squeeze", ["_cls", "_ax1"], ["sentence_embedding"]),
        ]
    elif mode == "mean":
        nodes = [
            helper.make_node("Constant", [], ["_axes2"], value=numpy_helper.from_array(
                np.array([2], dtype=np.int64), "_axes2_v")),
            helper.make_node("Constant", [], ["_axes1"], value=numpy_helper.from_array(
                np.array([1], dtype=np.int64), "_axes1_v")),
            helper.make_node("Constant", [], ["_eps"], value=numpy_helper.from_array(
                np.array(1e-9, dtype=np.float32), "_eps_v")),
            helper.make_node("Cast", [mask_in], ["_maskf"], to=TensorProto.FLOAT),
            helper.make_node("Unsqueeze", ["_maskf", "_axes2"], ["_mask3"]),
            helper.make_node("Mul", [hidden, "_mask3"], ["_masked"]),
            helper.make_node("ReduceSum", ["_masked", "_axes1"], ["_summed"], keepdims=0),
            helper.make_node("ReduceSum", ["_mask3", "_axes1"], ["_counts"], keepdims=0),
            helper.make_node("Clip", ["_counts", "_eps"], ["_safe"]),
            helper.make_node("Div", ["_summed", "_safe"], ["sentence_embedding"]),
        ]
    else:
        raise SystemExit(f"unknown pooling mode {mode!r}")

    g.node.extend(nodes)
    width = g.output[0].type.tensor_type.shape.dim[-1].dim_value or "width"
    del g.output[:]
    g.output.extend([
        helper.make_tensor_value_info(
            "sentence_embedding", TensorProto.FLOAT, ["batch_size", width])
    ])
    onnx.save(m, dst, save_as_external_data=False)

    check = onnx.load(dst, load_external_data=False)
    domains = set(n.domain for n in check.graph.node)
    if domains - {""}:
        raise SystemExit(
            f"graph uses non-standard operator domains {domains}; rten will "
            "reject it. Re-export without ONNX Runtime fusion."
        )
    print(f"  pooling={mode}, outputs={[o.name for o in check.graph.output]}, domains ok")


def detect_pooling(src_dir: str) -> str:
    cfg = os.path.join(src_dir, "1_Pooling", "config.json")
    if not os.path.exists(cfg):
        return "cls"
    with open(cfg) as f:
        p = json.load(f)
    if p.get("pooling_mode_mean_tokens"):
        return "mean"
    if p.get("pooling_mode_cls_token"):
        return "cls"
    raise SystemExit(f"unsupported pooling in {cfg}: {p}")


def collapse_dense(src_dir: str, out_dir: str) -> str | None:
    """Compose consecutive bias-free Identity Dense layers into one matrix."""
    mats = []
    for name in sorted(os.listdir(src_dir)):
        if not name.endswith("_Dense"):
            continue
        d = os.path.join(src_dir, name)
        with open(os.path.join(d, "config.json")) as f:
            cfg = json.load(f)
        if cfg.get("bias") or "Identity" not in cfg.get("activation_function", ""):
            raise SystemExit(
                f"{name} has a bias or a non-Identity activation, so it cannot be "
                "collapsed. Leave it in the graph instead."
            )
        w = load_file(os.path.join(d, "model.safetensors"))["linear.weight"].float()
        mats.append((name, w))

    if not mats:
        return None

    combined = mats[0][1]
    for _, w in mats[1:]:
        combined = w @ combined
    combined = combined.contiguous()
    path = os.path.join(out_dir, "dense_collapsed.bin")
    combined.numpy().astype("<f4").tofile(path)
    print(f"  collapsed {[n for n, _ in mats]} -> {tuple(combined.shape)} "
          f"({combined.numel() * 4} bytes)")
    return "dense_collapsed.bin"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True, help="downloaded model directory")
    ap.add_argument("--onnx", required=True, help="bare transformer export directory")
    ap.add_argument("--out", required=True, help="artist model directory to write")
    ap.add_argument("--query-prefix", default="")
    ap.add_argument("--document-prefix", default="")
    ap.add_argument("--max-tokens", type=int, default=512)
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)

    mode = detect_pooling(args.src)
    print(f"exporting from {args.src} (pooling: {mode})")
    append_pooling(os.path.join(args.onnx, "model.onnx"),
                   os.path.join(args.out, "model.onnx"), mode)

    shutil.copy(os.path.join(args.src, "tokenizer.json"),
                os.path.join(args.out, "tokenizer.json"))

    dense = collapse_dense(args.src, args.out)

    cfg = {
        "query_prefix": args.query_prefix,
        "document_prefix": args.document_prefix,
        "max_tokens": args.max_tokens,
    }
    if dense:
        cfg["dense"] = dense
    with open(os.path.join(args.out, "embed.json"), "w") as f:
        json.dump(cfg, f, indent=2)
        f.write("\n")
    print(f"  embed.json {cfg}")
    print(f"done: {args.out}")


if __name__ == "__main__":
    main()
