// Assigning to a place a closure captured drops the value it held (rustc MIR: `Drop` of the
// place, then the write). app_main!'s event closure keeps `app_value: Option<ScriptObjectRef>`
// by move and sets it inside `cx.with_vm(|vm| ..)` on each design re-run: under Relax the old
// ref was never released, so every re-run kept the previous app tree alive.
use std::cell::Cell;

thread_local! { static DROPPED: Cell<u32> = Cell::new(0); }

struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        DROPPED.with(|d| d.set(d.get() + self.0));
    }
}

fn dropped() -> u32 {
    DROPPED.with(|d| d.replace(0))
}

fn with<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[test]
fn assign_through_a_nested_closure_drops_the_old_value() {
    dropped();
    let mut value: Option<D> = None;
    let mut handler = move |n: u32| {
        with(|| {
            value = Some(D(n));
        });
    };
    handler(1);
    assert_eq!(dropped(), 0);
    handler(10);
    assert_eq!(dropped(), 1);
    handler(100);
    assert_eq!(dropped(), 10);
    drop(handler);
    assert_eq!(dropped(), 100);
}

#[test]
fn assign_to_a_moved_capture_drops_the_old_value() {
    dropped();
    let mut value: Option<D> = None;
    let mut handler = move |n: u32| {
        value = Some(D(n));
    };
    handler(1);
    handler(10);
    assert_eq!(dropped(), 1);
    drop(handler);
    assert_eq!(dropped(), 10);
}

#[test]
fn assign_in_a_boxed_fnmut_drops_the_old_value() {
    dropped();
    let mut value: Option<D> = None;
    let mut handler: Box<dyn FnMut(u32)> = Box::new(move |n: u32| {
        if n > 0 {
            with(|| {
                if let Some(x) = Some(n) {
                    value = Some(D(x));
                }
            });
        }
    });
    handler(1);
    handler(10);
    assert_eq!(dropped(), 1);
    drop(handler);
    assert_eq!(dropped(), 10);
}

/// makepad-widgets' `child_in`: a fn's own local set from a `&mut dyn FnMut` callback. The empty
/// ref it starts with was never released under Relax (a Tweaker sidebar frame leaked one per
/// lookup, ~0.7 MB/s).
fn find_in(id: u32, children: &mut dyn FnMut(&mut dyn FnMut(u32, D))) -> D {
    let mut found = D(1000);
    children(&mut |name, child| {
        if found.0 == 1000 && name == id {
            found = child;
        }
    });
    found
}

#[test]
fn assign_to_a_local_from_a_dyn_fnmut_callback_drops_the_old_value() {
    dropped();
    let mut kids = |f: &mut dyn FnMut(u32, D)| {
        f(1, D(1));
        f(2, D(10));
    };
    let got = find_in(2, &mut kids);
    assert_eq!(dropped(), 1001, "the child not taken and the replaced start value");
    assert_eq!(got.0, 10);
    drop(got);
    assert_eq!(dropped(), 10);
}
