//! Live edits that add or remove items: a `use` or a const in a fn body that is added or
//! replaced, an item a macro makes in a fn body, a removed impl (inherent, derived, a trait impl
//! code reaches without naming it, one another impl replaces, a `Drop` impl), and a statement
//! attribute on a `let`. After a patch the next call runs the new code; an edit the running
//! code cannot take restarts, and a program that no longer compiles is refused.
use crate::reload::Rung;
use crate::reload_async_tests::{in_child, live_app, LIVE_UNIT};

const LIB: &str = r#"mod other;
pub use other::*;
use std::sync::atomic::{AtomicU64, Ordering};

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
/// tools/run/fixture/hot_fail.py), which never parks.
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
        });
    });
}

#[test]
fn edits_of_items_apply_live() {
    in_child("reload_items_tests::edits_of_items_apply_live", &["patch", "restart"], |case| {
        if case == "restart" {
            let lib = format!("{LIB}{SPEAK}");
            live_app("items-restart", &[("lib.rs", &lib), ("other.rs", OTHER)], &["talk"], |call, edit| {
                assert_eq!(call("talk"), 3);
                // a trait object of `Dog` holds the removed impl's `say`: the code that makes them
                // is not known to the edit, and `Dog` stays -- the edit restarts
                let outcome = edit("lib.rs", &[("impl Speak for Dog {\n    fn say(&self) -> u64 { 3 }\n}\n", ""), ("let d: Box<dyn Speak> = Box::new(Dog);\n    *out = d.say();", "*out = 4;")]);
                assert_eq!(outcome.rung, Rung::Restart, "{:?}", outcome.lines);
                assert_eq!(call("talk"), 3, "nothing of the edit installed");
            });
            return;
        }
        let names = ["sig", "uses_gone", "same", "same_gen", "drops", "cfg_let", "tls"];
        live_app("items-patch", &[("lib.rs", LIB), ("other.rs", OTHER)], &names, |call, edit| {
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
            // a `let` with a statement attribute in a match arm's block
            assert_eq!(call("cfg_let"), 1);
            patch(&mut *edit, "lib.rs", &[("            let mut v = vec![1u64];\n            #[cfg(feature = \"absent\")]\n            v.push(2);\n            v.len() as u64", "            let v = vec![1u64];\n            // a crate without the feature keeps the vector\n            #[cfg(feature = \"absent\")]\n            let v = { let mut v = v; v.push(2); v };\n            v.len() as u64 + 10")]);
            assert_eq!(call("cfg_let"), 11);
            // items a macro makes in a fn body
            patch(&mut *edit, "lib.rs", &[("pub fn tls(out: &mut u64) {\n    *out = 1;\n}", "pub fn tls(out: &mut u64) {\n    thread_local! { static T: std::cell::Cell<u64> = std::cell::Cell::new(5); }\n    *out = T.with(|t| t.get());\n}")]);
            assert_eq!(call("tls"), 5);
        });
    });
}
