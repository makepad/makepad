pub mod a {
    pub enum A { Same { x: u32 } }
}

#[allow(non_upper_case_globals)]
pub mod b {
    pub const Same: u32 = 1;
}

#[allow(unused_imports, non_upper_case_globals)]
pub mod globs {
    use super::a::A::*;
    use super::b::*;

    fn pick(v: u32) -> u32 {
        match v { Same => 1, _ => 2 }
    }

    fn let_else(v: u32) -> u32 {
        let Same = v else { return 2 };
        1
    }

    #[cfg_attr(test, test)]
    pub fn glob_const_beside_braced_variant() {
        let super::a::A::Same { x } = super::a::A::Same { x: 7 };
        assert_eq!(x, 7);
        assert_eq!(super::b::Same, 1);
        assert_eq!(pick(0), 2);
        assert_eq!(pick(1), 1);
        assert_eq!(let_else(0), 2);
        assert_eq!(let_else(1), 1);
        assert_eq!(if let Same = 0u32 { 1 } else { 2 }, 2);
        assert!(!matches!(0u32, Same));
        assert!(matches!(1u32, Same));
        assert!(!matches!((0u32, 0u32), (0, Same)));
        assert!(matches!((0u32, 1u32), (0, Same)));
        assert!(!matches!(Some(0u32), Some(Same)));
    }
}

#[cfg_attr(test, test)]
#[allow(unused_imports, non_upper_case_globals)]
pub fn single_import_beside_const_glob() {
    use a::A::Same;
    use b::*;
    let pick = |v: u32| match v { Same => 1, _ => 2 };
    assert_eq!(pick(0), 2);
    assert_eq!(pick(1), 1);
}

#[allow(non_upper_case_globals, unused_imports)]
pub mod outer {
    const Same: u32 = 1;

    #[cfg_attr(test, test)]
    pub fn block_glob_over_outer_const() {
        use super::a::A::*;
        assert_eq!(super::outer::Same, 1);
        assert_eq!(match 0u32 { Same => 1, _ => 2 }, 2);
        assert_eq!(match 1u32 { Same => 1, _ => 2 }, 1);
    }
}

pub mod unit {
    pub enum B { Hit, Other }
    pub use B::Hit as Same;
}

#[cfg_attr(test, test)]
#[allow(unused_imports)]
pub fn glob_unit_variant_beside_braced_variant() {
    use a::A::*;
    use unit::*;
    assert_eq!(match unit::B::Other { Same => 1, _ => 2 }, 2);
    assert_eq!(match unit::B::Hit { Same => 1, _ => 2 }, 1);
    assert_eq!(match &unit::B::Other { Same => 1, _ => 2 }, 2);
    assert_eq!(match &unit::B::Hit { Same => 1, _ => 2 }, 1);
}
