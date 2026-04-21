#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

WORKFLOW="${FLUX_WORKFLOW:-examples/comfyui/flux_dev_full_text_to_image.json}"
MODEL_ROOT="${FLUX_MODEL_ROOT:-local/diffusion_models}"
WIDTH="${FLUX_WIDTH:-256}"
HEIGHT="${FLUX_HEIGHT:-}"
STEPS="${FLUX_STEPS:-20}"

DUMP_DIR="${FLUX_DUMP_DIR:-/tmp/flux_cond_ref}"
STEP_DIR="${FLUX_STEP_DIR:-/tmp/flux_step_ref}"
REF_OUTPUT="${FLUX_REF_OUTPUT:-/tmp/sdcpp_flux_cond_dump.png}"
NATIVE_OUTPUT="${FLUX_NATIVE_OUTPUT:-/tmp/flux_native_oracle.png}"
HANDOFF_OUTPUT="${FLUX_HANDOFF_OUTPUT:-/tmp/flux_handoff_oracle.png}"
WARM_OUTPUT="${FLUX_WARM_OUTPUT:-}"

DIFFUSION_MODEL="${FLUX_REF_DIFFUSION_MODEL:-}"
VAE_MODEL="${FLUX_REF_VAE_MODEL:-}"
CLIP_L_MODEL="${FLUX_REF_CLIP_L_MODEL:-}"
T5XXL_MODEL="${FLUX_REF_T5XXL_MODEL:-}"

REF_PROMPT="${FLUX_REF_PROMPT:-}"
REF_SEED="${FLUX_REF_SEED:-}"
REF_CFG_SCALE="${FLUX_REF_CFG_SCALE:-}"
DUMP_STEP_INDEX="${FLUX_DUMP_STEP_INDEX:-1}"
COND_DIR="${FLUX_COND_DIR:-}"
COMPARE_COND_DIR="${FLUX_COMPARE_COND_DIR:-}"
T5_DEBUG_DIR="${FLUX_T5_DEBUG_DIR:-}"
T5_STAGE_LAYER="${FLUX_T5_DEBUG_LAYER:-}"
T5_MODE="${FLUX_T5_MODE:-lazy}"
WARMUP_RUNS="${FLUX_WARMUP_RUNS:-1}"
MEASURED_RUNS="${FLUX_MEASURED_RUNS:-1}"
LHS_DIR="${FLUX_LHS_DIR:-}"
RHS_DIR="${FLUX_RHS_DIR:-}"
T5_CPU_MATH="${FLUX_T5_FORCE_CPU_MATH:-0}"
T5_CPU_ATTN="${FLUX_T5_FORCE_CPU_ATTN:-0}"
T5_F32_LINEAR="${FLUX_T5_FORCE_F32_LINEAR:-0}"

CARGO_MANIFEST="libs/diffusion/Cargo.toml"
SDCPP_DIR="local/stable-diffusion.cpp"
SDCPP_BUILD_DIR="$SDCPP_DIR/build"
SDCLI_BIN="$SDCPP_BUILD_DIR/bin/sd-cli"
REF_WARM_BENCH_SRC="tools/flux_ref_warm_bench.cpp"
REF_WARM_BENCH_BIN="$SDCPP_BUILD_DIR/bin/flux-ref-warm-bench"
CMAKE_BIN="${CMAKE_BIN:-/Applications/CMake.app/Contents/bin/cmake}"

finalize_args() {
    if [[ -z "$HEIGHT" ]]; then
        HEIGHT="$WIDTH"
    fi
    if [[ -z "$DIFFUSION_MODEL" ]]; then
        DIFFUSION_MODEL="$MODEL_ROOT/unet/flux1-dev.safetensors"
    fi
    if [[ -z "$VAE_MODEL" ]]; then
        VAE_MODEL="$MODEL_ROOT/vae/ae.safetensors"
    fi
    if [[ -z "$CLIP_L_MODEL" ]]; then
        CLIP_L_MODEL="$MODEL_ROOT/text_encoders/clip_l.safetensors"
    fi
    if [[ -z "$T5XXL_MODEL" ]]; then
        T5XXL_MODEL="$MODEL_ROOT/text_encoders/t5xxl_fp16.safetensors"
    fi
}

