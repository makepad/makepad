//! Live edits that add or remove items: a `use` or a const in a fn body that is added or
//! replaced, an item a macro makes in a fn body, a removed impl (inherent, derived, a trait impl
//! code reaches without naming it, one another impl replaces, a `Drop` impl), and a statement
//! attribute on a `let`. After a patch the next call runs the new code; an edit the running
//! code cannot take restarts, and a program that no longer compiles is refused.
use crate::reload::Rung;
use crate::reload_async_tests::{in_child, live_app, LIVE_UNIT};

/// A module whose items are in part an included file's (`include!` at item position: PARTS).
const INCLUDER: &str = r#"
pub mod parts {
    include!("parts.rs");
    pub fn base() -> u64 { 1 }
}

pub fn part(out: &mut u64) {
    *out = parts::base() + parts::add();
}
"#;
const PARTS: &str = "pub fn add() -> u64 { 2 }\n";

const LIB: &str = r#"mod other;
pub use other::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub static LEVEL: Mutex<u64> = Mutex::new(0);
pub fn level(out: &mut u64) {
    let mut l = LEVEL.lock().unwrap();
    *l += 1;
    *out = *l as u64;
}

pub fn sig(out: &mut u64) {
    *out = helper(1);
}

fn helper(x: u64) -> u64 {
    x + 10
}

pub struct Unused;
impl Unused {
    pub fn dead(&self) -> u64 { 1 }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Gone { n: u64 }
impl Gone {
    pub fn get(&self) -> u64 { self.n + 5 }
}

pub fn uses_gone(out: &mut u64) {
    let g = Gone::default();
    let h = g.clone();
    *out = h.get() + if g == h { 1 } else { 0 };
}

#[derive(PartialEq)]
pub struct Pt(pub u64);

pub static DROPS: AtomicU64 = AtomicU64::new(0);
pub struct Noisy;
impl Drop for Noisy {
    fn drop(&mut self) { DROPS.fetch_add(1, Ordering::Relaxed); }
}

pub fn drops(out: &mut u64) {
    {
        let _n = Noisy;
    }
    *out = DROPS.load(Ordering::Relaxed);
}

pub fn cfg_let(out: &mut u64) {
    *out = match *out {
        0 => {
            let mut v = vec![1u64];
            #[cfg(feature = "absent")]
            v.push(2);
            v.len() as u64
        }
        _ => 0,
    };
}

pub fn tls(out: &mut u64) {
    *out = 1;
}
"#;

/// A file the edits do not touch: it reaches `Pt`'s `PartialEq` impl through `==` only.
const OTHER: &str = r#"pub fn same(out: &mut u64) {
    *out = if crate::Pt(1) == crate::Pt(1) { 1 } else { 2 };
}

fn eq_of<T: PartialEq>(a: &T, b: &T) -> bool {
    a == b
}

pub fn same_gen(out: &mut u64) {
    *out = if eq_of(&crate::Pt(1), &crate::Pt(1)) { 1 } else { 2 };
}
"#;

/// (the "restart" case's) `Dog` held as a trait object
const SPEAK: &str = r#"
pub trait Speak { fn say(&self) -> u64; }
pub struct Dog;
impl Speak for Dog {
    fn say(&self) -> u64 { 3 }
}

pub fn talk(out: &mut u64) {
    let d: Box<dyn Speak> = Box::new(Dog);
    *out = d.say();
}
"#;

const SPEAK_CONST: &str = r#"
pub trait Speak { fn say(&self) -> u64; }
pub struct Dog;
impl Speak for Dog {
    fn say(&self) -> u64 { 3 }
}

static LOUD: &dyn Speak = &Dog;

pub fn talk(out: &mut u64) {
    *out = LOUD.say();
}
"#;

const HOLDER: &str = r#"pub enum Note { Thickness(u64), Other }

static mut STORED: Option<Box<dyn Fn() -> u64>> = None;

pub fn store(out: &mut u64) {
    let k = 3u64;
    let closure: Box<dyn Fn() -> u64> = Box::new(move || k + 1);
    unsafe { STORED = Some(closure); }
    *out = 1;
}

pub fn run_stored(out: &mut u64) {
    *out = unsafe { (*std::ptr::addr_of!(STORED)).as_ref().unwrap()() };
}

pub fn other(out: &mut u64) {
    let note = Note::Thickness(5);
    *out = match note { Note::Thickness(n) => n, Note::Other => 0 };
}
"#;

