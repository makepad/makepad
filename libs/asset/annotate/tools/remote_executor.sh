#!/usr/bin/env bash
# Remote vision executor for the asset-annotation pass.
#
# The pass runner (libs/asset/annotate/src/bin/asset_annotate.rs) owns all
# policy and reaches inference through exactly one seam: an executor
# subprocess speaking the batch protocol documented at the top of
# libs/ai/llm/src/bin/vlm_annotate.rs
#
#     <executor> --jobs J --prompt-file P --out O
#     jobs TSV:    <id>\t<image.ppm>[\t<context>]
#     results TSV: <id>\t<ok|err>\t<escaped text>
#
# This implementation runs that executor on a fleet box over the makepad
# tunnel instead of on this machine: it pushes every sheet, rewrites the jobs
# file to box paths, runs vlm-annotate.exe there with the CUDA vision backend,
# and pulls the replies back. Nothing about the runner changes.
#
# Second mode: `--probe` answers "can this box do the work right now" with a
# single line and an exit code, so a caller (the asset-ui) can choose between
# the remote box and a local fallback without launching a batch.
#
# Failure is always loud. The runner treats a missing reply as "skipped", so a
# silent failure here would look like a successful no-op run: every step below
# either succeeds or exits non-zero with a reason on stderr.
#
# One box, one job at a time. A second tunnel connection opened while a `run`
# is in flight drops the first one with `client error: failed to fill whole
# buffer` — measured, and it kills a 70-second batch outright. So every mode
# takes a host-local lock named after the box, and a `--probe` that finds the
# lock held answers from the running batch's own preflight instead of opening
# a second connection.
#
# Environment (all optional; defaults are the fleet's vision box):
#   MAKEPAD_VLM_BOX             tunnel endpoint          10.0.0.165:8384
#   MAKEPAD_VLM_REMOTE_REPO     box repo root (push/pull paths are relative
#                               to it)                   C:\Users\playe\makepad
#   MAKEPAD_VLM_REMOTE_WORK     work dir under that root _vlm
#   MAKEPAD_VLM_REMOTE_EXE      vlm-annotate.exe
#   MAKEPAD_VLM_REMOTE_MODEL    text model gguf
#   MAKEPAD_VLM_REMOTE_MMPROJ   vision projector gguf
#   MAKEPAD_VLM_LOCK_WAIT       seconds a batch waits for the box lock   1800
#   MAKEPAD_VLM_PROBE_LOCK_WAIT seconds --probe waits for it             5
#   MAKEPAD_VLM_MAX_NEW_TOKENS  passed through when set (exe default 220)
#   MAKEPAD_VLM_MAX_CONTEXT     passed through when set (exe default 4096)
#   CARGO_MAKEPAD               tunnel client   <repo>/target/release/cargo-makepad
set -euo pipefail

SELF="${BASH_SOURCE[0]}"
while [[ -L "$SELF" ]]; do SELF="$(readlink "$SELF")"; done
TOOLS_DIR="$(cd "$(dirname "$SELF")" && pwd)"
# tools/ -> annotate/ -> asset/ -> libs/ -> repo root
REPO_ROOT="$(cd "$TOOLS_DIR/../../../.." && pwd)"

BOX="${MAKEPAD_VLM_BOX:-10.0.0.165:8384}"
REMOTE_REPO="${MAKEPAD_VLM_REMOTE_REPO:-C:\\Users\\playe\\makepad}"
REMOTE_WORK="${MAKEPAD_VLM_REMOTE_WORK:-_vlm}"
REMOTE_EXE="${MAKEPAD_VLM_REMOTE_EXE:-C:\\ai\\qwen38vis\\target\\release\\vlm-annotate.exe}"
REMOTE_MODEL="${MAKEPAD_VLM_REMOTE_MODEL:-C:\\ai\\qwen38mtp\\Qwen3.8-27B-Q4_K_M.gguf}"
REMOTE_MMPROJ="${MAKEPAD_VLM_REMOTE_MMPROJ:-C:\\ai\\qwen38vis\\models\\Qwen3.8-27B-mmproj-F16.gguf}"
CARGO_MAKEPAD="${CARGO_MAKEPAD:-$REPO_ROOT/target/release/cargo-makepad}"

# `_vlm` on the wire (push/pull want a relative, forward-slash path); the same
# thing as a Windows absolute path for the jobs file the model reads.
REMOTE_WORK_REL="${REMOTE_WORK//\\//}"
REMOTE_WORK_WIN="${REMOTE_WORK//\//\\}"
REMOTE_WORK_ABS="${REMOTE_REPO}\\${REMOTE_WORK_WIN}"

die() { echo "remote_executor: $*" >&2; exit 1; }

[[ -x "$CARGO_MAKEPAD" ]] || die "tunnel client not found at $CARGO_MAKEPAD (build it: cargo build --release -p cargo-makepad, or set CARGO_MAKEPAD)"

