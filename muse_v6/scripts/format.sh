#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo +1.97.1 fmt --all
