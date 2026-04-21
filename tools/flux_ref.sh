#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
    cat <<'EOF'
usage: tools/flux_ref.sh <command> [options]

commands:
  build
  generate
  warm-bench
  dump
  step-dump

This is the reference stable-diffusion.cpp-only wrapper around tools/flux_debug.sh.
Pass options such as `--workflow`, `--model-root`, `--width`, `--height`,
`--steps`, `--dump-dir`, `--step-dir`, `--dump-step-index`, `--ref-output`,
`--ref-prompt`, `--ref-seed`, `--ref-cfg-scale` after the command.
EOF
}

main() {
    local command="${1:-}"
    case "$command" in
        build)
            shift
            exec tools/flux_debug.sh ref-build "$@"
            ;;
        generate)
            shift
            exec tools/flux_debug.sh ref-generate "$@"
            ;;
        warm-bench)
            shift
            exec tools/flux_debug.sh ref-warm-bench "$@"
            ;;
        dump)
            shift
            exec tools/flux_debug.sh ref-dump "$@"
            ;;
        step-dump)
            shift
            exec tools/flux_debug.sh ref-step-dump "$@"
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
