#!/usr/bin/env python3
"""Live patches always land: one edit per edit kind against a running program, visible effect asserted.

usage: live_kinds.py [--makepad <binary>] [--only k1,k2] [--jobs N]
  (default binary: $TARGET/makepad, TARGET default target/release; MAKEPAD_RUN_STD in the environment,
   default libs/relax/compiler/std; run from the workspace root)

For each kind: copy live_kinds/ to a worker dir, `makepad run --ahead=minimal relax-live-kinds` (a working
tree: live, hidden, muted, own MAKEPAD_RUN_HOME), wait for the live watcher (MAKEPAD_RUN_LIVE_READY) and
the first tick, save the kind's edit and wait for the probe's printed value to become the expected one.
The outcome is how the edit landed: patched (installed live), restarted (the program printed
"out restored": a state-preserving restart in place), refused, or pending (a restart nobody made).
A kind passes only when the new value is seen: an edit that installs with no visible effect fails.
Prints the table `kind | outcome | effect | ms` and `live_kinds: N/M landed`; exit 1 on any miss.
"""
import os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.join(HERE, "live_kinds")
args = sys.argv[1:]
def opt(name, default):
    return args[args.index(name) + 1] if name in args else default
relax = os.path.abspath(opt("--makepad", os.path.join(os.environ.get("TARGET", "target/release"), "makepad")))
only = [k for k in opt("--only", "").split(",") if k]
jobs = int(opt("--jobs", "3"))
os.environ.setdefault("MAKEPAD_RUN_STD", os.path.abspath("libs/relax/compiler/std"))
WAIT = float(os.environ.get("LIVE_KINDS_WAIT", "60"))

