#!/usr/bin/env python3
"""An edit of a const or a static reaches a program whose code came from the disk cache: its readers,
read directly or through other stored code, show the new value, or the edit asks for a restart. And
a new run of the edited sources shows the new value. "Patched" with the old value is the one wrong
answer, and a warm run with the old value the other.

usage: run.py --makepad <makepad binary> [--only p1,p2] [--cold] [--window]   (RELAX_STD in the environment)

Copies pkg/ to a temporary dir with a fresh disk cache and fills it (a run, then a warm run that must
prelink). Then each probe gets a live session of its own (`makepad run` of a working tree), started
warm: one edit, and within 20 s either
  landed:   the program prints the new value, or
  restart:  the JIT offers a restart and the program keeps its old value, and `session relaunch`
            brings the new value,
as the save's verdict says: its event (patchverified or patchrefused) and the restarts it left
waiting (a save that patches its other items and holds its restart-only ones back for a restart),
and in both a fresh run of the edited sources (read-only on the cache) prints the new value.
A probe that reads late first calls its reader after the edit: that code comes from the store then.
A const that only values read must land, and a const a type uses must restart, saying why (MUST).
const_every: one const of a dependency, every way code reads it, all from the warm cache.
field_*: a struct's field list edited with no reader's text changing; field_every reads one field every way.
A struct whose fields move takes its new layout where no running value of it is left: lands (MUST); where a
static holds one (field_every, field_widen) or a frame may (field_running_frame, field_blocked_frame,
field_short_walk) the edit restarts (MUST), naming the static or the running fn; the relaunch reads it new.
dyn_*: a trait object made before the edit, its trait's method list edited: the edit restarts (MUST),
its vtable linked from the disk cache as much as built by this run; dyn_impl_removed: the impl the
object's vtable holds removed with the code that made it changed: lands (MUST), the object keeps its vtable.
enum_insert*: a variant inserted before a held one: lands (MUST); restarts (MUST) when code that ran read
the numbers it moves, an `as` cast or a derived `Hash`, linked from the disk cache as much as built by this run.
enum_chan: a variant that grows an enum a channel carries, a static able to hold a sender of it: restarts
(MUST), its values' types laid out from the code the disk cache linked as much as from what this run built.
field_dyn: a struct only a trait object holds (a static's `Box<dyn ..>`): restarts (MUST), the object's vtable
linked from the disk cache as much as built by this run. field_dyn_closure: one a closure's trait object holds:
restarts (MUST), naming the closure by the fn that makes it and its line. field_spawn: one a live worker
thread's closure took: restarts (MUST), naming the program's closure, passed to std's spawn (not std's closure).
field_closure_box: one a closure made in one fn and called through its box from another captures two fields of:
lands (MUST), and the fresh run reads the new value (the closure's environment is the whole struct).
field_copy: a field added to a Copy struct no running value holds: lands (MUST) in its new layout, and the reader
whose text stays (it builds an outer struct holding it) is rebuilt for it. field_copy_frame: one the loop's frame
holds restarts (MUST), naming the running fn.
A stored entry can be older than the manifest of the run that reads it: the edited text's first run,
reaching none of the late readers, writes its own manifest (its keys are "unchanged" from then on),
and the next run's first late reads must still see that the entries they find are of the old text.
fewer_definitions: a session whose crate has fewer definitions than the cache's image (a module the
fill had is gone): the image's types still name the old crate's, past the program's; edits land.
const_skipped, skipped_offline: a module the boot skips (RELAX_LAZY_DEFER, every run) edited,
live or between runs: the new text's runs show the new value.
stale_*: the stale direction, code that holds a copy of what an edit changes where the edited item's own
code is no reader of it (src/stale.rs, src/stale_frames.rs). It lands (MUST) where that code is rebuilt:
a const only an always-inlined fn reads, held by its looping host; a callee edited to unwind, its caller's
guard running as the panic passes (once the session reads late: no watcher attached); a callee edited to keep the pointer it is given, its caller reading its
local again; and a thread's loop host with a leaf copy, entered before its tier-up (the session asks for its
O3 build before the edit), whose frame runs the version the tier-up retired. It restarts (MUST) where a
thread's frame keeps the copy for good: a reader with its own loop inlined into a thread's closure, a reader
blocked in a call inside its copy. An associated const read by a std instance's loop lands or restarts, as
the instance took the reader in or not, never stale.
gate_*: the digest gate's count of an edit (it decides nothing yet), the edit const_direct's: what it
must not misread (GATE). gate_closure_kept: a closure of the edited file the edit leaves alone keeps its
identity, so std's spawn over it reads the same across the commit, as its drop glue does, whether
that glue's key came from the startup image or was encoded after the commit. gate_install_digest:
code the first edit's patch built into a slot that held no RIR (a reader the const's change rebuilt)
carries its digest to the install, so the gate reads it, not "new code has no digest".
const_held: the const_direct edit, the session held past the landing check (a second after the
install, HOLD): field_spawn's worker, a thread spawned with a closure of the edited file, runs on in
std's run_main over that closure, which the edit leaves alone: it must not restart the app.
--cold: the same sessions with the disk cache off, for comparison.
--window: the same program with a window (hidden), so a restart keeps the process and the window: the
new value must show in the same pid.
Prints a row per probe and exits 1 on any that failed, its last line
`cache_warm_const_edit: ok` or `cache_warm_const_edit: <n> failed: <row id> ...`.
"""
import json, os, re, shutil, signal, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
relax = os.path.abspath(args[args.index("--makepad") + 1])
only = args[args.index("--only") + 1].split(",") if "--only" in args else None
cold = "--cold" in args
window = "--window" in args
ROOT = os.path.abspath(os.path.join(HERE, "../../../.."))
V = "src/values.rs"
F = "src/fields.rs"
M = "src/main.rs"
S = "src/stale.rs"
SF = "src/stale_frames.rs"
# probe -> (the printed name, file, old text, new text, the value it prints after the edit)
PROBES = [
    ("const_direct", V, "CONST_DIRECT: u64 = 1;", "CONST_DIRECT: u64 = 2;", 2),
    ("const_through", V, "CONST_THROUGH: u64 = 1;", "CONST_THROUGH: u64 = 2;", 2),
    ("gate_closure_kept", V, "CONST_DIRECT: u64 = 1;", "CONST_DIRECT: u64 = 2;", 2),
    ("gate_install_digest", V, "CONST_DIRECT: u64 = 1;", "CONST_DIRECT: u64 = 2;", 2),
    ("const_held", V, "CONST_DIRECT: u64 = 1;", "CONST_DIRECT: u64 = 2;", 2),
    ("const_inlined", V, "CONST_INLINED: u64 = 1;", "CONST_INLINED: u64 = 2;", 2),
    ("const_derived", V, "CONST_BASE: u64 = 1;", "CONST_BASE: u64 = 2;", 4),
    ("const_assoc", V, "for A {\n    const K: u64 = 1;", "for A {\n    const K: u64 = 2;", 2),
    ("const_assoc_named", V, "for B {\n    const K: u64 = 1;", "for B {\n    const K: u64 = 2;", 2),
    ("const_assoc_foreign", V, "for warm_const_dep::DepT {\n    const K: u64 = 1;", "for warm_const_dep::DepT {\n    const K: u64 = 2;", 2),
    ("const_assoc_dep_generic", V, "const DK: u64 = 1;", "const DK: u64 = 2;", 2),
    ("const_assoc_wrapped", V, "for Vec<W3> {\n    const K: u64 = 1;", "for Vec<W3> {\n    const K: u64 = 2;", 2),
    ("const_assoc_prim", V, "for u32 {\n    const K: u64 = 1;", "for u32 {\n    const K: u64 = 2;", 2),
    ("const_assoc_blanket", V, "const BL: u64 = 1;", "const BL: u64 = 2;", 2),
    ("const_assoc_closure", V, "for Cl {\n    const K: u64 = 1;", "for Cl {\n    const K: u64 = 2;", 2),
    ("const_assoc_chain", V, "for A4 {\n    const K: u64 = 1;", "for A4 {\n    const K: u64 = 2;", 3),
    ("const_opaque", V, "for Op {\n    const K: u64 = 1;", "for Op {\n    const K: u64 = 2;", 2),
    ("const_inferred", V, "for Inf {\n    const K: u64 = 1;", "for Inf {\n    const K: u64 = 2;", 2),
    ("trait_default", V, "const D: u64 = 1;", "const D: u64 = 2;", 2),
    ("method_default", V, "fn m() -> u64 {\n        1\n", "fn m() -> u64 {\n        2\n", 2),
    ("type_assoc", V, "for C {\n    type Out = u8;", "for C {\n    type Out = u16;", 2),
    ("type_assoc_field", V, "for C2 {\n    type Out = u8;", "for C2 {\n    type Out = u16;", 2),
    ("const_generic", V, "CONST_GEN: usize = 1;", "CONST_GEN: usize = 2;", 2),
    ("const_generic_assoc", V, "CONST_GEN_TY: usize = 1;", "CONST_GEN_TY: usize = 2;", 2),
    ("const_array_arg", V, "CONST_ARR: usize = 1;", "CONST_ARR: usize = 2;", 2),
    ("const_array_assoc", V, "for D1 {\n    const LK: u64 = 1;", "for D1 {\n    const LK: u64 = 2;", 2),
    ("const_array_ty", V, "CONST_ARR_TY: usize = 1;", "CONST_ARR_TY: usize = 2;", 2),
    ("impl_added", V, "// impl_added\n", "impl Drop for Plain {\n    fn drop(&mut self) {}\n}\n", 1),
    ("impl_removed", V, "impl Drop for Droppy {\n    fn drop(&mut self) {}\n}\n", "", 0),
    ("impl_added_glue", V, "// impl_added_glue\n", "impl Drop for Glue {\n    fn drop(&mut self) {\n        GLUE_DROPS.store(1, std::sync::atomic::Ordering::Relaxed);\n    }\n}\n", 1),
    # a marker impl: no call reaches it, but it picks which of std's specializing impls an instance takes
    ("impl_added_spec", V, "// impl_added_spec\n", "impl Copy for Sp {}\n", 1),
    ("const_macro", V, "CONST_MACRO: u64 = 1;", "CONST_MACRO: u64 = 2;", 2),
    ("const_macro_same", M, "CONST_MACRO_SAME: u64 = 1;", "CONST_MACRO_SAME: u64 = 2;", 2),
    ("const_pattern", V, "CONST_PATTERN: u64 = 1;", "CONST_PATTERN: u64 = 2;", 2),
    ("const_len", V, "CONST_LEN: usize = 1;", "CONST_LEN: usize = 2;", 2),
    ("const_dep", "dep/src/lib.rs", "DEP_VALUE: u64 = 1;", "DEP_VALUE: u64 = 2;", 2),
    ("const_other_new", V, "CONST_FOR_NEW: u64 = 1;", "CONST_FOR_NEW: u64 = 2;", 2),
    ("static_through_new", V, "CONST_THROUGH_NEW: u64 = 1;", "CONST_THROUGH_NEW: u64 = 2;", 2),
    ("const_array_fn", V, "CONST_LEN_FN: usize = 1;", "CONST_LEN_FN: usize = 2;", 2),
    ("const_array_macro", V, "CONST_LEN_MAC: usize = 1;", "CONST_LEN_MAC: usize = 2;", 2),
    ("const_array_alias", V, "CONST_LEN_AL: usize = 1;", "CONST_LEN_AL: usize = 2;", 2),
    ("const_dep_array", "dep/src/lib.rs", "DEP_LEN: usize = 1;", "DEP_LEN: usize = 2;", 2),
    ("const_dep_generic", "dep/src/lib.rs", "DEP_GEN: usize = 1;", "DEP_GEN: usize = 2;", 2),
    ("const_dep_generic_field", "dep/src/lib.rs", "DEP_FIELD: usize = 1;", "DEP_FIELD: usize = 2;", 2),
    ("const_dep_generic_alias", "dep/src/lib.rs", "DEP_ALIAS: usize = 1;", "DEP_ALIAS: usize = 2;", 2),
    ("const_dep_in_main_type", "dep/src/lib.rs", "DEP_FOR_MAIN: usize = 1;", "DEP_FOR_MAIN: usize = 2;", 2),
    ("const_same", V, "CONST_SAME: u64 = 1;", "CONST_SAME: u64 = 2;", 2),
    ("const_include", "src/data.txt", "x", "xx", 2),
    ("body_include", "src/body.txt", "y", "yy", 2),
    ("const_late", V, "CONST_LATE: u64 = 1;", "CONST_LATE: u64 = 2;", 2),
    ("body_inlined", V, "pub fn tiny() -> u64 {\n    1\n", "pub fn tiny() -> u64 {\n    2\n", 2),
    ("body_late", V, "pub fn late_tiny() -> u64 {\n    1\n", "pub fn late_tiny() -> u64 {\n    2\n", 2),
    ("always_late", V, "pub fn late_always() -> u64 {\n    1\n", "pub fn late_always() -> u64 {\n    2\n", 2),
    ("dyn_method_add", V, "pub trait Dm {\n    fn a(&self) -> u64;\n}\npub struct DmX;\nimpl Dm for DmX {\n    fn a(&self) -> u64 {\n        1\n", "pub trait Dm {\n    fn z(&self) -> u64 {\n        9\n    }\n    fn a(&self) -> u64;\n}\npub struct DmX;\nimpl Dm for DmX {\n    fn a(&self) -> u64 {\n        2\n", 2),
    ("dyn_method_add_late", V, "pub trait Dm {\n    fn a(&self) -> u64;\n}\npub struct DmX;\nimpl Dm for DmX {\n    fn a(&self) -> u64 {\n        1\n", "pub trait Dm {\n    fn z(&self) -> u64 {\n        9\n    }\n    fn a(&self) -> u64;\n}\npub struct DmX;\nimpl Dm for DmX {\n    fn a(&self) -> u64 {\n        2\n", 2),
    ("dyn_vtable_fresh", V, "pub trait Dv {\n    fn a(&self) -> u64;\n}\npub struct DvX;\nimpl Dv for DvX {\n    fn a(&self) -> u64 {\n        1\n    }\n}\npub fn dv_read() -> u64 {\n    dv_held().a()\n}\n", "pub trait Dv {\n    fn z(&self) -> u64 {\n        9\n    }\n    fn a(&self) -> u64;\n}\npub struct DvX;\nimpl Dv for DvX {\n    fn a(&self) -> u64 {\n        2\n    }\n}\npub fn dv_read() -> u64 {\n    dv_held().a() * 10 + dv_held().z() - 9\n}\n", 20),
    ("dyn_impl_removed", V, "impl std::fmt::Display for Ri {\n    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {\n        write!(f, \"{}\", self.0)\n    }\n}\npub const RI_K: u64 = 0;\npub fn ri_make() -> Box<dyn std::fmt::Display + Send + Sync> {\n    Box::new(Ri(7))\n", "pub const RI_K: u64 = 1;\npub fn ri_make() -> Box<dyn std::fmt::Display + Send + Sync> {\n    Box::new(7u64)\n", 2),
    ("field_copy", V, "pub struct Cp {\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cp_make(i: u32) -> Cp {\n    Cp { a: i, b: (i as u64) << 8 }\n", "pub struct Cp {\n    pub a: u32,\n    pub c: u16,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cp_make(i: u32) -> Cp {\n    Cp { a: i, c: 3, b: (i as u64) << 9 }\n", 1540),
    ("field_dyn", V, "pub struct DjS {\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn dj_new() -> Box<dyn Dj + Send> {\n    Box::new(DjS { a: 1, b: 1 })", "pub struct DjS {\n    pub z: u8,\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn dj_new() -> Box<dyn Dj + Send> {\n    Box::new(DjS { z: 0, a: 1, b: 2 })", 3),
    ("field_dyn_closure", V, "pub struct DcS {\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn dc_new() -> Box<dyn Fn() -> u64 + Send> {\n    let s = DcS { a: 1, b: 1 };", "pub struct DcS {\n    pub z: u8,\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn dc_new() -> Box<dyn Fn() -> u64 + Send> {\n    let s = DcS { z: 0, a: 1, b: 2 };", 3),
    ("field_closure_box", V, "pub struct CbS {\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cb_new() -> Box<dyn Fn() -> u64 + Send> {\n    let s = CbS { a: 1, b: 1 };", "pub struct CbS {\n    pub z: u8,\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cb_new() -> Box<dyn Fn() -> u64 + Send> {\n    let s = CbS { z: 0, a: 1, b: 2 };", 3),
    ("field_spawn", V, "pub struct SpS {\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\nfn sp_start() -> std::sync::mpsc::Sender<std::sync::mpsc::Sender<u64>> {\n    let s = SpS { a: 1, b: 1 };", "pub struct SpS {\n    pub z: u8,\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\nfn sp_start() -> std::sync::mpsc::Sender<std::sync::mpsc::Sender<u64>> {\n    let s = SpS { z: 0, a: 1, b: 2 };", 3),
    ("field_copy_frame", V, "pub struct CpF {\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cpf_new() -> CpF {\n    CpF { a: 1, b: 1 }\n", "pub struct CpF {\n    pub z: u8,\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cpf_new() -> CpF {\n    CpF { z: 0, a: 1, b: 2 }\n", 3),
    ("enum_insert", V, "pub enum Ev {\n    A,\n    B(u32),\n    C { level: u8 },\n}\npub static EV: std::sync::Mutex<Option<Ev>> = std::sync::Mutex::new(None);\n#[inline(never)]\npub fn ev_read() -> u64 {\n    let mut g = EV.lock().unwrap();\n    let held = g.get_or_insert(Ev::C { level: 4 });\n    let level = match held {\n        Ev::C { level } => *level as u64,\n        _ => 0,\n    };\n    level + if Ev::A < *held { 10 } else { 0 }\n}\n", "pub enum Ev {\n    A,\n    X(u16),\n    B(u32),\n    C { level: u8 },\n}\npub static EV: std::sync::Mutex<Option<Ev>> = std::sync::Mutex::new(None);\n#[inline(never)]\npub fn ev_read() -> u64 {\n    let mut g = EV.lock().unwrap();\n    let held = g.get_or_insert(Ev::C { level: 4 });\n    let level = match held {\n        Ev::C { level } => *level as u64,\n        _ => 0,\n    };\n    level + if Ev::A < *held { 10 } else { 0 } + if Ev::X(3) < *held && Ev::A < Ev::X(3) { 300 } else { 0 }\n}\n", 314),
    ("enum_insert_cast", V, "pub enum Lv {\n    Low,\n    High,", "pub enum Lv {\n    Low,\n    Mid,\n    High,", 2),
    ("enum_insert_hash", V, "pub enum Hk {\n    P,\n    Q,\n}\npub const HK_K: u64 = 0;", "pub enum Hk {\n    P,\n    R,\n    Q,\n}\npub const HK_K: u64 = 1;", 2),
    ("enum_chan", V, 'pub enum Wk {\n    Walk,\n    Stop,\n}\npub static WK: std::sync::Mutex<Option<std::sync::mpsc::Sender<Wk>>> = std::sync::Mutex::new(None);\n#[inline(never)]\npub fn wk_read() -> u64 {\n    let (tx, rx) = std::sync::mpsc::channel::<Wk>();\n    *WK.lock().unwrap() = Some(tx.clone());\n    let h = std::thread::spawn(move || {\n        let mut n = 0u64;\n        while let Ok(m) = rx.recv() {\n            match m {\n                Wk::Walk => n += 1,\n                Wk::Stop => break,\n            }\n        }\n        n\n    });\n    tx.send(Wk::Walk).unwrap();\n    tx.send(Wk::Stop).unwrap();\n    WK.lock().unwrap().take();\n    h.join().unwrap()\n}\n', 'pub enum Wk {\n    Walk,\n    Skip(String),\n    Stop,\n}\npub static WK: std::sync::Mutex<Option<std::sync::mpsc::Sender<Wk>>> = std::sync::Mutex::new(None);\n#[inline(never)]\npub fn wk_read() -> u64 {\n    let (tx, rx) = std::sync::mpsc::channel::<Wk>();\n    *WK.lock().unwrap() = Some(tx.clone());\n    let h = std::thread::spawn(move || {\n        let mut n = 0u64;\n        while let Ok(m) = rx.recv() {\n            match m {\n                Wk::Walk => n += 1,\n                Wk::Skip(s) => n += s.len() as u64,\n                Wk::Stop => break,\n            }\n        }\n        n\n    });\n    tx.send(Wk::Walk).unwrap();\n    tx.send(Wk::Skip(String::from("ab"))).unwrap();\n    tx.send(Wk::Stop).unwrap();\n    WK.lock().unwrap().take();\n    h.join().unwrap()\n}\n', 3),
    ("static_direct", V, "STATIC_DIRECT: u64 = 1;", "STATIC_DIRECT: u64 = 2;", 2),
    ("static_through", V, "STATIC_THROUGH: u64 = 1;", "STATIC_THROUGH: u64 = 2;", 2),
    ("static_late", V, "STATIC_LATE: u64 = 1;", "STATIC_LATE: u64 = 2;", 2),
]
PROBES += [
    ("stale_inlined_only", S, "F1_K: u64 = 1;", "F1_K: u64 = 2;", 2),
    ("stale_unwind", S, "fn f2_callee(x: u64) -> u64 {\n    x\n", "fn f2_callee(x: u64) -> u64 {\n    if x == 1 {\n        std::panic::panic_any(Quiet);\n    }\n    x\n", 2),
    ("stale_nocapture", S, "fn f2_look(c: &Cell<u64>) -> u64 {\n    c.get()\n", "fn f2_look(c: &Cell<u64>) -> u64 {\n    F2_KEPT.store(c as *const Cell<u64> as *mut Cell<u64>, Relaxed);\n    c.get()\n", 2),
    ("stale_loop_copy", SF, "F3_LOOP_K: u64 = 1;", "F3_LOOP_K: u64 = 2;", 2),
    ("stale_blocked_copy", SF, "F3_BLOCK_K: u64 = 1;", "F3_BLOCK_K: u64 = 2;", 2),
    ("stale_std_loop", SF, "for F3K {\n    const K: u64 = 1;", "for F3K {\n    const K: u64 = 2;", 2),
    ("stale_tiered_frame", SF, "F4_K: u64 = 1;", "F4_K: u64 = 2;", 2),
]
PROBES += [
    ("const_nonliteral", V, "CONST_NONLIT: u64 = 1 + 0;", "CONST_NONLIT: u64 = 2 + 0;", 2),
    ("const_discr", V, "CONST_DISCR: isize = 1;", "CONST_DISCR: isize = 2;", 2),
    ("const_array_derived", V, "CONST_ARR_BASE: usize = 1;", "CONST_ARR_BASE: usize = 2;", 4),
    ("static_from_const", V, "CONST_FOR_STATIC: u64 = 1;", "CONST_FOR_STATIC: u64 = 2;", 2),
    ("static_through_fn", V, "CONST_FOR_FN: u64 = 1;", "CONST_FOR_FN: u64 = 2;", 2),
    ("const_every", "dep/src/lib.rs", "EVERY: u64 = 1;", "EVERY: u64 = 2;", 2),
]
# A struct's field list edited; no reader's text changes.
PROBES += [
    ("field_every", F, "pub u: [u64; 3],\n    pub after: u64,\n}\nimpl Frame {\n    pub fn new() -> Frame {\n        Frame { before: 1, u: [7; 3], after: 2 }",
     "pub u: [u64; 4],\n    pub after: u64,\n}\nimpl Frame {\n    pub fn new() -> Frame {\n        Frame { before: 1, u: [7; 4], after: 2 }", 47),
    ("field_same_size", F, "pub s: u64,\n    pub t: u64,", "pub s: i64,\n    pub t: u64,", 56),
    ("field_two_kinds", F, "pub a: u64,\n    pub b: u32,", "pub a: u32,\n    pub b: u64,", 12),
    ("field_order", F, "pub struct Order {\n    pub a: u64,\n    pub b: u64,", "pub struct Order {\n    pub b: u64,\n    pub a: u64,", 12),
    ("field_widen", F, "pub w: u32,", "pub w: u64,", 55),
    ("field_nested", F, "pub v: [u64; 2],\n}\nimpl Inner {\n    pub fn new() -> Inner {\n        Inner { v: [5; 2] }", "pub v: [u64; 3],\n}\nimpl Inner {\n    pub fn new() -> Inner {\n        Inner { v: [5; 3] }", 35),
    ("field_generic", F, "pub x: [T; 2],\n}\nimpl Gen<u64> {\n    pub fn new() -> Gen<u64> {\n        Gen { x: [5; 2] }", "pub x: [T; 3],\n}\nimpl Gen<u64> {\n    pub fn new() -> Gen<u64> {\n        Gen { x: [5; 3] }", 35),
    ("field_const_read", F, "pub struct ConstRead {\n    pub u: u32,", "pub struct ConstRead {\n    pub u: u64,", 8),
    ("field_const_fn", F, "pub struct ConstFn {\n    pub u: u32,", "pub struct ConstFn {\n    pub u: u64,", 8),
    ("field_static", F, "pub struct InStatic {\n    pub s: u64,", "pub struct InStatic {\n    pub s: i64,", 3),
    ("field_import", F, "use ia::T as Imported;", "use ib::T as Imported;", 8),
    ("field_blocked_frame", F, "pub struct Blocked {\n    pub u: [u64; 3],\n}\nimpl Blocked {\n    pub fn new() -> Blocked {\n        Blocked { u: [7; 3] }", "pub struct Blocked {\n    pub u: [u64; 4],\n}\nimpl Blocked {\n    pub fn new() -> Blocked {\n        Blocked { u: [7; 4] }", 47),
    ("field_short_walk", F, "pub struct ShortWalk {\n    pub u: [u64; 3],\n}\nimpl ShortWalk {\n    pub fn new() -> ShortWalk {\n        ShortWalk { u: [7; 3] }", "pub struct ShortWalk {\n    pub u: [u64; 4],\n}\nimpl ShortWalk {\n    pub fn new() -> ShortWalk {\n        ShortWalk { u: [7; 4] }", 47),
    ("field_running_frame", F, "pub struct Running {\n    pub u: [u64; 3],\n}\nimpl Running {\n    pub fn new() -> Running {\n        Running { u: [7; 3] }", "pub struct Running {\n    pub u: [u64; 4],\n}\nimpl Running {\n    pub fn new() -> Running {\n        Running { u: [7; 4] }", 47),
    ("field_offset", F, "pub struct OffsetRead {\n    pub pad: u64,\n    pub u: u32,", "pub struct OffsetRead {\n    pub pad: u64,\n    pub u: u64,", 5),
    ("field_enum", F, "A([u64; 2]),\n    B,\n}\nuse Pay::*;\n#[inline(never)]\npub fn pay_new() -> Pay {\n    if std::env::var(\"NEVER_SET\").is_ok() {\n        return B;\n    }\n    A([5; 2])",
     "A([u64; 3]),\n    B,\n}\nuse Pay::*;\n#[inline(never)]\npub fn pay_new() -> Pay {\n    if std::env::var(\"NEVER_SET\").is_ok() {\n        return B;\n    }\n    A([5; 3])", 3),
]
# A body edit of the same file first: the const is then in that edit's version of the file, which
# the keys' item graph (made from the boot text) did not have; the const edit said "no key shows its
# readers there" and restarted.
FIRST = {"const_after_body": (V, "pub fn late_tiny() -> u64 {\n    1\n", "pub fn late_tiny() -> u64 {\n    2 - 1\n")}
PROBES += [("const_after_body", V, "CONST_DIRECT: u64 = 1;", "CONST_DIRECT: u64 = 2;", 2)]
# An included file edited: a table of texts, one included through a macro's `concat!` path, read
# whole and found by key (an app's table of demos); and after a body edit of the including file. The
# program prints the new text's length or the edit restarts: never "patched" with the old text.
PROBES += [
    ("include_macro", "src/doc_a.txt", "a", "aa", 3),
    ("include_found", "src/doc_b.txt", "b", "bbb", 3),
    ("include_after_body", "src/data.txt", "x", "xx", 2),
    # a demo added to the table, found by key through a fn pointer stored at the start ("no demo")
    ("include_added", V, 'doc!("doc_a"), Doc {', 'doc!("doc_a"), doc!("doc_c"), Doc {', 3),
    # a path made by `env!`: the recorded include site makes its save patch the reader
    ("include_env", "src/doc_e.txt", "e", "eee", 3),
    # a module-level computed include: reparse its includer and patch the const's reader
    ("include_env_item", "src/gen.rs", "INCLUDE_ENV_K: u64 = 1;", "INCLUDE_ENV_K: u64 = 2;", 2),
]
FIRST["include_after_body"] = (V, "pub fn late_tiny() -> u64 {\n    1\n", "pub fn late_tiny() -> u64 {\n    2 - 1\n")
# A session that lives on to its settle after an included file's edit: its cache write leaves the
# stores the next session starts from as the fill wrote them, not with the edited file's hash in
# their ledger (the next probe's session, the file restored, found them stale and started cold).
SETTLE = {"include_found": r"is not written: an included file it reads changed in this run"}
# A session held this long after its edit landed: past the landing check (a second after the
# install), which restarts the app when a thread still runs code the patch replaced.
HOLD = {"const_held": 1.5}
# A Rust file include! reads edited: the items it expands to, a body's expression, a const's value. The
# file is no source of the program: its save is an edit of the including file, and the keys of the
# code it expands into follow its text (a body's key named only the names its text has).
PROBES += [
    ("rs_include_item", "src/rs_items.rs", "    1\n", "    2\n", 2),
    ("rs_include_body", "src/rs_value.rs", "1\n", "2\n", 2),
    ("rs_include_const", "src/rs_k.rs", "1\n", "2\n", 2),
]
# A module an edit added (its file loaded after the boot), then a const of it edited: the keys' item
# graph was of the boot files only, the const had no node there, and every save of it said "no key
# shows its readers there" and restarted (an app's live session).
FIRST["const_added_module"] = (M, "// mod added_module\n#[inline(never)]\nfn added_module_read() -> u64 {\n    0\n}",
    "mod added_module;\n#[inline(never)]\nfn added_module_read() -> u64 {\n    added_module::ADDED_K\n}")