/// A closure's first call after a refused edit: it is lowered from the checked body of the
/// version that made it, an older one than the running text when an edit of another fn of its
/// file was applied before. A refused edit drops what was built from its own text, not that.
#[test]
fn a_refused_enum_edit_keeps_the_older_body_a_closure_needs() {
    in_child("reload_items_tests::a_refused_enum_edit_keeps_the_older_body_a_closure_needs", &["only"], |_| {
        live_app("refused-keeps-body", &[("lib.rs", HOLDER)], &["store", "run_stored", "other"], |call, edit| {
            assert_eq!(call("store"), 1, "the closure is made, not called");
            assert_eq!(call("other"), 5);
            let applied = edit("lib.rs", &[("Note::Thickness(5)", "Note::Thickness(6)")]);
            assert_eq!(applied.rung, Rung::Patch, "{:?}", applied.lines);
            assert_eq!(call("other"), 6);
            // a variant of the enum changes (running values could hold it) and the closure's
            // owner is edited with it
            let refused = edit("lib.rs", &[("Thickness", "Width"), ("k + 1", "k + 2")]);
            assert_eq!(refused.rung, Rung::Restart, "{:?}", refused.lines);
            assert_eq!(call("run_stored"), 4, "the closure runs the code of the version that made it");
            assert_eq!(call("other"), 6, "nothing of the refused edit installed");
        });
    });
}

/// A closure of a replaced version whose body is gone is no error of the program, so no edit of
/// it can resume the call: it is an internal compiler error, a failed hot load (jit.rs compile_ice,
/// tools/run/fixture/hot_fail.py), which never parks. Its one line carries what the report needs
/// (jit.rs missing_owner_facts): the owner, both versions, the bodies held, what the edit kept.
#[test]
fn a_closure_of_a_replaced_version_without_its_body_is_an_internal_error() {
    in_child("reload_items_tests::a_closure_of_a_replaced_version_without_its_body_is_an_internal_error", &["only"], |_| {
        live_app("closure-no-body", &[("lib.rs", HOLDER)], &["store", "run_stored", "other"], |call, edit| {
            assert_eq!(call("store"), 1);
            let applied = edit("lib.rs", &[("Note::Thickness(5)", "Note::Thickness(6)")]);
            assert_eq!(applied.rung, Rung::Patch, "{:?}", applied.lines);
            let up = LIVE_UNIT.load(std::sync::atomic::Ordering::Relaxed) as *mut crate::jit::Unit;
            crate::owner::call(crate::owner::Class::Program, || unsafe {
                (*up).bodies.clear();
                (*up).old_bodies.clear();
            });
            let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call("run_stored"))).unwrap_err();
            let message = failed.downcast_ref::<String>().cloned().unwrap_or_default();
            assert!(message.contains("closure parent body missing") && crate::jit::is_ice_stub_message(message.trim_start_matches("JIT call panicked: ")), "{message}");
            for fact in ["owner `", "store", "lib.rs:", "the closure's version v", "replaced by v", "owner bodies held: none", "the edit that replaced it kept this body", "owner code ", "inlined into "] {
                assert!(message.contains(fact), "{fact:?} in {message}");
            }
        });
    });
}

