// Capture normalization, rustc_hir_typeck/upvar.rs:629, 775, 2546.
// The 2229 UI cases exercising these rules use nightly capture-analysis attributes.
//@ edition:2021
//@ run-pass

fn ancestor_before_descendant() {
    let mut value = 0;
    let r = &mut value; // the binding itself is intentionally not mutable
    let mut c = || {
        let _read = &r;
        *r += 1;
    };
    c();
    assert_eq!(value, 1);
}

fn descendant_before_ancestor() {
    let mut value = 0;
    let r = &mut value;
    let mut c = || {
        *r += 1;
        let _read = &r;
    };
    c();
    assert_eq!(value, 1);
}

fn raw_pointer_move_closure_is_fn() {
    let mut value = 0;
    let p = &raw mut value;
    let c = move || unsafe { *p += 1; };
    c();
    c();
    assert_eq!(value, 2);
}

fn main() {
    ancestor_before_descendant();
    descendant_before_ancestor();
    raw_pointer_move_closure_is_fn();
}

#[test]
fn capture_normalization() { main(); }