K = "src/kinds.rs"
M = "src/main.rs"
# kind -> (probe key, [(file, old, new)], accepted values (None: any value >= the first))
KINDS = [
    ("fn_body", "body", [(K, "pub fn body() -> u64 { 1 }", "pub fn body() -> u64 { 2 }")], {2}),
    ("inlined_into_running_loop", "inlined", [(K, "pub fn tiny() -> u64 { 1 }", "pub fn tiny() -> u64 { 2 }")], {2}),
    ("running_loop_body", "main_loop", [(M, "n, 1, kinds", "n, 2, kinds")], {2}),
    ("const", "const", [(K, "pub const K: u64 = 10;", "pub const K: u64 = 20;")], {20}),
    ("static_init", "static", [(K, "pub static S: u64 = 100;", "pub static S: u64 = 200;")], {200}),
    ("static_interior_mut", "counter", [(K, "AtomicU64::new(0);", "AtomicU64::new(1000000);")], (1000000,)),
    ("thread_local_init", "tls", [(K, "Cell::new(5); }", "Cell::new(6); }")], {6}),
    ("struct_field_add", "field_add", [(K, "pub struct Rec { pub a: u64, pub c: u64 }", "pub struct Rec { pub a: u64, pub c: u64, pub b: u64 }"),
        (K, "Rec { a: 1, c: 2 }", "Rec { a: 1, c: 2, b: 3 }"), (K, "{ r.a * 10 + r.c }", "{ r.a * 100 + r.c * 10 + r.b }")], {120, 123}),
    ("struct_field_remove", "field_remove", [(K, "pub struct Rec { pub a: u64, pub c: u64 }", "pub struct Rec { pub a: u64 }"),
        (K, "Rec { a: 1, c: 2 }", "Rec { a: 1 }"), (K, "{ r.a * 10 + r.c }", "{ r.a * 10 + 5 }")], {15}),
    ("struct_field_reorder", "field_reorder", [(K, "pub struct Rec { pub a: u64, pub c: u64 }", "pub struct Rec { pub c: u64, pub a: u64 }"),
        (K, "{ r.a * 10 + r.c }", "{ r.a * 100 + r.c }")], {102}),
    ("struct_field_type", "field_type", [(K, "pub struct Wide { pub x: u32 }", "pub struct Wide { pub x: u64 }"), (K, "{ w.x as u64 }", "{ w.x + 1000 }")], {1007}),
    ("enum_variant_add", "enum_variant", [(K, "pub enum Mode { A, B }", "pub enum Mode { A, B, C }"),
        (K, "Mode::A => 1, Mode::B => 2 }", "Mode::A => 11, Mode::B => 2, Mode::C => 3 }")], {11}),
    ("fn_signature", "signature", [(K, "pub fn sig(a: u64) -> u64 { a }", "pub fn sig(a: u64, b: u64) -> u64 { a + b }"), (K, "{ sig(1) }", "{ sig(1, 2) }")], {3}),
    ("fn_added", "fn_added", [(K, "pub fn newfn_val() -> u64 { 0 }", "pub fn added() -> u64 { 42 }\npub fn newfn_val() -> u64 { added() }")], {42}),
    ("fn_removed", "fn_removed", [(K, "pub fn doomed() -> u64 { 3 }\n", ""), (K, "{ doomed() }", "{ 4 }")], {4}),
    ("struct_impl_added", "struct_added", [(K, "pub fn news_val() -> u64 { 0 }", "pub struct NewS(u64);\nimpl NewS { pub fn get(&self) -> u64 { self.0 } }\npub fn news_val() -> u64 { NewS(9).get() }")], {9}),
    ("mod_added", "mod_added", [(K, "pub fn mod_val() -> u64 { 0 }", "mod extra { pub fn v() -> u64 { 77 } }\npub fn mod_val() -> u64 { extra::v() }")], {77}),
    ("vtable_method_body", "vtable", [(K, "impl Shape for Sq { fn area(&self) -> u64 { 1 } }", "impl Shape for Sq { fn area(&self) -> u64 { 2 } }")], {2}),
    ("trait_impl_added", "impl_added", [(K, "pub fn impl_add_val() -> u64 { 0 }", "impl Shape for Tri { fn area(&self) -> u64 { 3 } }\npub fn impl_add_val() -> u64 { Tri.area() }")], {3}),
    ("trait_impl_removed", "impl_removed", [(K, "impl Shape for Circ { fn area(&self) -> u64 { 5 } }\n", ""), (K, "{ Circ.area() }", "{ 6 }")], {6}),
    ("trait_method_added", "trait_items", [(K, "pub trait Named { fn name_len(&self) -> u64; }", "pub trait Named { fn name_len(&self) -> u64; fn extra(&self) -> u64 { 30 } }"),
        (K, "{ s.name_len() }", "{ s.name_len() + s.extra() }")], {32}),
    ("generic_fn_body", "generic", [(K, "{ t.into() }", "{ t.into() + 1 }")], {2}),
    ("closure_per_call", "closure", [(K, "|x: u64| x + 1; f(0)", "|x: u64| x + 2; f(0)")], {2}),
    ("closure_stored", "closure_stored", [(K, "Box::new(|| 1)", "Box::new(|| 2)")], {2}),
    ("macro_rules", "macro", [(K, "() => { 1 } }", "() => { 2 } }")], {2}),
    ("use_changed", "use", [(K, "use std::cmp::max as pick;", "use std::cmp::min as pick;")], {1}),
    ("type_alias", "alias", [(K, "pub type Num = u64;", "pub type Num = u32;")], {4}),
]
# the UI cases (makepad-example-counter, seen in its live widget tree through makepad inspect):
# a script_mod! (Splash DSL) edit; an event handler's body (clicked after the landing check);
# an edit that restarts the app in place (a trait added), window kept (the counter's count lives in
# its script state, which it does not hand over: AppMain::restart_state)
CM = "examples/counter/src/main.rs"
KINDS += [
    ("ui_dsl_script_mod", None, [(CM, 'text: "Increment"', 'text: "Plus one"')], {"expect": ["Plus one"]}),
    ("ui_handler_body", None, [(CM, "mod.state.counter += 1", "mod.state.counter += 10")], {"after": 2.5, "click": 1, "expect": ["Count: 10"]}),
    ("ui_restart_in_place", None, [(CM, 'text: "Increment"', 'text: "Plus one"'), (CM, "#[derive(Script, ScriptHook)]", "pub trait LiveKindsMarker {}\n\n#[derive(Script, ScriptHook)]")],
        {"expect": ["Plus one"]}),
]
if only:
    KINDS = [k for k in KINDS if k[0] in only]

def probe(line, key):
    for tok in line.split():
        if tok.startswith(key + "="):
            try:
                return int(tok[len(key) + 1:])
            except ValueError:
                return None
    return None

def inspect(pid, tmp, *q):
    r = subprocess.run([relax, "inspect", str(pid)] + list(q), env=dict(os.environ, TMPDIR=tmp), capture_output=True, text=True, timeout=30)
    return r.stdout + r.stderr

def click(pid, tmp, widget):
    reply = inspect(pid, tmp, "ui", "act", "q=path=" + widget, "press", "wait=1")
    return 'handled:' in reply or 'fallback:' in reply

UI_LOCK = threading.Lock()

def run_ui(slot, kind):
    # edits the workspace's own example in place (restored at the end): its path dependencies
    # resolve only inside the workspace, so the UI cases run one at a time
    with UI_LOCK:
        return run_ui_locked(slot, kind)

