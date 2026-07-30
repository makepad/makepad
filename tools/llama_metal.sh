#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
    cat <<'EOF'
Usage:
  tools/llama_metal.sh compare [model.gguf] [--debug-masks] [prompt...]
  tools/llama_metal.sh generate [model.gguf] [--debug-masks] [llama-generate args...]
  tools/llama_metal.sh tokenize [model.gguf] [llama-tokenize args...]
  tools/llama_metal.sh benchmark [model.gguf] [--debug-masks] [--max-new-tokens N] [--prefill-batches a,b,c] [--prompt TEXT]
  tools/llama_metal.sh upstream-completion [model.gguf] [--max-new-tokens N] [--context-tokens N] [prompt...]
  tools/llama_metal.sh upstream-capture [model.gguf] [--max-new-tokens N] [--context-tokens N] [--log-disable] [prompt...]
  tools/llama_metal.sh debug-files [model.gguf] [prompt...]
  tools/llama_metal.sh debug-files-step [model.gguf] [prompt...]

Examples:
  tools/llama_metal.sh compare
  tools/llama_metal.sh compare local/models/Qwen3.5-4B-Q5_K_M.gguf "Hello world"
  tools/llama_metal.sh compare local/models/Qwen3.5-4B-Q5_K_M.gguf --debug-masks "Hello world"
  tools/llama_metal.sh generate local/models/Qwen3.5-4B-Q5_K_M.gguf --max-new-tokens 64 --prefill-batch-size 32 --no-stream --prompt "Hello world"
  tools/llama_metal.sh generate local/models/Qwen3.5-4B-Q5_K_M.gguf --debug-masks --max-new-tokens 64 --prefill-batch-size 32 --no-stream --prompt "Hello world"
  tools/llama_metal.sh tokenize local/models/Qwen3.5-4B-Q5_K_M.gguf --ids --prompt "<|im_start|>user"
  tools/llama_metal.sh benchmark local/models/Qwen3.5-4B-Q5_K_M.gguf --max-new-tokens 256 --prefill-batches 8,32,64 --prompt "Hello world"
  tools/llama_metal.sh upstream-completion local/models/Qwen3.5-4B-Q5_K_M.gguf --max-new-tokens 256 "Hello world"
  tools/llama_metal.sh upstream-capture local/models/Qwen3.5-4B-Q5_K_M.gguf --max-new-tokens 32 "Hello world"
  tools/llama_metal.sh upstream-capture local/models/Qwen3.5-4B-Q5_K_M.gguf --max-new-tokens 32 --log-disable "Hello world"
  tools/llama_metal.sh debug-files local/models/Qwen3.5-4B-Q5_K_M.gguf "Hello world"
  tools/llama_metal.sh debug-files-step local/models/Qwen3.5-4B-Q5_K_M.gguf "Hello world"

Notes:
  - `compare` builds and runs the release `llama-compare` binary.
  - `generate` builds and runs the release `llama-generate` binary.
  - `tokenize` builds and runs the release native Rust `llama-tokenize` binary.
  - `compare --debug-masks` and `generate --debug-masks` set `LLAMA_DEBUG_MASKS=1` internally.
  - `benchmark` runs a small release throughput sweep over multiple prefill batch sizes.
  - `upstream-completion` runs deterministic upstream `llama-completion` with the same sampler/cache flags used by `llama-generate --verify-upstream`.
  - `upstream-capture` runs upstream `llama-completion`, captures stdout/stderr separately, and prints file paths plus a short preview.
  - `upstream-capture --log-disable` is currently not a trustworthy exact-text capture mode on this local build because it can yield empty stdout.
  - `debug-files` runs upstream `llama-debug` with saved logits into a fresh temp dir and lists the generated files.
  - `debug-files-step` rebuilds upstream `llama-debug`, runs the prompt token-by-token, and saves final plus per-step logits.
  - If no model is provided, the default is `local/models/Qwen3.5-4B-Q5_K_M.gguf`.
EOF
}

DEFAULT_MODEL="local/models/Qwen3.5-4B-Q5_K_M.gguf"
DEFAULT_PROMPT="Hello world"
ACTUAL_LLAMA_CPP_ROOT="/Users/admin/llama.cpp"
ACTUAL_LLAMA_CPP_BUILD_ROOT="$ACTUAL_LLAMA_CPP_ROOT/build-arm64-apple-clang-release"
DEFAULT_UPSTREAM_DEBUG_BIN="$ACTUAL_LLAMA_CPP_BUILD_ROOT/bin/llama-debug"
DEFAULT_UPSTREAM_COMPLETION_BIN="$ACTUAL_LLAMA_CPP_BUILD_ROOT/bin/llama-completion"
SUBCOMMAND="${1:-}"
DEFAULT_BENCH_PREFILL_BATCHES="8,32,64"
DEFAULT_BENCH_MAX_NEW_TOKENS=256

