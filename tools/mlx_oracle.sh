#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

CMAKE_BIN="/Applications/CMake.app/Contents/bin/cmake"
XCRUN_WRAPPER_DIR="$ROOT_DIR/tools/mlx_oracle/bin"
BUILD_DIR="$ROOT_DIR/build/mlx-oracle-release-escalated"
DEFAULT_MODEL_PATH="local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors"
RUST_BIN_DIR="$ROOT_DIR/target/release"
ARTIFACT_DIR="$ROOT_DIR/build/mlx-port-artifacts"

usage() {
    cat <<'EOF'
Usage:
  tools/mlx_oracle.sh configure
  tools/mlx_oracle.sh build <target>
  tools/mlx_oracle.sh run <target> [args...]
  tools/mlx_oracle.sh affine-dequantize-row [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh quantized-matmul-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh quantized-matmul-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh rms-norm-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh rms-norm-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh rms-norm-qproj-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh rms-norm-qproj-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh rms-norm-qkv-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-rope-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-rope-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh qk-attention-logits-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh qk-attention-logits-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh qkv-attention-output-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh qkv-attention-output-cached-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh qkv-attention-output-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-oproj-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-oproj-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-post-attn-norm-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-post-attn-norm-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-post-attn-residual-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-post-attn-residual-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-router-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-router-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-expert-gate-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-expert-gate-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-expert-up-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-expert-up-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-expert-geglu-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-expert-geglu-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-expert-down-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-expert-down-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-experts-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-experts-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-post-ffn-norm2-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-post-ffn-norm2-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-merge-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-merge-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-pre-ffn-norm-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-pre-ffn-norm-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-pre-ffn-gate-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-pre-ffn-gate-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-pre-ffn-up-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-pre-ffn-up-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-pre-ffn-geglu-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-pre-ffn-geglu-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-pre-ffn-down-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-pre-ffn-down-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-post-ffn-norm-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-post-ffn-norm-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-moe-post-ffn-norm1-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-moe-post-ffn-norm1-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh attention-post-ffn-residual-case [model.safetensors] [--device cpu|gpu]
  tools/mlx_oracle.sh attention-post-ffn-residual-bench [model.safetensors] [--warmup N] [--iters N]
  tools/mlx_oracle.sh rust-run <bin> [args...]
  tools/mlx_oracle.sh pair-diff <qk-attention-logits|qkv-attention-output|attention-oproj|attention-post-attn-norm|attention-post-attn-residual|attention-router|attention-pre-ffn-norm|attention-pre-ffn-gate|attention-pre-ffn-up|attention-pre-ffn-geglu|attention-pre-ffn-down|attention-post-ffn-norm|attention-post-ffn-residual>
  tools/mlx_oracle.sh pair-bench <qk-attention-logits|qkv-attention-output|attention-oproj|attention-post-attn-norm|attention-post-attn-residual|attention-router|attention-pre-ffn-norm|attention-pre-ffn-gate|attention-pre-ffn-up|attention-pre-ffn-geglu|attention-pre-ffn-down|attention-post-ffn-norm|attention-post-ffn-residual> [--warmup N] [--iters N]

Examples:
  tools/mlx_oracle.sh configure
  tools/mlx_oracle.sh build quantized_matmul_case
  tools/mlx_oracle.sh run quantized_matmul_case local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors
  tools/mlx_oracle.sh affine-dequantize-row
  tools/mlx_oracle.sh affine-dequantize-row --device gpu
  tools/mlx_oracle.sh quantized-matmul-case
  tools/mlx_oracle.sh quantized-matmul-case --device gpu
  tools/mlx_oracle.sh quantized-matmul-bench --warmup 10 --iters 50
  tools/mlx_oracle.sh quantized-matmul-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh rms-norm-case --device gpu
  tools/mlx_oracle.sh rms-norm-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh rms-norm-qproj-case --device gpu
  tools/mlx_oracle.sh rms-norm-qproj-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh rms-norm-qkv-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-case --device gpu
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-rope-case --device gpu
  tools/mlx_oracle.sh rms-norm-qproj-qnorm-rope-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh qk-attention-logits-case --device gpu
  tools/mlx_oracle.sh qk-attention-logits-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh qkv-attention-output-case --device gpu
  tools/mlx_oracle.sh qkv-attention-output-cached-case --device gpu
  tools/mlx_oracle.sh qkv-attention-output-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-oproj-case --device gpu
  tools/mlx_oracle.sh attention-oproj-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-post-attn-norm-case --device gpu
  tools/mlx_oracle.sh attention-post-attn-norm-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-post-attn-residual-case --device gpu
  tools/mlx_oracle.sh attention-post-attn-residual-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-router-case --device gpu
  tools/mlx_oracle.sh attention-router-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-expert-gate-case --device gpu
  tools/mlx_oracle.sh attention-moe-expert-gate-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-expert-up-case --device gpu
  tools/mlx_oracle.sh attention-moe-expert-up-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-expert-geglu-case --device gpu
  tools/mlx_oracle.sh attention-moe-expert-geglu-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-expert-down-case --device gpu
  tools/mlx_oracle.sh attention-moe-expert-down-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-experts-case --device gpu
  tools/mlx_oracle.sh attention-moe-experts-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-post-ffn-norm2-case --device gpu
  tools/mlx_oracle.sh attention-moe-post-ffn-norm2-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-merge-case --device gpu
  tools/mlx_oracle.sh attention-moe-merge-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-pre-ffn-norm-case --device gpu
  tools/mlx_oracle.sh attention-pre-ffn-norm-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-pre-ffn-gate-case --device gpu
  tools/mlx_oracle.sh attention-pre-ffn-gate-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-pre-ffn-up-case --device gpu
  tools/mlx_oracle.sh attention-pre-ffn-up-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-pre-ffn-geglu-case --device gpu
  tools/mlx_oracle.sh attention-pre-ffn-geglu-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-pre-ffn-down-case --device gpu
  tools/mlx_oracle.sh attention-pre-ffn-down-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-post-ffn-norm-case --device gpu
  tools/mlx_oracle.sh attention-post-ffn-norm-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-moe-post-ffn-norm1-case --device gpu
  tools/mlx_oracle.sh attention-moe-post-ffn-norm1-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh attention-post-ffn-residual-case --device gpu
  tools/mlx_oracle.sh attention-post-ffn-residual-bench --device gpu --warmup 10 --iters 100
  tools/mlx_oracle.sh rust-run metal_attention_oproj_row --dump-all-f32-bits
  tools/mlx_oracle.sh pair-diff attention-oproj
  tools/mlx_oracle.sh pair-bench attention-oproj --warmup 20 --iters 1000

Notes:
  - Uses the source-tree MLX checkout in `local/mlx`.
  - Builds into `build/mlx-oracle-release-escalated`.
  - Prepends the local `xcrun` wrapper needed on this machine.
  - Writes pairwise dump artifacts into `build/mlx-port-artifacts`.
EOF
}

configure_oracle() {
    PATH="$XCRUN_WRAPPER_DIR:$PATH" "$CMAKE_BIN" \
        -S local/mlx \
        -B "$BUILD_DIR" \
        -DCMAKE_BUILD_TYPE=Release \
        -DMLX_BUILD_TESTS=OFF \
        -DMLX_BUILD_EXAMPLES=ON \
        -DMLX_BUILD_BENCHMARKS=OFF \
        -DMLX_BUILD_PYTHON_BINDINGS=OFF \
        -DMLX_BUILD_METAL=ON \
        -DMLX_BUILD_CPU=ON \
        -DMLX_BUILD_SAFETENSORS=ON \
        -DMLX_BUILD_GGUF=ON \
        -DMLX_METAL_DEBUG=OFF
}

build_target() {
    local target="$1"
    if [ ! -f "$BUILD_DIR/CMakeCache.txt" ]; then
        configure_oracle
    fi
    PATH="$XCRUN_WRAPPER_DIR:$PATH" "$CMAKE_BIN" \
        --build "$BUILD_DIR" \
        --config Release \
        --target "$target" \
        -j4
}

run_target() {
    local target="$1"
    shift
    build_target "$target"
    exec "$BUILD_DIR/examples/cpp/$target" "$@"
}

rust_bin_path() {
    local bin="$1"
    echo "$RUST_BIN_DIR/$bin"
}

validate_rust_bin() {
    local bin="$1"
    case "$bin" in
        metal_qk_attention_logits_row|metal_qkv_attention_output_row|metal_qkv_attention_output_cached_row|metal_attention_oproj_row|metal_attention_post_attention_norm_row|metal_attention_post_attention_residual_row|metal_attention_pre_feedforward_norm_row|metal_attention_pre_feedforward_gate_row|metal_attention_pre_feedforward_up_row|metal_attention_pre_feedforward_geglu_row|metal_attention_pre_feedforward_down_row)
            ;;
        *)
            echo "unsupported rust binary: $bin" >&2
            exit 1
            ;;
    esac
}

