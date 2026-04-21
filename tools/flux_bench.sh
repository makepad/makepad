#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

WORKFLOW="examples/comfyui/flux_dev_full_text_to_image.json"
MODEL_ROOT="local/diffusion_models"
WIDTH="256"
HEIGHT=""
STEPS="20"
NATIVE_OUTPUT="/tmp/flux_bench_native.png"
REF_OUTPUT="/tmp/flux_bench_ref.png"
NATIVE_RETRIES="5"
T5_MODE="lazy"
WARMUP_NATIVE="1"
WARMUP_REFERENCE="1"
MEASURED_RUNS="2"

usage() {
    cat <<'EOF'
usage: tools/flux_bench.sh <command> [options]

commands:
  native
  reference
  compare
  native-warm
  reference-warm
  compare-warm

options:
  --workflow PATH
  --model-root PATH
  --width N
  --height N
  --steps N
  --native-output PATH
  --ref-output PATH
  --native-retries N
  --t5-mode MODE
  --warmup-native N
  --warmup-reference N
  --measured-runs N

examples:
  tools/flux_bench.sh compare
  tools/flux_bench.sh compare-warm
  tools/flux_bench.sh native --width 512 --height 512 --steps 28 --t5-mode compiled
EOF
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
            --native-output)
                NATIVE_OUTPUT="$2"
                shift 2
                ;;
            --ref-output)
                REF_OUTPUT="$2"
                shift 2
                ;;
            --native-retries)
                NATIVE_RETRIES="$2"
                shift 2
                ;;
            --t5-mode)
                T5_MODE="$2"
                shift 2
                ;;
            --warmup-native)
                WARMUP_NATIVE="$2"
                shift 2
                ;;
            --warmup-reference)
                WARMUP_REFERENCE="$2"
                shift 2
                ;;
            --measured-runs)
                MEASURED_RUNS="$2"
                shift 2
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                printf 'unknown option: %s\n\n' "$1" >&2
                usage >&2
                exit 1
                ;;
        esac
    done
    if [[ -z "$HEIGHT" ]]; then
        HEIGHT="$WIDTH"
    fi
}

parse_real_seconds() {
    local log_path="$1"
    sanitize_log "$log_path" | sed -n 's/^real //p' | tail -n 1
}

parse_metric() {
    local log_path="$1"
    local key="$2"
    sanitize_log "$log_path" | sed -n "s/^${key}=//p" | tail -n 1
}

sanitize_log() {
    local log_path="$1"
    LC_ALL=C tr '\r' '\n' <"$log_path" | sed -E $'s/\x1B\\[[0-9;?]*[[:alpha:]]//g'
}

run_timed() {
    local log_path="$1"
    shift
    /usr/bin/time -p "$@" >"$log_path" 2>&1
}

run_native_once() {
    local log_path="$1"
    run_timed \
        "$log_path" \
        tools/flux_rust.sh \
        native-generate \
        --workflow "$WORKFLOW" \
        --model-root "$MODEL_ROOT" \
        --width "$WIDTH" \
        --height "$HEIGHT" \
        --steps "$STEPS" \
        --t5-mode "$T5_MODE" \
        --native-output "$NATIVE_OUTPUT"
}

run_ref_once() {
    local log_path="$1"
    run_timed \
        "$log_path" \
        tools/flux_ref.sh \
        generate \
        --workflow "$WORKFLOW" \
        --model-root "$MODEL_ROOT" \
        --width "$WIDTH" \
        --height "$HEIGHT" \
        --steps "$STEPS" \
        --ref-output "$REF_OUTPUT"
}

bench_native() {
    local log_path
    log_path="$(mktemp -t flux_bench_native)"
    local warmup_log
    warmup_log="$(mktemp -t flux_bench_native_warmup)"
    local warmup=1
    while (( warmup <= WARMUP_NATIVE )); do
        run_native_once "$warmup_log" >/dev/null 2>&1 || true
        warmup=$((warmup + 1))
    done
    local attempt=1
    while (( attempt <= NATIVE_RETRIES )); do
        if run_native_once "$log_path"; then
            local real
            real="$(parse_real_seconds "$log_path")"
            printf 'native.real=%s\n' "$real"
            printf 'native.t5_mode=%s\n' "$T5_MODE"
            printf 'native.model_total_ms=%s\n' "$(parse_metric "$log_path" "timing.total_ms")"
            printf 'native.text_execute_ms=%s\n' "$(parse_metric "$log_path" "timing.text_execute_ms")"
            printf 'native.transformer_compile_ms=%s\n' "$(parse_metric "$log_path" "timing.transformer_compile_ms")"
            printf 'native.denoise_ms=%s\n' "$(parse_metric "$log_path" "timing.denoise_ms")"
            printf 'native.vae_execute_ms=%s\n' "$(parse_metric "$log_path" "timing.vae_execute_ms")"
            printf 'native.output=%s\n' "$NATIVE_OUTPUT"
            return 0
        fi
        printf 'native.attempt_%d_failed\n' "$attempt" >&2
        tail -n 20 "$log_path" >&2 || true
        attempt=$((attempt + 1))
        sleep 1
    done
    return 1
}