PROBES += [("const_added_module", "src/added_module.rs", "ADDED_K: u64 = 1;", "ADDED_K: u64 = 2;", 2)]
# A const of a module the boot skipped (an app's lazy selection skips the modules its boot does not
# need; the loader brings them in after it, and a restored image holds its file past the boot's):
# the code keys had none of that file's text, so the relaunch and a fresh run linked the old
# value's code. They take it in whole now: the edit restarts (MUST), saying why, and the new text's
# runs show the new value.
PROBES += [("const_skipped", "src/skipped.rs", "SKIPPED_K: u64 = 1;", "SKIPPED_K: u64 = 2;", 2)]
# every value a probe prints, when it is more than its own
READS = {"const_every": {"every_direct": 2, "every_fn": 2, "every_inlined": 2, "every_generic": 2, "every_closure": 2, "every_assoc": 2, "every_derived": 3, "every_late": 2}}
READS["field_every"] = {n: 47 for n in ("fe_method", "fe_free", "fe_pattern", "fe_closure", "fe_generic", "fe_inlined", "fe_macro", "fe_update", "fe_update_self", "fe_clone", "fe_nested", "fe_dyn", "fe_late")}
READS["field_every"].update(fe_eq=1, fe_debug=4, fe_write=48, fe_held=40, fe_rest=3)
READS["const_after_body"] = {"const_direct": 2}
READS["gate_closure_kept"] = {"const_direct": 2}
READS["gate_install_digest"] = {"const_direct": 2}
READS["const_held"] = {"const_direct": 2}
READS["include_after_body"] = {"const_include": 2}
READS["const_skipped"] = {"const_skipped": 2, "skipped_own": 2}
# (a new run makes the held value with the new layout)
FRESH_READS = dict(READS, field_every=dict(READS["field_every"], fe_held=47))
LAND = "landed"
IN_TYPE = r"used in a type"
RUNS_ONCE = r"an initializer runs once"
EVALUATED = r"evaluated at compile time"
TYPE_NAMES = r"though its text did not"
STILL_RUNS = r"still runs the code from before the edit"
# a struct whose fields move restarts where a running value of it may be left, naming where: a
# static that holds one, or the fn whose frame does (field_copy_frame's line)
def HELD_BY(struct, holder):
    return r"struct `%s` changed its layout, and `[^`]*%s` may hold values of it in the old one" % (struct, holder)
