mod no_import {
    // The builtin removes the attributed item before duplicate-name checking.
    pub fn dropped() {}

    #[test]
    fn dropped() {}
}

mod imported {
    #[test]
    fn picked() { panic!("attribute did not expand"); }

    // Item imports also apply before their declaration.
    use mac::test;

    pub mod child {
        #[test]
        fn dropped() {}
        pub fn dropped() {}
    }
}

mod qualified {
    #[mac::test]
    fn by_path() { panic!("attribute did not expand"); }
}

mod grouped {
    use mac::{test};
    #[test]
    fn grouped() { panic!("attribute did not expand"); }
}

mod aliased {
    use mac::other as test;
    #[test]
    fn aliased() { panic!("attribute did not expand"); }
}

mod other_builtins {
    use mac::{other as bench, other as test_case};
    #[bench]
    fn bench_alias() { panic!("attribute did not expand"); }
    #[test_case]
    fn test_case_alias() { panic!("attribute did not expand"); }
}

mod facade {
    pub use mac::test;
}

// a glob bringing `test` is ambiguous with the prelude's builtin (rustc E0659): not tested here
mod via_facade {
    use crate::facade::test;
    #[test]
    fn via_facade() { panic!("attribute did not expand"); }
}

mod unrelated_glob {
    use core::mem::*;
    #[test]
    fn dropped() {}
    pub fn dropped() { assert_eq!(size_of::<u8>(), 1); }
}

fn block_import() {
    #[test]
    fn in_block() { panic!("attribute did not expand"); }
    use mac::{other as test};
    in_block();
}

fn dropped() {}
#[test]
fn dropped() {}

fn main() {
    dropped();
    no_import::dropped();
    imported::child::dropped();
    unrelated_glob::dropped();
    imported::picked();
    qualified::by_path();
    grouped::grouped();
    aliased::aliased();
    via_facade::via_facade();
    block_import();
    other_builtins::bench_alias();
    other_builtins::test_case_alias();
    bare_test_prelude::via_macro_use();
    println!("bare test attrs ok");
}
