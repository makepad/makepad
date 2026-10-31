#![feature(rustc_attrs)]
#[rustc_legacy_const_generics(1)]
fn own<const N: i32>(value: i32) -> i32 { value + N }

#[test]
fn own_crate_uses_explicit_generics() { assert_eq!(own::< -2>(5), 3); }
