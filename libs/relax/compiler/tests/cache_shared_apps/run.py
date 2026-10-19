#!/usr/bin/env python3
"""usage: run.py <makepad binary> [scenario ...]: code-cache sharing between apps, and the stale hits it must not make.

One workspace (fw = the shared framework crate; app_a/app_c use fw with feature `fast`, app_b without, app_p adds a
crate the loader binds). Each scenario: a fresh copy of the tree and a fresh disk cache; the FIRST app runs cold, an
optional edit is applied, the SECOND app runs on the same cache. Checks: (1) the second app's output equals the output
of the same app on the same (edited) tree with an empty cache; (2) the fw fns that must hit do, the fns the edit or the
feature difference reaches are compiled again. Prints "cache_shared_apps <scenario>: ok|FAILED ..." per scenario;
exits 1 on a failure."""
import os, re, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.normpath(os.path.join(HERE, "..", "..", "..", "..", ".."))
STD = os.path.normpath(os.path.join(HERE, "..", "..", "std"))
# fw fns that no scenario's edit or feature difference reaches (the plain ones)
PLAIN = ["unrelated", "uses_unrelated", "area", "sum_to", "add"]
FW = "fw/src/lib.rs"
E_ADD = (FW, "pub fn add(a: u32, b: u32) -> u32 { a + b } // EDIT add", "pub fn add(a: u32, b: u32) -> u32 { a + b + 100 } // EDIT add")
E_SHAPE = (FW, "pub struct Shape { pub w: u32, pub h: u32 } // EDIT shape", "pub struct Shape { pub w: u32, pub h: u32, pub d: u64 } // EDIT shape")
# a scenario: first/second app, edits applied to the tree between the runs (or, with "tree2", to a second copy of the
# tree the second app runs in: same crate names at other paths), env of the two runs, the first command (run|check),
# the fns that must hit / be compiled in the second run, a minimum of shared slots, a std edit
SCENARIOS = {
    # same fw features, another app: the framework's code is shared
    "share": dict(first="app_a", second="app_c", hit=PLAIN + ["from_macro", "cfg_macro", "gated"]),
    # the first app holds a crate the loader binds (the provided-crate fixture, run from the repository's own workspace),
    # the second does not: the std code both compile is shared
    "provided": dict(first="repo:relax-provided-fixture", second="app_b", min_hits=100),
    # fw built with feature `fast` by one app, without by the other: what reads the feature is never shared
    "features": dict(first="app_a", second="app_b", compiled=["cfg_macro", "gated"]),
    "body": dict(first="app_a", second="app_a", edits=[E_ADD], hit=[x for x in PLAIN if x != "add"] + ["from_macro"], compiled=["add"], observable=True),
    "macro": dict(first="app_a", second="app_a", edits=[(FW, "mk_fn!(from_macro = 40); // EDIT macro", "mk_fn!(from_macro = 41); // EDIT macro")],
                  hit=PLAIN, compiled=["from_macro"], observable=True),
    "layout": dict(first="app_a", second="app_c", edits=[E_SHAPE], compiled=["area", "describe"]),
    "std_body": dict(first="app_a", second="app_c", compiled=["sum_from"],
                     std=("core/iter/mod.rs", "    iter.fold(zero, |a, b| a + b)\n", "    iter.fold(zero, |a, b| b + a)\n")),
    "std_iface": dict(first="app_a", second="app_c", compiled=PLAIN,
                      std=("core/num/int_impls.rs", "    pub const fn pow(self, exp: u32) -> u32 { self.wrapping_pow(exp) }", "    pub const fn pow(self, exp: u32) -> u32 { self.wrapping_pow(exp) + 0 }")),
    # `check` (binds no provided crate) then `run` (binds them) on one cache: the collected image is shared by both
    "check_run": dict(first="app_c", second="app_c", first_cmd="check"),
    "run_check": dict(first="app_c", second="app_c", second_cmd="check"),
    # the same crate (name) at another version/path with another body
    "two_versions": dict(first="app_a", second="app_a", tree2=[("fw/Cargo.toml", 'version = "0.1.0"', 'version = "0.2.0"'), E_ADD],
                         compiled=["add", "unrelated"]),
    # a build script's cfg differing per run (the script reads the environment)
    "build_cfg": dict(first="app_a", second="app_a", env2={"SC_FLAVOR": "b"}, compiled=["flavor"], observable=True),
    # env!/option_env!, include_str!, #[path] inputs
    "env": dict(first="app_a", second="app_a", env2={"SC_ENVV": "xyz"}, compiled=["envv"], observable=True),
    "include": dict(first="app_a", second="app_a", files=[("fw/data.txt", "0123456789\n", "0123456789abcdef\n")], compiled=["data"], observable=True),
    "pathmod": dict(first="app_a", second="app_a", edits=[("fw/src/alt.rs", "pub fn val() -> u32 { 10 } // EDIT alt", "pub fn val() -> u32 { 20 } // EDIT alt")],
                    compiled=["val"], observable=True),
    # a generic fw instance over an app's own type, both runs at the same path: the type's layout / its Default body changes
    "own_layout": dict(first="app_a", second="app_a", edits=[("app_a/src/main.rs", "struct Mine { x: u32 }", "struct Mine { x: u32, y: u64 }"),
                       ("app_a/src/main.rs", "Mine { x: 1 }", "Mine { x: 1, y: 2 }")], observable=True, compiled=["wrap"]),
    "own_body": dict(first="app_a", second="app_a", edits=[("app_a/src/main.rs", "fn mine_make() -> Mine { Mine { x: 1 } }", "fn mine_make() -> Mine { Mine { x: 123456 } }")], compiled=["mine_make"], observable=True),
    # app -> mid -> fw: fw's interface changes, mid's source does not
    "mid_iface": dict(first="app_g", second="app_g", edits=[E_SHAPE], compiled=["mid_area", "mid_shape_size"]),
    "mid_body": dict(first="app_g", second="app_g", edits=[E_ADD], hit=["mid_area", "mid_shape_size"], compiled=["add"]),
}
MIN_HITS = {}


