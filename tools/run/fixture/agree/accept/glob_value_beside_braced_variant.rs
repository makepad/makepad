pub mod a {
    pub enum A { Same { x: u32 } }
}

#[allow(non_upper_case_globals)]
pub mod b {
    pub const Same: u32 = 1;
}

#[allow(unused_imports)]
pub mod re {
    pub use super::a::A::*;
    pub use super::b::*;
}

#[cfg_attr(test, test)]
#[allow(unused_imports, non_upper_case_globals)]
pub fn glob_const_beside_braced_variant() {
    use a::A::*;
    use b::*;

    let y = Same;
    assert_eq!(y, 1);
    let Same { x } = Same { x: 7 };
    assert_eq!(x, 7);
    let pick = |v: u32| match v { Same | 2 => 1, _ => 2 };
    assert_eq!(pick(0), 2);
    assert_eq!(pick(1), 1);
    assert_eq!(pick(2), 1);
    assert!(matches!((1u32, 1u32), (Same, Same)));
    assert!(!matches!((1u32, 0u32), (Same, Same)));
    const X: u32 = Same;
    assert_eq!(X, 1);
    assert_eq!(format!("{Same}"), "1");
    assert_eq!(match 0u32 { re::Same => 1, _ => 2 }, 2);
    assert_eq!(re::Same, 1);
}

#[cfg_attr(test, test)]
#[allow(unused_imports, non_upper_case_globals)]
pub fn reversed_const_globs() {
    use b::*;
    use a::A::*;

    let pick = |v: u32| match v { Same => 1, _ => 2 };
    assert_eq!(pick(0), 2);
    assert_eq!(pick(1), 1);
    assert_eq!(Same, 1);
    let Same { x } = Same { x: 7 };
    assert_eq!(x, 7);
    assert_eq!(if let Same = 0u32 { 1 } else { 2 }, 2);
    assert!(matches!(Some(1u32), Some(Same)));
    assert!(!matches!((0u32, 0u32), (0, Same)));
    assert!(matches!((1u32, 1u32), (Same, Same)));
    assert!(matches!(2u32, Same | 2));
    const X: u32 = Same;
    assert_eq!(X, 1);
    assert_eq!(format!("{Same}"), "1");
}

#[allow(unused_imports, non_upper_case_globals)]
pub mod explicit {
    use super::a::A::Same;
    use super::b::Same;

    #[cfg_attr(test, test)]
    pub fn two_explicit_imports() {
        let Same { x } = Same { x: 7 };
        assert_eq!(x, 7);
        assert_eq!(Same, 1);
        assert!(!matches!(0u32, Same));
    }
}

#[allow(non_upper_case_globals, unused_imports)]
pub mod local_const {
    use super::a::A::Same;
    const Same: u32 = 1;

    #[cfg_attr(test, test)]
    pub fn explicit_import_beside_local_const() {
        let Same { x } = Same { x: 7 };
        assert_eq!(x, 7);
        assert_eq!(Same, 1);
        assert!(!matches!(0u32, Same));
    }
}

#[cfg_attr(test, test)]
#[allow(unused_imports, non_snake_case)]
pub fn block_glob_over_local_binding() {
    let Same = 42u32;
    {
        use a::A::*;
        assert_eq!(Same, 42);
    }
}

pub mod unit_variant {
    pub enum B { Same, Other }
}

pub mod unit_struct {
    pub struct Same;
}

#[cfg_attr(test, test)]
#[allow(unused_imports)]
pub fn glob_unit_variant_and_unit_struct_beside_braced_variant() {
    {
        use a::A::*;
        use unit_variant::B::*;
        assert!(matches!(Same, unit_variant::B::Same));
        assert_eq!(match unit_variant::B::Other { Same => 1, _ => 2 }, 2);
        assert_eq!(match unit_variant::B::Same { Same => 1, _ => 2 }, 1);
        assert_eq!(match &unit_variant::B::Other { Same => 1, _ => 2 }, 2);
        assert_eq!(match &unit_variant::B::Same { Same => 1, _ => 2 }, 1);
    }
    {
        use unit_variant::B::*;
        use a::A::*;
        assert!(matches!(Same, unit_variant::B::Same));
        assert_eq!(match unit_variant::B::Other { Same => 1, _ => 2 }, 2);
        assert_eq!(match unit_variant::B::Same { Same => 1, _ => 2 }, 1);
        assert_eq!(match &unit_variant::B::Same { Same => 1, _ => 2 }, 1);
        assert_eq!(match &unit_variant::B::Other { Same => 1, _ => 2 }, 2);
    }
    {
        use a::A::*;
        use unit_struct::*;
        let Same = Same;
        fn take_two(Same: unit_struct::Same, Same: unit_struct::Same) -> u32 { 1 }
        assert_eq!(take_two(Same, Same), 1);
        let closure = |Same: unit_struct::Same, Same: unit_struct::Same| 1;
        assert_eq!(closure(Same, Same), 1);
    }
}

pub mod tuple {
    pub struct Same(pub u32);
}

#[cfg_attr(test, test)]
#[allow(unused_imports)]
pub fn glob_tuple_struct_beside_braced_variant() {
    {
        use a::A::*;
        use tuple::*;
        let s = Same(9);
        assert_eq!(match s { Same(v) => v }, 9);
        let out;
        Same(out) = Same(5);
        assert_eq!(out, 5);
    }
    {
        use tuple::*;
        use a::A::*;
        let s = Same(9);
        assert_eq!(match s { Same(v) => v }, 9);
        let out;
        Same(out) = Same(5);
        assert_eq!(out, 5);
    }
}

#[allow(non_upper_case_globals)]
pub mod statics {
    pub static Same: u32 = 11;
}

#[allow(non_snake_case)]
pub mod functions {
    pub fn Same() -> u32 { 13 }
}

#[cfg_attr(test, test)]
#[allow(unused_imports, non_snake_case)]
pub fn glob_static_and_fn_beside_braced_variant() {
    {
        use a::A::*;
        use statics::*;
        assert_eq!(Same, 11);
        assert!(matches!(Same { x: 4 }, Same { x: 4 }));
    }
    {
        use a::A::*;
        use functions::*;
        assert_eq!(Same(), 13);
        assert_eq!(match 0u32 { Same => Same + 1 }, 1);
        assert!(matches!(Same { x: 6 }, Same { x: 6 }));
    }
}
