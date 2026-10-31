// Drop counts on early exits (`makepad test`). rustc: every test passes.
// a  REGRESSION from 9c6e9454d3 (F39 fix "early exits drop enclosing statement temporaries"): `return Some(d)` with
//    `d` moved out of the `if let` scrutinee drops the scrutinee too -> D dropped twice (a after return 1, end 2).
//    passes at babad85dd6. Likely the counter's SIGABRT "pointer being freed was not allocated" at 9c6e9454d3.
// i  (older, fails at babad85dd6 too) `if let Some(b) = opt_box { return *b; }`: moving out of a Box then returning
//    drops the D twice (after return 1, end 2) -- double drop / double free.
// e  (older) `for d in v { if .. { return d.0; } }`: the IntoIter is not dropped on return, the rest of the Vec leaks
//    (2 drops instead of 3).
// rustc: a moved-out binding / scrutinee place is not dropped again (MIR drop elaboration of moved paths); the loop
// iterator is dropped on every exit (Reference destructors.scope, destructors.moved).
use std::cell::{Cell, RefCell};
thread_local! { static DROPS: Cell<u32> = Cell::new(0); }
struct D(u32);
impl Drop for D { fn drop(&mut self) { DROPS.with(|d| d.set(d.get() + 1)); } }
fn drops() -> u32 { DROPS.with(|d| d.get()) }
fn reset() { DROPS.with(|d| d.set(0)); }
fn mk(x: u32) -> D { D(x) }
fn opt(x: u32) -> Option<D> { if x > 0 { Some(D(x)) } else { None } }

fn ret_moved_scrutinee(x: u32) -> Option<D> { if let Some(d) = opt(x) { return Some(d); } None }
fn ret_moved_match(x: u32) -> D { match opt(x) { Some(d) => return d, None => {} } D(0) }
fn q(x: u32) -> Option<u32> { let d = opt(x)?; Some(d.0) }
fn ret_temp_in_call(x: u32) -> u32 { if mk(x).0 > 0 { return 1; } 0 }
fn ret_from_loop(v: Vec<D>) -> u32 { for d in v { if d.0 == 2 { return d.0; } } 0 }
fn break_value(v: Vec<D>) -> u32 { let r = loop { for d in v.iter() { if d.0 == 2 { break; } } break 7; }; r }
fn ret_field_of_temp(x: u32) -> u32 { match opt(x).map(|d| d.0) { Some(n) => return n, None => 0 } }
fn ret_string(s: &RefCell<Vec<String>>) -> Option<String> { if let Some(x) = s.borrow().get(0) { return Some(x.clone()); } None }
fn ret_moved_out_of_box(b: Option<Box<D>>) -> D { if let Some(b) = b { return *b; } D(0) }

#[test] fn a() { reset(); { let r = ret_moved_scrutinee(1); assert!(drops() == 0); drop(r); } assert!(drops() == 1); }
#[test] fn b() { reset(); { let r = ret_moved_match(1); assert!(drops() == 0); drop(r); } assert!(drops() == 1); }
#[test] fn c() { reset(); assert!(q(1) == Some(1)); assert!(drops() == 1); assert!(q(0).is_none()); assert!(drops() == 1); }
#[test] fn d() { reset(); assert!(ret_temp_in_call(1) == 1); assert!(drops() == 1); }
#[test] fn e() { reset(); assert!(ret_from_loop(vec![D(1), D(2), D(3)]) == 2); assert!(drops() == 3); }
#[test] fn f() { reset(); assert!(break_value(vec![D(1), D(2)]) == 7); assert!(drops() == 2); }
#[test] fn g() { reset(); assert!(ret_field_of_temp(3) == 3); assert!(drops() == 1); }
#[test] fn h() { let c = RefCell::new(vec!["x".to_string()]); assert!(ret_string(&c).unwrap() == "x"); assert!(c.try_borrow_mut().is_ok()); }
#[test] fn i() { reset(); { let r = ret_moved_out_of_box(Some(Box::new(D(4)))); assert!(drops() == 0); drop(r); } assert!(drops() == 1); }
