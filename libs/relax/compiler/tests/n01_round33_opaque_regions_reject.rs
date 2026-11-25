// rejects: E0521
// rejects: lifetime may not live long enough
// Opaque types and their captured regions: rustc rejects both escapes (defining uses are checked
// against the declared bounds, nightly outlives/obligations.rs alias_ty_must_outlive): escapes2's
// returned reference with "lifetime may not live long enough", escapes3's argument with E0521.
// (A call's opaque result returned as `impl Sized + 'static`: n01_round33_opaque_return_reject.rs.)
#![allow(dead_code)]
fn mk<'a>(x: &'a u8) -> impl Sized + 'a { x }
fn escapes2<'a>(x: &'a u8) -> impl Sized + 'static { x }
fn keep<T: 'static>(_: T) {}
fn escapes3<'a>(x: &'a u8) { keep(mk(x)) }

#[test]
fn opaque_lifetime_regression() {
    let value = 1u8;
    escapes3(&value);
}
