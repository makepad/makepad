#!/usr/bin/env python3
"""usage: run.py <makepad binary> [scenario ...]: code-cache sharing between apps, and the stale hits it must not make.

One workspace (fw = the shared framework crate; app_a/app_c use fw with feature `fast`, app_b without, app_p adds a
crate the loader binds). Each scenario: a fresh copy of the tree and a fresh disk cache; the FIRST app runs cold, an
optional edit is applied, the SECOND app runs on the same cache. Checks: (1) the second app's output equals the output
of the same app on the same (edited) tree with an empty cache; (2) the fw fns that must hit do, the fns the edit or the
feature difference reaches are compiled again. Prints "cache_shared_apps <scenario>: ok|FAILED ..." per scenario;
exits 1 on a failure.

Four scenarios run at once, each in a slot of its own (a fixed folder: the cache keys name the workspace's path). Most
scenarios start with the same cold run (app_a, the tree as it is): a slot runs it once and copies the folder it left
(tree, std, cache) back in place for each such scenario, as it stood after that run."""
import os, queue, re, shutil, subprocess, sys, tempfile, threading, traceback

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.normpath(os.path.join(HERE, "..", "..", "..", "..", ".."))
STD = os.path.normpath(os.path.join(HERE, "..", "..", "std"))
# fw fns that no scenario's edit or feature difference reaches (the plain ones)
PLAIN = ["unrelated", "uses_unrelated", "area", "sum_to", "add"]
FW = "fw/src/lib.rs"
E_ADD = (FW, "pub fn add(a: u32, b: u32) -> u32 { a + b } // EDIT add", "pub fn add(a: u32, b: u32) -> u32 { a + b + 100 } // EDIT add")
E_WRAP = 'pub fn wrap<T: Default + std::fmt::Debug>() -> usize { format!("{:?}", T::default()).len() }'
E_SHAPE = (FW, "pub struct Shape { pub w: u32, pub h: u32 } // EDIT shape", "pub struct Shape { pub w: u32, pub h: u32, pub d: u64 } // EDIT shape")
# a scenario: first/second app, edits applied to the tree between the runs, env of the two runs, the fns that must
# hit / be compiled in the second run, a minimum of shared slots, a std edit. Every second run links slots from the
# cache: one that links none ran as the empty-cache reference does, and its checks prove nothing (a std interface
# edit, another tree's path, a check's image: the stored code is not this run's to take)
SCENARIOS = {
    # same fw features, another app: the framework's code is shared
    "share": dict(first="app_a", second="app_c", hit=PLAIN + ["from_macro", "cfg_macro", "gated"]),
    # the first app holds a crate the loader binds (the provided-crate fixture, run from the repository's own workspace),
    # the second does not: the std code both compile is shared. Two workspaces share only a sealed crate's code (std,
    # outside both), in the shared code folder (`<relax home>/code`), which RELAX_CACHE_DIR turns off: shared_home
    "provided": dict(first="repo:relax-provided-fixture", second="app_b", min_hits=100, shared_home=True),
    # fw built with feature `fast` by one app, without by the other: what reads the feature is never shared
    "features": dict(first="app_a", second="app_b", compiled=["cfg_macro", "gated"]),
    "body": dict(first="app_a", second="app_a", edits=[E_ADD], hit=[x for x in PLAIN if x != "add"] + ["from_macro"], compiled=["add"], observable=True),
    "macro": dict(first="app_a", second="app_a", edits=[(FW, "mk_fn!(from_macro = 40); // EDIT macro", "mk_fn!(from_macro = 41); // EDIT macro")],
                  hit=PLAIN, compiled=["from_macro"], observable=True),
    "layout": dict(first="app_a", second="app_c", edits=[E_SHAPE], compiled=["area", "describe"]),
    "std_body": dict(first="app_a", second="app_c", compiled=["prim_traits::sum"],
                     std=("core/num/prim_traits.rs", "impl Sum for u32 { fn sum<I: Iterator<Item = Self>>(iter: I) -> Self { iter.fold(0, #[rustc_inherit_overflow_checks] |a, b| a + b) } }",
                          "impl Sum for u32 { fn sum<I: Iterator<Item = Self>>(iter: I) -> Self { iter.fold(0, #[rustc_inherit_overflow_checks] |a, b| b + a) } }")),
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
    # JITCOLD2: an edit outside fn bodies recompiles what names it, not the crate (cache/deps.rs). Each must
    # recompile the fn whose meaning changes (and show it in the output) and keep the plain fns linked.
    "add_item": dict(first="app_a", second="app_a", edits=[(FW, E_WRAP, E_WRAP + "\npub fn brand_new() -> u32 { 9 }")],
                     hit=PLAIN + ["uses_which", "sink_len", "uses_seven", "greet_plain", "apply", "from_macro"]),
    "shadow": dict(first="app_a", second="app_a", edits=[(FW, "// EDIT shadow", "pub fn which() -> u32 { 2 } // EDIT shadow")],
                   hit=PLAIN, compiled=["uses_which"], observable=True),
    "inherent": dict(first="app_a", second="app_a", edits=[(FW, "// EDIT inherent", "impl Sink { pub fn write_fmt(&mut self, _a: std::fmt::Arguments) -> std::fmt::Result { self.0 += 100; Ok(()) } } // EDIT inherent")],
                     hit=PLAIN, compiled=["sink_len"], observable=True),
    "seven": dict(first="app_a", second="app_a", edits=[(FW, "macro_rules! seven { () => { 7 } } // EDIT seven", "macro_rules! seven { () => { 8 } } // EDIT seven")],
                  hit=PLAIN, compiled=["uses_seven"], observable=True),
    "greet": dict(first="app_a", second="app_a", edits=[(FW, "impl Greet for Plain {} // EDIT greet", "impl Greet for Plain { fn greet(&self) -> u32 { 2 } } // EDIT greet")],
                  hit=PLAIN, compiled=["greet_plain"], observable=True),
    "apit": dict(first="app_a", second="app_a", edits=[(FW, "pub fn apply(f: impl Fn(u32) -> u32) -> u32 { f(2) } // EDIT apit", "pub fn apply(f: impl Fn(u32) -> u32) -> u32 { f(3) } // EDIT apit")],
                 hit=PLAIN, compiled=["apply"], observable=True),
    # JITCOLD7: a static's/constant's value entry holds while the bodies its evaluation ran keep their code keys:
    # an edit of a const fn another crate's static (or a constant it reads, or a static it points to) ran must
    # evaluate it again; an edit of the app's own const fn too
    "const_fn": dict(first="app_a", second="app_a", edits=[(FW, "pub const fn cval(x: u32) -> u32 { x * 3 } // EDIT cval", "pub const fn cval(x: u32) -> u32 { x * 5 } // EDIT cval")],
                     hit=PLAIN, observable=True),
    "own_const_fn": dict(first="app_a", second="app_a", edits=[("app_a/src/main.rs", "const fn own_c(x: u32) -> u32 { x + 4 } // EDIT own_c", "const fn own_c(x: u32) -> u32 { x + 40 } // EDIT own_c")],
                         hit=PLAIN, observable=True),
    # JITCOLD7: an edited crate's unchanged file takes its earlier image only while its macro lookups replay to the
    # same rules: the macro's rules change in another file (alt.rs uses mk_fn! of lib.rs)
    "macro_rules_other_file": dict(first="app_a", second="app_a", edits=[(FW, "pub fn $name() -> u32 { $v + 1 } // EDIT mk_fn", "pub fn $name() -> u32 { $v + 2 } // EDIT mk_fn")],
                                   hit=["unrelated", "area", "sum_to", "add"], observable=True),
    # JITCOLD7: a dependency's edit keeps a dependent crate's unchanged files from their images only while their
    # macro lookups replay: the rules of an exported macro app_a uses change in fw
    "dep_macro_rules": dict(first="app_a", second="app_a", edits=[(FW, "macro_rules! fw_twice { ($e:expr) => { $e * 2 } } // EDIT fw_twice", "macro_rules! fw_twice { ($e:expr) => { $e * 3 } } // EDIT fw_twice")],
                            hit=["unrelated", "area", "sum_to", "add"], observable=True),
    # an app's own new item: the framework and the app's other fns stay linked
    "own_item": dict(first="app_a", second="app_a", edits=[("app_a/src/main.rs", "fn own_a(x: u32) -> u32 { x * 9 + 2 }", "fn own_a(x: u32) -> u32 { x * 9 + 2 }\nfn own_b() -> u32 { 3 }")],
                     hit=PLAIN + ["own_a", "uses_which", "sink_len"]),
}
MIN_HITS = {}