usage() {
    cat <<'EOF'
usage: tools/flux_debug.sh <command> [options]

commands:
  check
  ref-build
  ref-generate
  ref-warm-bench
  text-smoke
  t5-smoke
  t5-stage-compare
  t5-debug-compare
  transformer-smoke
  native-generate
  warm-bench
  ref-dump
  ref-step-dump
  handoff-generate
  transformer-ref-compare
  transformer-ref-stage-compare
  transformer-ref-compare-f32
  oracle

options:
  --workflow PATH          comfy workflow json
  --model-root PATH        model root with unet/, vae/, text_encoders/
  --width N                output width
  --height N               output height
  --steps N                denoise step count
  --dump-dir PATH          reference conditioning dump directory
  --step-dir PATH          reference step dump directory
  --dump-step-index N      reference step index to dump
  --ref-output PATH        output image path for reference sd-cli run
  --native-output PATH     output image path for Rust native generation
  --warm-output PATH       output image path for the final measured warm bench run
  --handoff-output PATH    output image path for Rust handoff generation
  --warmup-runs N          in-process warmup run count for warm-bench
  --measured-runs N        in-process measured run count for warm-bench
  --cond-dir PATH          conditioning override directory for Rust runs
  --compare-cond-dir PATH  reference conditioning directory to diff against
  --t5-debug-dir PATH      dump per-block T5 hidden states to this directory
  --t5-stage-layer N       dump stage tensors for this T5 layer when stage debugging is enabled
  --t5-mode MODE           T5 backend: lazy or compiled
  --t5-cpu-math            force the lazy Rust T5 path to run with CPU math for debugging
  --t5-cpu-attn            force the lazy Rust T5 attention kernels onto the CPU
  --t5-f32-linear          decode T5 linear weights to F32 before lazy Metal matmuls
  --lhs-dir PATH           left-hand debug directory for compare commands
  --rhs-dir PATH           right-hand debug directory for compare commands
  --ref-prompt TEXT        override prompt used for reference sd-cli runs
  --ref-seed N             override seed used for reference sd-cli runs
  --ref-cfg-scale N        override cfg scale used for reference sd-cli runs

examples:
  tools/flux_debug.sh oracle
  tools/flux_debug.sh transformer-smoke --width 384 --height 384
  tools/flux_debug.sh ref-dump --ref-prompt test
  tools/flux_debug.sh ref-step-dump --dump-step-index 2
  tools/flux_debug.sh native-generate --compare-cond-dir /tmp/flux_cond_ref
EOF
}

log() {
    printf '[flux-debug] %s\n' "$*"
}

run() {
    printf '+'
    for arg in "$@"; do
        printf ' %q' "$arg"
    done
    printf '\n'
    "$@"
}

require_file() {
    local path="$1"
    if [[ ! -f "$path" ]]; then
        printf 'missing file: %s\n' "$path" >&2
        exit 1
    fi
}

