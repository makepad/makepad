// `return` / `break` / `continue` / `?` from inside an `if let` / `match` / `while` drop the
// temporaries of the statement being evaluated (Reference destructors.md
// [destructors.scope.nesting]; boot lane: makepad-script ip_to_loc's
// `if let Some(b) = self.bodies.borrow().get(i) { .. return .. }` left the RefCell borrowed).
// rustc: all pass.
use std::cell::{Cell, RefCell};

struct C {
    bodies: RefCell<Vec<u32>>,
}

impl C {
    fn loc(&self, i: usize) -> Option<u32> {
        if let Some(b) = self.bodies.borrow().get(i) {
            if *b > 1 {
                return Some(*b);
            }
        }
        None
    }
    fn via_match(&self, i: usize) -> u32 {
        match self.bodies.borrow().get(i) {
            Some(b) if *b > 1 => return *b,
            _ => {}
        }
        0
    }
    fn via_try(&self, i: usize) -> Option<u32> {
        let v = *self.bodies.borrow().get(i)?;
        Some(v)
    }
    fn in_loop(&self) -> u32 {
        let mut n = 0;
        for i in 0..4usize {
            if let Some(b) = self.bodies.borrow().get(i) {
                if *b == 1 {
                    continue;
                }
                if *b == 9 {
                    break;
                }
                n += *b;
            }
        }
        n
    }
}

struct Noisy<'a>(u32, &'a Cell<u32>);
impl Drop for Noisy<'_> {
    fn drop(&mut self) {
        self.1.set(self.1.get() * 10 + self.0);
    }
}
fn make<'a>(id: u32, log: &'a Cell<u32>) -> Noisy<'a> {
    Noisy(id, log)
}

fn order(log: &Cell<u32>) -> u32 {
    let _a = Noisy(1, log);
    if let Noisy(2, _) = make(2, log) {
        let _b = Noisy(3, log);
        return 7;
    }
    0
}

#[test]
fn temporaries_dropped_on_jumps() {
    let c = C { bodies: RefCell::new(vec![5, 1, 9, 4]) };
    assert!(c.loc(0) == Some(5));
    assert!(c.bodies.try_borrow_mut().is_ok());
    assert!(c.loc(1).is_none());
    assert!(c.via_match(0) == 5 && c.via_match(1) == 0);
    assert!(c.bodies.try_borrow_mut().is_ok());
    assert!(c.via_try(3) == Some(4) && c.via_try(7).is_none());
    assert!(c.bodies.try_borrow_mut().is_ok());
    assert!(c.in_loop() == 5);
    assert!(c.bodies.try_borrow_mut().is_ok());
    let log = Cell::new(0);
    assert!(order(&log) == 7);
    // the inner local, then the scrutinee temporary, then the outer local
    assert!(log.get() == 321);
}
