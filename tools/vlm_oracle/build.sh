#!/usr/bin/env bash
# Build the clip.cpp oracle against the local llama.cpp checkout.
set -euo pipefail
LLAMA_CPP="${LLAMA_CPP:-/Users/admin/llama.cpp}"
# NOTE: build/ is stale (Aug 2025); build-arm64-apple-clang-release matches the
# April 2026 source and has qwen3vl mmproj support.
LLAMA_BIN="${LLAMA_BIN:-$LLAMA_CPP/build-arm64-apple-clang-release/bin}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

clang++ -std=c++17 -O2 \
    -I "$LLAMA_CPP/tools/mtmd" \
    -I "$LLAMA_CPP/ggml/include" \
    -I "$LLAMA_CPP/include" \
    -I "$LLAMA_CPP/vendor" \
    "$DIR/clip_dump.cpp" \
    -L "$LLAMA_BIN" -lmtmd -lggml -lggml-base -lllama \
    -Wl,-rpath,"$LLAMA_BIN" \
    -o "$DIR/clip_dump"
echo "built $DIR/clip_dump"
