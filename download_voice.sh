#!/usr/bin/env bash
set -euo pipefail

URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin?download=true"
OUT="${1:-ggml-large-v3-turbo.bin}"
TMP="${OUT}.part"

if [[ -f "${OUT}" ]]; then
    echo "Model already exists: ${OUT}"
    exit 0
fi

echo "Downloading ${OUT}..."
curl -L --fail --retry 3 --retry-delay 2 -o "${TMP}" "${URL}"
mv "${TMP}" "${OUT}"
echo "Done: ${OUT}"
