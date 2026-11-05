#!/bin/bash
# keep_logs.sh <dir>: a failed step's logs into the lease's keep dir ($DEVHUB_KEEP/<dir's name>/), which
# the lease's holder copies next to its own log (the master's bucket-logs, a leg's log dir) before the
# lease ends and the node's scratch goes with it: whoever chases an intermittent has the evidence.
# The logs only: the files right in <dir>, and below it *.log, *.out, *.fail and report files and
# crash captures; never an app home's data or a JIT cache. Not under a lease (no DEVHUB_KEEP): nothing.
[ -n "${DEVHUB_KEEP:-}" ] && [ -d "$1" ] || exit 0
to=$DEVHUB_KEEP/$(basename "$1")
cd "$1" || exit 0
{ find . -mindepth 1 -maxdepth 1 -type f
  find . -type f \( -name '*.log' -o -name '*.out' -o -name '*.fail' -o -name report -o -path '*/captures/*' \); } | sort -u |
while IFS= read -r f; do
  mkdir -p "$to/$(dirname "$f")" && cp -p "$f" "$to/$f"
done