sync_upstream_debug_source() {
    if [ ! -d "$ACTUAL_LLAMA_CPP_ROOT" ]; then
        return
    fi

    if ! cmp -s \
        local/llama.cpp/examples/debug/debug.cpp \
        "$ACTUAL_LLAMA_CPP_ROOT/examples/debug/debug.cpp"; then
        cp \
            local/llama.cpp/examples/debug/debug.cpp \
            "$ACTUAL_LLAMA_CPP_ROOT/examples/debug/debug.cpp"
    fi
}

build_upstream_debug() {
    sync_upstream_debug_source
    make -C local/llama.cpp/build-arm64-apple-clang-release llama-debug -j8
    if [ \
        "$ACTUAL_LLAMA_CPP_BUILD_ROOT/examples/debug/CMakeFiles/llama-debug.dir/debug.cpp.o" \
        -nt "$DEFAULT_UPSTREAM_DEBUG_BIN" \
    ]; then
        make -C "$ACTUAL_LLAMA_CPP_BUILD_ROOT" \
            -f examples/debug/CMakeFiles/llama-debug.dir/build.make \
            bin/llama-debug
    fi
}

exec_with_optional_debug_masks() {
    local debug_masks="$1"
    shift
    if [ "$debug_masks" -eq 1 ]; then
        exec env LLAMA_DEBUG_MASKS=1 "$@"
    fi
    exec "$@"
}

