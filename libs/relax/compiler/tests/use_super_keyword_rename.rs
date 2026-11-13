// relax #90: an import path that ends in `super` names that module once renamed, at any depth and
// in a group (rustc_resolve: `use super::super as a;`); before, every form but the one-segment
// `use super as p;` failed with E0432. `rustc --test` and `makepad test` both pass it.
mod a {
    pub const X: u32 = 7;
    pub mod b {
        pub const Z: u32 = 2;
        pub mod c {
            use super::super as a_mod;
            use super::super::super as root;
            use self::super as b_mod;
            use super::{super as a_group};
            use super::{super::{super as root_nested}};
            use super::super::{self as a_self};
            use {super::super as a_braced};
            pub(crate) use super::super as a_crate;

            pub fn f() -> u32 {
                a_mod::X + root::Y
            }

            pub fn g() -> u32 {
                b_mod::Z + a_group::X + root_nested::Y + a_self::X + a_braced::X + a_crate::X
            }
        }
    }
}

const Y: u32 = 1;

#[test]
fn renamed_super_paths_name_their_modules() {
    assert_eq!(a::b::c::f(), 8, "the issue's program");
    assert_eq!(a::b::c::g(), 2 + 7 + 1 + 7 + 7 + 7, "self::super, groups, {{self}} and a pub(crate) re-export");
    assert_eq!(a::b::c::a_crate::X, 7, "the pub(crate) re-export from outside");
}
