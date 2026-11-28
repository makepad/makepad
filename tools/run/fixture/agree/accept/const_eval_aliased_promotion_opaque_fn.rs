trait Tr {
    type Idx;
    type Div;
}
impl Tr for () {
    type Idx = usize;
    type Div = u8;
}

const IDX: <() as Tr>::Idx = 1;
const DIV: <() as Tr>::Div = 2;

fn promoted_plain() -> (&'static u8, &'static u8) {
    (&[10u8, 20u8][IDX], &(10u8 / DIV))
}

fn promoted_in_opaque_fn() -> impl Copy + PartialEq<u8> {
    let a: &'static u8 = &[10u8, 20u8][IDX];
    let b: &'static u8 = &(10u8 / DIV);
    *a + *b
}

#[test]
fn aliased_const_promotion_works_in_plain_and_opaque_fns() {
    let (a, b) = promoted_plain();
    assert_eq!((*a, *b), (20, 5));
    assert!(promoted_in_opaque_fn() == 25);
}
