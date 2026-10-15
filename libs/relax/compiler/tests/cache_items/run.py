#!/usr/bin/env python3
"""usage: run.py <makepad binary> [scenario ...]: per-item code cache invalidation.

For each scenario: a fresh copy of app/ + dep/ and a fresh disk cache; run cold, run warm, apply
one edit to dep/src/lib.rs, run (the edit run), run again (warm on the edit). The edit run must
print the edited program's result, recompile the code that depends on the edited item, and link
from the cache the fns that do not (unchanged fns of the edited file included). Prints one
"cache_items <scenario>: ok|FAILED ..." line per scenario and exits 1 on any failure.
"""
import os, re, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
STD = os.path.normpath(os.path.join(HERE, "..", "..", "std"))
# `makepad run` of a working tree is a live session: the app crate's structs carry the live extension
# block (size_of::<Pad>() is 16, not 4); the dependency crate's do not
BASE = [101, 8, 6, 10, 1, 42, 8, 9, 6, 2, 16, 7, 71, 21]
# result fields: use_leaf, use_shape, use_const, use_dyn, use_loud, unrelated, untouched_user, stable_after, use_static, local_size, use_pad, from_mac, mac_static, mac_body
SCENARIOS = {
    # a fn body: the fn recompiles; nothing else in its file or above it does
    "body": ("fn speak(&self) -> u32 { 10 } // EDIT body", "fn speak(&self) -> u32 { 11 } // EDIT body",
             {3: 11}, ["speak"], ["unrelated", "use_leaf", "use_shape", "use_const", "use_loud", "stable_before", "untouched_user", "stable_after"]),
    # an inlined callee's body: the code that inlined it (main's) is not linked again
    "inline": ("pub fn leaf() -> u32 { 100 } // EDIT inline", "pub fn leaf() -> u32 { 200 } // EDIT inline",
               {0: 201}, [], []),
    # a struct layout: its users recompile
    "layout": ("pub struct Shape { pub a: u32, pub b: u32 } // EDIT layout", "pub struct Shape { pub a: u32, pub b: u32, pub c: u64 } // EDIT layout",
               {1: 16}, ["use_shape"], []),
    # a const: its readers recompile
    "const": ("pub const SCALE: u32 = 3; // EDIT const", "pub const SCALE: u32 = 4; // EDIT const",
              {2: 8}, ["use_const"], []),
    # a static's initializer: the value changes (it is read at its address), its readers and
    # the rest of its file stay linked
    "static": ("pub static LEVEL: u32 = 5; // EDIT static", "pub static LEVEL: u32 = 50; // EDIT static",
               {8: 51}, [], ["use_static", "unrelated", "use_shape", "use_const", "stable_before", "untouched_user", "stable_after"]),
    # a struct defined in a fn body: the code of its type (size_of::<Local>) follows the body
    "local": ("    struct Local { a: u8 } // EDIT local", "    struct Local { a: u8, b: u64 } // EDIT local",
              {9: 17}, ["local_size"], ["unrelated", "use_shape", "use_const", "stable_before", "untouched_user", "stable_after"]),
    # a struct layout in the app (the top crate): the app's code recompiles, the dependency
    # crate below it does not
    "applayout": ("struct Pad { a: u32 } // EDIT applayout", "struct Pad { a: u32, b: u64 } // EDIT applayout",
                  {10: 24}, ["use_pad"], ["stable_before", "untouched_user", "stable_after", "local_size", "speak"]),
    # content inside a macro invocation that expands to an item fn / a static / a fn body: the code it
    # feeds follows the invocation's tokens, the rest of the file stays linked
    "macro_item": ("mk_fn!(from_mac = 7); // EDIT macro_item", "mk_fn!(from_mac = 8); // EDIT macro_item",
                   {11: 8}, ["from_mac"], ["unrelated", "use_shape", "use_const", "stable_before", "untouched_user", "stable_after"]),
    "macro_static": ("mk_static!(MAC_LEVEL = 70); // EDIT macro_static", "mk_static!(MAC_LEVEL = 80); // EDIT macro_static",
                     {12: 81}, [], ["unrelated", "use_shape", "use_const", "stable_before", "untouched_user", "stable_after"]),
    "macro_body": ("pub fn mac_body() -> u32 { plus_one!(20) } // EDIT macro_body", "pub fn mac_body() -> u32 { plus_one!(30) } // EDIT macro_body",
                   {13: 31}, ["mac_body"], ["unrelated", "use_shape", "use_const", "stable_before", "untouched_user", "stable_after"]),
    # a trait impl (it now overrides a default method): its users recompile
    "impl": ("    // EDIT impl", "    fn loud(&self) -> u32 { 5 } // EDIT impl",
             {4: 5}, ["use_loud"], []),
}


