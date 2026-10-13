// Edition 2024 (let chains): run by `makepad test` and by rustc --edition 2024 --test.
use std::cell::RefCell;

thread_local!(static LOG: RefCell<Vec<&'static str>> = RefCell::new(Vec::new()));

struct D(&'static str);

impl Drop for D {
    fn drop(&mut self) {
        LOG.with(|l| l.borrow_mut().push(self.0));
    }
}

fn take_log() -> Vec<&'static str> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

// drop/if-let-guards.rs, if-let-guards-2.rs: the bindings of a let chain whose later condition
// fails drop before the else path / the next arm; an if-let guard's temporaries drop when it
// fails, else after its bindings at the arm's end
#[test]
fn let_chain_drops_on_failure() {
    take_log();
    if let _z = D("z") && std::hint::black_box(false) {
        unreachable!();
    } else {
        LOG.with(|l| l.borrow_mut().push("else"));
    }
    match (D("x"), D("y")) {
        (_x, _y) if let _g = D("g") && std::hint::black_box(false) => unreachable!(),
        _ => LOG.with(|l| l.borrow_mut().push("arm2")),
    }
    assert_eq!(take_log(), ["z", "else", "g", "arm2", "x", "y"]);
}
