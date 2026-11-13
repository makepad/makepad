#!/bin/bash
# usage: run.sh <makepad binary>: `makepad check` of a crate whose root re-exports a proc-macro crate
# by glob (a derive `Item`) and a trait `Item` by name, and imports `crate::Item` in a module.
# rustc accepts it; the check must report no errors (it reported E0432 `unresolved import
# crate::Item` when the replay of rustc's import resolution left the macro namespace undetermined).
# Prints "import_beside_derive_glob: ok" or "import_beside_derive_glob: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off RELAX_STD=$H/../../std
R=$(cd "$H" && "$1" check ibdg-lib 2>&1); rc=$?
if [ $rc -eq 0 ] && echo "$R" | grep -q ", 0 errors$"; then echo "import_beside_derive_glob: ok"; else echo "$R" | grep -E ':[0-9]+:[0-9]+: |errors$|panicked'; echo "import_beside_derive_glob: FAILED"; exit 1; fi