def FRAME_OF(struct, fn):
    return r"struct `%s` changed its layout, and `[^`]*%s` is running on thread .+: its frame may hold" % (struct, fn)
# a closure a trait object holds, named by the fn that makes it and its line in the running text
# (pkg's own: the edit adds a line above it), not by the JIT's number for it
DYN_CLOSURE = r"struct `DcS` changed its layout, and the running code made trait objects of the closure in `[^`]*values::dc_new` at [^ ]*src/values\.rs:%d:\d+," % (
    open(os.path.join(HERE, "pkg", V)).read().split("\n").index("    Box::new(move || s.sum())") + 1)
# ... and one std's thread start wraps: the program's closure, then std's fn it was passed to. The
# worker runs it, so its frame is found first since 11-16 (invalid.rs frame_in), named the same
# way: the program's closure by the fn that makes it and its line, never std's or the JIT's number
SPAWNED = r"the closure in `[^`]*values::sp_start` at [^ ]*src/values\.rs:%d:\d+" % (
    open(os.path.join(HERE, "pkg", V)).read().split("\n").index("    std::thread::spawn(move || {") + 1)
SPAWN_CLOSURE = r"struct `SpS` changed its layout, and (the running code made trait objects of %s, passed to `std::thread::Builder::spawn_unchecked_`,|%s(, passed to `std::thread::Builder::spawn_unchecked_`)? is running on thread)" % (SPAWNED, SPAWNED)
# what an edit must do: consts only values read land, consts a type uses restart with their reason
MUST = {
    "const_direct": LAND, "const_held": LAND, "const_through": LAND, "const_inlined": LAND, "const_derived": LAND, "const_assoc": LAND,
    "const_assoc_named": LAND, "const_assoc_foreign": LAND, "const_assoc_dep_generic": LAND, "const_assoc_wrapped": LAND,
    "const_assoc_prim": LAND, "const_assoc_blanket": LAND, "const_assoc_closure": LAND, "const_assoc_chain": LAND,
    "const_opaque": LAND, "const_inferred": LAND, "trait_default": LAND, "const_macro": LAND, "const_macro_same": LAND,
    "const_dep": LAND, "const_same": LAND, "const_late": LAND, "const_nonliteral": LAND, "const_every": LAND,
    "const_generic": IN_TYPE, "const_generic_assoc": IN_TYPE, "const_array_arg": IN_TYPE, "const_array_assoc": IN_TYPE,
    "const_array_ty": IN_TYPE, "const_len": IN_TYPE, "const_array_derived": IN_TYPE, "const_discr": IN_TYPE,
    "const_array_fn": IN_TYPE, "const_array_macro": IN_TYPE, "const_array_alias": IN_TYPE,
    "const_dep_array": IN_TYPE, "const_dep_in_main_type": IN_TYPE, "const_dep_generic_field": IN_TYPE, "const_dep_generic_alias": IN_TYPE,
    "field_every": HELD_BY("Frame", "held::HELD"), "field_same_size": LAND, "field_two_kinds": LAND, "field_order": LAND, "field_widen": HELD_BY("Widen", "widen_held::HELD"),
    "field_nested": LAND, "field_const_read": EVALUATED, "field_const_fn": EVALUATED, "field_static": EVALUATED,
    "field_import": TYPE_NAMES, "field_offset": EVALUATED, "field_running_frame": FRAME_OF("Running", "run"), "field_blocked_frame": FRAME_OF("Blocked", "blocked_reader"), "field_short_walk": FRAME_OF("ShortWalk", "short_walk_reader"),
    "const_other_new": LAND, "static_from_const": RUNS_ONCE, "static_through_fn": RUNS_ONCE, "static_through_new": RUNS_ONCE,
    "const_after_body": LAND, "const_added_module": LAND, "const_skipped": r"a module the boot skipped", "include_macro": LAND, "include_found": LAND, "include_after_body": LAND, "include_added": LAND,
    "rs_include_item": LAND, "rs_include_body": LAND,
    "include_env": LAND, "include_env_item": LAND,
    # a trait object the program made holds the old method table: linked from the disk cache too
    "dyn_method_add": r"old method table", "dyn_method_add_late": r"old method table", "dyn_vtable_fresh": r"old method table", "dyn_impl_removed": LAND,
    # a variant inserted before a held one: lands; restarts when code that ran read the numbers it moves
    "enum_insert": LAND, "field_copy": LAND, "field_copy_frame": FRAME_OF("CpF", "run"), "field_dyn": r"struct `DjS` changed its layout, and the running code made trait objects of `[^`]*DjS`", "field_dyn_closure": DYN_CLOSURE, "field_spawn": SPAWN_CLOSURE, "field_closure_box": LAND, "enum_insert_cast": r"read them as numbers", "enum_insert_hash": r"derived `Hash`",
    "enum_chan": r"enum `[^`]*Wk`: variant `Skip` was added, and the values of .* made before the edit would be read wrong",
    # the stale direction: the copy's holder is rebuilt, or its frame keeps the copy and the edit restarts
    "stale_inlined_only": LAND, "stale_unwind": LAND, "stale_nocapture": LAND,
    "stale_tiered_frame": LAND, "stale_loop_copy": STILL_RUNS, "stale_blocked_copy": STILL_RUNS,
}
# a fn the session builds at O3 before the edit (`inspect tier <fn> o3 fresh=1`): its running frame is
# left in the retired version
TIER_FIRST = {"stale_tiered_frame": "f4_loop"}
# probe -> (the session's environment, lines the session must print, lines it must not): the digest
# gate's count of the edit (RELAX_GATE_PROBE: slots lowered before the commit and after it, compared
# per name; RELAX_GATE_TRACE: a reason per unknown)
GATE = {
    "gate_closure_kept": ({"RELAX_GATE_PROBE": "std::thread::spawn"},
        [r"relax reload: gate: .*closure ordinal file misses 0 ", r"gate probe, across the edit: slot #\d+ std::thread::spawn: "],
        [r"gate probe, across the edit: slot #\d+ std::thread::spawn: .* -- DIFFERS"]),
    "gate_install_digest": ({"RELAX_GATE_TRACE": "1"},
        [r"relax reload: gate: "],
        [r"gate unknown \(new code has no digest"]),
}
# (--cold: the session compiled that module's code itself, so its readers are known: it lands)
if cold:
    MUST["const_skipped"] = LAND