ensure_rust_bin() {
    local bin="$1"
    local bin_path
    bin_path="$(rust_bin_path "$bin")"
    if [ ! -x "$bin_path" ]; then
        local package="makepad-mlx"
        echo "missing rust binary: $bin_path" >&2
        echo "build it with: cargo build --release -p $package --bin $bin" >&2
        exit 1
    fi
}

run_rust_bin() {
    local bin="$1"
    shift
    validate_rust_bin "$bin"
    ensure_rust_bin "$bin"
    exec "$(rust_bin_path "$bin")" "$@"
}

PAIR_ORACLE_EXTRA_ARGS=()
PAIR_RUST_EXTRA_ARGS=()

resolve_pair() {
    local pair="$1"
    PAIR_ORACLE_EXTRA_ARGS=()
    PAIR_RUST_EXTRA_ARGS=()
    case "$pair" in
        qk-attention-logits)
            PAIR_ORACLE_CASE="qk-attention-logits-case"
            PAIR_ORACLE_BENCH="qk-attention-logits-bench"
            PAIR_RUST_BIN="metal_qk_attention_logits_row"
            ;;
        qkv-attention-output)
            PAIR_ORACLE_CASE="qkv-attention-output-case"
            PAIR_ORACLE_BENCH="qkv-attention-output-bench"
            PAIR_RUST_BIN="metal_qkv_attention_output_row"
            ;;
        attention-oproj)
            PAIR_ORACLE_CASE="attention-oproj-case"
            PAIR_ORACLE_BENCH="attention-oproj-bench"
            PAIR_RUST_BIN="metal_attention_oproj_row"
            ;;
        attention-post-attn-norm)
            PAIR_ORACLE_CASE="attention-post-attn-norm-case"
            PAIR_ORACLE_BENCH="attention-post-attn-norm-bench"
            PAIR_RUST_BIN="metal_attention_post_attention_norm_row"
            ;;
        attention-post-attn-residual)
            PAIR_ORACLE_CASE="attention-post-attn-residual-case"
            PAIR_ORACLE_BENCH="attention-post-attn-residual-bench"
            PAIR_RUST_BIN="metal_attention_post_attention_residual_row"
            ;;
        attention-router)
            PAIR_ORACLE_CASE="attention-router-case"
            PAIR_ORACLE_BENCH="attention-router-bench"
            PAIR_RUST_BIN="metal_attention_post_attention_residual_row"
            PAIR_RUST_EXTRA_ARGS=(--router)
            ;;
        attention-pre-ffn-norm)
            PAIR_ORACLE_CASE="attention-pre-ffn-norm-case"
            PAIR_ORACLE_BENCH="attention-pre-ffn-norm-bench"
            PAIR_RUST_BIN="metal_attention_pre_feedforward_norm_row"
            ;;
        attention-pre-ffn-gate)
            PAIR_ORACLE_CASE="attention-pre-ffn-gate-case"
            PAIR_ORACLE_BENCH="attention-pre-ffn-gate-bench"
            PAIR_RUST_BIN="metal_attention_pre_feedforward_gate_row"
            ;;
        attention-pre-ffn-up)
            PAIR_ORACLE_CASE="attention-pre-ffn-up-case"
            PAIR_ORACLE_BENCH="attention-pre-ffn-up-bench"
            PAIR_RUST_BIN="metal_attention_pre_feedforward_up_row"
            ;;
        attention-pre-ffn-geglu)
            PAIR_ORACLE_CASE="attention-pre-ffn-geglu-case"
            PAIR_ORACLE_BENCH="attention-pre-ffn-geglu-bench"
            PAIR_RUST_BIN="metal_attention_pre_feedforward_geglu_row"
            ;;
        attention-pre-ffn-down)
            PAIR_ORACLE_CASE="attention-pre-ffn-down-case"
            PAIR_ORACLE_BENCH="attention-pre-ffn-down-bench"
            PAIR_RUST_BIN="metal_attention_pre_feedforward_down_row"
            ;;
        attention-post-ffn-norm)
            PAIR_ORACLE_CASE="attention-post-ffn-norm-case"
            PAIR_ORACLE_BENCH="attention-post-ffn-norm-bench"
            PAIR_RUST_BIN="metal_attention_pre_feedforward_down_row"
            PAIR_RUST_EXTRA_ARGS=(--post-feedforward-norm)
            ;;
        attention-post-ffn-residual)
            PAIR_ORACLE_CASE="attention-post-ffn-residual-case"
            PAIR_ORACLE_BENCH="attention-post-ffn-residual-bench"
            PAIR_RUST_BIN="metal_attention_pre_feedforward_down_row"
            PAIR_RUST_EXTRA_ARGS=(--final-residual)
            ;;
        *)
            echo "unsupported pair: $pair" >&2
            exit 1
            ;;
    esac
}

