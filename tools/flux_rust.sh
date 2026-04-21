#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
    cat <<'EOF'
usage: tools/flux_rust.sh <command> [options]

commands:
  check
  text-smoke
  t5-smoke
  t5-stage-compare
  t5-debug-compare
  transformer-smoke
  transformer-ref-compare
  transformer-ref-stage-compare
  transformer-ref-compare-f32
  native-generate
  warm-bench
  handoff-generate

This is the Rust/ggml-only wrapper around tools/flux_debug.sh.
Pass options such as `--workflow`, `--model-root`, `--width`, `--height`,
`--steps`, `--compare-cond-dir`, `--cond-dir`, `--native-output`, `--t5-mode`
after the command.
EOF
}

main() {
    local command="${1:-}"
    case "$command" in
        check|text-smoke|t5-smoke|t5-stage-compare|t5-debug-compare|transformer-smoke|transformer-ref-compare|transformer-ref-stage-compare|transformer-ref-compare-f32|native-generate|warm-bench|handoff-generate)
            exec tools/flux_debug.sh "$@"
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
