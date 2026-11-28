trait Tr {
    type A;
    const VAL: Self::A;
}
impl Tr for () {
    type A = bool;
    const VAL: Self::A = true;
}

const T: <() as Tr>::A = true;

fn match_bool(b: bool) -> u8 {
    match b {
        T => 0,
        false => 1,
    }
}

fn match_alias(b: <() as Tr>::A) -> u8 {
    match b {
        T => 0,
        false => 1,
    }
}

fn match_assoc_const(b: bool) -> u8 {
    match b {
        <() as Tr>::VAL => 0,
        false => 1,
    }
}

const N: usize = 1;
const ARR_T: [bool; N] = [true];
const ARR_F: [bool; N] = [false];

fn match_array(x: [bool; N]) -> u8 {
    match x {
        ARR_T => 0,
        ARR_F => 1,
    }
}

#[test]
fn exhaustive_aliased_const_patterns() {
    assert_eq!(match_bool(true), 0);
    assert_eq!(match_bool(false), 1);
    assert_eq!(match_alias(true), 0);
    assert_eq!(match_alias(false), 1);
    assert_eq!(match_assoc_const(true), 0);
    assert_eq!(match_assoc_const(false), 1);
    assert_eq!(match_array([true]), 0);
    assert_eq!(match_array([false]), 1);
}
