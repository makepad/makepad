#!/bin/bash
# tree_timing.sh [n]: times the git steps of a devhub tree move/restore in the node's tree (run it under a rev lease:
# `makepad devhub lease test --only <node> --rev <branch>,main -- bash tools/run/fixture/tree_timing.sh`). Per repo:
# the dirty check (status), the index refresh, a refresh after a full index rebuild (on a copy of the index: what a
# move pays when HEAD and the checkout ref disagree), and a real move of the tree by the last <n> commits (default 30)
# and back with update-index --refresh + read-tree -m -u (what lease-start and the restore run). The tree ends where
# it started.
n=${1:-30}
if [ "$n" = payload ]; then  # a lease's typical scratch: copies of the tree's source into its TMPDIR (left for the teardown)
  for i in 1 2 3; do cp -R libs "${TMPDIR:?run it under a lease}/payload$i"; done
  echo "payload: $(find "$TMPDIR" -type f | wc -l | tr -d ' ') files, $(du -sk "$TMPDIR" | cut -f1) KB in $TMPDIR; job done at $(date +%T)"
  exit 0
fi
TIMEFORMAT='%R'
t() { local label=$1; shift; local s; s=$( { time "$@" > /dev/null 2>&1; } 2>&1 ); printf '  %-46s %6s s\n' "$label" "$s"; }
echo "node $(uname -nsm); tree $PWD"
for d in . apps/commercial; do
  [ -e "$d/.git" ] || continue
  head=$(git -C "$d" rev-parse HEAD)
  echo "== $d: $(git -C "$d" ls-files | wc -l | tr -d ' ') tracked files, index $(du -k "$(git -C "$d" rev-parse --git-path index)" | cut -f1) KB, untrackedCache=$(git -C "$d" config core.untrackedCache || echo unset) fsmonitor=$(git -C "$d" config core.fsmonitor || echo unset) splitIndex=$(git -C "$d" config core.splitIndex || echo unset) preloadIndex=$(git -C "$d" config core.preloadIndex || echo unset)"
  t "status --untracked-files=no (1st)" git -C "$d" status --porcelain --untracked-files=no --ignore-submodules=all
  t "status --untracked-files=no (2nd)" git -C "$d" status --porcelain --untracked-files=no --ignore-submodules=all
  t "update-index -q --refresh" git -C "$d" update-index -q --refresh
  t "diff-index --quiet HEAD (stat only)" git -C "$d" diff-index --quiet HEAD --
  # the worst case, on a copy: an index rebuilt from HEAD has no stat data, so a refresh hashes every file
  idx=${TMPDIR:?run it under a lease (its scratch dir)}/tree-timing.index  # the lease's scratch: removed at its end
  GIT_INDEX_FILE=$idx git -C "$d" read-tree HEAD
  t "refresh after read-tree HEAD (no stat data)" env GIT_INDEX_FILE="$idx" git -C "$d" update-index -q --refresh
  base=$(git -C "$d" rev-parse -q --verify "HEAD~$n") || { echo "  (no HEAD~$n)"; continue; }
  files=$(git -C "$d" diff --name-only "$base" HEAD | wc -l | tr -d ' ')
  t "read-tree -m -u HEAD -> HEAD~$n ($files files)" git -C "$d" read-tree -m -u HEAD "$base"
  git -C "$d" update-ref --no-deref HEAD "$base"
  t "update-index -q --refresh (after the move)" git -C "$d" update-index -q --refresh
  t "read-tree -m -u back ($files files)" git -C "$d" read-tree -m -u "$base" "$head"
  git -C "$d" update-ref --no-deref HEAD "$head"
  [ "$(git -C "$d" rev-parse HEAD)" = "$head" ] && git -C "$d" diff-index --quiet HEAD -- && echo "  (back at ${head:0:10}, clean)" || echo "  WARNING: $d is not back clean at ${head:0:10}"
done