case "$SUBCOMMAND" in
    compare)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        DEBUG_MASKS=0
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --debug-masks)
                    DEBUG_MASKS=1
                    shift
                    ;;
                *)
                    break
                    ;;
            esac
        done
        PROMPT="${*:-Hello world}"

        cargo build --manifest-path libs/llama/Cargo.toml --release --bin llama-compare
        exec_with_optional_debug_masks \
            "$DEBUG_MASKS" \
            libs/llama/target/release/llama-compare \
            "$MODEL_PATH" \
            "$PROMPT"
        ;;

    generate)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        DEBUG_MASKS=0
        FORWARD_ARGS=()
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --debug-masks)
                    DEBUG_MASKS=1
                    shift
                    ;;
                *)
                    FORWARD_ARGS+=("$1")
                    shift
                    ;;
            esac
        done

        cargo build --manifest-path libs/llama/Cargo.toml --release --bin llama-generate
        exec_with_optional_debug_masks \
            "$DEBUG_MASKS" \
            libs/llama/target/release/llama-generate \
            "$MODEL_PATH" \
            "${FORWARD_ARGS[@]}"
        ;;

    tokenize)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        FORWARD_ARGS=("$@")

        cargo build --manifest-path libs/llama/Cargo.toml --release --bin llama-tokenize
        exec \
            libs/llama/target/release/llama-tokenize \
            "$MODEL_PATH" \
            "${FORWARD_ARGS[@]}"
        ;;

    benchmark)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        DEBUG_MASKS=0
        MAX_NEW_TOKENS="$DEFAULT_BENCH_MAX_NEW_TOKENS"
        PREFILL_BATCHES="$DEFAULT_BENCH_PREFILL_BATCHES"
        PROMPT="$DEFAULT_PROMPT"
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --debug-masks)
                    DEBUG_MASKS=1
                    shift
                    ;;
                --max-new-tokens)
                    MAX_NEW_TOKENS="$2"
                    shift 2
                    ;;
                --prefill-batches)
                    PREFILL_BATCHES="$2"
                    shift 2
                    ;;
                --prompt)
                    PROMPT="$2"
                    shift 2
                    ;;
                *)
                    echo "benchmark: unknown arg: $1" >&2
                    exit 1
                    ;;
            esac
        done

        cargo build --manifest-path libs/llama/Cargo.toml --release --bin llama-generate

        IFS=',' read -r -a PREFILL_BATCH_ARRAY <<<"$PREFILL_BATCHES"
        for PREFILL_BATCH_SIZE in "${PREFILL_BATCH_ARRAY[@]}"; do
            printf 'benchmark.prefill_batch_size: %s\n' "$PREFILL_BATCH_SIZE"
            if [ "$DEBUG_MASKS" -eq 1 ]; then
                env LLAMA_DEBUG_MASKS=1 \
                    libs/llama/target/release/llama-generate \
                    "$MODEL_PATH" \
                    --max-new-tokens "$MAX_NEW_TOKENS" \
                    --prefill-batch-size "$PREFILL_BATCH_SIZE" \
                    --no-stream \
                    --prompt "$PROMPT"
            else
                libs/llama/target/release/llama-generate \
                    "$MODEL_PATH" \
                    --max-new-tokens "$MAX_NEW_TOKENS" \
                    --prefill-batch-size "$PREFILL_BATCH_SIZE" \
                    --no-stream \
                    --prompt "$PROMPT"
            fi
            printf '\n'
        done
        ;;

    upstream-completion)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        CONTEXT_TOKENS=256
        MAX_NEW_TOKENS=32
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --max-new-tokens)
                    MAX_NEW_TOKENS="$2"
                    shift 2
                    ;;
                --context-tokens)
                    CONTEXT_TOKENS="$2"
                    shift 2
                    ;;
                *)
                    break
                    ;;
            esac
        done
        PROMPT="${*:-$DEFAULT_PROMPT}"

        exec "$DEFAULT_UPSTREAM_COMPLETION_BIN" \
            -m "$MODEL_PATH" \
            -p "$PROMPT" \
            -n "$MAX_NEW_TOKENS" \
            -c "$CONTEXT_TOKENS" \
            -no-cnv \
            --simple-io \
            --no-display-prompt \
            --no-warmup \
            --seed 0 \
            --temp 0 \
            --top-k 1 \
            --top-p 1 \
            --repeat-penalty 1 \
            --presence-penalty 0 \
            --frequency-penalty 0 \
            --dry-multiplier 0 \
            -fa on \
            -ctk f16 \
            -ctv f16
        ;;

    upstream-capture)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        CONTEXT_TOKENS=256
        MAX_NEW_TOKENS=32
        LOG_DISABLE=0
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --max-new-tokens)
                    MAX_NEW_TOKENS="$2"
                    shift 2
                    ;;
                --context-tokens)
                    CONTEXT_TOKENS="$2"
                    shift 2
                    ;;
                --log-disable)
                    LOG_DISABLE=1
                    shift
                    ;;
                *)
                    break
                    ;;
            esac
        done
        PROMPT="${*:-$DEFAULT_PROMPT}"
        TMPDIR="$(mktemp -d /tmp/llama-upstream-capture.XXXXXX)"
        STDOUT_PATH="$TMPDIR/stdout.txt"
        STDERR_PATH="$TMPDIR/stderr.txt"

        COMMAND=(
            "$DEFAULT_UPSTREAM_COMPLETION_BIN"
            -m "$MODEL_PATH"
            -p "$PROMPT"
            -n "$MAX_NEW_TOKENS"
            -c "$CONTEXT_TOKENS"
            -no-cnv
            --simple-io
            --no-display-prompt
            --no-warmup
            --seed 0
            --temp 0
            --top-k 1
            --top-p 1
            --repeat-penalty 1
            --presence-penalty 0
            --frequency-penalty 0
            --dry-multiplier 0
            -fa on
            -ctk f16
            -ctv f16
        )
        if [ "$LOG_DISABLE" -eq 1 ]; then
            COMMAND+=(--log-disable)
        fi

        "${COMMAND[@]}" >"$STDOUT_PATH" 2>"$STDERR_PATH"

        printf 'tmpdir=%s\n' "$TMPDIR"
        printf 'stdout=%s\n' "$STDOUT_PATH"
        printf 'stderr=%s\n' "$STDERR_PATH"
        wc -c "$STDOUT_PATH" "$STDERR_PATH"
        printf 'STDOUT\n'
        sed -n '1,30p' "$STDOUT_PATH"
        printf 'STDERR_TAIL\n'
        tail -n 40 "$STDERR_PATH"
        ;;

    debug-files)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        PROMPT="${*:-$DEFAULT_PROMPT}"
        TMPDIR="$(mktemp -d /tmp/llama-debug-files.XXXXXX)"

        build_upstream_debug

        "$DEFAULT_UPSTREAM_DEBUG_BIN" \
            -m "$MODEL_PATH" \
            -p "$PROMPT" \
            --tensor-filter result_output \
            --save-logits \
            --logits-output-dir "$TMPDIR" \
            -ngl 0 \
            -fa 0 \
            --seed 0 \
            --temp 0 \
            --top-k 1

        printf 'tmpdir=%s\n' "$TMPDIR"
        ls -1 "$TMPDIR"
        ;;

    debug-files-step)
        shift || true
        MODEL_PATH="${1:-$DEFAULT_MODEL}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        PROMPT="${*:-$DEFAULT_PROMPT}"
        TMPDIR="$(mktemp -d /tmp/llama-debug-step-files.XXXXXX)"

        build_upstream_debug

        MAKEPAD_LLAMA_DEBUG_STEP_PROMPT=1 \
        MAKEPAD_LLAMA_DEBUG_SAVE_STEP_LOGITS=1 \
        "$DEFAULT_UPSTREAM_DEBUG_BIN" \
            -m "$MODEL_PATH" \
            -p "$PROMPT" \
            --tensor-filter result_output \
            --save-logits \
            --logits-output-dir "$TMPDIR" \
            -ngl 0 \
            -fa 0 \
            --seed 0 \
            --temp 0 \
            --top-k 1

        printf 'tmpdir=%s\n' "$TMPDIR"
        ls -1 "$TMPDIR"
        ;;

    ""|-h|--help|help)
        usage
        ;;

    *)
        echo "unknown subcommand: $SUBCOMMAND" >&2
        usage >&2
        exit 1
        ;;
esac