#[test]
fn edits_of_items_apply_live() {
    in_child("reload_items_tests::edits_of_items_apply_live", &["patch", "dyn_maker", "dyn_const", "const_reader"], |case| {
        if case == "const_reader" {
            let lib = "use std::cell::Cell;\npub use self::C as CELL_VALUE;\nmacro_rules! borrow_c { () => { &crate::CELL_VALUE } }\npub mod reader;\npub const C: Option<Cell<u32>> = None;\n";
            let reader = "use crate::C;\nuse std::cell::Cell;\npub fn r() -> &'static Option<Cell<u32>> { &C }\n";
            live_app("const-reader", &[("lib.rs", lib), ("reader.rs", reader)], &[], |_, edit| {
                let check_reader = || crate::owner::call(crate::owner::Class::Program, || {
                    let u = unsafe { &*(LIVE_UNIT.load(std::sync::atomic::Ordering::Relaxed) as *const crate::jit::Unit) };
                    let r = u.prog.defs.iter().enumerate().map(|(i, _)| crate::program::DefId(i as u32))
                        .find(|&d| u.prog.name(d) == "r" && u.prog.file_paths[u.prog.def(d).file as usize].ends_with("reader.rs")).unwrap();
                    let check = crate::check::live_lints(&u.prog, &u.tcx, &[r]);
                    assert!(check.borrows.is_empty(), "the reference to None is promotable: {:?}", check.borrows);
                });
                check_reader();
                let bad = edit("lib.rs", &[("= None;", "= Some(Cell::new(1));")]);
                assert_eq!(bad.rung, Rung::Refuse, "the const edit invalidates r's promotion: {:?}", bad.lines);
                assert!(bad.lines.iter().any(|l| l.contains("E0515") && l.contains("reader.rs:3:")), "{:?}", bad.lines);
                let fixed = edit("lib.rs", &[("= Some(Cell::new(1));", "= None;")]);
                assert_eq!(fixed.rung, Rung::Patch, "{:?}", fixed.lines);
                check_reader();
                let via_macro = edit("reader.rs", &[("&C }", "borrow_c!() }")]);
                assert_eq!(via_macro.rung, Rung::Patch, "{:?}", via_macro.lines);
                check_reader();
                let bad = edit("lib.rs", &[("= None;", "= Some(Cell::new(1));")]);
                assert_eq!(bad.rung, Rung::Refuse, "the macro reads the changed const: {:?}", bad.lines);
                assert!(bad.lines.iter().any(|l| l.contains("E0515")), "{:?}", bad.lines);
                // One save changes both files: r goes back to reading C directly.
                let up = LIVE_UNIT.load(std::sync::atomic::Ordering::Relaxed) as *mut crate::jit::Unit;
                let files = crate::owner::call(crate::owner::Class::Program, || {
                    let prog = unsafe { &(*up).prog };
                    prog.current_source_files(|p| {
                        let p = std::path::Path::new(p);
                        p.ends_with("app/src/lib.rs") || p.ends_with("app/src/reader.rs")
                    })
                });
                assert_eq!(files.len(), 2);
                for check_first in [false, true] {
                    crate::jit::CHECK_FIRST.store(check_first, std::sync::atomic::Ordering::Relaxed);
                    let edits = files.iter().map(|(path, &file)| {
                        let text = if path.ends_with("reader.rs") { reader.to_owned() } else { lib.replace("= None;", "= Some(Cell::new(1));") };
                        crate::reload::Edit { file, path: path.clone(), src: text.into_bytes() }
                    }).collect();
                    let both = unsafe { crate::reload::apply_update(up, edits) };
                    assert_eq!(both.rung, Rung::Refuse, "combined save (check_first={check_first}): {:?}", both.lines);
                    assert_eq!(both.lines.join("\n").matches("E0515").count(), 1, "check_first={check_first}: {:?}", both.lines);
                }
            });
            return;
        }
        if case == "dyn_maker" {
            let lib = format!("{LIB}{SPEAK}");
            live_app("items-dyn-maker", &[("lib.rs", &lib), ("other.rs", OTHER)], &["talk"], |call, edit| {
                assert_eq!(call("talk"), 3);
                // the trait objects of `Dog` that hold the removed impl's `say` keep their vtable
                // and its code; the code that makes them (Unit::vtable_makers: `talk`) is rebuilt
                // with the edit, which makes none
                let outcome = edit("lib.rs", &[("impl Speak for Dog {\n    fn say(&self) -> u64 { 3 }\n}\n", ""), ("let d: Box<dyn Speak> = Box::new(Dog);\n    *out = d.say();", "*out = 4;")]);
                assert_eq!(outcome.rung, Rung::Patch, "{:?}", outcome.lines);
                assert_eq!(call("talk"), 4);
            });
            return;
        }
        if case == "dyn_const" {
            let lib = format!("{LIB}{SPEAK_CONST}");
            live_app("items-dyn-const", &[("lib.rs", &lib), ("other.rs", OTHER)], &["talk"], |call, edit| {
                assert_eq!(call("talk"), 3);
                // the only trait object of `Dog` is a static's value, which the edit removes with
                // its reader: the object keeps its vtable and its code, and nothing reads it again
                let outcome = edit("lib.rs", &[("impl Speak for Dog {\n    fn say(&self) -> u64 { 3 }\n}\n", ""), ("static LOUD: &dyn Speak = &Dog;\n", ""), ("*out = LOUD.say();", "*out = 4;")]);
                assert_eq!(outcome.rung, Rung::Patch, "{:?}", outcome.lines);
                assert_eq!(call("talk"), 4);
            });
            return;
        }
        let names = ["sig", "uses_gone", "same", "same_gen", "drops", "cfg_let", "tls", "level", "part"];
        let lib = format!("{LIB}{INCLUDER}");
        live_app("items-patch", &[("lib.rs", &lib), ("other.rs", OTHER), ("parts.rs", PARTS)], &names, |call, edit| {
            let patch = |edit: &mut dyn FnMut(&str, &[(&str, &str)]) -> crate::reload::Outcome, file: &str, subs: &[(&str, &str)]| {
                let outcome = edit(file, subs);
                assert_eq!(outcome.rung, Rung::Patch, "{subs:?}: {:?}", outcome.lines);
            };
            assert_eq!((call("sig"), call("uses_gone"), call("same"), call("same_gen"), call("drops")), (11, 6, 1, 1, 1));
            // a `use` and a const in the body of a fn replaced for its new signature
            patch(&mut *edit, "lib.rs", &[("fn helper(x: u64) -> u64 {\n    x + 10\n}", "fn helper(x: u32) -> u64 {\n    use std::convert::TryFrom;\n    const K: u64 = 20;\n    u64::try_from(x).unwrap() + K\n}")]);
            assert_eq!(call("sig"), 21);
            // ... and removed with that body when it is replaced again
            patch(&mut *edit, "lib.rs", &[("fn helper(x: u32) -> u64 {\n    use std::convert::TryFrom;\n    const K: u64 = 20;\n    u64::try_from(x).unwrap() + K\n}", "fn helper(x: u64) -> u64 {\n    x + 30\n}")]);
            assert_eq!(call("sig"), 31);
            // a `use` in the body of a fn of an added impl
            patch(&mut *edit, "lib.rs", &[("*out = helper(1);", "*out = helper(1) + Fresh.val();"), ("pub struct Unused;", "pub struct Fresh;\nimpl Fresh {\n    fn val(&self) -> u64 {\n        use std::ops::Add;\n        2u64.add(3)\n    }\n}\n\npub struct Unused;")]);
            assert_eq!(call("sig"), 36);
            // an inherent impl nothing calls
            patch(&mut *edit, "lib.rs", &[("impl Unused {\n    pub fn dead(&self) -> u64 { 1 }\n}\n", "")]);
            assert_eq!(call("sig"), 36);
            // a type with its derives and impls, all removed with the code using them
            patch(&mut *edit, "lib.rs", &[
                ("#[derive(Clone, Default, Debug, PartialEq)]\npub struct Gone { n: u64 }\nimpl Gone {\n    pub fn get(&self) -> u64 { self.n + 5 }\n}\n", ""),
                ("    let g = Gone::default();\n    let h = g.clone();\n    *out = h.get() + if g == h { 1 } else { 0 };", "    *out = 77;"),
            ]);
            assert_eq!(call("uses_gone"), 77);
            // a derived impl that a file the edit does not touch reaches through `==`: that code
            // no longer compiles, and the edit is refused (before, it went on calling the old impl)
            let outcome = edit("lib.rs", &[("#[derive(PartialEq)]\npub struct Pt", "pub struct Pt")]);
            assert_eq!(outcome.rung, Rung::Refuse, "{:?}", outcome.lines);
            assert!(outcome.lines.iter().any(|l| l.contains("other.rs")), "{:?}", outcome.lines);
            assert_eq!((call("same"), call("same_gen")), (1, 1));
            // the derive replaced by an impl of its own: `==` there and in the generic code runs it
            patch(&mut *edit, "lib.rs", &[("pub struct Pt(pub u64);", "pub struct Pt(pub u64);\nimpl PartialEq for Pt {\n    fn eq(&self, o: &Self) -> bool { self.0 != o.0 }\n}")]);
            assert_eq!((call("same"), call("same_gen")), (2, 2), "the untouched callers run the new impl");
            // a `Drop` impl removed: its type is no longer dropped by it
            patch(&mut *edit, "lib.rs", &[("impl Drop for Noisy {\n    fn drop(&mut self) { DROPS.fetch_add(1, Ordering::Relaxed); }\n}\n", "")]);
            assert_eq!(call("drops"), 1, "no more drop");
            // a static keeps its running value through patches of the code that reads it, of its
            // initializer's text and of the code that writes it (a live patch seemed to empty an
            // app's static once, 11-22; none of these reproduces it)
            patch(&mut *edit, "lib.rs", &[("    *out = DROPS.load(Ordering::Relaxed);", "    *out = DROPS.load(Ordering::Relaxed) + 100;")]);
            assert_eq!(call("drops"), 101, "the patched reader reads the running value");
            patch(&mut *edit, "lib.rs", &[("AtomicU64::new(0);", "AtomicU64::new(/* drops */ 0);")]);
            assert_eq!(call("drops"), 101, "the respelled initializer does not run again");
            patch(&mut *edit, "lib.rs", &[("pub struct Noisy;\n", "pub struct Noisy;\nimpl Drop for Noisy {\n    fn drop(&mut self) { DROPS.fetch_add(10, Ordering::Relaxed); }\n}\n")]);
            assert_eq!(call("drops"), 111, "the patched writer adds onto the running value");
            // a `let` with a statement attribute in a match arm's block
            assert_eq!(call("cfg_let"), 1);
            patch(&mut *edit, "lib.rs", &[("            let mut v = vec![1u64];\n            #[cfg(feature = \"absent\")]\n            v.push(2);\n            v.len() as u64", "            let v = vec![1u64];\n            // a crate without the feature keeps the vector\n            #[cfg(feature = \"absent\")]\n            let v = { let mut v = v; v.push(2); v };\n            v.len() as u64 + 10")]);
            assert_eq!(call("cfg_let"), 11);
            // items a macro makes in a fn body
            patch(&mut *edit, "lib.rs", &[("pub fn tls(out: &mut u64) {\n    *out = 1;\n}", "pub fn tls(out: &mut u64) {\n    thread_local! { static T: std::cell::Cell<u64> = std::cell::Cell::new(5); }\n    *out = T.with(|t| t.get());\n}")]);
            assert_eq!(call("tls"), 5);
            // two items of one name in one body, or in one module, are E0428, as a fresh build
            // refuses them (such an edit applied, a lookup taking one for both): the edit is
            // refused beside the running item, the running code goes on, and the edit that names
            // the second ones apart applies. The running items keep their names through the
            // refusal: the refused second `helper` had taken it in the module, and unbound left it
            // to nothing (a later patch of `helper` never reached `sig`, which went on with the old)
            patch(&mut *edit, "lib.rs", &[("    *out = T.with(|t| t.get());", "    static ONE: u64 = 1;\n    *out = T.with(|t| t.get()) + ONE;")]);
            assert_eq!(call("tls"), 6);
            let outcome = edit("lib.rs", &[("    static ONE: u64 = 1;\n", "    static ONE: u64 = 1;\n    static ONE: u64 = 2;\n"), ("pub fn tls(", "pub static TWICE: u64 = 1;\npub static TWICE: u64 = 2;\n\npub fn tls("), ("fn helper(x: u64) -> u64 {\n    x + 30\n}\n", "fn helper(x: u64) -> u64 {\n    x + 30\n}\nfn helper(x: u64) -> u64 {\n    x + 40\n}\n")]);
            assert_eq!(outcome.rung, Rung::Refuse, "{:?}", outcome.lines);
            for name in ["ONE", "TWICE", "helper"] {
                assert!(outcome.lines.iter().any(|l| l.contains(&format!("E0428: the name `{name}` is defined multiple times"))), "{name}: {:?}", outcome.lines);
            }
            assert_eq!(call("tls"), 6, "the running code goes on");
            patch(&mut *edit, "lib.rs", &[("    static ONE: u64 = 2;\n", "    static TWO: u64 = 2;\n"), ("pub static TWICE: u64 = 2;\n", "pub static TWO: u64 = 2;\n"), ("t.get()) + ONE;", "t.get()) + ONE + TWO + TWICE;"), ("fn helper(x: u64) -> u64 {\n    x + 40\n}\n", "")]);
            assert_eq!(call("tls"), 9, "the block's TWO shadows the module's");
            assert_eq!(call("sig"), 36);
            patch(&mut *edit, "lib.rs", &[("    x + 30\n}", "    x + 50\n}")]);
            assert_eq!(call("sig"), 56, "the running `helper`'s patch reaches its caller after the refusal");
            // an included file's items are its includer's module's: its save is an edit of the
            // includer (reload.rs include_edits), judged with the includer's items. The edits above
            // moved the include's call, and the offsets of the includer's earlier versions lie in
            // other items now: a plain save patches (those items' code "included" the file, and
            // unchanged statics' initializers took the restart rung). A name the included file
            // gives twice, or one the includer's module has already, is refused. (The includer's
            // second `base` is at its line, as rustc says; an error in the included text is at the
            // include's call, where rustc says the included file's line, a fresh build alike.)
            let save_parts = |text: &str| {
                let up = LIVE_UNIT.load(std::sync::atomic::Ordering::Relaxed) as *mut crate::jit::Unit;
                let edits = crate::owner::call(crate::owner::Class::Program, || {
                    let prog = unsafe { &(*up).prog };
                    let (lib, _) = prog.current_source_files(|p| std::path::Path::new(p).ends_with("app/src/lib.rs")).into_iter().next().unwrap();
                    let parts = std::path::Path::new(&lib).with_file_name("parts.rs").to_string_lossy().into_owned();
                    std::fs::write(&parts, text).unwrap();
                    let mut edits = crate::reload::include_edits(prog, &[parts.clone()], &[]);
                    assert_eq!(edits.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), [lib.as_str()], "the includer is the edit");
                    // the saved text goes with the batch, as the watcher sends it
                    edits.push(crate::reload::Edit { file: u32::MAX, path: parts, src: text.as_bytes().to_vec() });
                    edits
                });
                unsafe { crate::reload::apply_update(up, edits) }
            };
            assert_eq!(call("part"), 3);
            let outcome = save_parts("pub fn add() -> u64 { 3 }\n");
            assert_eq!(outcome.rung, Rung::Patch, "{}: {:?}", "a plain save of the included file", outcome.lines);
            assert_eq!(call("part"), 4);
            let outcome = save_parts("pub fn add() -> u64 { 3 }\npub fn add() -> u64 { 4 }\npub fn base() -> u64 { 5 }\n");
            assert_eq!(outcome.rung, Rung::Refuse, "{:?}", outcome.lines);
            for (name, at) in [("add", ""), ("base", "lib.rs:")] {
                assert!(outcome.lines.iter().any(|l| l.contains(&format!("E0428: the name `{name}` is defined multiple times")) && l.contains(at)), "{name} at {at}: {:?}", outcome.lines);
            }
            assert_eq!(call("part"), 4, "the running code goes on");
            let outcome = save_parts("pub fn add() -> u64 { 5 }\n");
            assert_eq!(outcome.rung, Rung::Patch, "{}: {:?}", "the fixed save", outcome.lines);
            assert_eq!(call("part"), 6, "the fixed save applies");
            // Qualifying the type keeps the stored value. A different element type needs a restart.
            assert_eq!(call("level"), 1);
            assert_eq!(call("level"), 2);
            patch(&mut *edit, "lib.rs", &[("pub static LEVEL: Mutex<u64>", "pub static LEVEL: std::sync::Mutex<u64>")]);
            assert_eq!(call("level"), 3);
            let outcome = edit("lib.rs", &[("std::sync::Mutex<u64> = Mutex::new(0)", "std::sync::Mutex<u32> = Mutex::new(0)")]);
            assert_eq!(outcome.rung, Rung::Restart, "{:?}", outcome.lines);
            assert!(outcome.lines.iter().any(|line| line.contains("changed its type from")), "{:?}", outcome.lines);
            assert_eq!(call("level"), 4);
        });
    });
}

