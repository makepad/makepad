//! Latent typeck gaps found by the boundary audit (bodies `makepad check` never reached); each = rustc --test.
//! 1. `let [a, b, c, d] = array::from_fn(..)`: the array pattern gives the open length (rustc pat.rs
//!    check_array_pat_len), metal.rs:3067.
//! 2. `Cell::as_slice_of_cells(Cell::from_mut(&mut vec))`: the expected `&Cell<[T]>` gives from_mut's
//!    expected input `&mut [T]` and the Vec derefs to it (expected_inputs_for_expected_output for a
//!    reference result; rustybuzz find_syllables).
//! 3. `Words { inner: s.split(..).filter(fn_item) }` with the field `Filter<_, fn(&&str) -> bool>`:
//!    a trait method's expected inputs come from the expected result too, and a fn item / closure
//!    argument coerces to the fn pointer (unicode_segmentation).
use std::cell::Cell;
#[test]
fn from_fn_array_pattern() {
    let host = |t: u64| (t != 0).then_some(t as f64);
    let [a, b, c, d] = std::array::from_fn(|i| host(i as u64));
    assert_eq!((a, b, c, d), (None, Some(1.0), Some(2.0), Some(3.0)));
}
#[test]
fn cell_from_mut_vec_as_slice() {
    let mut v = vec![1u8, 2, 3];
    let cells = Cell::as_slice_of_cells(Cell::from_mut(&mut v));
    cells[1].set(9);
    assert_eq!(v, vec![1, 9, 3]);
}
fn alnum(s: &&str) -> bool { s.chars().any(|c| c.is_alphanumeric()) }
struct Words<'a> { inner: std::iter::Filter<std::str::Split<'a, char>, fn(&&str) -> bool> }
struct Idx<'a> { inner: std::iter::Filter<std::str::CharIndices<'a>, fn(&(usize, char)) -> bool> }
#[test]
fn fn_item_and_closure_to_fn_ptr_in_expected_struct() {
    let w = Words { inner: "a - b".split(' ').filter(alnum) };
    assert_eq!(w.inner.count(), 2);
    let i = Idx { inner: "a b".char_indices().filter(|(_, c)| *c != ' ') };
    assert_eq!(i.inner.count(), 2);
}