def run(relax, tree, std, cache, app, tag, env_extra=None, shared_home=False):
    if app.startswith("repo:"):
        tree, pkg = REPO, app[5:]
    else:
        pkg = "sc-" + app.replace("_", "-")
    env = dict(os.environ, RELAX_CACHE_DIR=os.path.join(cache, "cache"), RELAX_STD=std, RUSTC_BOOTSTRAP="1",
               RELAX_CACHE_TRACE=os.path.join(cache, tag + ".trace"))
    env.pop("RELAX_CACHE", None)
    if shared_home:
        # no RELAX_CACHE_DIR: a private relax home (its shared code folder) under `cache`, and each workspace's
        # run dir in a target folder of its own there (`<target dir>/relax`)
        del env["RELAX_CACHE_DIR"]
        env["RELAX_HOME"] = os.path.join(cache, "home")
        os.makedirs(env["RELAX_HOME"], 0o700, exist_ok=True)
        env["CARGO_TARGET_DIR"] = os.path.join(cache, "target-" + ("repo" if app.startswith("repo:") else os.path.basename(tree)))
    env.update(env_extra or {})
    p = subprocess.run([relax, "run", "--quiet", pkg], cwd=tree, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=900)
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
        elif name == "prim_traits::sum" and slot.startswith("core::num::prim_traits::") and re.search(r"::sum(,|$)", slot):
            # (std_body's std fn: u32's Sum, `core::num::prim_traits::<impl>::sum,<its iterator type>`)
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


