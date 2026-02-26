#!/usr/bin/env bash
set -euo pipefail

SOURCE_BRANCH="${1:-rik2}"
TARGET_BRANCH="${2:-dev}"
REMOTE="${REMOTE:-origin}"
STASH_CREATED=0
STASH_REF=""

is_git_operation_in_progress() {
    [ -d .git/rebase-merge ] || [ -d .git/rebase-apply ] || [ -f .git/MERGE_HEAD ]
}

restore_stash_if_needed() {
    if [ "${STASH_CREATED}" -ne 1 ]; then
        return
    fi

    if is_git_operation_in_progress; then
        echo "Leaving auto-stash in stash list while merge/rebase is in progress."
        echo "Restore later with: git stash pop ${STASH_REF}"
        return
    fi

    set +e
    git stash pop --index "${STASH_REF}"
    local status=$?
    set -e

    if [ "${status}" -ne 0 ]; then
        echo "Auto-stash could not be applied cleanly."
        echo "It is still available as ${STASH_REF}"
    fi
}

trap restore_stash_if_needed EXIT

if [ -d .git/rebase-merge ] || [ -d .git/rebase-apply ]; then
    echo "A rebase is already in progress."
    echo "Finish it with 'git rebase --continue' or abort with 'git rebase --abort'."
    exit 1
fi

if [ -f .git/MERGE_HEAD ]; then
    echo "A merge is already in progress."
    echo "Finish it with 'git commit' or abort with 'git merge --abort'."
    exit 1
fi

if ! git show-ref --verify --quiet "refs/heads/${SOURCE_BRANCH}"; then
    echo "Local branch '${SOURCE_BRANCH}' does not exist."
    exit 1
fi

if ! git show-ref --verify --quiet "refs/heads/${TARGET_BRANCH}"; then
    echo "Local branch '${TARGET_BRANCH}' does not exist."
    exit 1
fi

if [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
    BEFORE_STASH_REF="$(git rev-parse -q --verify refs/stash 2>/dev/null || true)"
    STASH_LABEL="auto-stash merge ${SOURCE_BRANCH} -> ${TARGET_BRANCH}"
    git stash push --include-untracked -m "${STASH_LABEL}" >/dev/null
    AFTER_STASH_REF="$(git rev-parse -q --verify refs/stash 2>/dev/null || true)"
    if [ -n "${AFTER_STASH_REF}" ] && [ "${AFTER_STASH_REF}" != "${BEFORE_STASH_REF}" ]; then
        STASH_CREATED=1
        STASH_REF="${AFTER_STASH_REF}"
        echo "Saved local changes to ${STASH_REF}."
    fi
fi

git fetch "${REMOTE}"

git switch "${TARGET_BRANCH}"
git merge --ff-only "${REMOTE}/${TARGET_BRANCH}"

git switch "${SOURCE_BRANCH}"
if ! git rebase "${TARGET_BRANCH}"; then
    echo
    echo "Rebase stopped due to conflicts."
    echo "Resolve conflicts, then run:"
    echo "  git add <resolved files>"
    echo "  git rebase --continue"
    echo
    echo "Or abort with:"
    echo "  git rebase --abort"
    exit 1
fi

git switch "${TARGET_BRANCH}"
git merge "${SOURCE_BRANCH}" --ff-only
git push "${REMOTE}" "${TARGET_BRANCH}"

git switch "${SOURCE_BRANCH}"
