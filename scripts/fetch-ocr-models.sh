#!/usr/bin/env bash
# Fetch the PP-OCRv5 mobile weights used by rung 3 text localization.
#
# Kept out of git because they are ~21 MB of binary that changes never. Apache
# 2.0, same licence as PaddleOCR itself.
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/crates/artist-computer/models"
base="https://huggingface.co/bukuroo/PPOCRv5-ONNX/resolve/main"

mkdir -p "$dir"
for file in ppocrv5-mobile-det.onnx ppocrv5-mobile-rec.onnx ppocrv5_dict.txt; do
  if [ -s "$dir/$file" ]; then
    echo "have $file"
    continue
  fi
  echo "fetching $file"
  curl -sSL --fail -o "$dir/$file" "$base/$file"
done
echo "models are in $dir"