/// An import only the items inside a fn body use (a struct and its `Drop` impl): an edit that
/// removes those inner items keeps the import, and the next removes the import too. Neither edit
/// leaves code that names it, so both patch: the old version's inner items are not checked again
/// against the new text (M83: their bodies failed with "undeclared type" after the import went).
const INNER: &str = r#"use std::time::Duration;

pub fn holds(out: &mut u64) {
    struct Guard(u64);
    impl Drop for Guard {
        fn drop(&mut self) { let _ = Duration::from_millis(self.0); }
    }
    let _g = Guard(1);
    *out = 1;
}
"#;

/// A body whose nested fn returns `impl Trait` (its opaque type, which captures the elided
/// lifetime), after an item before it in the body, and an inner block with an item of its own
/// that calls the nested fn: platform remote.rs `publish_windows`, whose edit removing such a
/// `static` stopped with an internal compiler error (rpitit.rs "collected opaque captures",
/// 11-27). The edit gives the body's block a new module and the nested fn a new def; the inner
/// block's module, carried over by its unchanged items, still looked names up in the old one.
const NESTED_OPAQUE: &str = r#"pub mod imp {
    use std::sync::OnceLock;
    pub struct Info { pub id: usize, pub title: String, pub visible: bool }
    pub fn publish(out: &mut u64) {
        static HIDDEN: OnceLock<bool> = OnceLock::new();
        let hidden = *HIDDEN.get_or_init(|| false);
        let windows: Vec<Info> = vec![Info { id: 1, title: "a".into(), visible: !hidden }];
        fn advertised(windows: &[Info]) -> impl Iterator<Item = (usize, &str)> {
            windows.iter().filter(|w| w.visible).map(|w| (w.id, w.title.as_str()))
        }
        *out = advertised(&windows).count() as u64 + 10;
        if *out > 0 {
            // an item in an inner block: the block keeps its module through the edit
            const ONE: u64 = 1;
            *out += advertised(&windows).count() as u64 * ONE;
        }
    }
}
pub fn evens(out: &mut u64) { imp::publish(out) }
"#;