def run_ui_locked(slot, kind):
    name, _, edits, spec = kind
    work = os.path.join(root, "w%d" % slot)
    tmp = os.path.join(work, "tmp")
    os.makedirs(tmp, exist_ok=True)
    home = os.path.join(work, "uihome")
    originals = {rel: open(os.path.abspath(rel)).read() for rel, _, _ in edits}
    env = dict(os.environ, MAKEPAD_RUN_HOME=home, MAKEPAD_RUN_CACHE_DIR=os.path.join(home, "cache"), MAKEPAD_RUN_TARGET_DIR=os.path.join(home, "target"),
               MAKEPAD_HIDE_WINDOWS="1", SANDBOX_MUTE="1", MAKEPAD_MUTE="1", MAKEPAD_REMOTE="1", MAKEPAD_HOME=os.path.join(work, "mphome"), TMPDIR=tmp,
               MAKEPAD_RUN_LIVE_TRACE="1", MAKEPAD_WINDOW="640x400@1")
    logpath = os.path.join(work, "ui-%s.log" % name)
    log = open(logpath, "w")
    p = subprocess.Popen([relax, "run", "--ahead=minimal", "makepad-example-counter"], env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    res = {"kind": name, "outcome": "-", "effect": "no", "ms": "-", "value": None, "note": ""}
    def logtext():
        return open(logpath, errors="replace").read()
    def snap():
        return inspect(p.pid, tmp, "ui")
    try:
        t = time.time()
        up = False
        while time.time() - t < 400 and p.poll() is None:
            if "watcher gate open" in logtext() and "Increment" in snap():
                up = True
                break
            time.sleep(0.5)
        if not up:
            res["outcome"] = "no-start"
            res["note"] = logtext()[-200:].replace("\n", " ")
            return res
        for _ in range(spec.get("before_click", 0)):
            click(p.pid, tmp, "increment_button")
        if spec.get("before_click") and not wait_for(lambda: "Count: %d" % spec["before_click"] in snap(), 10):
            res["note"] = "the click before the edit did not count"
            return res
        n_log = len(logtext())
        for rel, old, new in edits:
            path = os.path.abspath(rel)
            text = open(path).read()
            assert old in text, (name, old)
            open(path, "w").write(text.replace(old, new, 1))
        t_save = time.time()
        if spec.get("after"):
            time.sleep(spec["after"])
        def seen():
            if spec.get("click") and time.time() - t_save > spec.get("after", 0):
                cur = snap()
                if "Count: 0" in cur:
                    click(p.pid, tmp, "increment_button")
            cur = snap()
            return all(n in cur for n in spec["expect"])
        # (the process is the same pid across an in-place restart: exec, or a domain relaunch)
        if wait_for(seen, WAIT):
            res["effect"] = "yes"
            res["ms"] = "%.0f" % ((time.time() - t_save) * 1e3)
        e = logtext()[n_log:]
        res["value"] = "+".join(spec["expect"]) if res["effect"] == "yes" else "unchanged"
        res["outcome"] = "restarted" if "restarting" in e else "patched" if ": patched (" in e else "refused" if "NOT applied" in e else "silent"
        res["note"] = "; ".join(l.strip()[:140] for l in e.splitlines() if "live design" in l or "restart" in l)[:220]
        if os.environ.get("LIVE_KINDS_VERBOSE"):
            sys.stderr.write("---- %s log\n%s\n" % (name, e[-4000:]))
        return res
    finally:
        for rel, text in originals.items():
            open(os.path.abspath(rel), "w").write(text)
        try:
            os.killpg(p.pid, 9)
        except ProcessLookupError:
            pass
        p.wait()

def wait_for(pred, secs):
    t = time.time()
    while time.time() - t < secs:
        if pred():
            return True
        time.sleep(0.1)
    return False

def run_kind(slot, kind):
    name, key, edits, accept = kind
    if key is None:
        return run_ui(slot, kind)
    work = os.path.join(root, "w%d" % slot)
    pkg = os.path.join(work, "pkg")
    if os.path.isdir(pkg):
        shutil.rmtree(pkg)
    shutil.copytree(PKG, pkg)
    ready = os.path.join(work, "ready")
    if os.path.exists(ready):
        os.remove(ready)
    home = os.path.join(work, "home")
    env = dict(os.environ, MAKEPAD_RUN_LIVE_READY=ready, MAKEPAD_RUN_HOME=home, MAKEPAD_RUN_CACHE_DIR=os.path.join(home, "cache"),
               MAKEPAD_RUN_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_HIDE_WINDOWS="1", SANDBOX_MUTE="1", MAKEPAD_MUTE="1",
               LIVE_KINDS_SECS=str(int(WAIT) + 120), MAKEPAD_RUN_LIVE_TRACE="1")
    p = subprocess.Popen([relax, "run", "--quiet", "--ahead=minimal", "relax-live-kinds"], cwd=pkg, env=env,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    out, err = [], []
    def pump(f, acc):
        for line in f:
            acc.append((time.time(), line.rstrip("\n")))
    for f, acc in ((p.stdout, out), (p.stderr, err)):
        threading.Thread(target=pump, args=(f, acc), daemon=True).start()
    def wait(pred, secs):
        t = time.time()
        while time.time() - t < secs:
            if pred():
                return True
            if p.poll() is not None:
                return pred()
            time.sleep(0.02)
        return False
    ticks = lambda: [l for _, l in out if l.startswith("out ") and not l.startswith("out restored")]
    res = {"kind": name, "outcome": "-", "effect": "no", "ms": "-", "value": None, "note": ""}
    # the watcher applies saves once its startup gate opens (MAKEPAD_RUN_LIVE_TRACE says when)
    started = wait(lambda: os.path.exists(ready) and len(ticks()) >= 3 and any("watcher gate open" in l for _, l in err), 300)
    if not started:
        res["outcome"] = "no-start"
        res["note"] = (err[-1][1] if err else "")[:160]
    else:
        base = probe(ticks()[-1], key)
        for rel, old, new in edits:
            path = os.path.join(pkg, rel)
            text = open(path).read()
            assert old in text, (name, old)
            open(path, "w").write(text.replace(old, new, 1))
        for rel, _, _ in edits:
            path = os.path.join(pkg, rel)
            st = os.stat(path)
            os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))
        t_save = time.time()
        n_out, n_err = len(out), len(err)
        def hit():
            for t, l in out[n_out:]:
                if l.startswith("out ") and not l.startswith("out restored"):
                    v = probe(l, key)
                    if v is not None and (v in accept if isinstance(accept, set) else v >= accept[0]):
                        res["value"] = v
                        res["ms"] = "%.0f" % ((t - t_save) * 1e3)
                        return True
            return False
        seen = wait(hit, WAIT)
        res["effect"] = "yes" if seen else "no"
        if not seen:
            vals = [probe(l, key) for _, l in out[n_out:] if l.startswith("out ") and not l.startswith("out restored")]
            res["value"] = vals[-1] if vals else base
        e = [l for _, l in err[n_err:]]
        restored = any(l.startswith("out restored") for _, l in out[n_out:])
        if restored:
            res["outcome"] = "restarted"
        elif any(": patched (" in l for l in e):
            res["outcome"] = "patched"
        elif any("NOT applied" in l for l in e):
            res["outcome"] = "refused"
        elif any("restart" in l for l in e):
            res["outcome"] = "pending"
        else:
            res["outcome"] = "silent"
        why = [l.replace("makepad live:", "").strip() for l in e if l.startswith("makepad live:   ") or "note:" in l or "kept:" in l]
        res["note"] = "; ".join(why)[:220]
        if os.environ.get("LIVE_KINDS_VERBOSE"):
            sys.stderr.write("---- %s stderr\n%s\n" % (name, "\n".join(l for _, l in err[0 if os.environ["LIVE_KINDS_VERBOSE"] == "2" else n_err:] if "watch round" not in l)[-6000:]))
    try:
        os.killpg(p.pid, 9)
    except ProcessLookupError:
        pass
    p.wait()
    return res

root = tempfile.mkdtemp(prefix="live-kinds-")
results = [None] * len(KINDS)
lock = threading.Lock()
queue = list(range(len(KINDS)))
def worker(slot):
    while True:
        with lock:
            if not queue:
                return
            i = queue.pop(0)
        results[i] = run_kind(slot, KINDS[i])
        r = results[i]
        with lock:
            print("%-28s %-10s %-4s %6s  value=%s  %s" % (r["kind"], r["outcome"], r["effect"], r["ms"], r["value"], r["note"]), flush=True)
threads = [threading.Thread(target=worker, args=(s,)) for s in range(max(1, jobs))]
for t in threads:
    t.start()
for t in threads:
    t.join()
print()
print("| edit kind | outcome | effect seen | ms | note |")
print("|---|---|---|---|---|")
for r in results:
    print("| %s | %s | %s | %s | %s |" % (r["kind"], r["outcome"], r["effect"], r["ms"], r["note"].replace("|", "/")))
landed = sum(1 for r in results if r["effect"] == "yes")
print("live_kinds: %d/%d landed" % (landed, len(results)))
shutil.rmtree(root, ignore_errors=True)
sys.exit(0 if landed == len(results) else 1)