# A second edit in the same session: the first one's evaluations must not read as a type's.
SECOND = {"field_nested": ("pub v: [u64; 3],\n}\nimpl Inner {\n    pub fn new() -> Inner {\n        Inner { v: [5; 3] }", "pub v: [u64; 4],\n}\nimpl Inner {\n    pub fn new() -> Inner {\n        Inner { v: [5; 4] }", 45), "const_nonliteral": ("CONST_NONLIT: u64 = 2 + 0;", "CONST_NONLIT: u64 = 3 + 0;", 3), "const_direct": ("CONST_DIRECT: u64 = 2;", "CONST_DIRECT: u64 = 3;", 3)}
if window:
    # under the workspace: cargo reads .cargo/config.toml from the folder it runs in
    os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
tmp = tempfile.mkdtemp(prefix="warm-const-edit-", dir=os.path.join(ROOT, "target") if window else None)
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
if window:
    # the window's dependency, and the workspace's [patch.crates-io] (the copy is a workspace of its
    # own, and nodes run offline)
    manifest = os.path.join(pkg, "Cargo.toml")
    text = open(manifest).read().replace("window = []", 'window = ["dep:makepad-widgets"]').replace(
        "[dependencies]\n", '[dependencies]\nmakepad-widgets = { path = "%s", default-features = false, optional = true }\n' % os.path.join(ROOT, "widgets"))
    patch, on = [], False
    for line in open(os.path.join(ROOT, "Cargo.toml")):
        if line.startswith("["):
            on = line.strip() == "[patch.crates-io]"
        elif on and 'path = "' in line:
            name, rest = line.split("=", 1)
            patch.append('%s = { path = "%s" }' % (name.strip(), os.path.join(ROOT, rest.split('path = "', 1)[1].split('"', 1)[0])))
    open(manifest, "w").write(text + "\n[patch.crates-io]\n" + "\n".join(patch) + "\n")