TMP="$(mktemp -d)"
LOCK="${TMPDIR:-/tmp}/makepad-vlm-${BOX//[^A-Za-z0-9]/_}.lock"
HELD=0
cleanup() { rm -rf "$TMP"; [[ $HELD -eq 1 ]] && rm -rf "$LOCK"; return 0; }
trap cleanup EXIT

# mkdir is the atomic primitive macOS gives us without flock. A lock whose
# owner is gone is stale and taken over; otherwise we wait.
take_lock() {
    local wait_s="$1" waited=0
    while ! mkdir "$LOCK" 2>/dev/null; do
        local owner
        owner="$(cat "$LOCK/pid" 2>/dev/null || true)"
        if [[ -n "$owner" ]] && ! kill -0 "$owner" 2>/dev/null; then
            rm -rf "$LOCK"
            continue
        fi
        if [[ $waited -ge $wait_s ]]; then return 1; fi
        sleep 1
        waited=$((waited + 1))
    done
    HELD=1
    echo $$ > "$LOCK/pid"
    return 0
}

# One place that knows how to run a script on the box. --no-sync is mandatory:
# a plain run uploads this machine's whole (dirty) checkout.
# Windows hands back CRLF; strip the CR so line anchors and greps behave.
remote_run() { "$CARGO_MAKEPAD" tunnel "$BOX" --no-sync run "$1" 2>&1 | tr -d '\r'; }
remote_push() { "$CARGO_MAKEPAD" tunnel "$BOX" push "$1" "$2" >/dev/null; }
remote_pull() { "$CARGO_MAKEPAD" tunnel "$BOX" pull "$1" "$2" >/dev/null; }

# Preflight, shared by --probe and a real batch: does the box have the three
# files, and what does its GPU look like right now. Exit 3 == box reachable
# but not equipped, which is a different problem from "box is down".
write_probe_script() {
    cat > "$TMP/probe.ps1" <<EOF
\$ok = \$true
foreach (\$p in @('$REMOTE_EXE','$REMOTE_MODEL','$REMOTE_MMPROJ')) {
    if (-not (Test-Path \$p)) { Write-Output "MISSING=\$p"; \$ok = \$false }
}
Write-Output "HOST=\$env:COMPUTERNAME"
\$g = (& nvidia-smi --query-gpu=name,memory.used,memory.total --format=csv,noheader) 2>\$null
if (\$LASTEXITCODE -ne 0 -or -not \$g) { Write-Output "GPU=none"; \$ok = \$false }
else { Write-Output ("GPU=" + (\$g -join '; ').Trim()) }
if (-not \$ok) { exit 3 }
Write-Output "PROBE_OK"
EOF
}

if [[ "${1:-}" == "--probe" ]]; then
    if ! take_lock "${MAKEPAD_VLM_PROBE_LOCK_WAIT:-5}"; then
        # A batch holds the box. Its preflight already proved the exe, model
        # and mmproj are there, and it left the answer behind — reuse it
        # rather than opening the connection that would kill the batch.
        if [[ -r "$LOCK/probe.line" ]]; then
            sed 's/$/ state=busy/' "$LOCK/probe.line"
            exit 0
        fi
        die "box $BOX is busy with another batch and left no probe answer"
    fi
    write_probe_script
    out="$(remote_run "$TMP/probe.ps1")" || {
        echo "$out" >&2
        die "probe failed on $BOX (box unreachable, tunnel daemon down, or files missing)"
    }
    grep -q '^PROBE_OK$' <<<"$out" || { echo "$out" >&2; die "probe did not complete on $BOX"; }
    host="$(sed -n 's/^HOST=//p' <<<"$out" | head -1)"
    gpu="$(sed -n 's/^GPU=//p' <<<"$out" | head -1)"
    echo "ok host=${host:-unknown}@$BOX exe=$REMOTE_EXE model=$REMOTE_MODEL mmproj=$REMOTE_MMPROJ gpu=$gpu"
    exit 0
fi

JOBS=""; PROMPT=""; OUT=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --jobs) JOBS="${2:-}"; shift 2 ;;
        --prompt-file) PROMPT="${2:-}"; shift 2 ;;
        --out) OUT="${2:-}"; shift 2 ;;
        --probe) shift ;;
        *) die "unknown argument $1 (expected --jobs J --prompt-file P --out O, or --probe)" ;;
    esac
done
[[ -n "$JOBS" && -n "$PROMPT" && -n "$OUT" ]] || die "missing --jobs/--prompt-file/--out"
[[ -r "$JOBS" ]] || die "jobs file not readable: $JOBS"
[[ -r "$PROMPT" ]] || die "prompt file not readable: $PROMPT"

