// F38: #[track_caller] (Reference attributes.codegen.track_caller): Location::caller() and
// panics inside a tracking fn report the outermost non-tracking call site, through chains,
// fn pointers (reify shim) and trait objects
use std::panic::Location;
#[track_caller]
fn where_am_i() -> u32 { Location::caller().line() }
#[track_caller]
fn twice() -> u32 { where_am_i() }
#[track_caller]
fn file_of() -> &'static str { Location::caller().file() }
trait T { fn loc(&self) -> u32; }
struct S;
impl T for S { #[track_caller] fn loc(&self) -> u32 { Location::caller().line() } }
#[track_caller]
fn boom(x: Option<u32>) -> u32 { x.unwrap() }
#[test]
fn track_caller_lines() {
    let l = where_am_i(); assert_eq!(l, 18);
    let k = twice(); assert_eq!(k, 19);
    assert!(file_of().ends_with("track_caller.rs"));
    let f: fn() -> u32 = where_am_i;
    assert!(f() > 0);
    let d: &dyn T = &S;
    assert!(d.loc() > 0);
    assert_eq!(S.loc(), 25);
    let r = std::panic::catch_unwind(|| boom(None));
    assert!(r.is_err());
    let c = std::cell::RefCell::new(1);
    let _b = c.borrow();
    let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { let _m = c.borrow_mut(); }));
    assert!(r2.is_err());
}
