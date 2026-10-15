// F39: early exits (return / ? / break / continue / labeled break) drop the temporaries of every
// enclosing statement (if-let / match scrutinees); if-let bindings drop at the end of the block
use std::cell::{Cell, RefCell};
struct C { v: RefCell<Vec<(u32, u32)>> }
fn chk(c: &C) { c.v.borrow_mut().push((0, 0)); c.v.borrow_mut().pop(); }
fn g(c: &C) -> Option<u32> {
    if let Some(b) = c.v.borrow().get(0) {
        if b.0 == 1 { return Some(b.1); }
        return Some(0);
    }
    None
}
fn h(c: &C) -> u32 { match c.v.borrow().get(0) { Some(b) => return b.0, None => {} } 9 }
fn k(c: &C) -> u32 { let n = c.v.borrow().len() as u32; if c.v.borrow().len() > 0 { return n; } 0 }
#[test]
fn a() { let c = C { v: RefCell::new(vec![(1, 2)]) }; assert_eq!(g(&c), Some(2)); chk(&c); }
#[test]
fn b() { let c = C { v: RefCell::new(vec![(1, 2)]) }; assert_eq!(h(&c), 1); chk(&c); }
#[test]
fn d() { let c = C { v: RefCell::new(vec![(1, 2)]) }; assert_eq!(k(&c), 1); chk(&c); }
fn t1(c: &C) -> Option<u32> { let x = c.v.borrow().get(0)?.0; if c.v.borrow().get(5).copied().map(|p| p.0)? == 1 { return Some(9); } Some(x) }
#[test]
fn e_try() { let c = C { v: RefCell::new(vec![(1, 2)]) }; assert_eq!(t1(&c), None); chk(&c); }
#[test]
fn f_break() { let c = C { v: RefCell::new(vec![(1, 2)]) }; let mut n = 0; loop { match c.v.borrow().get(0) { Some(_) => break, None => {} } n += 1; } assert_eq!(n, 0); chk(&c); for _ in 0..2 { if let Some(_) = c.v.borrow().get(0) { continue; } } chk(&c); }
#[test]
fn g_labeled() { let c = C { v: RefCell::new(vec![(1, 2)]) }; let r = 'b: { if let Some(x) = c.v.borrow().get(0) { break 'b x.0; } 0 }; assert_eq!(r, 1); chk(&c); }
struct G(*const Cell<u32>);
impl Drop for G { fn drop(&mut self) { unsafe { (*self.0).set((*self.0).get() + 1) } } }
fn mk_g(c: &Cell<u32>) -> Option<G> { Some(G(c)) }
#[test]
fn c_iflet_nonniche() { let c = Cell::new(0); if let Some(g) = mk_g(&c) { let _ = &g; } assert_eq!(c.get(), 1); }
#[test]
fn d_iflet_value() { let c = Cell::new(0); let o = mk_g(&c); if let Some(_g) = o {} assert_eq!(c.get(), 1); }
#[test]
fn e_match_temp() { let c = Cell::new(0); match mk_g(&c) { Some(_g) => {}, None => {} } assert_eq!(c.get(), 1); }
// a by-value if-let binding moved out of the scrutinee temporary is not dropped again by the
// early exit (makepad_script format_value_brief: `if let Some(s) = f(|s| s.to_string()) { return .. }`)
fn mk_str(b: bool) -> Option<String> { if b { Some("abc".to_string()) } else { None } }
fn brief(b: bool) -> String {
    if let Some(s) = mk_str(b) {
        return format!("\"{}\"", s);
    }
    if let Some(t) = mk_str(true).map(|x| x + "d") { if t.len() == 4 { return t; } }
    "none".to_string()
}
#[test]
fn h_moved_binding_return() { assert_eq!(brief(true), "\"abc\""); assert_eq!(brief(false), "abcd"); }