bench_reference() {
    local log_path
    log_path="$(mktemp -t flux_bench_ref)"
    local warmup_log
    warmup_log="$(mktemp -t flux_bench_ref_warmup)"
    local warmup=1
    while (( warmup <= WARMUP_REFERENCE )); do
        run_ref_once "$warmup_log" >/dev/null 2>&1 || true
        warmup=$((warmup + 1))
    done
    run_ref_once "$log_path"
    local real
    real="$(parse_real_seconds "$log_path")"
    printf 'reference.real=%s\n' "$real"
    printf 'reference.output=%s\n' "$REF_OUTPUT"
}

bench_compare() {
    local native_report
    local native_real
    local ref_real
    native_report="$(bench_native)"
    native_real="$(printf '%s\n' "$native_report" | sed -n 's/^native.real=//p')"
    ref_real="$(bench_reference | sed -n 's/^reference.real=//p')"
    printf '%s\n' "$native_report"
    printf 'reference.real=%s\n' "$ref_real"
    python3 - "$native_real" "$ref_real" <<'PY'
import sys
native = float(sys.argv[1])
reference = float(sys.argv[2])
print(f"ratio.native_over_reference={native / reference:.4f}")
print(f"delta.seconds={native - reference:.2f}")
PY
}

bench_native_warm() {
    local log_path
    log_path="$(mktemp -t flux_bench_native_warm)"
    tools/flux_rust.sh \
        warm-bench \
        --workflow "$WORKFLOW" \
        --model-root "$MODEL_ROOT" \
        --width "$WIDTH" \
        --height "$HEIGHT" \
        --steps "$STEPS" \
        --t5-mode "$T5_MODE" \
        --warmup-runs "$WARMUP_NATIVE" \
        --measured-runs "$MEASURED_RUNS" >"$log_path" 2>&1
    printf 'native_warm.t5_mode=%s\n' "$T5_MODE"
    printf 'native_warm.load_total_ms=%s\n' "$(parse_metric "$log_path" "load.total_ms")"
    printf 'native_warm.mean_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.total_ms.mean")"
    printf 'native_warm.best_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.total_ms.best")"
    printf 'native_warm.worst_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.total_ms.worst")"
    printf 'native_warm.denoise_mean_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.denoise_ms.mean")"
    printf 'native_warm.vae_mean_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.vae_execute_ms.mean")"
}

bench_reference_warm() {
    local log_path
    log_path="$(mktemp -t flux_bench_reference_warm)"
    tools/flux_ref.sh \
        warm-bench \
        --workflow "$WORKFLOW" \
        --model-root "$MODEL_ROOT" \
        --width "$WIDTH" \
        --height "$HEIGHT" \
        --steps "$STEPS" \
        --warmup-runs "$WARMUP_REFERENCE" \
        --measured-runs "$MEASURED_RUNS" >"$log_path" 2>&1
    printf 'reference_warm.load_ctx_init_ms=%s\n' "$(parse_metric "$log_path" "load.ctx_init_ms")"
    printf 'reference_warm.mean_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.total_ms.mean")"
    printf 'reference_warm.best_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.total_ms.best")"
    printf 'reference_warm.worst_ms=%s\n' "$(parse_metric "$log_path" "measured.summary.total_ms.worst")"
}

bench_compare_warm() {
    local native_report
    local reference_report
    local native_mean
    local reference_mean
    native_report="$(bench_native_warm)"
    reference_report="$(bench_reference_warm)"
    native_mean="$(printf '%s\n' "$native_report" | sed -n 's/^native_warm.mean_ms=//p')"
    reference_mean="$(printf '%s\n' "$reference_report" | sed -n 's/^reference_warm.mean_ms=//p')"
    printf '%s\n' "$native_report"
    printf '%s\n' "$reference_report"
    python3 - "$native_mean" "$reference_mean" <<'PY'
import sys
native_ms = float(sys.argv[1])
reference_ms = float(sys.argv[2])
print(f"ratio.native_over_reference={native_ms / reference_ms:.4f}")
print(f"delta.ms={native_ms - reference_ms:.3f}")
PY
}

main() {
    local command="${1:-}"
    shift || true
    parse_args "$@"
    case "$command" in
        native)
            bench_native
            ;;
        reference)
            bench_reference
            ;;
        compare)
            bench_compare
            ;;
        native-warm)
            bench_native_warm
            ;;
        reference-warm)
            bench_reference_warm
            ;;
        compare-warm)
            bench_compare_warm
            ;;
        -h|--help|help|"")
            usage
            ;;
        *)
            printf 'unknown command: %s\n\n' "$command" >&2
            usage >&2
            exit 1
            ;;
    esac
}

main "$@"