extract_all_f32_bits() {
    local file="$1"
    sed -n 's/^all_f32_bits=//p' "$file" | tail -n 1
}

extract_all_u32_words() {
    local file="$1"
    sed -n 's/^all_u32_words=//p' "$file" | tail -n 1
}

extract_fnv_line() {
    local file="$1"
    grep -E 'fnv1a64=0x' "$file" | tail -n 1 || true
}

extract_first16_line() {
    local file="$1"
    grep -E 'first16_f32_bits=' "$file" | tail -n 1 || true
}

compare_dump_files() {
    local oracle_file="$1"
    local rust_file="$2"
    local oracle_bits_csv rust_bits_csv oracle_words_csv rust_words_csv
    oracle_bits_csv="$(extract_all_f32_bits "$oracle_file")"
    rust_bits_csv="$(extract_all_f32_bits "$rust_file")"
    oracle_words_csv="$(extract_all_u32_words "$oracle_file")"
    rust_words_csv="$(extract_all_u32_words "$rust_file")"

    if [ -z "$oracle_bits_csv" ] && [ -z "$oracle_words_csv" ]; then
        echo "status=error" >&2
        echo "reason=missing_oracle_dump_payload" >&2
        echo "oracle_output=$oracle_file" >&2
        return 2
    fi
    if [ -n "$oracle_bits_csv" ] && [ -z "$rust_bits_csv" ]; then
        echo "status=error" >&2
        echo "reason=missing_rust_all_f32_bits" >&2
        echo "rust_output=$rust_file" >&2
        return 2
    fi
    if [ -n "$oracle_words_csv" ] && [ -z "$rust_words_csv" ]; then
        echo "status=error" >&2
        echo "reason=missing_rust_all_u32_words" >&2
        echo "rust_output=$rust_file" >&2
        return 2
    fi

    if [ -n "$oracle_bits_csv" ]; then
        local -a oracle_bits rust_bits
        IFS=',' read -r -a oracle_bits <<< "$oracle_bits_csv"
        IFS=',' read -r -a rust_bits <<< "$rust_bits_csv"

        local oracle_len="${#oracle_bits[@]}"
        local rust_len="${#rust_bits[@]}"
        local count="$oracle_len"
        if [ "$rust_len" -lt "$count" ]; then
            count="$rust_len"
        fi

        local i
        for ((i = 0; i < count; ++i)); do
            local oracle_word rust_word
            oracle_word="$(printf '%s' "${oracle_bits[i]}" | tr '[:upper:]' '[:lower:]')"
            rust_word="$(printf '%s' "${rust_bits[i]}" | tr '[:upper:]' '[:lower:]')"
            if [ "$oracle_word" != "$rust_word" ]; then
                echo "status=mismatch"
                echo "first_mismatch_index=$i"
                echo "oracle_bits=$oracle_word"
                echo "rust_bits=$rust_word"
                return 1
            fi
        done

        if [ "$oracle_len" -ne "$rust_len" ]; then
            echo "status=length-mismatch"
            echo "oracle_len=$oracle_len"
            echo "rust_len=$rust_len"
            return 1
        fi
    fi

    if [ -n "$oracle_words_csv" ]; then
        local -a oracle_words rust_words
        IFS=',' read -r -a oracle_words <<< "$oracle_words_csv"
        IFS=',' read -r -a rust_words <<< "$rust_words_csv"

        local oracle_words_len="${#oracle_words[@]}"
        local rust_words_len="${#rust_words[@]}"
        local count_words="$oracle_words_len"
        if [ "$rust_words_len" -lt "$count_words" ]; then
            count_words="$rust_words_len"
        fi

        local i
        for ((i = 0; i < count_words; ++i)); do
            local oracle_word rust_word
            oracle_word="$(printf '%s' "${oracle_words[i]}" | tr '[:upper:]' '[:lower:]')"
            rust_word="$(printf '%s' "${rust_words[i]}" | tr '[:upper:]' '[:lower:]')"
            if [ "$oracle_word" != "$rust_word" ]; then
                echo "status=mismatch"
                echo "first_mismatch_index=$i"
                echo "oracle_u32=$oracle_word"
                echo "rust_u32=$rust_word"
                return 1
            fi
        done

        if [ "$oracle_words_len" -ne "$rust_words_len" ]; then
            echo "status=length-mismatch"
            echo "oracle_u32_len=$oracle_words_len"
            echo "rust_u32_len=$rust_words_len"
            return 1
        fi
    fi

    echo "status=exact"
    if [ -n "$oracle_bits_csv" ]; then
        echo "element_count=${#oracle_bits[@]}"
    elif [ -n "$oracle_words_csv" ]; then
        echo "element_count=${#oracle_words[@]}"
    fi
    return 0
}

