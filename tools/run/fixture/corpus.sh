#!/bin/bash
# Regression test: every test file of libs/relax/compiler/tests (its *.rs, std/*.rs and closure_owner's program)
# that agree.sh names in none of its lists runs here in the single-file form, under rustc and the JIT:
# `rustc -O --test` builds it (the profile `makepad test <file>` runs as; the edition of its `//@ edition:` header;
# nightly features as under RUSTC_BOOTSTRAP) and its tests pass, and `makepad test <file>` passes them too. A new
# file there runs here without being listed. (Until 11-23 no check ran 425 of them, and one had stopped compiling
# under the JIT unseen.) rustc's builds are kept in $W/rustc, by rustc's version and the sources each reads. A file
# whose run is not a plain pass says what it is, each in a line of its own:
#   // corpus: fails <test> ..  `makepad test` reports exactly those tests FAILED and passes the others: a fault the
#                               JIT reports and runs on past (a hang, a null read, a stack overflow, a missing
#                               symbol); rustc only checks it (its build dies of the fault, or does not link)
#   // corpus: aborts           the process aborts (SIGABRT) under rustc's build of the file and `makepad test` alike
#   // corpus: env <K=V> ..     `makepad test` runs with those variables set
#   // corpus: rustc-arch <a>   rustc's verdict is taken on <a> hosts only (core::arch::x86_64 is Relax's alone elsewhere)
#   // corpus: relax-only <why> rustc cannot build it (a Relax-only std item): the JIT's verdict alone
# The runners run with them: RUNNERS names each run.sh / run.py of the tests folders (and the few elsewhere) with
# its arguments. OPEN names the files and runners that do not pass yet, each with its reason (`<name>@<uname -s>`:
# on that system only). They run too, and one that passes is named so its entry can go; they fail nothing.
# Files and runners run CORPUS_JOBS at a time (default the lease's MAKEPAD_CPU_CORES, else 4), slowest first by
# the last run's seconds on this box ($W/times); the summary names the slowest. A failure prints the last lines of
# its output, unfiltered, and where all of it is. Run from the workspace root.
root=$PWD
# makepad is built once, here, as agree.sh builds it: a failed or warning build fails the run
build=$(cargo build --profile host -q --manifest-path "$root/Cargo.toml" -p makepad-run --bin makepad 2>&1)
if [ $? -ne 0 ] || [ -n "$build" ]; then
  printf '%s\n' "$build" | head -30 | cut -c1-200
  echo "corpus FAILED: building makepad failed or warned"; exit 1
