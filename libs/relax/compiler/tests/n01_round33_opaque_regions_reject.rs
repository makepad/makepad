// T1 opaque-lifetime checks; owner: conform. Known failing fixture, deferred by ROUND 35.
// OPEN (round 33 OB report): rustc rejects all three; Relax's BIR borrowck accepts them. Its
// region skeleton gives an opaque no slots for its captured regions (rty.rs skel_into) and
// params_in has no opaque type test (nightly outlives/obligations.rs alias_ty_must_outlive),
// and defining uses get no region constraints against the declared bounds.
#![allow(dead_code)]
fn mk<'a>(x: &'a u8) -> impl Sized + 'a { x }
fn escapes<'a>(x: &'a u8) -> impl Sized + 'static { mk(x) }
fn escapes2<'a>(x: &'a u8) -> impl Sized + 'static { x }
fn keep<T: 'static>(_: T) {}
fn escapes3<'a>(x: &'a u8) { keep(mk(x)) }

#[test]
#[ignore = "T1: opaque-lifetime checks; conform"]
fn opaque_lifetime_regression() {
    let value = 1u8;
    escapes3(&value);
}
