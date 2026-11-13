//! `&usize == &&usize` is unmet: the two impls for a reference on the left (`PartialEq<&B>` and
//! `PartialEq<&mut B>`) leave the right operand's type open, so it is not coerced, and
//! `usize: PartialEq<&usize>` does not hold (rustc E0277; relax issue 87: it compiled).
// rejects: can't compare `usize` with `&usize`
pub fn ref_against_ref_ref() -> bool {
    let one: &usize = &1;
    let zero: &usize = &0;
    one == &zero
}
