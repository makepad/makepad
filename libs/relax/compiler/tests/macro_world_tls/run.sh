#!/bin/bash
# usage: run.sh <makepad binary>: a program whose crate the macro world expands a derive of (the
# derive keeps a HashMap) runs its own HashMaps on the same thread: the world's thread-locals and
# the program's never share bytes of the thread's block. Run with them in the block
# (RELAX_OPT=-statictls, as a host without static TLS places them) and as this host places
# them. Prints "macro_world_tls: ok" or "macro_world_tls: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
J=$1
WANT="macro world tls: fields 3 map 3 thread map 1"
export RELAX_CACHE=off RELAX_STD="$H/../../std"
fail=0
check() { # <case> <exit code> <output>
  if [ "$2" -eq 0 ] && echo "$3" | grep -qxF "$WANT"; then
    echo "macro_world_tls: $1: ok"
  else
    echo "macro_world_tls: $1: FAILED (exit $2)"
    echo "$3" | tail -20
    fail=1
  fi
}
out=$(cd "$H" && RELAX_OPT=-statictls "$J" run --quiet macro-world-tls-app 2>&1)
check "in the block" $? "$out"
out=$(cd "$H" && "$J" run --quiet macro-world-tls-app 2>&1)
check "as the host places them" $? "$out"
if [ $fail -eq 0 ]; then echo "macro_world_tls: ok"; else echo "macro_world_tls: FAILED"; exit 1; fi