#[test]
fn removed_inner_items_are_not_checked_against_the_next_edit() {
    in_child("reload_items_tests::removed_inner_items_are_not_checked_against_the_next_edit", &["two_saves", "one_save", "nested_opaque"], |case| {
        if case == "nested_opaque" {
            live_app("inner-opaque", &[("lib.rs", NESTED_OPAQUE)], &["evens"], |call, edit| {
                assert_eq!(call("evens"), 12);
                let outcome = edit("lib.rs", &[
                    ("        static HIDDEN: OnceLock<bool> = OnceLock::new();\n        let hidden = *HIDDEN.get_or_init(|| false);\n", ""),
                    ("visible: !hidden", "visible: true"),
                    ("+ 10;", "+ 20;"),
                ]);
                assert_eq!(outcome.rung, Rung::Patch, "an item removed before a nested fn returning impl Trait: {:?}", outcome.lines);
                assert_eq!(call("evens"), 22, "the inner block calls the edited body's nested fn");
            });
            return;
        }
        live_app("inner-items", &[("lib.rs", INNER)], &["holds"], |call, edit| {
            assert_eq!(call("holds"), 1);
            let inner = "    struct Guard(u64);\n    impl Drop for Guard {\n        fn drop(&mut self) { let _ = Duration::from_millis(self.0); }\n    }\n    let _g = Guard(1);\n    *out = 1;";
            let subs: &[(&str, &str)] = if case == "two_saves" {
                let first = edit("lib.rs", &[(inner, "    *out = 2;")]);
                assert_eq!(first.rung, Rung::Patch, "{:?}", first.lines);
                assert_eq!(call("holds"), 2);
                &[("use std::time::Duration;\n", ""), ("*out = 2;", "*out = 3;")]
            } else {
                &[(inner, "    *out = 3;"), ("use std::time::Duration;\n", "")]
            };
            let outcome = edit("lib.rs", subs);
            assert_eq!(outcome.rung, Rung::Patch, "{case}: {:?}", outcome.lines);
            assert_eq!(call("holds"), 3);
        });
    });
}