# Idempotent per run: the box's work dir is wiped and recreated before
# anything is pushed, so a stale replies.tsv from an earlier batch can never
# be pulled back and mistaken for this batch's answers.
take_lock "${MAKEPAD_VLM_LOCK_WAIT:-1800}" || die "box $BOX still busy after ${MAKEPAD_VLM_LOCK_WAIT:-1800}s"
cat > "$TMP/preflight.ps1" <<EOF
\$ok = \$true
foreach (\$p in @('$REMOTE_EXE','$REMOTE_MODEL','$REMOTE_MMPROJ')) {
    if (-not (Test-Path \$p)) { Write-Output "MISSING=\$p"; \$ok = \$false }
}
if (-not \$ok) { exit 3 }
if (Test-Path '$REMOTE_WORK_ABS') { Remove-Item '$REMOTE_WORK_ABS' -Recurse -Force }
New-Item -ItemType Directory -Force -Path '$REMOTE_WORK_ABS' | Out-Null
Write-Output "HOST=\$env:COMPUTERNAME"
\$g = (& nvidia-smi --query-gpu=name,memory.used,memory.total --format=csv,noheader) 2>\$null
Write-Output ("GPU=" + ((\$g -join '; ').Trim()))
Write-Output "PREFLIGHT_OK"
EOF
pre="$(remote_run "$TMP/preflight.ps1")" || {
    echo "$pre" >&2
    die "preflight failed on $BOX — box unreachable or executor/model/mmproj missing"
}
grep -q '^PREFLIGHT_OK$' <<<"$pre" || { echo "$pre" >&2; die "preflight did not complete on $BOX"; }
HOST="$(sed -n 's/^HOST=//p' <<<"$pre" | head -1)"
GPU="$(sed -n 's/^GPU=//p' <<<"$pre" | head -1)"
echo "ok host=${HOST:-unknown}@$BOX exe=$REMOTE_EXE model=$REMOTE_MODEL mmproj=$REMOTE_MMPROJ gpu=$GPU" > "$LOCK/probe.line"

# Push each sheet and rewrite the path column to where it landed on the box.
: > "$TMP/jobs.tsv"
n=0
while IFS=$'\t' read -r id path ctx || [[ -n "${id:-}" ]]; do
    [[ -n "$id" ]] || continue
    [[ -r "$path" ]] || die "sheet not readable: $path (job $id)"
    remote_push "$path" "$REMOTE_WORK_REL/$id.ppm" || die "push failed for $path"
    printf '%s\t%s\\%s.ppm\t%s\n' "$id" "$REMOTE_WORK_ABS" "$id" "${ctx:-}" >> "$TMP/jobs.tsv"
    n=$((n + 1))
done < "$JOBS"
[[ $n -gt 0 ]] || die "no jobs in $JOBS"
remote_push "$TMP/jobs.tsv" "$REMOTE_WORK_REL/jobs.tsv" || die "push jobs.tsv failed"
remote_push "$PROMPT" "$REMOTE_WORK_REL/prompt.txt" || die "push prompt.txt failed"

EXTRA=""
[[ -n "${MAKEPAD_VLM_MAX_NEW_TOKENS:-}" ]] && EXTRA="$EXTRA --max-new-tokens $MAKEPAD_VLM_MAX_NEW_TOKENS"
[[ -n "${MAKEPAD_VLM_MAX_CONTEXT:-}" ]] && EXTRA="$EXTRA --max-context $MAKEPAD_VLM_MAX_CONTEXT"

cat > "$TMP/run.ps1" <<EOF
\$env:MAKEPAD_VISION_BACKEND = 'cuda'
& '$REMOTE_EXE' '$REMOTE_MODEL' '$REMOTE_MMPROJ' --jobs '$REMOTE_WORK_ABS\\jobs.tsv' --prompt-file '$REMOTE_WORK_ABS\\prompt.txt' --out '$REMOTE_WORK_ABS\\replies.tsv'$EXTRA 2>&1 | Select-Object -Last 25
Write-Output "EXECUTOR_EXIT=\$LASTEXITCODE"
if (-not (Test-Path '$REMOTE_WORK_ABS\\replies.tsv')) { Write-Output "NO_REPLIES"; exit 4 }
EOF
echo "remote_executor: $n sheets -> ${HOST:-?}@$BOX" >&2
run_out="$(remote_run "$TMP/run.ps1")" || {
    echo "$run_out" >&2
    die "executor run failed on $BOX"
}
echo "$run_out" >&2
grep -q '^EXECUTOR_EXIT=0$' <<<"$run_out" || die "vlm-annotate exited non-zero on $BOX (see output above)"

remote_pull "$REMOTE_WORK_REL/replies.tsv" "$OUT" || die "pull replies.tsv failed"
lines="$(grep -c '' "$OUT" || true)"
[[ "${lines:-0}" -gt 0 ]] || die "replies.tsv came back empty"
echo "replies: $lines lines"
