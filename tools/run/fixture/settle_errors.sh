#!/bin/bash
# settle_errors.sh <run dir>: the panics and JIT compile errors of a present's run (its out.log and its report), as
# one line of tab-separated records, for land_gate.sh's present summary. A bucket keys a failure by its file and
# message, so a record says the same break the same way in every run. Before each record is cut to 200 chars, the
# fields that vary from run to run are dropped: the JIT's `[t=N ms]`, pids and thread ids, addresses, and the run's,
# the lease's and the tree's paths. When the cut fell elsewhere in each run, the same JIT break in a provided crate
# was named for whoever landed next. Records end at a tab, so a message never runs into the record after it.
# Run from the tree's root, as land_gate.sh runs.
set -u
run=${1:?usage: settle_errors.sh <run dir>}
{ grep -E 'panic parked|stale access parked|panicked|FATAL|compile error' "$run/out.log" 2>/dev/null
  grep -E $'^(panic|error)\t' "$run/report" 2>/dev/null; } | head -3 |
  awk -v run="$(dirname "$run")/" -v tmp="${TMPDIR:-/tmp}/" -v tree="$PWD/" '
    function drop(s, p,   i) { if (p == "/") return s; while ((i = index(s, p)) > 0) s = substr(s, 1, i - 1) substr(s, i + length(p)); return s }
    {
      s = drop(drop(drop($0, run), tmp), tree)
      gsub(/\[t=[0-9]+ ms\] ?/, "", s)
      gsub(/0x[0-9a-fA-F]+/, "0x_", s)
      gsub(/inspect [0-9]+/, "inspect <pid>", s); gsub(/pid [0-9]+/, "pid <pid>", s); gsub(/process [0-9]+/, "process <pid>", s)
      gsub(/\([0-9]+\)/, "(<tid>)", s)
      gsub(/-[0-9]+-[0-9]+\.log/, ".log", s)
      out = out (NR > 1 ? "\t" : "") substr(s, 1, 200)
    }
    END { printf "%s", out }'
