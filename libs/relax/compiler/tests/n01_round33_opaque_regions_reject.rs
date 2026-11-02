// Opaque types and their captured regions: rustc rejects all three escapes (defining uses are
// checked against the declared bounds, nightly outlives/obligations.rs alias_ty_must_outlive).
#![allow(dead_code)]
fn mk<'a>(x: &'a u8) -> impl Sized + 'a { x }
fn escapes<'a>(x: &'a u8) -> impl Sized + 'static { mk(x) }
fn escapes2<'a>(x: &'a u8) -> impl Sized + 'static { x }
fn keep<T: 'static>(_: T) {}
fn escapes3<'a>(x: &'a u8) { keep(mk(x)) }

#[test]
fn opaque_lifetime_regression() {
    let value = 1u8;
    escapes3(&value);
}