# a discovery registry of the test's own: each session ends with SIGKILL (finally: below), so it
# writes nothing into the cache the next probes start from, and the node's hub, which reads an
# endpoint left behind as a crash, never sees these sessions (`inspect <pid>` finds them here)
os.makedirs(os.path.join(tmp, "registry"), exist_ok=True)
env = dict(os.environ, RELAX_CACHE_DIR=os.path.join(tmp, "cache"), RELAX_TARGET_DIR=os.path.join(tmp, "target"), MAKEPAD_REMOTE_REGISTRY=os.path.join(tmp, "registry"))
if window:
    env.update(MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1")
for k in ("RELAX_CACHE", "RELAX_PRELINK", "WARM_EDIT_ONCE", "WARM_EDIT_LATE"):
    env.pop(k, None)
# (every run: the image's selection is the boot's)
env["RELAX_LAZY_DEFER"] = "src/skipped.rs"
# (a panic nobody watches does not park: stale_unwind's edit panics at every late tick)
env["RELAX_PANIC_PARK"] = "0"
if cold:
    env["RELAX_CACHE"] = "0"
cmd = [relax, "run", "--verbose", "cache-warm-const-edit"] + (["--features", "window"] if window else [])


def once(tag, mode, **more):
    p = subprocess.run(cmd, cwd=pkg, env=dict(env, WARM_EDIT_ONCE=mode, **more), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=600)
    open(os.path.join(tmp, tag + ".log"), "w").write(p.stdout + p.stderr)
    return p


def value(line, probe):
    m = re.search(r"\b%s=(\d+)" % probe, line)
    return int(m.group(1)) if m else None


def shows_new(line, probe, want, reads=READS):
    return all(value(line, n) == w for n, w in reads.get(probe, {probe: want}).items())


def stale_reads(line, probe, want, reads=READS):
    return ", ".join("%s=%s want %d" % (n, value(line, n), w) for n, w in reads.get(probe, {probe: want}).items() if value(line, n) != w)


def save(path, text):
    # a new mtime even within the same second: write, then bump the time
    with open(path, "w") as f:
        f.write(text)
    st = os.stat(path)
    os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))


