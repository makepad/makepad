// Valid code next to what rustc_resolve / rustc_parse reject (see tests/ui macros/macro-expansion-tests.rs,
// range/range-inclusive-pattern-precedence.rs, hygiene/no_implicit_prelude-2018.rs): the legacy textual scope
// of macro_rules!, a macro call ending a block, parenthesised range patterns and 2015-style paths.
#![allow(dead_code, unused_macros)]

macro_rules! top { () => { 1 } }

#[macro_use]
mod escapes {
    macro_rules! m { () => { 3 + 4 } }
    mod inner {
        macro_rules! deep { () => { 5 } }
        pub fn d() -> i32 { deep!() }
    }
    pub fn u() -> i32 { m!() }
    pub fn inner_d() -> i32 { inner::d() }
}

#[macro_use]
mod outer {
    #[macro_use]
    mod inner2 { macro_rules! k { () => { 9 } } }
    pub fn z() -> i32 { k!() }
}

fn scopes() -> i32 {
    macro_rules! loc { () => { 2 } }
    fn h() -> i32 { loc!() }
    let c = || loc!();
    {
        macro_rules! loc { () => { 3 } }
        let _ = loc!();
    }
    m!() + top!() + k!() + loc!() + h() + c()
}

mod later {
    pub fn q() -> i32 { top!() }
}

// a block-ending macro call expands as one expression
macro_rules! one { () => { 40 + 2 } }
fn tail() -> i32 { one!() }

fn ranges(v: &i32) -> bool {
    match v {
        &(10..=15) => true,
        &(0..=9) => true,
        _ => false,
    }
}

#[no_implicit_prelude]
mod bare {
    fn f() {
        ::std::print!("");
    }
}

#[test]
fn front5_scopes() {
    assert_eq!(scopes(), 7 + 1 + 9 + 2 + 2 + 2);
    assert_eq!(later::q(), 1);
    assert_eq!(escapes::u() + outer::z() + escapes::inner_d(), 7 + 9 + 5);
    assert_eq!(tail(), 42);
    assert!(ranges(&12) && ranges(&3) && !ranges(&40));
}
