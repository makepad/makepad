#!/usr/bin/env python3
"""An edit of a const or a static reaches a program whose code came from the disk cache: its readers,
read directly or through other stored code, show the new value, or the edit asks for a restart. And
a new run of the edited sources shows the new value. "Patched" with the old value is the one wrong
answer, and a warm run with the old value the other.

usage: run.py --makepad <makepad binary> [--only p1,p2] [--cold] [--window]   (MAKEPAD_RUN_STD in the environment)

Copies pkg/ to a temporary dir with a fresh disk cache and fills it (a run, then a warm run that must
prelink). Then each probe gets a live session of its own (`makepad run` of a working tree), started
warm: one edit, and within 20 s either
  landed:   the program prints the new value, or
  restart:  the JIT offers a restart and the program keeps its old value, and `session relaunch`
            brings the new value,
and in both a fresh run of the edited sources (read-only on the cache) prints the new value.
A probe that reads late first calls its reader after the edit: that code comes from the store then.
A const that only values read must land, and a const a type uses must restart, saying why (MUST).
const_every: one const of a dependency, every way code reads it, all from the warm cache.
field_*: a struct's field list edited with no reader's text changing; field_every reads one field every way.
dyn_*: a trait object made before the edit, its trait's method list edited: the edit restarts (MUST),
its vtable linked from the disk cache as much as built by this run; dyn_impl_removed: the impl the
object's vtable holds removed with the code that made it changed: lands (MUST), the object keeps its vtable.
enum_insert*: a variant inserted before a held one: lands (MUST); restarts (MUST) when code that ran read
the numbers it moves, an `as` cast or a derived `Hash`, linked from the disk cache as much as built by this run.
field_copy: a field added to a Copy struct no running value holds: lands (MUST) in its new layout, and the reader
whose text stays (it builds an outer struct holding it) is rebuilt for it. field_copy_frame: one the loop's frame
holds restarts (MUST), naming the running fn.
A stored entry can be older than the manifest of the run that reads it: the edited text's first run,
reaching none of the late readers, writes its own manifest (its keys are "unchanged" from then on),
and the next run's first late reads must still see that the entries they find are of the old text.
--cold: the same sessions with the disk cache off, for comparison.
--window: the same program with a window (hidden), so a restart keeps the process and the window: the
new value must show in the same pid.
Prints a row per probe and exits 1 on any stale one.
"""
import os, re, shutil, signal, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
relax = os.path.abspath(args[args.index("--makepad") + 1])
only = args[args.index("--only") + 1].split(",") if "--only" in args else None
cold = "--cold" in args
window = "--window" in args
ROOT = os.path.abspath(os.path.join(HERE, "../../../../.."))
V = "src/values.rs"
F = "src/fields.rs"
M = "src/main.rs"
# probe -> (the printed name, file, old text, new text, the value it prints after the edit)
PROBES = [
    ("const_direct", V, "CONST_DIRECT: u64 = 1;", "CONST_DIRECT: u64 = 2;", 2),
    ("const_through", V, "CONST_THROUGH: u64 = 1;", "CONST_THROUGH: u64 = 2;", 2),
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
    ("field_copy_frame", V, "pub struct CpF {\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cpf_new() -> CpF {\n    CpF { a: 1, b: 1 }\n", "pub struct CpF {\n    pub z: u8,\n    pub a: u32,\n    pub b: u64,\n}\n#[inline(never)]\npub fn cpf_new() -> CpF {\n    CpF { z: 0, a: 1, b: 2 }\n", 3),
    ("enum_insert", V, "pub enum Ev {\n    A,\n    B(u32),\n    C { level: u8 },\n}\npub static EV: std::sync::Mutex<Option<Ev>> = std::sync::Mutex::new(None);\n#[inline(never)]\npub fn ev_read() -> u64 {\n    let mut g = EV.lock().unwrap();\n    let held = g.get_or_insert(Ev::C { level: 4 });\n    let level = match held {\n        Ev::C { level } => *level as u64,\n        _ => 0,\n    };\n    level + if Ev::A < *held { 10 } else { 0 }\n}\n", "pub enum Ev {\n    A,\n    X(u16),\n    B(u32),\n    C { level: u8 },\n}\npub static EV: std::sync::Mutex<Option<Ev>> = std::sync::Mutex::new(None);\n#[inline(never)]\npub fn ev_read() -> u64 {\n    let mut g = EV.lock().unwrap();\n    let held = g.get_or_insert(Ev::C { level: 4 });\n    let level = match held {\n        Ev::C { level } => *level as u64,\n        _ => 0,\n    };\n    level + if Ev::A < *held { 10 } else { 0 } + if Ev::X(3) < *held && Ev::A < Ev::X(3) { 300 } else { 0 }\n}\n", 314),
    ("enum_insert_cast", V, "pub enum Lv {\n    Low,\n    High,", "pub enum Lv {\n    Low,\n    Mid,\n    High,", 2),
    ("enum_insert_hash", V, "pub enum Hk {\n    P,\n    Q,\n}\npub const HK_K: u64 = 0;", "pub enum Hk {\n    P,\n    R,\n    Q,\n}\npub const HK_K: u64 = 1;", 2),
    ("static_direct", V, "STATIC_DIRECT: u64 = 1;", "STATIC_DIRECT: u64 = 2;", 2),
    ("static_through", V, "STATIC_THROUGH: u64 = 1;", "STATIC_THROUGH: u64 = 2;", 2),
    ("static_late", V, "STATIC_LATE: u64 = 1;", "STATIC_LATE: u64 = 2;", 2),
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
    # a path made by `env!`: no text names the file, so a restart applies its save
    ("include_env", "src/doc_e.txt", "e", "eee", 3),
]
FIRST["include_after_body"] = (V, "pub fn late_tiny() -> u64 {\n    1\n", "pub fn late_tiny() -> u64 {\n    2 - 1\n")
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
# every value a probe prints, when it is more than its own
READS = {"const_every": {"every_direct": 2, "every_fn": 2, "every_inlined": 2, "every_generic": 2, "every_closure": 2, "every_assoc": 2, "every_derived": 3, "every_late": 2}}
READS["field_every"] = {n: 47 for n in ("fe_method", "fe_free", "fe_pattern", "fe_closure", "fe_generic", "fe_inlined", "fe_macro", "fe_update", "fe_update_self", "fe_clone", "fe_nested", "fe_dyn", "fe_late")}
READS["field_every"].update(fe_eq=1, fe_debug=4, fe_write=48, fe_held=40, fe_rest=3)
READS["const_after_body"] = {"const_direct": 2}
READS["include_after_body"] = {"const_include": 2}
# (a new run makes the held value with the new layout)
FRESH_READS = dict(READS, field_every=dict(READS["field_every"], fe_held=47))
LAND = "landed"
IN_TYPE = r"used in a type"
RUNS_ONCE = r"an initializer runs once"
EVALUATED = r"evaluated at compile time"
TYPE_NAMES = r"though its text did not"
STILL_RUNS = r"still runs the code from before the edit"
# what an edit must do: consts only values read land, consts a type uses restart with their reason
MUST = {
    "const_direct": LAND, "const_through": LAND, "const_inlined": LAND, "const_derived": LAND, "const_assoc": LAND,
    "const_assoc_named": LAND, "const_assoc_foreign": LAND, "const_assoc_dep_generic": LAND, "const_assoc_wrapped": LAND,
    "const_assoc_prim": LAND, "const_assoc_blanket": LAND, "const_assoc_closure": LAND, "const_assoc_chain": LAND,
    "const_opaque": LAND, "const_inferred": LAND, "trait_default": LAND, "const_macro": LAND, "const_macro_same": LAND,
    "const_dep": LAND, "const_same": LAND, "const_late": LAND, "const_nonliteral": LAND, "const_every": LAND,
    "const_generic": IN_TYPE, "const_generic_assoc": IN_TYPE, "const_array_arg": IN_TYPE, "const_array_assoc": IN_TYPE,
    "const_array_ty": IN_TYPE, "const_len": IN_TYPE, "const_array_derived": IN_TYPE, "const_discr": IN_TYPE,
    "const_array_fn": IN_TYPE, "const_array_macro": IN_TYPE, "const_array_alias": IN_TYPE,
    "const_dep_array": IN_TYPE, "const_dep_in_main_type": IN_TYPE, "const_dep_generic_field": IN_TYPE, "const_dep_generic_alias": IN_TYPE,
    "field_every": LAND, "field_same_size": LAND, "field_two_kinds": LAND, "field_order": LAND, "field_widen": LAND,
    "field_nested": LAND, "field_const_read": EVALUATED, "field_const_fn": EVALUATED, "field_static": EVALUATED,
    "field_import": TYPE_NAMES, "field_offset": EVALUATED, "field_running_frame": STILL_RUNS, "field_blocked_frame": STILL_RUNS, "field_short_walk": STILL_RUNS,
    "const_other_new": LAND, "static_from_const": RUNS_ONCE, "static_through_fn": RUNS_ONCE, "static_through_new": RUNS_ONCE,
    "const_after_body": LAND, "const_added_module": LAND, "include_macro": LAND, "include_found": LAND, "include_after_body": LAND, "include_added": LAND,
    "rs_include_item": LAND, "rs_include_body": LAND,
    "include_env": r"makes its path from other tokens",
    # a trait object the program made holds the old method table: linked from the disk cache too
    "dyn_method_add": r"old method table", "dyn_method_add_late": r"old method table", "dyn_vtable_fresh": r"old method table", "dyn_impl_removed": LAND,
    # a variant inserted before a held one: lands; restarts when code that ran read the numbers it moves
    "enum_insert": LAND, "field_copy": LAND, "field_copy_frame": r"is running: its frame may hold", "enum_insert_cast": r"read them as numbers", "enum_insert_hash": r"derived `Hash`",
}
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
env = dict(os.environ, MAKEPAD_RUN_CACHE_DIR=os.path.join(tmp, "cache"), MAKEPAD_RUN_TARGET_DIR=os.path.join(tmp, "target"))
if window:
    env.update(MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", SANDBOX_MUTE="1", MUSIC_MUTE="1")
for k in ("MAKEPAD_RUN_CACHE", "MAKEPAD_RUN_PRELINK", "WARM_EDIT_ONCE", "WARM_EDIT_LATE"):
    env.pop(k, None)
if cold:
    env["MAKEPAD_RUN_CACHE"] = "0"
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
    p = subprocess.Popen(cmd, cwd=pkg, env=dict(env, MAKEPAD_RUN_LIVE_READY=ready, MAKEPAD_RUN_RELOAD_TRACE="1", WARM_EDIT_LATE=late, MAKEPAD_REMOTE="1"), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    out, err = [], []
    def pump(f, acc):
        for line in f:
            acc.append(line.rstrip("\n"))
    threading.Thread(target=pump, args=(p.stdout, out), daemon=True).start()
    threading.Thread(target=pump, args=(p.stderr, err), daemon=True).start()
    def last():
        return next((l for l in reversed(out) if l.startswith("out tick ")), None)
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
            mark = len(err)
            save(first_path, first_base.replace(first_old, first_new))
            if not wait(lambda: any(": patched (" in l for l in err[mark:]), 20):
                row["outcome"] = "FIRST EDIT DID NOT LAND"
                return row, err
        t_save = time.time()
        mark = len(err)
        save(path, edited.replace(old, new))
        said = lambda: next((l for l in err[mark:] if ": patched (" in l or "a restart applies it" in l or "REFUSED" in l), None)
        wait(lambda: said() is not None, 20)
        line = said() or ""
        restart = "a restart applies it" in line
        if not restart:
            # the late readers' first call, once the edit is in
            open(late, "w").close()
        # a tick from after the patch: a value the edit keeps must be read by the new code
        tick = lambda: int(last().split()[2]) if last() else -1
        since = tick()
        landed = not restart and wait(lambda: tick() > since + 1 and shows_new(last(), probe, want), 2 if line else 0)
        # the landing check: old code still on a stack a second after the patch asks for a restart
        if not landed and not restart and line:
            restart = wait(lambda: any(re.search(STILL_RUNS, l) for l in err), 5)
        row["ms"] = int((time.time() - t_save) * 1e3)
        row["value"] = stale_reads(last() or "", probe, want) or want if probe in READS else value(last() or "", probe)
        row["patch"] = re.sub(r"^.*?: ", "", line)[:96]
        t = next((l for l in reversed(err) if l.startswith("reload-trace:")), "")
        row["trace"] = " ".join(x for x in t.split() if re.match(r"(rung|total_ms|watch_ms|sub\.look_ms|changed_consts|changed_statics|selected_slots|installed)=", x))
        if landed and probe in SECOND:
            old2, new2, want = SECOND[probe]
            mark = len(err)
            save(path, base.replace(old, new).replace(old2, new2))
            wait(lambda: any(": patched (" in l or "a restart applies it" in l or "REFUSED" in l for l in err[mark:]), 20)
            landed = wait(lambda: last() is not None and shows_new(last(), probe, want), 2)
            if not landed:
                row["outcome"], row["patch"] = "SECOND EDIT DID NOT LAND", next((l for l in err[mark:] if "a restart applies it" in l or "REFUSED" in l), "")[:96]
                return row, err
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
        elif not line:
            row["outcome"] = "no answer"
        else:
            row["outcome"] = "STALE"
        return row, err
    finally:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        p.wait()
        if row["outcome"] != "no start":
            open(late, "w").close()
            r = once("fresh-" + probe, "1", MAKEPAD_RUN_CACHE="readonly", WARM_EDIT_LATE=late) if not cold else once("fresh-" + probe, "1", WARM_EDIT_LATE=late)
            line = next((l for l in r.stdout.splitlines() if l.startswith("out tick ")), "")
            row["fresh"] = want if shows_new(line, probe, want, FRESH_READS) else stale_reads(line, probe, want, FRESH_READS)
            if not shows_new(line, probe, want, FRESH_READS):
                row["outcome"] += " + FRESH RUN STALE"
        save(path, base)
        for other, text in restore:
            save(other, text)


fails = 0
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


if not cold and (not only or "older_than_manifest" in only):
    why = older_than_manifest()
    print("%-23s %s" % ("older_than_manifest", why or "ok"))
    fails += 1 if why else 0
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
    fails += 0 if ok else 1
    print("%-23s %-15s %6s ms value=%-4s want=%-2s relaunch=%-4s fresh=%-4s %s | %s" % (probe, row["outcome"], row.get("ms", "-"), row["value"], want, row.get("relaunch", "-"), row.get("fresh", "-"), row["trace"], row["patch"]), flush=True)
    if not ok and "-v" in args:
        for l in [l for l in err[:-12] if "restart the app" in l or "new layout" in l] + err[-12:]:
            print("    | " + l[:400])
shutil.rmtree(tmp, ignore_errors=True)
print("cache_warm_const_edit: %s" % ("ok" if not fails else "%d stale" % fails))
sys.exit(1 if fails else 0)
