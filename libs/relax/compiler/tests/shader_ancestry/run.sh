#!/bin/bash
# usage: run.sh <makepad binary>: makepad's DrawShader derive pattern (rr.rs) -- a derive emits, per
# shader type, an ancestry macro under the type's own name (macro namespace, `pub use .. as Name`);
# a derived type invokes its base's through `use Base as alias` (the base reached by a glob import
# of another crate re-exported by a third, by a path into another module, in the same module, or in an
# include!d file whose imports only the expansion has), whose expansion calls
# back the runtime's `chain!` through a module alias (`use dep::rr as rt; super::rt::chain`), which
# defines the derived type's macro with an expansion-made `#[macro_export] macro_rules!`.
# Prints "shader_ancestry: ok" or "shader_ancestry: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
OUT=$(cd "$H" && MAKEPAD_RUN_STD=$H/../../std "$1" run --quiet anc-app 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
echo "$OUT"
if [ "$OUT" = "ancestry ok 0 1 2 2 1 7 3" ]; then echo "shader_ancestry: ok"; else echo "shader_ancestry: FAILED"; exit 1; fi