run_pair_diff() {
    local pair="$1"
    shift
    resolve_pair "$pair"
    mkdir -p "$ARTIFACT_DIR"
    local oracle_file="$ARTIFACT_DIR/${pair}_oracle.txt"
    local rust_file="$ARTIFACT_DIR/${pair}_rust.txt"
    local rust_status

    "$0" "$PAIR_ORACLE_CASE" --device gpu --dump-all-f32-bits "$@" >"$oracle_file"

    set +e
    if [ "${#PAIR_RUST_EXTRA_ARGS[@]}" -gt 0 ]; then
        "$0" rust-run "$PAIR_RUST_BIN" "${PAIR_RUST_EXTRA_ARGS[@]}" --dump-all-f32-bits "$@" >"$rust_file" 2>&1
    else
        "$0" rust-run "$PAIR_RUST_BIN" --dump-all-f32-bits "$@" >"$rust_file" 2>&1
    fi
    rust_status=$?
    set -e

    echo "pair=$pair"
    echo "oracle_output=$oracle_file"
    echo "rust_output=$rust_file"
    local oracle_fnv rust_fnv oracle_first16 rust_first16
    oracle_fnv="$(extract_fnv_line "$oracle_file")"
    rust_fnv="$(extract_fnv_line "$rust_file")"
    oracle_first16="$(extract_first16_line "$oracle_file")"
    rust_first16="$(extract_first16_line "$rust_file")"
    if [ -n "$oracle_fnv" ]; then
        echo "oracle_${oracle_fnv}"
    fi
    if [ -n "$rust_fnv" ]; then
        echo "rust_${rust_fnv}"
    fi
    if [ -n "$oracle_first16" ]; then
        echo "oracle_${oracle_first16}"
    fi
    if [ -n "$rust_first16" ]; then
        echo "rust_${rust_first16}"
    fi
    echo "rust_exit_code=$rust_status"
    compare_dump_files "$oracle_file" "$rust_file"
}

