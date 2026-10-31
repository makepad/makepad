// Commercial-lane blockers (Amp --check, 2026-10-04), each = rustc:
//  a/c/fo_*: catch_unwind(AssertUnwindSafe(&mut **Box<dyn FnMut()>)) -- R from the dyn Fn signature (platform thread.rs:2487)
//  b: closure param type from an F: Fn(Event) bound when the closure is a block's tail (live_reload_core lib.rs:77)
//  c_as_deref: a method's result normalized (Option<String>::as_deref() -> Option<&str>; ai hub peer_fetch.rs:180)
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
// 1. catch_unwind on `&mut **Box<dyn FnMut()>`
fn run_interval(cb: &mut Box<dyn FnMut() + Send>) -> bool { catch_unwind(AssertUnwindSafe(&mut **cb)).is_ok() }
#[test] fn a_catch_unwind_box_dyn() { let mut n = 0u32; { let mut b: Box<dyn FnMut() + Send> = Box::new(|| { }); assert!(run_interval(&mut b)); } n += 1; assert_eq!(n, 1); }
// 2. closure param type from an `F: Fn(Event)` bound on an associated fn
pub struct Event { pub mount: String, pub path: PathBuf }
pub struct Watcher { n: usize }
impl Watcher {
    pub fn start<F>(roots: Vec<u32>, on_event: F) -> Result<Self, String> where F: Fn(Event) + Send + Sync + 'static {
        on_event(Event { mount: "m".to_string(), path: PathBuf::from("/p") });
        Ok(Watcher { n: roots.len() })
    }
}
fn forward(mount: String, path: PathBuf, tag: &str) -> usize { mount.len() + path.as_os_str().len() + tag.len() }
#[test] fn b_closure_param_from_bound() {
    let tag = String::from("xy");
    let w = Watcher::start(vec![1, 2], { let tag = tag.clone(); move |event| { let _ = forward(event.mount, event.path, &tag); } });
    assert_eq!(w.unwrap().n, 2);
}
// 3. Option<String>::as_deref() result (&<String as Deref>::Target) compared and returned
fn check(sha: &Option<String>, actual: String) -> Result<Option<String>, String> {
    let Some(digest) = sha.as_deref() else { return Err("none".into()) };
    if actual == digest { return Ok(Some(actual)); }
    Err(format!("mismatch {} {}", actual, digest))
}
fn to_hex(b: &[u8]) -> String { b.iter().map(|x| format!("{:02x}", x)).collect() }
#[test] fn c_as_deref_target() {
    let fin = [0xabu8; 2];
    let actual = to_hex(&fin.clone());
    assert_eq!(check(&Some("abab".to_string()), actual).unwrap(), Some("abab".to_string()));
}

#[test] fn b2_direct_closure() { let w = Watcher::start(vec![1], move |event| { let _ = forward(event.mount, event.path, "t"); }); assert_eq!(w.unwrap().n, 1); }
fn call_once_g<F: FnOnce() -> R, R>(f: F) -> R { f() }
#[test] fn fo_ref_mut_dyn() { let mut n = 0; { let mut c = || n += 1; let d: &mut dyn FnMut() = &mut c; call_once_g(d); } assert_eq!(n, 1); }
#[test] fn fo_ref_mut_box_dyn() { let mut n = 0; { let mut b: Box<dyn FnMut() + '_> = Box::new(|| n += 2); call_once_g(&mut *b); } assert_eq!(n, 2); }
#[test] fn fo_assert_unwind() { let mut n = 0; { let mut b: Box<dyn FnMut() + '_> = Box::new(|| n += 3); let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut *b)); } assert_eq!(n, 3); }
