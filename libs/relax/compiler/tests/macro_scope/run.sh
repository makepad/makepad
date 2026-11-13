#!/bin/bash
# usage: run.sh <makepad binary>: macro resolution independent of load order and through import
# chains (= rustc, expected.txt); prints the outputs, then "macro_scope: ok" or "macro_scope: FAILED"
# (exit 1). Cases:
#   the same user crate in two programs (one also loads dep_a, which exports another `trace!`)
#   a re-export chain (top: `pub use mid::inner;`, a module glob-importing it, a root glob) and
#   an import inside a fn body, with dep_a exporting decoys of the same names
H=$(cd "$(dirname "$0")" && pwd)
# no disk cache: its "relax cache:" lines and stored code are not part of what is tested
export RELAX_CACHE=off
T=$(mktemp -d)
{
for v in with without; do
  if [ $v = with ]; then DEPS='[{"index":0,"extern_crate_name":"dep_a"},{"index":2,"extern_crate_name":"user"}]'; else DEPS='[{"index":2,"extern_crate_name":"user"}]'; fi
  sed -e "s#@R@#$H#g" -e "s#@MAIN@#main_${v}_a.rs#" -e "s#@DEPS@#$DEPS#" $H/units.json.in > $T/units-$v.json
  RELAX_STD=$H/../../std RELAX_CACHE_DIR=$T/target-$v $1 run --quiet --units $T/units-$v.json app 2>>$T/err || true
done
sed -e "s#@R@#$H#g" $H/units-chain.json.in > $T/units-chain.json
RELAX_STD=$H/../../std RELAX_CACHE_DIR=$T/target-chain $1 run --quiet --units $T/units-chain.json app 2>>$T/err || true
} > $T/out.txt
cat $T/out.txt
if cmp -s $T/out.txt $H/expected.txt; then echo "macro_scope: ok"; else tail -5 $T/err; echo "macro_scope: FAILED"; exit 1; fi