def run(relax, tree, std, cache, app, tag, env_extra=None, cmd="run"):
    if app.startswith("repo:"):
        tree, pkg = REPO, app[5:]
    else:
        pkg = "sc-" + app.replace("_", "-")
    env = dict(os.environ, MAKEPAD_RUN_CACHE_DIR=os.path.join(cache, "cache"), MAKEPAD_RUN_STD=std, RUSTC_BOOTSTRAP="1",
               MAKEPAD_RUN_CACHE_TRACE=os.path.join(cache, tag + ".trace"))
    env.pop("MAKEPAD_RUN_CACHE", None)
    env.update(env_extra or {})
    p = subprocess.run([relax, cmd, "--quiet", pkg], cwd=tree, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=900)
    out = p.stdout.decode("utf-8", "replace")
    open(os.path.join(cache, tag + ".log"), "w").write(out)
    m = re.search(r"shared_apps result ([0-9 ]+)", out)
    res = m.group(1).strip() if m else None
    trace = {}
    try:
        for line in open(os.path.join(cache, tag + ".trace"), errors="replace"):
            f = line.rstrip("\n").split("\t")
            if len(f) >= 6:
                trace.setdefault(f[5], set()).add(f[1])
    except FileNotFoundError:
        pass
    return p.returncode, res, out, trace


def status(trace, name):
    """'hit', 'compiled' or None for the fw fns called `name`"""
    seen = set()
    for slot, whats in trace.items():
        if slot.startswith(("sc_fw::", "sc_mid::", "sc_app_")) and re.search(r"::(" + re.escape(name) + r")(,|$)", slot):
            seen |= whats
        elif name in ("sum_from",) and re.search(r"::sum_from(,|$)", slot):
            seen |= whats
    if seen & {"miss", "linkfail", "cert:stale", "cert:undecodable"}:
        return "compiled"
    if seen & {"hit"}:
        return "hit"
    return None


def edit_file(path, old, new):
    text = open(path).read()
    assert text.count(old) == 1, (path, old)
    open(path, "w").write(text.replace(old, new))


def scenario(relax, name):
    sc = SCENARIOS[name]
    first, second = sc["first"], sc["second"]
    hit, compiled = sc.get("hit", []), sc.get("compiled", [])
    cmd2 = sc.get("second_cmd", "run")
    tmp = tempfile.mkdtemp(prefix="relax-cache-shared-apps-")
    bad = []
    try:
        tree = os.path.join(tmp, "tree")
        shutil.copytree(HERE, tree, ignore=shutil.ignore_patterns("run.py", "target", "__pycache__"))
        shutil.copytree(STD, os.path.join(tmp, "std"))
        std = os.path.join(tmp, "std")
        cache = os.path.join(tmp, "c")
        os.mkdir(cache, 0o700)
        rc, res1, out, _ = run(relax, tree, std, cache, first, "1-first", sc.get("env1"), sc.get("first_cmd", "run"))
        if rc != 0 or (res1 is None and not first.startswith("repo:") and sc.get("first_cmd", "run") == "run"):
            bad.append(f"first app: exit {rc}, no result\n{out[-600:]}")
        run_tree = tree
        if sc.get("tree2"):
            run_tree = os.path.join(tmp, "tree2")
            shutil.copytree(tree, run_tree)
        for e in list(sc.get("edits", [])) + list(sc.get("files", [])) + list(sc.get("tree2", [])):
            edit_file(os.path.join(run_tree, e[0]), e[1], e[2])
        if sc.get("std"):
            edit_file(os.path.join(std, sc["std"][0]), sc["std"][1], sc["std"][2])
        rc, res2, out, tr = run(relax, run_tree, std, cache, second, "2-second", sc.get("env2"), cmd2)
        ref = os.path.join(tmp, "ref")
        os.mkdir(ref, 0o700)
        rc0, want, out0, _ = run(relax, run_tree, std, ref, second, "ref", sc.get("env2"), cmd2)
        if cmd2 == "check":
            if rc != 0 or "0 errors" not in out:
                bad.append(f"check on the shared cache: exit {rc}\n{out[-400:]}")
        elif rc != 0 or res2 is None or res2 != want:
            bad.append(f"second app (shared cache): exit {rc}, result {res2}; empty cache gives {want} (exit {rc0})")
        if sc.get("observable") and first == second and cmd2 == "run" and res1 is not None and res2 == res1:
            bad.append("the edit is not observable in the output (the test proves nothing)")
        if sc.get("min_hits", 0) > sum(1 for w in tr.values() if "hit" in w):
            bad.append(f"fewer than {sc['min_hits']} slots shared")
        for f in hit:
            if status(tr, f) != "hit":
                bad.append(f"{f} should be shared but was {status(tr, f)}")
        for f in compiled:
            if status(tr, f) == "hit":
                bad.append(f"{f} was linked from the cache but must be compiled again")
        n_hit = sum(1 for w in tr.values() if "hit" in w)
        n_miss = sum(1 for w in tr.values() if w & {"miss", "linkfail", "cert:stale"})
        print(f"cache_shared_apps {name}: " + ("ok" if not bad else "FAILED " + "; ".join(bad)) + f" ({first} then {second}: {n_hit} slots linked, {n_miss} compiled)", flush=True)
    finally:
        if not bad and os.path.basename(tmp).startswith("relax-cache-shared-apps-"):
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