run_pair_bench() {
    local pair="$1"
    shift
    resolve_pair "$pair"
    echo "pair=$pair"
    echo "mlx_oracle_subcommand=$PAIR_ORACLE_BENCH"
    "$0" "$PAIR_ORACLE_BENCH" --device gpu "$@"
    echo
    echo "rust_bin=$PAIR_RUST_BIN"
    if [ "${#PAIR_RUST_EXTRA_ARGS[@]}" -gt 0 ]; then
        "$0" rust-run "$PAIR_RUST_BIN" "${PAIR_RUST_EXTRA_ARGS[@]}" "$@"
    else
        "$0" rust-run "$PAIR_RUST_BIN" "$@"
    fi
}

SUBCOMMAND="${1:-}"

case "$SUBCOMMAND" in
    configure)
        configure_oracle
        ;;

    build)
        if [ "$#" -lt 2 ]; then
            usage >&2
            exit 1
        fi
        build_target "$2"
        ;;

    run)
        if [ "$#" -lt 2 ]; then
            usage >&2
            exit 1
        fi
        target="$2"
        shift 2
        run_target "$target" "$@"
        ;;

    affine-dequantize-row)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target affine_dequantize_row "$model_path" "$@"
        ;;

    quantized-matmul-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target quantized_matmul_case "$model_path" "$@"
        ;;

    quantized-matmul-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target quantized_matmul_bench "$model_path" "$@"
        ;;

    rms-norm-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_case "$model_path" "$@"
        ;;

    rms-norm-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_bench "$model_path" "$@"
        ;;

    rms-norm-qproj-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_qproj_case "$model_path" "$@"
        ;;

    rms-norm-qproj-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_qproj_bench "$model_path" "$@"
        ;;

    rms-norm-qkv-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_qkv_bench "$model_path" "$@"
        ;;

    rms-norm-qproj-qnorm-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_qproj_qnorm_case "$model_path" "$@"
        ;;

    rms-norm-qproj-qnorm-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_qproj_qnorm_bench "$model_path" "$@"
        ;;

    rms-norm-qproj-qnorm-rope-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_qproj_qnorm_rope_case "$model_path" "$@"
        ;;

    rms-norm-qproj-qnorm-rope-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target rms_norm_qproj_qnorm_rope_bench "$model_path" "$@"
        ;;

    qk-attention-logits-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target qk_attention_logits_case "$model_path" "$@"
        ;;

    qk-attention-logits-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target qk_attention_logits_bench "$model_path" "$@"
        ;;

    qkv-attention-output-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target qkv_attention_output_case "$model_path" "$@"
        ;;

    qkv-attention-output-cached-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target qkv_attention_output_cached_case "$model_path" "$@"
        ;;

    qkv-attention-output-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target qkv_attention_output_bench "$model_path" "$@"
        ;;

    attention-oproj-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_oproj_case "$model_path" "$@"
        ;;

    attention-oproj-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_oproj_bench "$model_path" "$@"
        ;;

    attention-post-attn-norm-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_norm_case "$model_path" "$@"
        ;;

    attention-post-attn-norm-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_norm_bench "$model_path" "$@"
        ;;

    attention-post-attn-residual-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" "$@"
        ;;

    attention-post-attn-residual-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" "$@"
        ;;

    attention-router-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --router "$@"
        ;;

    attention-router-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --router "$@"
        ;;

    attention-moe-expert-gate-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --moe-expert-gate "$@"
        ;;

    attention-moe-expert-gate-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --moe-expert-gate "$@"
        ;;

    attention-moe-expert-up-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --moe-expert-up "$@"
        ;;

    attention-moe-expert-up-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --moe-expert-up "$@"
        ;;

    attention-moe-expert-geglu-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --moe-expert-geglu "$@"
        ;;

    attention-moe-expert-geglu-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --moe-expert-geglu "$@"
        ;;

    attention-moe-expert-down-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --moe-expert-down "$@"
        ;;

    attention-moe-expert-down-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --moe-expert-down "$@"
        ;;

    attention-moe-experts-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --moe-experts "$@"
        ;;

    attention-moe-experts-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --moe-experts "$@"
        ;;

    attention-moe-post-ffn-norm2-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --moe-post-ffn-norm2 "$@"
        ;;

    attention-moe-post-ffn-norm2-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --moe-post-ffn-norm2 "$@"
        ;;

    attention-moe-merge-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_case "$model_path" --moe-merge "$@"
        ;;

    attention-moe-merge-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_post_attention_residual_bench "$model_path" --moe-merge "$@"
        ;;

    attention-pre-ffn-norm-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_norm_case "$model_path" "$@"
        ;;

    attention-pre-ffn-norm-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_norm_bench "$model_path" "$@"
        ;;

    attention-pre-ffn-gate-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_gate_case "$model_path" "$@"
        ;;

    attention-pre-ffn-gate-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_gate_bench "$model_path" "$@"
        ;;

    attention-pre-ffn-up-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_up_case "$model_path" "$@"
        ;;

    attention-pre-ffn-up-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_up_bench "$model_path" "$@"
        ;;

    attention-pre-ffn-geglu-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_geglu_case "$model_path" "$@"
        ;;

    attention-pre-ffn-geglu-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_geglu_bench "$model_path" "$@"
        ;;

    attention-pre-ffn-down-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_case "$model_path" "$@"
        ;;

    attention-pre-ffn-down-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_bench "$model_path" "$@"
        ;;

    attention-post-ffn-norm-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_case "$model_path" --post-feedforward-norm "$@"
        ;;

    attention-post-ffn-norm-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_bench "$model_path" --post-feedforward-norm "$@"
        ;;

    attention-moe-post-ffn-norm1-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_case "$model_path" --moe-post-ffn-norm1 "$@"
        ;;

    attention-moe-post-ffn-norm1-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_bench "$model_path" --moe-post-ffn-norm1 "$@"
        ;;

    attention-post-ffn-residual-case)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_case "$model_path" --final-residual "$@"
        ;;

    attention-post-ffn-residual-bench)
        shift || true
        if [ "$#" -gt 0 ] && [[ "$1" != --* ]]; then
            model_path="$1"
            shift
        else
            model_path="$DEFAULT_MODEL_PATH"
        fi
        run_target attention_pre_feedforward_down_bench "$model_path" --final-residual "$@"
        ;;

    rust-run)
        if [ "$#" -lt 2 ]; then
            usage >&2
            exit 1
        fi
        bin="$2"
        shift 2
        run_rust_bin "$bin" "$@"
        ;;

    pair-diff)
        if [ "$#" -lt 2 ]; then
            usage >&2
            exit 1
        fi
        pair="$2"
        shift 2
        run_pair_diff "$pair" "$@"
        ;;

    pair-bench)
        if [ "$#" -lt 2 ]; then
            usage >&2
            exit 1
        fi
        pair="$2"
        shift 2
        run_pair_bench "$pair" "$@"
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
