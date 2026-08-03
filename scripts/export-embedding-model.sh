#!/usr/bin/env bash
# Build an artist embedding-model directory from a Hugging Face model.
#
# Kept out of the build because it needs PyTorch, and the runtime does not: rten
# loads the resulting `.onnx` directly, so nothing here ships. Run it once per
# model and point `memory.model_dir` at the output.
#
#   scripts/export-embedding-model.sh google/embeddinggemma-300m ~/models/gemma
#   scripts/export-embedding-model.sh nomic-ai/CodeRankEmbed ~/models/coderank \
#       --query-prefix 'Represent this query for searching relevant code: '
#
# Note the published ONNX exports on the Hub are usually NOT usable: they are
# run through ONNX Runtime's fusion pass, which emits `com.microsoft` contrib
# operators (SimplifiedLayerNormalization, RotaryEmbedding, MultiHeadAttention)
# that rten rejects. This script exports from the PyTorch weights instead, which
# yields standard operators only.
#
# Gated models (EmbeddingGemma among them) need the terms accepted on the Hub
# and a token in place — `hf auth login`.
set -euo pipefail

if [[ $# -lt 2 ]]; then
    sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
    exit 2
fi

model="$1"
out="$2"
shift 2

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="${ARTIST_EXPORT_WORKDIR:-$(mktemp -d)}"
venv="$work/.venv"

echo "model:    $model"
echo "output:   $out"
echo "workdir:  $work"

if [[ ! -x "$venv/bin/python" ]]; then
    echo "== creating export venv"
    python3 -m venv "$venv"
    "$venv/bin/python" -m pip install --quiet --upgrade pip
    # torch from the CPU index: this never runs inference, only tracing, and the
    # CUDA wheels are an order of magnitude larger.
    "$venv/bin/python" -m pip install --quiet torch \
        --index-url https://download.pytorch.org/whl/cpu
    # optimum 2.x moved the ONNX exporter into a separate distribution, and the
    # export dies in its dynamic-axes fixup without onnxruntime present.
    "$venv/bin/python" -m pip install --quiet \
        "transformers>=4.56" optimum optimum-onnx onnx onnxruntime \
        safetensors sentencepiece numpy
fi

src="$work/src"
if [[ -d "$model" ]]; then
    src="$model"
    echo "== using local model directory"
else
    echo "== downloading $model"
    hf download "$model" --local-dir "$src" >/dev/null
fi

echo "== exporting bare transformer to ONNX"
rm -rf "$work/onnx"
"$venv/bin/optimum-cli" export onnx \
    --model "$src" --task feature-extraction --opset 17 "$work/onnx" 2>&1 \
    | grep -viE '^\s*$|warning' | tail -3 || true

echo "== appending pooling, collapsing dense, writing embed.json"
"$venv/bin/python" "$root/scripts/embed_export.py" \
    --src "$src" --onnx "$work/onnx" --out "$out" "$@"

echo
echo "wrote:"
ls -1 "$out"
echo
echo "point memory.model_dir at $out"
