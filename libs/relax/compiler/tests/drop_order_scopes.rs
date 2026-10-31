//@ edition: 2024
// Drop order of temporaries against bindings (sequences = rustc --edition 2024 --test):
// - parameter patterns (`fn f((_x, _): (D, D))`, closures too): the argument is a variable
//   declared before the pattern's bindings, so it drops after them (closures leaked it);
// - operands already evaluated when a later one unwinds / exits (`(D(4), &D(5), panic)`) drop
//   before the statement's temporaries (their scope is the using expression's, nested inside);
// - arrays iterated by value (`for (_a, _) in [(D(6), D(5))]`): items whose type needs drop go
//   through `IntoIterator for [T; N]` (the index loop leaked `_` parts and the rest after a break);
// - edition 2024 `if let` / match-guard `let` chains: each scrutinee's temporaries drop with the
//   chain's bindings in reverse creation order, at the end of the then block / arm.
use std::cell::RefCell;
thread_local! { static LOG: RefCell<Vec<u32>> = RefCell::new(Vec::new()); }
fn take() -> Vec<u32> { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())) }
struct D(u32);
impl Drop for D { fn drop(&mut self) { LOG.with(|l| l.borrow_mut().push(self.0)); } }
fn ok(m: u32) -> Result<D, D> { Ok(D(m)) }
fn boom() { panic!() }

fn params((_x, _): (D, D), (_, _y): (D, D)) {}

#[test]
fn parameter_patterns() {
    params((D(3), D(4)), (D(2), D(1)));
    assert_eq!(take(), [1, 2, 3, 4]);
    let c = |(_x, _): (D, D), (_, _y): (D, D)| {};
    c((D(3), D(4)), (D(2), D(1)));
    assert_eq!(take(), [1, 2, 3, 4]);
    // arrays by value: each item drops per iteration, the rest when the loop is left
    for (_a, _) in [(D(6), D(5))] {}
    assert_eq!(take(), [6, 5]);
    for x in [D(1), D(2), D(3)] { if x.0 == 1 { break; } }
    assert_eq!(take(), [1, 2, 3]);
    for _ in [D(4)] {}
    assert_eq!(take(), [4]);
}

#[test]
fn operands_before_statement_temporaries() {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| { (D(4), &D(5), D(6), &D(7), boom()); });
    assert_eq!(take(), [6, 4, 7, 5]);
    let _ = std::panic::catch_unwind(|| { (&D(1), D(2), { let _x = &D(9); let _y = D(8); boom() }); });
    assert_eq!(take(), [8, 9, 2, 1]);
    let _ = std::panic::take_hook();
    let r = (|| -> Option<u32> { let _t = (D(1), &D(2), None::<u32>?); Some(1) })();
    assert_eq!((r, take()), (None, vec![1, 2]));
}

#[test]
fn if_let_chain_rescope() {
    if ok(1).is_ok() && let true = ok(9).is_ok() && let Ok(_v) = ok(8) && let Ok(_) = ok(7) && let Ok(_) = ok(6).as_ref() && let Ok(_v) = ok(5) && let Ok(_) = ok(4).as_ref() {
        LOG.with(|l| l.borrow_mut().push(3));
    }
    assert_eq!(take(), [1, 3, 4, 5, 6, 7, 8, 9]);
    let x = (D(1), if let Some(_) = D(2).0.checked_add(0) && let D(..) = D(3) { D(4) } else { D(5) });
    assert_eq!(take(), [3, 2]);
    drop(x);
    assert_eq!(take(), [1, 4]);
    match () {
        () if let Ok(_v) = ok(8) && let Ok(_) = ok(7).as_ref() && let Err(_) = ok(6) => {}
        _ => LOG.with(|l| l.borrow_mut().push(9)),
    }
    assert_eq!(take(), [6, 7, 8, 9]);
    match () {
        () if let Ok(_v) = ok(5) && let Ok(_) = ok(4).as_ref() => LOG.with(|l| l.borrow_mut().push(3)),
        _ => {}
    }
    assert_eq!(take(), [3, 4, 5]);
    // a guard run once per or-alternative: each failed run drops its own temporaries once
    let mut n = 1;
    match D(8) {
        _ | _ | _ if let _ = D(n) && { n += 1; false } => unreachable!(),
        _ if let _ = D(5) && { LOG.with(|l| l.borrow_mut().push(4)); false } => unreachable!(),
        _ if let _ = D(7) && true => { LOG.with(|l| l.borrow_mut().push(6)); }
        _ => unreachable!(),
    }
    assert_eq!(take(), [1, 2, 3, 4, 5, 6, 7, 8]);
}