fi
export J=${CARGO_TARGET_DIR:-$root/target}/host/makepad
export T=libs/relax/compiler/tests
export W=${CORPUS_DIR:-$root/target/corpus}
export RV="$(rustc -V)"
mkdir -p "$W/rustc"
# the aborting files' processes leave no core dumps
ulimit -c 0
OPEN="
"
# the files agree.sh runs, in any of its forms
named=$(grep -o '\$T/[A-Za-z0-9_/]*\.rs' tools/run/fixture/agree.sh | sed 's,^\$T/,,' | sort -u)
files=$(cd "$T" && ls *.rs std/*.rs closure_owner/main.rs | sort | comm -23 - <(printf '%s\n' "$named"))
sources() { # sources <file>: the file, its `mod m;` files, the folders of the modules its #[path]s name and the files
  local d=${1%/*} p #               its include*!s name
  echo "$1"
  sed -n -E 's,^ *(pub )?mod ([A-Za-z0-9_]+);.*,\2,p' "$1" | while read -r p; do echo "$d/$p.rs"; echo "$d/$p/mod.rs"; done
  sed -n -E 's,.*#\[path *= *"([^"]+)"\].*,\1,p' "$1" | while read -r p; do find "$d/${p%/*}" -name '*.rs'; done
  sed -n -E 's,.*include(_str|_bytes)?!\( *"([^"]+)".*,\2,p' "$1" | while read -r p; do echo "$d/$p"; done
}
export -f sources
one() { # one <file, relative to $T>: writes $W/<name>.v, "ok" or the failure's lines
  local f=$T/$1 n out rc ed arch fails env got want key bin
  n=$(printf '%s' "${1%.rs}" | tr / _)
  ed=$(awk '!/^\/\//{exit} /^\/\/@ edition:/{print $3; exit}' "$f")
  fails=$(sed -n 's,^// corpus: fails ,,p' "$f" | head -1)
  env=$(sed -n 's,^// corpus: env ,,p' "$f" | head -1)
  arch=$(sed -n 's,^// corpus: rustc-arch ,,p' "$f" | head -1)
  {
    if grep -q '^// corpus: relax-only ' "$f" || { [ -n "$arch" ] && [ "$arch" != "$(uname -m)" ]; }; then
      :
    elif [ -n "$fails" ]; then
      out=$(RUSTC_BOOTSTRAP=1 rustc --edition "${ed:-2021}" -O --test --emit=metadata -o "$W/rustc/$n.rmeta" "$f" 2>&1) \
        || { echo "rustc:$n: rustc rejects it:"; printf '%s\n' "$out" | tail -40; }
    else
      key=$({ printf '%s %s\n' "$RV" "$ed"; sources "$f" | sort -u | xargs cat 2>/dev/null; } | cksum | cut -d' ' -f1)
      bin=$W/rustc/$n.$key
      if [ ! -x "$bin" ]; then
        rm -f "$W/rustc/$n".*
        out=$(RUSTC_BOOTSTRAP=1 rustc --edition "${ed:-2021}" -O --test -o "$bin.tmp" "$f" 2>&1) && mv "$bin.tmp" "$bin" \
          || { echo "rustc:$n: rustc does not build it:"; printf '%s\n' "$out" | tail -40; }
      fi
      if [ -x "$bin" ]; then
        { out=$(perl -e 'alarm 120; exec @ARGV' "$bin" 2>&1); rc=$?; } 2>/dev/null
        if grep -q '^// corpus: aborts' "$f"; then
          [ $rc = 134 ] || echo "rustc:$n: rustc's build exits $rc, not an abort (134)"
        elif [ $rc != 0 ]; then
          echo "rustc:$n: rustc's build exits $rc:"; printf '%s\n' "$out" | tail -40
        fi
      fi
    fi
    { out=$(env $env MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 perl -e 'alarm 300; exec @ARGV' "$J" test "$f" 2>&1); rc=$?; } 2>/dev/null
    printf '%s\n' "$out" > "$W/$n.out"
    if grep -q '^// corpus: aborts' "$f"; then
      [ $rc = 134 ] || echo "test:$n: makepad test exits $rc, not an abort (134)"
    elif [ -n "$fails" ]; then
      got=$(printf '%s\n' "$out" | sed -n 's,^test test_crate::\([A-Za-z0-9_:]*\) \.\.\. FAILED.*,\1,p' | sort | tr '\n' ' ')
      want=$(printf '%s\n' $fails | sort | tr '\n' ' ')
      if [ $rc != 1 ] || [ "$got" != "$want" ] || ! printf '%s\n' "$out" | grep -q '^test result:'; then
        echo "test:$n: makepad test exits $rc, its FAILED tests [${got% }], expected [${want% }] FAILED and the others passed"
      fi
    elif [ $rc != 0 ]; then
      echo "test:$n: makepad test exits $rc"
    fi
  } > "$W/$n.v"
  if [ -s "$W/$n.v" ]; then
    if grep -q "^test:$n:" "$W/$n.v"; then echo "-- makepad test's last lines (all of it: $W/$n.out):"; tail -40 "$W/$n.out"; fi >> "$W/$n.v"
  else
    echo ok > "$W/$n.v"
  fi
}
export -f one
# the runners, each in its own folder as `bash|python3 <file> <args>` (@J: the JIT, @D: a fresh folder of its own),
# its exit status the verdict; `<file>:<tag>` names another run of a file. Paths are the tests folder's, or the
# workspace root's (tools/). RUNNERS run in the pool with the files; ALONE's after them, one at a time (each
# passes within a time: its program's tier-up, its exit or its signal's end).
RUNNERS="
cache_warm_edit_ids/run.sh @J
../bench_rt/interp/unwind_cmp.py @J
borrowck/run_live_regressions.sh @J
cache_shared_apps/run.py @J
relaxed/run.sh @J
cache_items/run.py @J
prepared_child/run.sh @J
bin_tests/run.sh @J
self_native/run.sh @J
allow_c_unused_member/run.sh @J
cache_body_verdict/run.py @J
abort_heap/run.sh @J
check_watch/run.sh:basic @J app check-watch app/main.rs basic.json
check_watch/run.sh:macro @J macro j2-macro macro/src/lib.rs macro.json
check_watch/run.sh:impl @J app check-watch app/main.rs impl.json
check_watch/run.sh:counter @J ../../../../../examples/counter makepad-example-counter ../../../../../examples/counter/src/main.rs counter.json
tools/run/fixture/program_stack.sh
ahead_const_len/run.sh @J
cache_warm_late_input/run.sh @J
cache_warm_dep_macro_impls/run.sh @J
cfg_macros/run.sh @J @D
expanded_macro_export/run.sh @J
include_spans/run.py --makepad @J
inline_target_tables/run.sh @J
layout/run.sh @J
non_exhaustive_enum_cast/run.sh @J
non_unicode_launch/run.sh @J
rust_dylib/run.sh @J
test_allocators/run.sh @J
web_diff/run.sh @J w64 @D
web_diff/run.sh:w32 @J w32 @D
web_layout/run.sh @J w64 @D
cache_warm_capture/run.sh @J
cache_warm_inlined_edit/run.sh @J
cargo_config_env/run.sh @J
compiler_thread_stack/run.sh @J
macro_rename_from_expansion/run.sh @J
macro_scope/run.sh @J
macro_world_tls/run.sh @J
pm_as_lib/run.sh @J
script_native_dep/run.sh @J
attr_macro/run.sh @J
auto_trait_cycle_outlives/run.sh @J
bare_test_attr/run.sh @J
bin_lib_same_name/run.sh @J
cache_shared_file/run.sh @J
cache_warm_anon_const_impls/run.sh @J
cache_warm_closure_once/run.sh @J
cache_warm_const_copy/run.sh @J
cache_warm_coroutine_arg/run.sh @J
cfg_attr_macro/run.sh @J
const_param_env/run.sh @J
ctfe_deferred/run.sh @J
dep_check_first/run.sh @J
derive_declared_by_dep_macro/run.sh @J
dyn_auto_xcrate/run.sh @J
edition2015/run.sh @J
empty_enum_glue/run.sh @J
glob_private_module/run.sh @J
impl_view_xcrate/run.sh @J
import_beside_derive_glob/run.sh @J
line_writer_writes/run.sh @J
local_derive/run.sh @J
macro_expanded_export/run.sh @J
many_bang_macros/run.sh @J
multi_version/run.sh @J
pm_call_site_edition/run.sh @J
pm_failed_crate_isolated/run.sh @J
pm_in_pm/run.sh @J
private_inherent_vs_trait/run.sh @J
proc_macro_tt_spacing/run.sh @J
script_cargo_is_loader/run.sh @J
shader_ancestry/run.sh @J
supertrait_assoc_dep/run.sh @J
test_bin_borrowck/run.sh @J
test_compile_error/run.sh @J
vendored/run.sh @J
cache_prelink_reload/run.py --makepad @J
live_closure_app/run.py --makepad @J
test_dep_borrowck/run.sh @J
"
ALONE="
retired_stub_rearm/run.sh @J
exit_persist/run.py @J
cache_warm_twin_tier/run.sh @J
exit_compiling/run.sh @J
signal_exit/run.sh @J
"
runner() { # runner <name> <args..>: writes $W/run/<name>.rc and .out
  local name=$1 f=${1%%:*} n d p i a args=()
  shift
  n=$(printf '%s' "$name" | tr '/:.' '___')
  d=$W/run/$n.d
  : "${d:?}"; rm -rf "$d"; mkdir -p "$d"
  for a in "$@"; do case $a in @J) args+=("$J") ;; @D) args+=("$d") ;; *) args+=("$a") ;; esac; done
  case $f in tools/*) p=$root/$f ;; *) p=$root/$T/$f ;; esac
  case $f in *.py) i=python3 ;; *) i=bash ;; esac
  (cd "${p%/*}" && MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 "$i" "$p" "${args[@]}") > "$W/run/$n.out" 2>&1 < /dev/null
  echo $? > "$W/run/$n.rc"
}
task() { # task f <file> | task r <runner> <args..>: runs it, its seconds in $W/t/
  local k=$1
  shift
  if [ "$k" = f ]; then one "$1"; else runner "$@"; fi
  printf '%s %s %s\n' "$SECONDS" "$k" "$*" > "$W/t/$(printf '%s' "$k $*" | tr -c 'A-Za-z0-9_\n' '_')"
}
export -f runner task
export root
mkdir -p "$W/run"
: "${W:?}"; rm -rf "$W/t"; mkdir -p "$W/t"
touch "$W/times"
jobs=${CORPUS_JOBS:-${MAKEPAD_CPU_CORES:-4}}
s=$SECONDS
# files and runners in one pool, slowest first by this box's last run ($W/times); what it has no time for first,
# the runners before the files
{ printf '%s\n' "$RUNNERS" | grep . | sed 's/^/r /'; printf 'f %s\n' $files; } \
  | awk -v tf="$W/times" 'BEGIN { while ((getline l < tf) > 0) t[substr(l, index(l, " ") + 1)] = l + 0 }
      { print (($0 in t) ? t[$0] : 1000000), NR, $0 }' \
  | sort -k1,1nr -k2,2n | cut -d' ' -f3- | xargs -P "$jobs" -L 1 bash -c 'task "$@"' _
printf '%s\n' "$ALONE" | grep . | sed 's/^/r /' | xargs -P 1 -L 1 bash -c 'task "$@"' _
cat "$W"/t/* > "$W/times"
fail=""; ok=0; open=0; closed=""
for f in $files; do
  n=$(printf '%s' "${f%.rs}" | tr / _)
  reason=$(printf '%s\n' "$OPEN" | sed -n "s,^$n ,,p; s,^$n@$(uname -s) ,,p")
  if [ -n "$reason" ]; then
    open=$((open + 1))
    [ "$(cat "$W/$n.v")" = ok ] && closed="$closed $n"
    continue
  fi
  if [ "$(cat "$W/$n.v")" = ok ]; then ok=$((ok + 1)); else cat "$W/$n.v"; fail="$fail $n"; fi
done
[ -z "$closed" ] || echo "corpus: open files that pass now (their OPEN entries can go):$closed"
closed=""
for r in $(printf '%s\n%s\n' "$RUNNERS" "$ALONE" | awk 'NF {print $1}'); do
  n=$(printf '%s' "$r" | tr '/:.' '___')
  rc=$(cat "$W/run/$n.rc" 2>/dev/null)
  # a runner whose verdict line says FAILED fails, whatever its exit status (seven once exited 0 after one)
  [ "$rc" = 0 ] && grep -qaE '^[a-z0-9_]+: FAILED' "$W/run/$n.out" && rc="0, but its verdict says FAILED"
  reason=$(printf '%s\n' "$OPEN" | sed -n "s,^$r ,,p; s,^$r@$(uname -s) ,,p")
  if [ -n "$reason" ]; then
    open=$((open + 1))
    [ "$rc" = 0 ] && closed="$closed $r"
    continue
  fi
  if [ "$rc" = 0 ]; then ok=$((ok + 1)); else echo "corpus: $r exit ${rc:-?}; its last lines (all of it: $W/run/$n.out):"; tail -40 "$W/run/$n.out"; fail="$fail $r"; fi
done
[ -z "$closed" ] || echo "corpus: open runners that pass now (their OPEN entries can go):$closed"
if [ -n "$fail" ]; then echo "corpus FAILED:$fail"; exit 1; fi
echo "corpus: ok, $ok files and runners ($open open) in $((SECONDS - s)) s, $jobs at once; the slowest: $(sort -rn "$W/times" | head -5 | awk '{printf "%s%s %s s", (NR > 1 ? ", " : ""), $3, $1}')"
