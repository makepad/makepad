#!/usr/bin/env python3
"""Live patches always land: one edit per edit kind against a running program, visible effect asserted.

usage: live_kinds.py [--only k1,k2] [--jobs N]
  (the JIT runs through `cargo run --profile host`; MAKEPAD_RUN_STD in the environment, default
   libs/relax/compiler/std; run from the workspace root)

For each kind: copy live_kinds/ to a worker dir, `makepad run --ahead=minimal relax-live-kinds` (a working
tree: live, hidden, muted, own MAKEPAD_RUN_HOME), wait for the live watcher (MAKEPAD_RUN_LIVE_READY) and
the first tick, save the kind's edit and wait for the probe's printed value to become the expected one.
The outcome is how the edit landed: patched (installed live), restarted (the program printed
"out restored": a state-preserving restart in place), refused, or pending (a restart nobody made).
The verdict is the user's rule, "restart yes, update yes, crash no". An edit that does not take effect
live gets the restart button (`session relaunch`) and passes when the restarted app shows the new value;
a kind fails on no effect after that, or on a crash, ICE, panic or parked thread at any point. An edit
rustc rejects (accepted: KEEPS) passes when the app keeps running its old code with nothing wrong.
partial_landing uses the fixture's native window and must patch automatically, clear its refusal,
and leave session view over_app false; it never requests a relaunch.
Prints the table `kind | outcome | effect | ms` and `live_kinds: N/M landed`; exit 1 on any miss.
"""
import json, os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.join(HERE, "live_kinds")
args = sys.argv[1:]
def opt(name, default):
    return args[args.index(name) + 1] if name in args else default
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.abspath("Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
KEEPS = "keeps"
BAD = ("panicked", "panic parked", "stale access parked", "internal compiler error", "FATAL", "the watcher stopped")
def wrong(lines):
    """The first line that says the app or the JIT broke."""
    return next((l.strip()[:200] for l in lines if any(b in l for b in BAD)), None)
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
    # a new fn with an impl Trait argument (a generic its source never names: screencap_dir_in, 10-31)
    ("fn_added_impl_arg", "fn_added", [(K, "pub fn newfn_val() -> u64 { 0 }", "fn added(starts: impl IntoIterator<Item = u64>) -> Option<u64> {\n    for start in starts {\n        return Some(start);\n    }\n    None\n}\npub fn newfn_val() -> u64 { added([42u64].into_iter()).unwrap_or(0) }")], {42}),
    ("fn_added_generic", "fn_added", [(K, "pub fn newfn_val() -> u64 { 0 }", "fn added<I: IntoIterator<Item = u64>>(starts: I) -> Option<u64> { starts.into_iter().next() }\npub fn newfn_val() -> u64 { added([42u64]).unwrap_or(0) }")], {42}),
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
# one edit per language construct: added, changed, removed
KINDS += [
    ("apit_body", "apit", [(K, "it.into_iter().sum() }\npub fn apit_val", "it.into_iter().sum::<u64>() + 1 }\npub fn apit_val")], {4}),
    ("apit_param_added", "apit2", [(K, "pub fn apit2(a: u64) -> u64 { a }", "pub fn apit2(a: u64, it: impl IntoIterator<Item = u64>) -> u64 { a + it.into_iter().sum::<u64>() }"), (K, "{ apit2(1) }", "{ apit2(1, [2, 3]) }")], {6}),
    ("apit_asref_added", "apit3", [(K, "pub fn apit3_val() -> u64 { 0 }", "fn len_of(p: impl AsRef<str>) -> u64 { p.as_ref().len() as u64 }\npub fn apit3_val() -> u64 { len_of(\"abc\") }")], {3}),
    ("generic_param_added", "gen2", [(K, "pub fn gen2<A: Into<u64>>(a: A) -> u64 { a.into() }", "pub fn gen2<A: Into<u64>, B: Into<u64>>(a: A, b: B) -> u64 { a.into() + b.into() }"), (K, "{ gen2(1u32) }", "{ gen2(1u32, 2u8) }")], {3}),
    ("const_generic_arg", "cgen", [(K, "{ Arr::<2>::len() }", "{ Arr::<3>::len() }")], {3}),
    ("generic_struct_added", "gstruct", [(K, "pub fn gstruct_val() -> u64 { 0 }", "struct W<T>(T);\nimpl<T: Copy + Into<u64>> W<T> { fn get(&self) -> u64 { self.0.into() } }\npub fn gstruct_val() -> u64 { W(9u32).get() }")], {9}),
    ("trait_added_blanket", "tblanket", [(K, "pub fn tblanket_val() -> u64 { 0 }", "trait Dbl { fn dbl(&self) -> u64; }\nimpl<T: Copy + Into<u64>> Dbl for T { fn dbl(&self) -> u64 { (*self).into() * 2 } }\npub fn tblanket_val() -> u64 { 4u32.dbl() }")], {8}),
    ("trait_removed", "trm", [(K, "pub trait Gone { fn g(&self) -> u64; }\nimpl Gone for u8 { fn g(&self) -> u64 { 6 } }\n", ""), (K, "{ 1u8.g() }", "{ 7 }")], {7}),
    ("trait_default_method", "tdef", [(K, "fn d(&self) -> u64 { 1 } }", "fn d(&self) -> u64 { 2 } }")], {2}),
    ("assoc_const", "aconst", [(K, "const V: u64 = 1;", "const V: u64 = 2;")], {2}),
    ("assoc_type", "atype", [(K, "type Out = u32;", "type Out = u8;")], {1}),
    ("closure_capture_added", "ccap", [(K, "let a = 1u64; let f = || a; f()", "let a = 1u64; let b = 2u64; let f = || a + b; f()")], {3}),
    ("closure_nested", "cnest", [(K, "(|| (|| 1u64)())()", "(|| (|| 2u64)())()")], {2}),
    ("rpit_body", "rpit", [(K, "(0..2u64).map(|x| x)", "(0..3u64).map(|x| x)")], {3}),
    ("static_added", "sadd", [(K, "pub fn sadd_val() -> u64 { 0 }", "static NEW_S: u64 = 12;\npub fn sadd_val() -> u64 { NEW_S }")], {12}),
    ("static_removed", "srm", [(K, "static OLD_S: u64 = 13;\n", ""), (K, "{ OLD_S }", "{ 14 }")], {14}),
    ("const_added", "cadd", [(K, "pub fn cadd_val() -> u64 { 0 }", "const NEW_C: u64 = 15;\npub fn cadd_val() -> u64 { NEW_C }")], {15}),
    ("const_removed", "crm", [(K, "const OLD_C: u64 = 16;\n", ""), (K, "{ OLD_C }", "{ 17 }")], {17}),
    ("const_array_len", "clen", [(K, "const LEN: usize = 2;", "const LEN: usize = 3;")], {3}),
    ("macro_added", "madd", [(K, "pub fn madd_val() -> u64 { 0 }", "macro_rules! m2 { ($e:expr) => { $e + 1 } }\npub fn madd_val() -> u64 { m2!(17) }")], {18}),
    ("macro_removed", "mrm", [(K, "macro_rules! m3 { () => { 19 } }\n", ""), (K, "{ m3!() }", "{ 20 }")], {20}),
    ("macro_makes_fn", "mfn", [(K, "pub fn mfn_val() -> u64 { 0 }", "macro_rules! mk { ($n:ident, $v:expr) => { fn $n() -> u64 { $v } } }\nmk!(made, 21);\npub fn mfn_val() -> u64 { made() }")], {21}),
    ("module_removed", "modrm", [(K, "mod oldm { pub fn v() -> u64 { 22 } }\n", ""), (K, "{ oldm::v() }", "{ 23 }")], {23}),
    ("enum_variant_removed", "evrm", [(K, "enum E2 { A, B }", "enum E2 { A }"), (K, "E2::A => 1, E2::B => 2 }", "E2::A => 2 }")], {2}),
    ("enum_payload_changed", "epay", [(K, "enum E3 { A(u8), B }", "enum E3 { A(u64), B }"), (K, "E3::A(1).v()", "E3::A(300).v()")], {300}),
    ("enum_discriminant", "edis", [(K, "enum E4 { A = 1 }", "enum E4 { A = 2 }")], {2}),
    ("derive_added", "dadd", [(K, "struct D1;\npub fn dadd_val() -> u64 { 0 }", "#[derive(Debug)]\nstruct D1;\npub fn dadd_val() -> u64 { format!(\"{:?}\", D1).len() as u64 }")], {2}),
    ("derive_removed", "drm", [(K, "#[derive(Clone)]\nstruct D2(u64);", "struct D2(u64);"), (K, "D2(3).clone().0", "D2(4).0")], {4}),
    ("cfg_attr_enabled", "cfg1", [(K, "#[cfg(any())]\npub fn cfg_on() -> u64 { 2 }\n#[cfg(not(any()))]\npub fn cfg_on() -> u64 { 1 }", "#[cfg(not(any()))]\npub fn cfg_on() -> u64 { 2 }\n#[cfg(any())]\npub fn cfg_on() -> u64 { 1 }")], {2}),
    ("cfg_in_body", "cfg2", [(K, "#[cfg(any())] { n = 2; }", "#[cfg(not(any()))] { n = 2; }")], {2}),
    ("async_body", "async1", [(K, "async fn af() -> u64 { 1 }", "async fn af() -> u64 { 2 }")], {2}),
    ("async_fn_added", "async2", [(K, "pub fn async2_val() -> u64 { 0 }", "async fn newaf(a: u64) -> u64 { af2(a).await + 1 }\nasync fn af2(a: u64) -> u64 { a }\npub fn async2_val() -> u64 { block_on(newaf(3)) }")], {4}),
    ("drop_impl_added", "dropi", [(K, "pub fn dropi_val() -> u64 { 0 }", "struct Dp<'a>(&'a Cell<u64>);\nimpl Drop for Dp<'_> { fn drop(&mut self) { self.0.set(5); } }\npub fn dropi_val() -> u64 { let c = Cell::new(0); { let _d = Dp(&c); } c.get() }")], {5}),
    ("lifetime_param_added", "life", [(K, "pub fn life_of(a: &u64) -> u64 { *a }", "pub fn life_of<'a>(a: &'a u64) -> u64 { *a + 1 }")], {2}),
    ("extern_c_fn_ptr", "ecfn", [(K, 'extern "C" fn ec() -> u64 { 1 }', 'extern "C" fn ec() -> u64 { 2 }')], {2}),
    ("fn_ptr_table", "fptr", [(K, "fn tbl_a() -> u64 { 1 }", "fn tbl_a() -> u64 { 2 }")], {2}),
    ("mod_file_added", "mfile", [("src/extra_mod.rs", None, "pub fn v() -> u64 { 24 }\n"), (K, "pub fn mfile_val() -> u64 { 0 }", "#[path = \"extra_mod.rs\"] mod extra_mod;\npub fn mfile_val() -> u64 { extra_mod::v() }")], {24}),
    # A landing writes the caller before its new module and declaration. The
    # driver leaves a 300 ms gap after the first edit; no manual relaunch is allowed.
    ("partial_landing", "partial_landing", [
        (K, "pub fn partial_landing_val() -> u64 { 0 }", "pub fn partial_landing_val() -> u64 { crate::landing_module::f() }"),
        ("src/landing_module.rs", None, "pub fn f() -> u64 { 25 }\n"),
        (M, "mod kinds;", "mod kinds;\nmod landing_module;")], {25}),
    # rustc rejects these: the old code runs on, nothing over the app
    ("removed_in_use", "rmuse", [(K, "pub fn in_use() -> u64 { 1 }\n", "")], KEEPS),
    ("type_error", "rmuse", [(K, "pub fn in_use() -> u64 { 1 }", "pub fn in_use() -> u64 { \"x\" }")], KEEPS),
    ("syntax_error", "rmuse", [(K, "pub fn in_use() -> u64 { 1 }", "pub fn in_use() -> u64 { 1 ")], KEEPS),
    ("trait_removed_in_use", "rmuse", [(K, "pub trait Kept { fn k(&self) -> u64; }\nimpl Kept for u16 { fn k(&self) -> u64 { 1 } }\n", "")], KEEPS),
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
    # a busy app (a cold start compiling its UI) may not answer yet: no answer is an empty reply
    try:
        r = subprocess.run(JIT + ["inspect", str(pid)] + list(q), env=dict(os.environ, TMPDIR=tmp), capture_output=True, text=True, timeout=120)
    except subprocess.TimeoutExpired:
        return ""
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
    p = subprocess.Popen(JIT + ["run", "--ahead=minimal", "makepad-example-counter"], env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
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
        offered = lambda: "a restart applies it" in logtext()[n_log:] or "NOT applied" in logtext()[n_log:]
        wait_for(lambda: seen() or offered(), WAIT)
        if not seen() and p.poll() is None:
            res["restart"] = inspect(p.pid, tmp, "session", "relaunch").strip()[:120]
        if wait_for(seen, WAIT):
            res["effect"] = "yes"
            res["ms"] = "%.0f" % ((time.time() - t_save) * 1e3)
        e = logtext()[n_log:]
        res["fail"] = wrong(e.splitlines()) or (None if p.poll() is None else "the app exited (%s)" % p.returncode)
        res["value"] = "+".join(spec["expect"]) if res["effect"] == "yes" else "unchanged"
        res["outcome"] = "restarted" if "restarting" in e or res.get("restart") else "patched" if ": patched (" in e else "refused" if "NOT applied" in e else "silent"
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
    # The copied fixture's optional native window uses this checkout's widgets.
    manifest = os.path.join(pkg, "Cargo.toml")
    widgets = os.path.relpath(os.path.abspath(os.path.join(HERE, "../../../widgets")), pkg)
    text = open(manifest).read().replace('"../../../../widgets"', json.dumps(widgets))
    open(manifest, "w").write(text)
    ready = os.path.join(work, "ready")
    if os.path.exists(ready):
        os.remove(ready)
    home = os.path.join(work, "home")
    env = dict(os.environ, MAKEPAD_RUN_LIVE_READY=ready, MAKEPAD_RUN_HOME=home, MAKEPAD_RUN_CACHE_DIR=os.path.join(home, "cache"),
               MAKEPAD_RUN_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_HIDE_WINDOWS="1", SANDBOX_MUTE="1", MAKEPAD_MUTE="1",
               LIVE_KINDS_SECS=str(int(WAIT) * 3 + 120), MAKEPAD_RUN_LIVE_TRACE="1", MAKEPAD_REMOTE="1")
    features = ["--features", "window"] if name == "partial_landing" else []
    p = subprocess.Popen(JIT + ["run", "--quiet", "--ahead=minimal", "relax-live-kinds"] + features, cwd=pkg, env=env,
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
    if started and name == "partial_landing":
        started = wait(lambda: "Live patch probe" in inspect(p.pid, os.environ.get("TMPDIR", "/tmp"), "ui"), WAIT)
    if not started:
        res["outcome"] = "no-start"
        res["note"] = (err[-1][1] if err else "")[:160]
    else:
        base = probe(ticks()[-1], key)
        t_save = time.time()
        n_out, n_err = len(out), len(err)
        for edit_index, (rel, old, new) in enumerate(edits):
            if name == "partial_landing" and edit_index == 1:
                time.sleep(0.3)
            path = os.path.join(pkg, rel)
            if old is None:
                open(path, "w").write(new)
                continue
            text = open(path).read()
            assert old in text, (name, old)
            open(path, "w").write(text.replace(old, new, 1))
        for rel, _, _ in edits:
            path = os.path.join(pkg, rel)
            st = os.stat(path)
            os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))
        def hit():
            if accept == KEEPS:
                return False
            for t, l in out[n_out:]:
                if l.startswith("out ") and not l.startswith("out restored"):
                    v = probe(l, key)
                    if v is not None and (v in accept if isinstance(accept, set) else v >= accept[0]):
                        res["value"] = v
                        res["ms"] = "%.0f" % ((t - t_save) * 1e3)
                        return True
            return False
        patched_at = []
        def said():
            # the JIT's answer: refused or offered at once; patched gets a moment to show (a loop
            # that never returns shows its edit only after a restart)
            e = [l for _, l in err[n_err:]]
            if any("NOT applied" in l or "a restart applies it" in l for l in e):
                return True
            if any(": patched (" in l for l in e):
                patched_at.append(time.time())
                return time.time() - patched_at[0] > 3
            return False
        seen = wait(lambda: hit() or (name != "partial_landing" and said()), WAIT) and hit()
        e = [l for _, l in err[n_err:]]
        if accept == KEEPS:
            # rustc rejects the edit: the old code runs on, and the probe still ticks
            n = len(ticks())
            seen = said() and wait(lambda: len(ticks()) > n + 5, 10) and probe(ticks()[-1], key) == base
        elif not seen and p.poll() is None and name != "partial_landing":
            res["restart"] = inspect(p.pid, os.environ.get("TMPDIR", "/tmp"), "session", "relaunch").strip()[:120]
            seen = wait(hit, WAIT)
        res["effect"] = "yes" if seen else "no"
        res["fail"] = wrong([l for _, l in err[n_err:]] + [l for _, l in out[n_out:]]) or (None if p.poll() is None else "the app exited (%s)" % p.returncode)
        if not seen:
            vals = [probe(l, key) for _, l in out[n_out:] if l.startswith("out ") and not l.startswith("out restored")]
            res["value"] = vals[-1] if vals else base
        e = [l for _, l in err[n_err:]]
        restored = any(l.startswith("out restored") for _, l in out[n_out:])
        if name == "partial_landing":
            # Historical refusals remain in the log, but the final verdict and
            # the run's current notice must both clear without any user action.
            verdicts = [l for l in e if "NOT applied" in l or ": patched (" in l]
            if not verdicts or ": patched (" not in verdicts[-1]:
                res["fail"] = res["fail"] or "a NOT applied verdict was left standing"
            if restored:
                res["fail"] = res["fail"] or "the partial landing restarted instead of patching"
            view = []
            def view_clear():
                view[:] = [inspect(p.pid, os.environ.get("TMPDIR", "/tmp"), "session", "view", "as=json")]
                try:
                    reply = view[0]
                    state = json.loads(reply[reply.index("{"):reply.rindex("}") + 1])
                    return state.get("over_app") is False and state.get("patch_failed") is False and state.get("patch_notice") is None
                except (ValueError, AttributeError, IndexError):
                    return False
            if not wait(view_clear, WAIT):
                res["fail"] = res["fail"] or "session view did not clear: " + (view[0].strip() if view else "no reply")
            if any("the run's view is over the app" in l for _, l in err[n_err:]):
                res["fail"] = res["fail"] or "a view covered the app during the partial landing"
        if restored or res.get("restart"):
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
landed = sum(1 for r in results if r["effect"] == "yes" and not r.get("fail"))
for r in results:
    if r.get("fail"):
        print("live_kinds FAIL %s: %s" % (r["kind"], r["fail"]))
print("live_kinds: %d/%d landed" % (landed, len(results)))
shutil.rmtree(root, ignore_errors=True)
# the app fixtures that need the disk cache: code from a run before the patch
subprocess.run(["cargo", "build", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad"], check=True)
binary = os.path.join(os.environ.get("CARGO_TARGET_DIR", "target"), "host", "makepad")
warm = subprocess.run([sys.executable, "libs/relax/compiler/tests/live_closure_owner_app/run.py", "--makepad", binary, "--warm"], capture_output=True, text=True)
print(warm.stdout[-3000:], flush=True)
sys.exit(0 if landed == len(results) and warm.returncode == 0 else 1)
