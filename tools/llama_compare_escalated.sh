#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

DEFAULT_MODEL="local/models/gemma-4-E4B-it-UD-Q5_K_XL.gguf"
DEFAULT_PROMPT="What model are you?"
DEFAULT_LOG_PATH="/tmp/llama_compare_escalated.log"

MODEL_PATH="${1:-$DEFAULT_MODEL}"
if [ "$#" -gt 0 ]; then
    shift
fi
PROMPT="${*:-$DEFAULT_PROMPT}"
LOG_PATH="${LLAMA_COMPARE_LOG_PATH:-$DEFAULT_LOG_PATH}"

tools/llama_metal.sh compare "$MODEL_PATH" "$PROMPT" >"$LOG_PATH" 2>&1 || {
    status=$?
    printf 'llama-compare failed (exit %s)\n' "$status"
    printf 'log: %s\n' "$LOG_PATH"
    tail -n 80 "$LOG_PATH" || true
    exit "$status"
}

printf 'llama-compare succeeded\n'
printf 'log: %s\n' "$LOG_PATH"
rg -n \
    "compare.max_abs_diff|compare.cosine_similarity|gemma_upstream.result_norm_max_abs_diff|gemma_upstream.result_output_max_abs_diff|gemma_probe.result_output_vs_hybrid_max_abs_diff|gemma_probe.result_output_vs_upstream_max_abs_diff|gemma_upstream.layer_stack.max_diff_max_abs_diff" \
    "$LOG_PATH" || true