require_dir() {
    local path="$1"
    if [[ ! -d "$path" ]]; then
        printf 'missing directory: %s\n' "$path" >&2
        exit 1
    fi
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --workflow)
                WORKFLOW="$2"
                shift 2
                ;;
            --model-root)
                MODEL_ROOT="$2"
                shift 2
                ;;
            --width)
                WIDTH="$2"
                shift 2
                ;;
            --height)
                HEIGHT="$2"
                shift 2
                ;;
            --steps)
                STEPS="$2"
                shift 2
                ;;
            --dump-dir)
                DUMP_DIR="$2"
                shift 2
                ;;
            --step-dir|--ref-step-dir)
                STEP_DIR="$2"
                shift 2
                ;;
            --dump-step-index)
                DUMP_STEP_INDEX="$2"
                shift 2
                ;;
            --ref-output)
                REF_OUTPUT="$2"
                shift 2
                ;;
            --native-output)
                NATIVE_OUTPUT="$2"
                shift 2
                ;;
            --warm-output)
                WARM_OUTPUT="$2"
                shift 2
                ;;
            --handoff-output)
                HANDOFF_OUTPUT="$2"
                shift 2
                ;;
            --warmup-runs)
                WARMUP_RUNS="$2"
                shift 2
                ;;
            --measured-runs)
                MEASURED_RUNS="$2"
                shift 2
                ;;
            --cond-dir)
                COND_DIR="$2"
                shift 2
                ;;
            --compare-cond-dir)
                COMPARE_COND_DIR="$2"
                shift 2
                ;;
            --t5-debug-dir)
                T5_DEBUG_DIR="$2"
                shift 2
                ;;
            --t5-stage-layer)
                T5_STAGE_LAYER="$2"
                shift 2
                ;;
            --t5-mode)
                T5_MODE="$2"
                shift 2
                ;;
            --t5-cpu-math)
                T5_CPU_MATH=1
                shift
                ;;
            --t5-cpu-attn)
                T5_CPU_ATTN=1
                shift
                ;;
            --t5-f32-linear)
                T5_F32_LINEAR=1
                shift
                ;;
            --lhs-dir)
                LHS_DIR="$2"
                shift 2
                ;;
            --rhs-dir)
                RHS_DIR="$2"
                shift 2
                ;;
            --ref-prompt)
                REF_PROMPT="$2"
                shift 2
                ;;
            --ref-seed)
                REF_SEED="$2"
                shift 2
                ;;
            --ref-cfg-scale)
                REF_CFG_SCALE="$2"
                shift 2
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            --)
                shift
                break
                ;;
            *)
                printf 'unknown option: %s\n\n' "$1" >&2
                usage >&2
                exit 1
                ;;
        esac
    done
    if [[ $# -gt 0 ]]; then
        printf 'unexpected extra arguments: %s\n\n' "$*" >&2
        usage >&2
        exit 1
    fi
    finalize_args
}

resolve_ref_prompt() {
    if [[ -n "$REF_PROMPT" ]]; then
        printf '%s\n' "$REF_PROMPT"
        return
    fi
    cargo run --release --quiet --manifest-path "$CARGO_MANIFEST" --bin flux-tokenize -- "$WORKFLOW" \
        | sed -n 's/^prompt\.t5xxl: //p' \
        | head -n 1
}

resolve_ref_seed() {
    if [[ -n "$REF_SEED" ]]; then
        printf '%s\n' "$REF_SEED"
        return
    fi
    sed -n 's/.*"seed"[[:space:]]*:[[:space:]]*\([-0-9][0-9]*\).*/\1/p' "$WORKFLOW" | head -n 1
}

resolve_ref_cfg_scale() {
    if [[ -n "$REF_CFG_SCALE" ]]; then
        printf '%s\n' "$REF_CFG_SCALE"
        return
    fi
    sed -n 's/.*"cfg"[[:space:]]*:[[:space:]]*\([-0-9.][0-9.]*\).*/\1/p' "$WORKFLOW" | head -n 1
}

ensure_common_inputs() {
    require_file "$WORKFLOW"
    require_dir "$MODEL_ROOT"
}

ensure_reference_inputs() {
    ensure_common_inputs
    require_file "$DIFFUSION_MODEL"
    require_file "$VAE_MODEL"
    require_file "$CLIP_L_MODEL"
    require_file "$T5XXL_MODEL"
}

ensure_ref_build() {
    require_file "$CMAKE_BIN"
    if [[ ! -f "$SDCPP_BUILD_DIR/CMakeCache.txt" ]]; then
        run "$CMAKE_BIN" -S "$SDCPP_DIR" -B "$SDCPP_BUILD_DIR" -DCMAKE_BUILD_TYPE=Release
    fi
    run "$CMAKE_BIN" --build "$SDCPP_BUILD_DIR" --config Release --target sd-cli -j4
    require_file "$SDCLI_BIN"
}

ensure_ref_warm_bench_bin() {
    ensure_ref_build
    require_file "$REF_WARM_BENCH_SRC"
    if [[ ! -x "$REF_WARM_BENCH_BIN" || "$REF_WARM_BENCH_SRC" -nt "$REF_WARM_BENCH_BIN" || "$SDCPP_BUILD_DIR/libstable-diffusion.a" -nt "$REF_WARM_BENCH_BIN" ]]; then
        local -a cmd=(
            /usr/bin/c++
            -O3
            -DNDEBUG
            -std=c++17
            -arch arm64
            -I "$SDCPP_DIR/include"
            "$REF_WARM_BENCH_SRC"
            -o "$REF_WARM_BENCH_BIN"
            "$SDCPP_BUILD_DIR/libstable-diffusion.a"
            "$SDCPP_BUILD_DIR/thirdparty/libwebp/libwebp.a"
            "$SDCPP_BUILD_DIR/thirdparty/libwebp/libwebpmux.a"
            "$SDCPP_BUILD_DIR/thirdparty/libwebm/libwebm.a"
            "$SDCPP_BUILD_DIR/ggml/src/libggml.a"
            "$SDCPP_BUILD_DIR/ggml/src/libggml-cpu.a"
            "$SDCPP_BUILD_DIR/ggml/src/ggml-blas/libggml-blas.a"
            -framework Accelerate
            "$SDCPP_BUILD_DIR/ggml/src/ggml-metal/libggml-metal.a"
            "$SDCPP_BUILD_DIR/ggml/src/libggml-base.a"
            -lm
            -framework Foundation
            -framework Metal
            -framework MetalKit
            "$SDCPP_BUILD_DIR/thirdparty/libwebp/libwebp.a"
            "$SDCPP_BUILD_DIR/thirdparty/libwebp/libsharpyuv.a"
        )
        run "${cmd[@]}"
    fi
    require_file "$REF_WARM_BENCH_BIN"
}

cmd_check() {
    ensure_common_inputs
    run cargo check --manifest-path "$CARGO_MANIFEST"
}

cmd_text_smoke() {
    ensure_common_inputs
    if [[ -n "$T5_DEBUG_DIR" ]]; then
        mkdir -p "$T5_DEBUG_DIR"
    fi
    local -a env_args=()
    if [[ -n "$COND_DIR" ]]; then
        env_args+=("FLUX_COND_DIR=$COND_DIR")
    fi
    if [[ -n "$T5_DEBUG_DIR" ]]; then
        env_args+=("FLUX_T5_DEBUG_DIR=$T5_DEBUG_DIR")
    fi
    if [[ -n "$T5_STAGE_LAYER" ]]; then
        env_args+=("FLUX_T5_DEBUG_LAYER=$T5_STAGE_LAYER")
    fi
    if [[ -n "$T5_MODE" ]]; then
        env_args+=("FLUX_T5_MODE=$T5_MODE")
    fi
    if [[ "$T5_CPU_MATH" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_MATH=1")
    fi
    if [[ "$T5_CPU_ATTN" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_ATTN=1")
    fi
    if [[ "$T5_F32_LINEAR" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_F32_LINEAR=1")
    fi
    if [[ ${#env_args[@]} -gt 0 ]]; then
        run env "${env_args[@]}" cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-text-smoke -- "$WORKFLOW" "$MODEL_ROOT"
    else
        run cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-text-smoke -- "$WORKFLOW" "$MODEL_ROOT"
    fi
}

cmd_t5_smoke() {
    ensure_common_inputs
    local -a env_args=("FLUX_T5_MODE=$T5_MODE")
    if [[ "$T5_CPU_MATH" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_MATH=1")
    fi
    if [[ "$T5_CPU_ATTN" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_ATTN=1")
    fi
    if [[ "$T5_F32_LINEAR" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_F32_LINEAR=1")
    fi
    if [[ ${#env_args[@]} -gt 0 ]]; then
        run env "${env_args[@]}" cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-t5-smoke -- "$WORKFLOW" "$MODEL_ROOT"
    else
        run cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-t5-smoke -- "$WORKFLOW" "$MODEL_ROOT"
    fi
}

cmd_t5_stage_compare() {
    ensure_common_inputs
    local debug_dir="${T5_DEBUG_DIR:-/tmp/flux_t5_ggml_dbg}"
    local stage_layer="${T5_STAGE_LAYER:-0}"
    mkdir -p "$debug_dir"
    rm -f "$debug_dir"/*.bin "$debug_dir"/*.txt
    local -a env_args=(
        "FLUX_T5_MODE=$T5_MODE"
        "FLUX_T5_DEBUG_DIR=$debug_dir"
        "FLUX_T5_DEBUG_STAGES=1"
        "FLUX_T5_DEBUG_LAYER=$stage_layer"
    )
    if [[ "$T5_CPU_MATH" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_MATH=1")
    fi
    if [[ "$T5_CPU_ATTN" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_ATTN=1")
    fi
    if [[ "$T5_F32_LINEAR" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_F32_LINEAR=1")
    fi
    run env "${env_args[@]}" cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-text-smoke -- "$WORKFLOW" "$MODEL_ROOT"
    run cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-t5-stage-compare -- "$WORKFLOW" "$MODEL_ROOT" "$debug_dir" "$stage_layer"
}

cmd_t5_debug_compare() {
    local lhs_dir="${LHS_DIR:-}"
    local rhs_dir="${RHS_DIR:-$T5_DEBUG_DIR}"
    if [[ -z "$lhs_dir" || -z "$rhs_dir" ]]; then
        printf 't5-debug-compare needs --lhs-dir and --rhs-dir (or --t5-debug-dir for rhs)\n' >&2
        exit 1
    fi
    require_dir "$lhs_dir"
    require_dir "$rhs_dir"
    run cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux_t5_debug_compare -- "$lhs_dir" "$rhs_dir"
}

cmd_transformer_smoke() {
    ensure_common_inputs
    run cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-transformer-smoke -- "$WORKFLOW" "$MODEL_ROOT" "$WIDTH" "$HEIGHT"
}

cmd_native_generate() {
    ensure_common_inputs
    local -a env_args=()
    if [[ -n "$COND_DIR" ]]; then
        env_args+=("FLUX_COND_DIR=$COND_DIR")
    fi
    if [[ -n "$COMPARE_COND_DIR" ]]; then
        env_args+=("FLUX_COMPARE_COND_DIR=$COMPARE_COND_DIR")
    fi
    if [[ -n "$T5_MODE" ]]; then
        env_args+=("FLUX_T5_MODE=$T5_MODE")
    fi
    if [[ "$T5_CPU_MATH" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_MATH=1")
    fi
    if [[ "$T5_CPU_ATTN" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_CPU_ATTN=1")
    fi
    if [[ "$T5_F32_LINEAR" != "0" ]]; then
        env_args+=("FLUX_T5_FORCE_F32_LINEAR=1")
    fi
    if [[ ${#env_args[@]} -gt 0 ]]; then
        run env "${env_args[@]}" cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-generate -- "$WORKFLOW" "$MODEL_ROOT" "$NATIVE_OUTPUT" "$WIDTH" "$HEIGHT" "$STEPS"
    else
        run cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-generate -- "$WORKFLOW" "$MODEL_ROOT" "$NATIVE_OUTPUT" "$WIDTH" "$HEIGHT" "$STEPS"
    fi
    run file "$NATIVE_OUTPUT"
}

cmd_warm_bench() {
    ensure_common_inputs
    local -a cmd=(
        cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-warm-bench --
        "$WORKFLOW" "$MODEL_ROOT" "$WIDTH" "$HEIGHT" "$STEPS" "$WARMUP_RUNS" "$MEASURED_RUNS"
    )
    local -a env_args=()
    if [[ -n "$T5_MODE" ]]; then
        env_args+=("FLUX_T5_MODE=$T5_MODE")
    fi
    if [[ -n "$WARM_OUTPUT" ]]; then
        cmd+=("$WARM_OUTPUT")
    fi
    if [[ ${#env_args[@]} -gt 0 ]]; then
        run env "${env_args[@]}" "${cmd[@]}"
    else
        run "${cmd[@]}"
    fi
    if [[ -n "$WARM_OUTPUT" ]]; then
        run file "$WARM_OUTPUT"
    fi
}

cmd_ref_dump() {
    ensure_reference_inputs
    ensure_ref_build
    mkdir -p "$DUMP_DIR"
    if [[ -n "$T5_DEBUG_DIR" ]]; then
        mkdir -p "$T5_DEBUG_DIR"
    fi
    rm -f "$DUMP_DIR"/*.bin "$DUMP_DIR"/*.txt
    local prompt
    local seed
    local cfg_scale
    prompt="$(resolve_ref_prompt)"
    seed="$(resolve_ref_seed)"
    cfg_scale="$(resolve_ref_cfg_scale)"
    if [[ -z "$prompt" ]]; then
        printf 'could not resolve reference prompt from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$seed" ]]; then
        printf 'could not resolve reference seed from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$cfg_scale" ]]; then
        printf 'could not resolve reference cfg scale from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    log "reference prompt: $prompt"
    log "reference seed: $seed cfg_scale: $cfg_scale"
    if [[ -n "$T5_DEBUG_DIR" ]]; then
        run env FLUX_DUMP_TEXT_COND_DIR="$DUMP_DIR" FLUX_DUMP_T5_DEBUG_DIR="$T5_DEBUG_DIR" "$SDCLI_BIN" \
        --diffusion-model "$DIFFUSION_MODEL" \
        --vae "$VAE_MODEL" \
        --clip_l "$CLIP_L_MODEL" \
        --t5xxl "$T5XXL_MODEL" \
        -p "$prompt" \
        -s "$seed" \
        --cfg-scale "$cfg_scale" \
        -W "$WIDTH" \
        -H "$HEIGHT" \
        --steps 1 \
        -o "$REF_OUTPUT"
    else
        run env FLUX_DUMP_TEXT_COND_DIR="$DUMP_DIR" "$SDCLI_BIN" \
        --diffusion-model "$DIFFUSION_MODEL" \
        --vae "$VAE_MODEL" \
        --clip_l "$CLIP_L_MODEL" \
        --t5xxl "$T5XXL_MODEL" \
        -p "$prompt" \
        -s "$seed" \
        --cfg-scale "$cfg_scale" \
        -W "$WIDTH" \
        -H "$HEIGHT" \
        --steps 1 \
        -o "$REF_OUTPUT"
    fi
    require_file "$DUMP_DIR/flux_clip_pooled.bin"
    require_file "$DUMP_DIR/flux_t5_hidden.bin"
    require_file "$DUMP_DIR/flux_t5_meta.txt"
    run file "$REF_OUTPUT"
    log "conditioning dump: $DUMP_DIR"
}

cmd_ref_generate() {
    ensure_reference_inputs
    ensure_ref_build
    local prompt
    local seed
    local cfg_scale
    prompt="$(resolve_ref_prompt)"
    seed="$(resolve_ref_seed)"
    cfg_scale="$(resolve_ref_cfg_scale)"
    if [[ -z "$prompt" ]]; then
        printf 'could not resolve reference prompt from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$seed" ]]; then
        printf 'could not resolve reference seed from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$cfg_scale" ]]; then
        printf 'could not resolve reference cfg scale from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    log "reference prompt: $prompt"
    log "reference seed: $seed cfg_scale: $cfg_scale"
    run "$SDCLI_BIN" \
        --diffusion-model "$DIFFUSION_MODEL" \
        --vae "$VAE_MODEL" \
        --clip_l "$CLIP_L_MODEL" \
        --t5xxl "$T5XXL_MODEL" \
        -p "$prompt" \
        -s "$seed" \
        --cfg-scale "$cfg_scale" \
        -W "$WIDTH" \
        -H "$HEIGHT" \
        --steps "$STEPS" \
        -o "$REF_OUTPUT"
    run file "$REF_OUTPUT"
}

cmd_ref_warm_bench() {
    ensure_reference_inputs
    ensure_ref_warm_bench_bin
    local prompt
    local seed
    local cfg_scale
    prompt="$(resolve_ref_prompt)"
    seed="$(resolve_ref_seed)"
    cfg_scale="$(resolve_ref_cfg_scale)"
    if [[ -z "$prompt" ]]; then
        printf 'could not resolve reference prompt from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$seed" ]]; then
        printf 'could not resolve reference seed from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$cfg_scale" ]]; then
        printf 'could not resolve reference cfg scale from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    log "reference prompt: $prompt"
    log "reference seed: $seed cfg_scale: $cfg_scale"
    run "$REF_WARM_BENCH_BIN" \
        --diffusion-model "$DIFFUSION_MODEL" \
        --vae "$VAE_MODEL" \
        --clip_l "$CLIP_L_MODEL" \
        --t5xxl "$T5XXL_MODEL" \
        --prompt "$prompt" \
        --seed "$seed" \
        --cfg-scale "$cfg_scale" \
        --width "$WIDTH" \
        --height "$HEIGHT" \
        --steps "$STEPS" \
        --warmup-runs "$WARMUP_RUNS" \
        --measured-runs "$MEASURED_RUNS"
}

cmd_ref_step_dump() {
    ensure_reference_inputs
    ensure_ref_build
    mkdir -p "$DUMP_DIR" "$STEP_DIR"
    rm -f "$DUMP_DIR"/*.bin "$DUMP_DIR"/*.txt "$STEP_DIR"/*.bin "$STEP_DIR"/*.txt
    local prompt
    local seed
    local cfg_scale
    local dump_step_index
    prompt="$(resolve_ref_prompt)"
    seed="$(resolve_ref_seed)"
    cfg_scale="$(resolve_ref_cfg_scale)"
    dump_step_index="$DUMP_STEP_INDEX"
    if [[ -z "$prompt" ]]; then
        printf 'could not resolve reference prompt from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$seed" ]]; then
        printf 'could not resolve reference seed from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    if [[ -z "$cfg_scale" ]]; then
        printf 'could not resolve reference cfg scale from workflow: %s\n' "$WORKFLOW" >&2
        exit 1
    fi
    log "reference prompt: $prompt"
    log "reference seed: $seed cfg_scale: $cfg_scale"
    run env FLUX_DUMP_TEXT_COND_DIR="$DUMP_DIR" FLUX_DUMP_STEP_DIR="$STEP_DIR" "$SDCLI_BIN" \
        --diffusion-model "$DIFFUSION_MODEL" \
        --vae "$VAE_MODEL" \
        --clip_l "$CLIP_L_MODEL" \
        --t5xxl "$T5XXL_MODEL" \
        -p "$prompt" \
        -s "$seed" \
        --cfg-scale "$cfg_scale" \
        -W "$WIDTH" \
        -H "$HEIGHT" \
        --steps "$dump_step_index" \
        -o "$REF_OUTPUT"
    require_file "$DUMP_DIR/flux_clip_pooled.bin"
    require_file "$DUMP_DIR/flux_t5_hidden.bin"
    require_file "$DUMP_DIR/flux_t5_meta.txt"
    require_file "$STEP_DIR/flux_noised_input.bin"
    require_file "$STEP_DIR/flux_cond_out.bin"
    require_file "$STEP_DIR/flux_step_meta.txt"
    run file "$REF_OUTPUT"
    log "conditioning dump: $DUMP_DIR"
    log "step dump: $STEP_DIR"
}

cmd_handoff_generate() {
    ensure_common_inputs
    local cond_dir="${COND_DIR:-$DUMP_DIR}"
    require_dir "$cond_dir"
    require_file "$cond_dir/flux_clip_pooled.bin"
    require_file "$cond_dir/flux_t5_hidden.bin"
    require_file "$cond_dir/flux_t5_meta.txt"
    run env FLUX_COND_DIR="$cond_dir" cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-generate -- "$WORKFLOW" "$MODEL_ROOT" "$HANDOFF_OUTPUT" "$WIDTH" "$HEIGHT" "$STEPS"
    run file "$HANDOFF_OUTPUT"
}

cmd_transformer_ref_compare() {
    ensure_common_inputs
    local cond_dir="${COND_DIR:-$DUMP_DIR}"
    require_dir "$cond_dir"
    require_dir "$STEP_DIR"
    require_file "$cond_dir/flux_clip_pooled.bin"
    require_file "$cond_dir/flux_t5_hidden.bin"
    require_file "$cond_dir/flux_t5_meta.txt"
    require_file "$STEP_DIR/flux_noised_input.bin"
    require_file "$STEP_DIR/flux_cond_out.bin"
    require_file "$STEP_DIR/flux_step_meta.txt"
    run env FLUX_COND_DIR="$cond_dir" FLUX_REF_STEP_DIR="$STEP_DIR" cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-transformer-smoke -- "$WORKFLOW" "$MODEL_ROOT" "$WIDTH" "$HEIGHT"
}

cmd_transformer_ref_stage_compare() {
    ensure_common_inputs
    local cond_dir="${COND_DIR:-$DUMP_DIR}"
    require_dir "$cond_dir"
    require_dir "$STEP_DIR"
    require_file "$cond_dir/flux_clip_pooled.bin"
    require_file "$cond_dir/flux_t5_hidden.bin"
    require_file "$cond_dir/flux_t5_meta.txt"
    require_file "$STEP_DIR/flux_noised_input.bin"
    require_file "$STEP_DIR/flux_cond_out.bin"
    require_file "$STEP_DIR/flux_step_meta.txt"
    run env FLUX_COND_DIR="$cond_dir" FLUX_REF_STEP_DIR="$STEP_DIR" FLUX_DEBUG_TRANSFORMER_STAGES=1 cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-transformer-smoke -- "$WORKFLOW" "$MODEL_ROOT" "$WIDTH" "$HEIGHT"
}

cmd_transformer_ref_compare_f32() {
    ensure_common_inputs
    local cond_dir="${COND_DIR:-$DUMP_DIR}"
    require_dir "$cond_dir"
    require_dir "$STEP_DIR"
    require_file "$cond_dir/flux_clip_pooled.bin"
    require_file "$cond_dir/flux_t5_hidden.bin"
    require_file "$cond_dir/flux_t5_meta.txt"
    require_file "$STEP_DIR/flux_noised_input.bin"
    require_file "$STEP_DIR/flux_cond_out.bin"
    require_file "$STEP_DIR/flux_step_meta.txt"
    run env FLUX_COND_DIR="$cond_dir" FLUX_REF_STEP_DIR="$STEP_DIR" FLUX_FORCE_F32_WEIGHTS=1 cargo run --release --manifest-path "$CARGO_MANIFEST" --bin flux-transformer-smoke -- "$WORKFLOW" "$MODEL_ROOT" "$WIDTH" "$HEIGHT"
}

cmd_oracle() {
    cmd_text_smoke
    cmd_t5_smoke
    cmd_ref_step_dump
    cmd_transformer_ref_compare
    cmd_native_generate
    cmd_handoff_generate
    log "reference image: $REF_OUTPUT"
    log "native image: $NATIVE_OUTPUT"
    log "handoff image: $HANDOFF_OUTPUT"
    log "conditioning dump: $DUMP_DIR"
    log "step dump: $STEP_DIR"
}

main() {
    local command="${1:-}"
    shift || true
    parse_args "$@"
    case "$command" in
        check) cmd_check ;;
        ref-build) ensure_ref_build ;;
        ref-generate) cmd_ref_generate ;;
        ref-warm-bench) cmd_ref_warm_bench ;;
        text-smoke) cmd_text_smoke ;;
        t5-smoke) cmd_t5_smoke ;;
        t5-stage-compare) cmd_t5_stage_compare ;;
        t5-debug-compare) cmd_t5_debug_compare ;;
        transformer-smoke) cmd_transformer_smoke ;;
        native-generate) cmd_native_generate ;;
        warm-bench) cmd_warm_bench ;;
        ref-dump) cmd_ref_dump ;;
        ref-step-dump) cmd_ref_step_dump ;;
        handoff-generate) cmd_handoff_generate ;;
        transformer-ref-compare) cmd_transformer_ref_compare ;;
        transformer-ref-stage-compare) cmd_transformer_ref_stage_compare ;;
        transformer-ref-compare-f32) cmd_transformer_ref_compare_f32 ;;
        oracle) cmd_oracle ;;
        -h|--help|help|"") usage ;;
        *)
            printf 'unknown command: %s\n\n' "$command" >&2
            usage >&2
            exit 1
            ;;
    esac
}

main "$@"
