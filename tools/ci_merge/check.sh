#!/usr/bin/env bash
# Run on a build node. All commits/configuration stay in this disposable repository.
set -euo pipefail
checkout=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/ci-merge.XXXXXXXX")
trap '[[ -n "${scratch:-}" ]] && rm -rf -- "$scratch"' EXIT
git_at() { git -C "$scratch" "$@"; }
quote() { printf "'%s'" "${1//\'/\'\\\'\'}"; }
driver="cargo run -q --release --manifest-path $(quote "$checkout/tools/ci_merge/Cargo.toml") --target-dir $(quote "$checkout/target/ci_merge") -- %O %A %B %P"
git_at init -q -b base
git_at config user.name ci-merge-check
git_at config user.email ci-merge-check@example.invalid
git_at config commit.gpgsign false
git_at config core.hooksPath "$scratch/no-hooks"
git_at config merge.ci-proof.name 'ci.toml proofs'
git_at config merge.ci-proof.driver "$driver"
printf 'ci.toml merge=ci-proof\n' > "$scratch/.gitattributes"
mkdir "$scratch/crate space"
proof="$scratch/crate space/ci.toml"
section() {
    printf '\n[%s]\ndigest = "%064d"\nwork = "%s"\ndate = "%s"\n' "$1" "$2" "$3" "$4"
}
write_proofs() {
    {
        printf '# CI proofs fixture\n'
        section app.linux "$1" "$2" "$3"
        section tests.macos "$4" "$5" '2026-10-02'
        section job.jit.windows "$6" "$7" '2026-10-02'
        section job.tie.linux "$8" "$2" '2026-10-02'
    } > "$proof"
}
# No crate manifest/lock: digest lookup must fall back to the dates.
write_proofs 1 base 2026-10-01 1 base-tests 1 base-job 1
git_at add .
git_at commit -qm base
git_at checkout -qb left
write_proofs 2 left 2026-10-02 2 left-tests 1 base-job 2
git_at commit -qam left
git_at checkout -qb right base
write_proofs 3 right 2026-10-03 1 base-tests 3 right-job 3
git_at commit -qam right
verify() {
    if [[ -n "$(git_at ls-files -u)" ]] || grep -Eq '^(<<<<<<<|=======|>>>>>>>|\|\|\|\|\|\|\|)' "$proof"; then
        printf 'unresolved proof conflict\n' >&2
        exit 1
    fi
    grep -Fq 'work = "right"' "$proof"
    grep -Fq 'work = "left-tests"' "$proof"
    grep -Fq 'work = "right-job"' "$proof"
    if grep -Fq '[job.tie.linux]' "$proof"; then
        printf 'ambiguous section was retained\n' >&2
        exit 1
    fi
}
git_at checkout -qb merge-check left
git_at merge --no-edit right
verify
git_at checkout -qb rebase-check left
GIT_SEQUENCE_EDITOR=: GIT_EDITOR=: git_at rebase right
verify
git_at checkout -qb pick-check right
git_at cherry-pick left
verify
git_at checkout -qb pull-check left
git_at pull --no-rebase --no-edit . right
verify
printf 'ci-merge: merge, rebase, cherry-pick and local pull passed\n'
