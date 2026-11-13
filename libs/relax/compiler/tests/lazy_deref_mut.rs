// core/src/cell/lazy.rs, std/src/sync/lazy_lock.rs: `DerefMut` for `LazyCell` and `LazyLock`
// (stable 1.89). krilla starts a PDF attribute list lazily and pushes into it through a second
// lazy cell whose closure captures the first one mutably:
// `LazyCell::new(|| attributes.push().list())`.
use std::cell::LazyCell;
use std::sync::LazyLock;

#[test]
fn a_lazy_cell_gives_its_value_mutably_and_starts_it_once() {
    let mut starts = 0;
    let mut list = LazyCell::new(|| {
        starts += 1;
        vec![1u32]
    });
    list.push(2);
    list.push(3);
    assert_eq!(*list, [1, 2, 3]);
    drop(list);
    assert_eq!(starts, 1);
}

#[test]
fn a_closure_captures_a_lazy_cell_mutably_through_deref_mut() {
    let mut outer: LazyCell<Vec<Vec<u8>>> = LazyCell::new(|| vec![vec![0]]);
    let mut inner = LazyCell::new(|| {
        outer.push(Vec::new());
        outer.last_mut().unwrap()
    });
    inner.push(7);
    inner.push(8);
    drop(inner);
    assert_eq!(*outer, [vec![0], vec![7, 8]]);
}

#[test]
fn a_lazy_cell_never_touched_never_starts() {
    let mut started = false;
    let cell = LazyCell::new(|| started = true);
    drop(cell);
    assert!(!started);
}

#[test]
fn a_lazy_lock_gives_its_value_mutably() {
    let mut names: LazyLock<Vec<String>> = LazyLock::new(|| vec!["a".to_string()]);
    names.push("b".to_string());
    assert_eq!(*names, ["a", "b"]);
    let mut fresh: LazyLock<String> = LazyLock::new(String::new);
    fresh.push_str("made on the first mutable use");
    assert_eq!(fresh.len(), 29);
}