def shares_first(sc):
    """the scenario's first run is a slot's shared one: app_a run cold on the tree as it is"""
    return sc["first"] == "app_a" and not sc.get("env1") and not sc.get("shared_home")


def scenario(relax, name, slot):
    sc = SCENARIOS[name]
    first, second = sc["first"], sc["second"]
    hit, compiled = sc.get("hit", []), sc.get("compiled", [])
    tmp = os.path.join(slot["dir"], "w")
    snap = os.path.join(slot["dir"], "first")
    bad = []
    try:
        tree, std, cache = os.path.join(tmp, "tree"), os.path.join(tmp, "std"), os.path.join(tmp, "c")
        if shares_first(sc) and slot["first"]:
            shutil.copytree(snap, tmp, symlinks=True)
            rc, res1, out = slot["first"]
        else:
            os.mkdir(tmp, 0o700)
            shutil.copytree(HERE, tree, ignore=shutil.ignore_patterns("run.py", "target", "__pycache__"))
            shutil.copytree(STD, std)
            os.mkdir(cache, 0o700)
            rc, res1, out, _ = run(relax, tree, std, cache, first, "1-first", sc.get("env1"), sc.get("shared_home", False))
            if shares_first(sc) and rc == 0 and res1 is not None:
                shutil.copytree(tmp, snap, symlinks=True)
                slot["first"] = (rc, res1, out)
        if rc != 0 or (res1 is None and not first.startswith("repo:")):
            bad.append(f"first app: exit {rc}, no result\n{out[-600:]}")
        for e in list(sc.get("edits", [])) + list(sc.get("files", [])):
            edit_file(os.path.join(tree, e[0]), e[1], e[2])
        if sc.get("std"):
            edit_file(os.path.join(std, sc["std"][0]), sc["std"][1], sc["std"][2])
        rc, res2, out, tr = run(relax, tree, std, cache, second, "2-second", sc.get("env2"), sc.get("shared_home", False))
        ref = os.path.join(tmp, "ref")
        os.mkdir(ref, 0o700)
        rc0, want, out0, _ = run(relax, tree, std, ref, second, "ref", sc.get("env2"), sc.get("shared_home", False))
        if rc != 0 or res2 is None or res2 != want:
            bad.append(f"second app (shared cache): exit {rc}, result {res2}; empty cache gives {want} (exit {rc0})")
        if sc.get("observable") and first == second and res1 is not None and res2 == res1:
            bad.append("the edit is not observable in the output (the test proves nothing)")
        n_hit = sum(1 for w in tr.values() if "hit" in w)
        n_miss = sum(1 for w in tr.values() if w & {"miss", "linkfail", "cert:stale"})
        if n_hit == 0:
            bad.append("the second run linked nothing from the cache: it ran as the empty-cache reference does (the test proves nothing)")
        if sc.get("min_hits", 0) > n_hit:
            bad.append(f"fewer than {sc['min_hits']} slots shared")
        for f in hit:
            if status(tr, f) != "hit":
                bad.append(f"{f} should be shared but was {status(tr, f)}")
        for f in compiled:
            if status(tr, f) == "hit":
                bad.append(f"{f} was linked from the cache but must be compiled again")
        print(f"cache_shared_apps {name}: " + ("ok" if not bad else "FAILED " + "; ".join(bad)) + f" ({first} then {second}: {n_hit} slots linked, {n_miss} compiled)", flush=True)
    finally:
        if bad and os.path.isdir(tmp):
            kept = os.path.join(slot["dir"], "failed-" + name)
            os.rename(tmp, kept)
            print(f"  logs kept in {kept}", flush=True)
        elif os.path.isdir(tmp):
            shutil.rmtree(tmp)
    return not bad


def main():
    relax = os.path.abspath(sys.argv[1])
    names = sys.argv[2:] or list(SCENARIOS)
    todo = queue.Queue()
    for n in names:
        todo.put(n)
    failed = []

    def work():
        slot = {"dir": tempfile.mkdtemp(prefix="relax-cache-shared-apps-"), "first": None}
        ok = True
        while True:
            try:
                n = todo.get_nowait()
            except queue.Empty:
                break
            try:
                passed = scenario(relax, n, slot)
            except Exception:
                print(f"cache_shared_apps {n}: FAILED\n{traceback.format_exc()}", flush=True)
                passed = False
            if not passed:
                ok = False
                failed.append(n)
        if ok and os.path.basename(slot["dir"]).startswith("relax-cache-shared-apps-"):
            shutil.rmtree(slot["dir"])

    slots = [threading.Thread(target=work) for _ in range(4)]
    for t in slots:
        t.start()
    for t in slots:
        t.join()
    sys.exit(1 if failed else 0)


main()
