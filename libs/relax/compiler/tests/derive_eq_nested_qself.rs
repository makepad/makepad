//! `#[derive(Eq)]` asserts that each field's type is `Eq` with the type as it is written: a nested
//! qualified path (`<<T as Outer>::Mid as Inner>::Leaf`) is a type there as in the field (relax
//! issue 29).
trait Inner {
    type Leaf: Eq;
}

trait Outer: Eq {
    type Mid: Inner + Eq;
}

#[derive(PartialEq, Eq)]
struct Holder<T: Outer> {
    leaf: <<T as Outer>::Mid as Inner>::Leaf,
    plain: <T as Outer>::Mid,
}

#[derive(PartialEq, Eq)]
struct Key;

impl Inner for u8 {
    type Leaf = u16;
}

impl Outer for Key {
    type Mid = u8;
}

#[test]
fn eq_derive_on_a_nested_qualified_path() {
    let a = Holder::<Key> { leaf: 5, plain: 1 };
    assert!(a == Holder { leaf: 5, plain: 1 }, "the same fields compare equal");
    assert!(a != Holder { leaf: 6, plain: 1 }, "the nested qualified field is compared");
}