def run(relax, d, cache, tag, extra_env=None):
    env = dict(os.environ, MAKEPAD_RUN_CACHE_DIR=os.path.join(cache, "cache"),
               MAKEPAD_RUN_STD=STD, RUSTC_BOOTSTRAP="1", MAKEPAD_RUN_CACHE_TRACE=os.path.join(cache, tag + ".trace"))
    env.pop("MAKEPAD_RUN_CACHE", None)
    env.update(extra_env or {})
    p = subprocess.run([relax, "run", "--quiet", "cache-items"], cwd=os.path.join(d, "app"), env=env,
                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=600)
    out = p.stdout.decode("utf-8", "replace")
    open(os.path.join(cache, tag + ".log"), "w").write(out)
    m = re.search(r"cache_items result ([0-9 ]+)", out)
    res = [int(x) for x in m.group(1).split()] if m else None
    trace = {}
    try:
        for line in open(os.path.join(cache, tag + ".trace"), errors="replace"):
            f = line.rstrip("\n").split("\t")
            if len(f) >= 6:
                trace.setdefault(f[4], set()).add(f[1])
    except FileNotFoundError:
        pass
    return p.returncode, res, out, trace


def status(trace, name):
    """'hit', 'compiled' or None (never looked up) for the slots whose name ends with `name`"""
    seen = set()
    for slot, whats in trace.items():
        if slot == name or slot.endswith("::" + name) or slot.endswith(" " + name):
            seen |= whats
    if seen & {"miss", "linkfail", "cert:stale", "cert:undecodable"}:
        return "compiled"
    if seen & {"hit"}:
        return "hit"
    return None


def scenario(relax, name):
    old, new, changes, recompiled, kept = SCENARIOS[name]
    tmp = tempfile.mkdtemp(prefix="relax-cache-items-")
    bad = []
    try:
        for sub in ("app", "dep"):
            shutil.copytree(os.path.join(HERE, sub), os.path.join(tmp, sub))
        cache = os.path.join(tmp, "c")
        os.mkdir(cache, 0o700)
        want = list(BASE)
        # tier 0 inlines (into main) only past the startup phase
        tier0 = {"MAKEPAD_RUN_STARTUP_PHASE": "0"} if name == "inline" else None
        rc, res, out, cold = run(relax, tmp, cache, "1-cold", tier0)
        if rc != 0 or res != want:
            bad.append(f"cold run: exit {rc}, result {res}, want {want}")
        rc, res, _, warm = run(relax, tmp, cache, "2-warm", tier0)
        if rc != 0 or res != want:
            bad.append(f"warm run: exit {rc}, result {res}")
        for f in recompiled + kept:
            if status(warm, f) == "compiled":
                bad.append(f"warm run compiled {f}")
        lib = os.path.join(tmp, "app", "src", "main.rs") if name == "applayout" else os.path.join(tmp, "dep", "src", "lib.rs")
        text = open(lib).read()
        assert text.count(old) == 1, old
        open(lib, "w").write(text.replace(old, new))
        for i, v in changes.items():
            want[i] = v
        rc, res, _, edit = run(relax, tmp, cache, "3-edit", tier0)
        if rc != 0 or res != want:
            bad.append(f"edit run: exit {rc}, result {res}, want {want}")
        for f in recompiled:
            if status(edit, f) == "hit":
                bad.append(f"edit run linked stale {f}")
        for f in kept:
            if status(edit, f) != "hit":
                bad.append(f"edit run did not link unchanged {f} ({status(edit, f)})")
        if name == "inline":
            # main's code inlined use_leaf and leaf: the warm run linked it; after the edit of leaf
            # its certificate refuses it (and the result is the edited one)
            if not any("hit" in w for k, w in warm.items() if k.endswith("::main")):
                bad.append("the warm run did not link main (the test needs it)")
            if not any("cert:stale" in w for k, w in edit.items() if k.endswith(("::main", "use_leaf"))):
                bad.append("edit run: no entry that inlined leaf was refused")
            if any("hit" in w for k, w in edit.items() if k.endswith(("::main", "use_leaf"))):
                bad.append("edit run linked code that inlined the old leaf")
        rc, res, _, again = run(relax, tmp, cache, "4-warm")
        if rc != 0 or res != want:
            bad.append(f"warm run after the edit: exit {rc}, result {res}")
        for f in recompiled + kept:
            if status(again, f) == "compiled":
                bad.append(f"warm run after the edit compiled {f}")
        n_edit = sum(1 for w in edit.values() if w & {"miss", "linkfail", "cert:stale"})
        n_hit = sum(1 for w in edit.values() if w & {"hit"})
        print(f"cache_items {name}: " + ("ok" if not bad else "FAILED " + "; ".join(bad)) + f" (edit run: {n_edit} slots compiled, {n_hit} linked)", flush=True)
    finally:
        if not bad and os.path.basename(tmp).startswith("relax-cache-items-"):
            shutil.rmtree(tmp)
        elif bad:
            print(f"  logs kept in {tmp}", flush=True)
    return not bad


def main():
    relax = os.path.abspath(sys.argv[1])
    names = sys.argv[2:] or list(SCENARIOS)
    ok = all([scenario(relax, n) for n in names])
    sys.exit(0 if ok else 1)


main()