/// One file declared as two modules (`#[path]` on both) is two versions, each parsed as its
/// module: a live edit of the file reaches both (relax issue 135's shape; the edit's fan-out took
/// one version per crate, and the other module ran its old code). `module_path!()` tells the
/// versions apart: each is its own module's after the edit too.
#[test]
fn a_live_edit_of_a_file_declared_as_two_modules_reaches_both() {
    in_child("reload_items_tests::a_live_edit_of_a_file_declared_as_two_modules_reaches_both", &["only"], |_| {
        let lib = "#[path = \"shared.rs\"]\nmod first;\n#[path = \"shared.rs\"]\nmod second;\n\npub fn first_value(out: &mut u64) {\n    *out = first::value();\n}\n\npub fn second_value(out: &mut u64) {\n    *out = second::value();\n}\n";
        let shared = "pub fn value() -> u64 {\n    module_path!().len() as u64 * 10 + 1\n}\n";
        live_app("two-module-file", &[("lib.rs", lib), ("shared.rs", shared)], &["first_value", "second_value"], |call, edit| {
            assert_eq!((call("first_value"), call("second_value")), (151, 161), "live_app::first, live_app::second");
            let applied = edit("shared.rs", &[("* 10 + 1", "* 10 + 2")]);
            assert_eq!(applied.rung, Rung::Patch, "{:?}", applied.lines);
            assert_eq!((call("first_value"), call("second_value")), (152, 162), "each module runs the edit, as its own module");
        });
    });
}