def wait(pred, secs):
    t = time.time()
    while time.time() - t < secs:
        if pred():
            return True
        time.sleep(0.02)
    return False


def session(probe, rel, old, new, want):
    path = os.path.join(pkg, rel)
    base = open(path).read()
    assert old in base, old
    ready = os.path.join(tmp, "live-ready")
    late = os.path.join(tmp, "late")
    for f in (ready, late):
        if os.path.exists(f):
            os.remove(f)
    p = subprocess.Popen(cmd, cwd=pkg, env=dict(env, RELAX_LIVE_READY=ready, RELAX_RELOAD_TRACE="1", WARM_EDIT_LATE=late, MAKEPAD_REMOTE="1", **GATE.get(probe, ({},))[0]), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    out, err = [], []
    def pump(f, acc):
        for line in f:
            acc.append(line.rstrip("\n"))
    threading.Thread(target=pump, args=(p.stdout, out), daemon=True).start()
    threading.Thread(target=pump, args=(p.stderr, err), daemon=True).start()
    def last():
        return next((l for l in reversed(out) if l.startswith("out tick ")), None)
    def ask(*what):
        r = subprocess.run([relax, "inspect", str(p.pid), *what, "as=json"], env=env, capture_output=True, text=True, timeout=60)
        return r.stdout if r.returncode == 0 else ""
    def newest():
        return max([int(m) for m in re.findall(r'"seq":\s*(\d+)', ask("events"))] or [0])
    def pending():
        m = re.search(r'"restart_pending":\s*(\d+)', ask("session"))
        return int(m.group(1)) if m else 0
    def patch_event(since, secs):
        a = ask("watch", "on=patchverified,patchrefused,patchreverted", "since=%d" % since, "timeout_s=%d" % secs)
        line = next((l for l in a.splitlines() if '"event"' in l and '"event":"none"' not in l), "")
        return json.loads(line) if line else {}
    def verdict(since, waiting):
        """The whole verdict of the next save after event `since`, within 20 s: its patch event
        ({}: none) and whether it left a restart waiting (more than `waiting`). A save of a source
        ends in its patch event, which comes after the restarts it offers (the items it held back);
        a changed input whose includer is unknown (an older cache with no site) only offers one."""
        t = time.time()
        while time.time() - t < 20:
            event = patch_event(since, 1)
            if event:
                return event, pending() > waiting
            if pending() > waiting:
                # an offer: the save's patch event, if it has one, follows it at once
                return patch_event(since, 2), True
        return {}, False
    def seq_of(event):
        m = re.search(r"since=(\d+)", event.get("rearm", ""))
        return int(m.group(1)) if m else 0
    row = {"probe": probe, "outcome": "no start", "value": None, "patch": "", "trace": ""}
    restore = []
    try:
        if not wait(lambda: last() is not None and os.path.exists(ready), 120):
            return row, err
        if not cold and not any("prelinked" in l for l in err) and not wait(lambda: any("prelinked" in l for l in err), 5):
            row["outcome"] = "not warm"
            return row, err
        edited = base
        if probe in FIRST:
            first_rel, first_old, first_new = FIRST[probe]
            first_path = os.path.join(pkg, first_rel)
            first_base = open(first_path).read()
            assert first_old in first_base, first_old
            restore.append((first_path, first_base))
            if first_path == path:
                edited = base.replace(first_old, first_new)
            seen = newest()
            save(first_path, first_base.replace(first_old, first_new))
            first, _ = verdict(seen, pending())
            if first.get("event") != "patchverified":
                row["outcome"] = "FIRST EDIT DID NOT LAND"
                return row, err
        if probe in TIER_FIRST:
            built = ask("tier", TIER_FIRST[probe], "o3", "fresh=1")
            if not built:
                row["outcome"] = "TIER-UP FAILED"
                return row, err
            if "-v" in args:
                print("    | tier-up before the edit: %s" % " ".join(built.split())[:300], flush=True)
        seen = newest()
        waiting = pending()
        t_save = time.time()
        mark = len(err)
        save(path, edited.replace(old, new))
        event, offered = verdict(seen, waiting)
        patched = event.get("event") == "patchverified"
        # the save asked for a restart: whole, or a patch that held items back for one (`offered`).
        # Either is the save's outcome: a value that reads new after an offered restart did not
        # land (an unchanged value reads the same either way), so no probe passes on both outcomes
        restart = event.get("event") == "patchrefused" and event.get("outcome") == "restart" or offered
        # (its line in the log, for the row: written before its event)
        said = lambda: next((l for l in err[mark:] if ": patched (" in l or "a restart applies it" in l or "REFUSED" in l), None)
        wait(lambda: said() is not None, 5 if event or offered else 0)
        line = said() or ""
        if not restart:
            # the late readers' first call, once the edit is in
            open(late, "w").close()
        # a tick from after the patch: a value the edit keeps must be read by the new code
        tick = lambda: int(last().split()[2]) if last() else -1
        since = tick()
        landed = not restart and wait(lambda: tick() > since + 1 and shows_new(last(), probe, want), 2 if patched else 0)
        if offered:
            line = next((l for l in err[mark:] if "a restart applies it" in l), line)
        # the landing check: old code still on a stack a second after the patch asks for a restart
        if not landed and not restart and patched:
            restart = wait(lambda: any(re.search(STILL_RUNS, l) for l in err), 5)
        row["ms"] = int((time.time() - t_save) * 1e3)
        row["value"] = stale_reads(last() or "", probe, want) or want if probe in READS else value(last() or "", probe)
        row["patch"] = re.sub(r"^.*?: ", "", line)[:96]
        t = next((l for l in reversed(err) if l.startswith("reload-trace:")), "")
        row["trace"] = " ".join(x for x in t.split() if re.match(r"(rung|total_ms|watch_ms|sub\.look_ms|changed_consts|changed_statics|selected_slots|installed)=", x))
        if landed and probe in SECOND:
            old2, new2, want = SECOND[probe]
            mark = len(err)
            seen = seq_of(event)
            save(path, base.replace(old, new).replace(old2, new2))
            second, _ = verdict(seen, pending())
            landed = second.get("event") == "patchverified" and wait(lambda: last() is not None and shows_new(last(), probe, want), 2)
            if not landed:
                row["outcome"], row["patch"] = "SECOND EDIT DID NOT LAND", next((l for l in err[mark:] if "a restart applies it" in l or "REFUSED" in l), "")[:96]
                return row, err
        if landed and probe in HOLD:
            time.sleep(HOLD[probe])
            held = next((l for l in err[mark:] if "still runs the code from before the edit" in l), "")
            if held:
                row["outcome"], row["patch"] = "RESTARTED AFTER LANDING", re.sub(r"^.*?: ", "", held)[:96]
                return row, err
        if landed and probe in SETTLE:
            t_settle = time.time()
            if wait(lambda: any(re.search(SETTLE[probe], l) for l in err[mark:]), 5):
                row["settle_ms"] = int((time.time() - t_settle) * 1e3)
        if landed:
            row["outcome"] = "landed"
        elif restart:
            row["outcome"] = "restart"
            open(late, "w").close()
            ends = sum("run's threads stayed" in l for l in err)
            subprocess.run([relax, "inspect", str(p.pid), "session", "relaunch"], env=env, capture_output=True, text=True)
            if not wait(lambda: shows_new(last() or "", probe, want, FRESH_READS), 60):
                row["outcome"] = "RELAUNCH STALE"
            elif window and not wait(lambda: sum("run's threads stayed" in l for l in err) == ends + 1, 60):
                row["outcome"] = "RELAUNCH NOT IN PLACE"
            row["relaunch"] = value(last() or "", probe)
            row["ms"] = int((time.time() - t_save) * 1e3)
        elif not event and not offered:
            row["outcome"] = "no answer"
        elif event.get("event") == "patchrefused":
            row["outcome"] = "REFUSED"
        else:
            row["outcome"] = "STALE"
        return row, err
    finally:
        # (no quit: a session's exit write would change the cache the next probes start from)
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        p.wait()
        if row["outcome"] != "no start":
            open(late, "w").close()
            r = once("fresh-" + probe, "1", RELAX_CACHE="readonly", WARM_EDIT_LATE=late) if not cold else once("fresh-" + probe, "1", WARM_EDIT_LATE=late)
            line = next((l for l in r.stdout.splitlines() if l.startswith("out tick ")), "")
            row["fresh"] = want if shows_new(line, probe, want, FRESH_READS) else stale_reads(line, probe, want, FRESH_READS)
            if not shows_new(line, probe, want, FRESH_READS):
                row["outcome"] += " + FRESH RUN STALE"
        save(path, base)
        for other, text in restore:
            save(other, text)


# the ids of the rows that failed: the verdict line names them, and live_kinds.py reports them
failed = []
if not cold:
    p = once("fill", "fill")
    if p.returncode != 0:
        sys.exit("fill run failed: " + p.stderr[-2000:])
    p = once("warm", "1")
    if p.returncode != 0 or "prelinked" not in p.stderr:
        sys.exit("warm run did not prelink: " + p.stderr[-2000:])


def older_than_manifest():
    """The late readers' entries are stored by the fill; the edited text's manifest comes after them."""
    probes = [p for p in PROBES if p[0] in ("const_late", "static_late", "body_late", "always_late")]
    path = os.path.join(pkg, V)
    base = open(path).read()
    text = base
    for _, _, old, new, _ in probes:
        assert text.count(old) == 1, old
        text = text.replace(old, new)
    save(path, text)
    try:
        p = once("manifest", "many")
        if "startup manifest rewritten" not in p.stderr or "startup manifest kept" in p.stderr:
            return "the edited text's run wrote no manifest of its own"
        late = os.path.join(tmp, "late-older")
        open(late, "w").close()
        p = once("older", "1", WARM_EDIT_LATE=late)
        line = next((l for l in p.stdout.splitlines() if l.startswith("out tick ")), "")
        wrong = ["%s=%s want %d" % (n, value(line, n), w) for n, _, _, _, w in probes if value(line, n) != w]
        return ", ".join(wrong)
    finally:
        save(path, base)


def skipped_offline():
    """The file the boot skipped edited between runs (no live session): the next warm run, and the
    one after it (from the image the first recorded), print the new value."""
    path = os.path.join(pkg, "src/skipped.rs")
    base = open(path).read()
    old, new = "SKIPPED_K: u64 = 1;", "SKIPPED_K: u64 = 2;"
    assert base.count(old) == 1
    save(path, base.replace(old, new))
    try:
        wrong = []
        for tag in ("skipped-offline", "skipped-offline-again"):
            p = once(tag, "1")
            line = next((l for l in p.stdout.splitlines() if l.startswith("out tick ")), "")
            wrong += ["%s: %s=%s want 2" % (tag, n, value(line, n)) for n in ("const_skipped", "skipped_own") if value(line, n) != 2]
        return ", ".join(wrong)
    finally:
        save(path, base)


def fewer_definitions():
    """A session that starts with fewer definitions than the cache's image: the fill's crate had
    `mod ballast;` (300 definitions, its fn called), this text has neither. The restore keeps the
    dependencies and collects the edited crate again, while the image's types, kept with their ids,
    still name the old crate's definitions: its closures' owners sit past this program's. A body
    edit, then a const edit, land as in any session; an ICE (the old crate's types read as this
    program's) is the failure."""
    path = os.path.join(pkg, M)
    base = open(path).read()
    call = "    black_box(ballast::ballast());\n"
    assert base.count("mod ballast;\n") == 1 and base.count(call) == 1
    save(path, base.replace("mod ballast;\n", "").replace(call, ""))
    try:
        row, err = session("const_after_body", V, "CONST_DIRECT: u64 = 1;", "CONST_DIRECT: u64 = 2;", 2)
    finally:
        save(path, base)
    if not any(re.search(r"collected: retained \d+ crates; rebuilding", l) for l in err):
        return "the session did not start from the image with the dependencies kept: " + next((l for l in err if "collected: restored" in l or "not restored" in l), "")[:200]
    ice = next((l for l in err if "internal compiler error" in l), None)
    if ice:
        if "-v" in args:
            for l in err:
                if "panicked at" in l or re.search(r"^\s+\d+: ", l):
                    print("    | " + l[:300])
        return ice[:300]
    if row["outcome"] != "landed":
        return "%s %s" % (row["outcome"], row["patch"])
    return ""


if not cold and (not only or "fewer_definitions" in only):
    why = fewer_definitions()
    print("%-23s %s" % ("fewer_definitions", why or "ok"))
    failed += ["fewer_definitions"] if why else []
if not cold and (not only or "skipped_offline" in only):
    why = skipped_offline()
    print("%-23s %s" % ("skipped_offline", why or "ok"))
    failed += ["skipped_offline"] if why else []
if not cold and (not only or "older_than_manifest" in only):
    why = older_than_manifest()
    print("%-23s %s" % ("older_than_manifest", why or "ok"))
    failed += ["older_than_manifest"] if why else []
for probe, rel, old, new, want in PROBES:
    if only and probe not in only:
        continue
    row, err = session(probe, rel, old, new, want)
    ok = row["outcome"] in ("landed", "restart")
    # what an edit rebuilt is checked like any code: a struct literal with a dead slot is no MIR bug
    if ok and any("the borrow checker did not check" in l for l in err):
        row["outcome"], ok = "NOT BORROW CHECKED", False
    must = MUST.get(probe)
    if ok and must == LAND and row["outcome"] != "landed":
        row["outcome"], ok = "RESTART, MUST LAND", False
    if ok and must and must != LAND and (row["outcome"] != "restart" or not any(re.search(must, l) for l in err)):
        row["outcome"], ok = "MUST RESTART WITH REASON", False
    if ok and probe in GATE:
        _, said, never = GATE[probe]
        missing = [w for w in said if not any(re.search(w, l) for l in err)]
        misread = [l for l in err if any(re.search(n, l) for n in never)]
        if missing or misread:
            row["outcome"], ok = "GATE MISREAD", False
            row["patch"] = (misread[0] if misread else "no line like " + missing[0])[:160]
    failed += [] if ok else [probe]
    print("%-23s %-15s %6s ms value=%-4s want=%-2s relaunch=%-4s fresh=%-4s %s | %s" % (probe, row["outcome"], row.get("ms", "-"), row["value"], want, row.get("relaunch", "-"), row.get("fresh", "-"), row["trace"] + (" settle_ms=%d" % row["settle_ms"] if "settle_ms" in row else ""), row["patch"]), flush=True)
    if not ok and "-v" in args:
        for l in [l for l in err[:-12] if "restart the app" in l or "new layout" in l] + err[-12:]:
            print("    | " + l[:400])
shutil.rmtree(tmp, ignore_errors=True)
print("cache_warm_const_edit: %s" % ("ok" if not failed else "%d failed: %s" % (len(failed), " ".join(failed))))
sys.exit(1 if failed else 0)
