#!/bin/bash
# usage: run.sh <makepad binary>: a program whose 8 threads keep hitting functions that are not compiled yet
# (3,000 generated fns on 24 threads, each a fresh lazy compile) calls process::exit while they do. The exit must not wait
# on their compiles (they park; builds in progress are abandoned): exit within 500 ms of the call, and the
# exit summary must still run. Prints "exit_compiling: ok (<ms> ms)" or "exit_compiling: FAILED ...".
# (The bug this guards: a closing Files app scanning on 8 threads took ~5 s to end; the exit summary waited
# up to 2 s for the unit lock while the threads kept compiling.)
R=$1
H=$(cd "$(dirname "$0")" && pwd)
C=$(mktemp -d)
chmod 700 "$C"
mkdir "$C/p" "$C/p/src"
printf '[package]\nname = "relax-exit-compiling"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n\n[workspace]\n' > "$C/p/Cargo.toml"
python3 - "$C/p/src/main.rs" <<'PY'
import sys
n = 3000
out = []
for i in range(n):
    out.append(f"fn f{i}(x: u64) -> u64 {{ let mut v: Vec<u64> = (0..{i % 50 + 10}u64).map(|k| k.wrapping_mul(x) ^ {i}).collect(); v.sort(); let s: std::collections::BTreeMap<u64, [u8; {i % 13 + 1}]> = v.iter().map(|k| (*k, [{i % 251}u8; {i % 13 + 1}])).collect(); format!(\"{{:?}}\", s.iter().next()).len() as u64 ^ v.iter().fold({i}u64, |a, b| a.wrapping_add(*b)) }}")
out.append("static FNS: [fn(u64) -> u64; %d] = [%s];" % (n, ", ".join(f"f{i}" for i in range(n))))
out.append("""
fn main() {
    for t in 0..24usize {
        std::thread::spawn(move || {
            let mut acc = 0u64;
            let mut i = t;
            loop {
                acc = acc.wrapping_add(FNS[i % FNS.len()](acc));
                i += 24;
                if i > 1_000_000 { println!("{}", acc); }
            }
        });
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    println!("exiting at {}", ms);
    std::process::exit(0);
}""")
open(sys.argv[1], "w").write("\n".join(out) + "\n")
PY
(cd "$C/p" && MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_CACHE_DIR="$C/target" MAKEPAD_RUN_STD="$H/../../std" RUSTC_BOOTSTRAP=1 "$R" run --verbose relax-exit-compiling) > "$C/run.log" 2>&1
END=$(python3 -c 'import time;print(int(time.time()*1000))')
python3 - "$C" "$END" <<'PY'
import os, re, shutil, sys
c, end = sys.argv[1], int(sys.argv[2])
t = open(os.path.join(c, "run.log"), errors="replace").read()
m = re.search(r"exiting at (\d+)", t)
bad = []
if not m:
    bad.append("no 'exiting at' line")
    dt = None
else:
    dt = end - int(m.group(1))
    if dt > 500:
        bad.append(f"exit took {dt} ms")
if "makepad-run: exit at" not in t and "makepad-run: main returned" not in t:
    bad.append("no exit summary")
if "no summary" in t:
    bad.append("summary skipped: " + re.search(r"no summary[^\n]*", t).group(0))
w = re.search(r"waited [0-9.]+ ms for the unit lock[^\n]*", t)
print("exit_compiling: " + ("ok" if not bad else "FAILED " + "; ".join(bad)) + f" ({dt} ms from exit() to process end" + ("; " + w.group(0) if w else "") + ")")
if bad:
    print("\n".join(t.splitlines()[-8:]))
# the temp dir this script made (mktemp -d), nothing else
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
