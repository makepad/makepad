// A name reached through a glob whose target was already visited inside a glob cycle in the
// same lookup: `rr_prelude` (`pub use super::*` of `core_root`) is first reached while
// `core_root` is still being looked through (core_root -> adaptive -> rr_prelude -> core_root),
// where it can only answer "not found"; that answer must not stand for the later, direct
// visit from `lib_rr`. rustc finds `ViewBase` through `rr_prelude`.
#![allow(non_snake_case)]

pub mod core_root {
    pub mod view {
        pub struct View { pub x: u32 }
        pub fn ViewBase(p: &mut View) { p.x += 1; }
    }
    pub mod adaptive {
        #[allow(unused_imports)]
        use crate::core_root::rr_prelude::*;
        pub fn adaptive() -> u32 { 2 }
    }
    pub use self::adaptive::*;
    pub use self::view::*;
    pub mod rr_prelude {
        pub use super::*;
    }
}

pub mod editor {
    pub mod runtime_view {
        #[allow(unused_imports)]
        use crate::core_root::*;
        pub fn runtime() -> u32 { 3 }
    }
    #[allow(unused_imports)]
    use self::runtime_view::*;
    pub use crate::core_root::rr_prelude;
    pub mod lib_rr {
        #[allow(unused_imports)]
        use super::*;
        #[allow(unused_imports)]
        use crate::editor::rr_prelude::*;
        pub fn EditorPanel(p: &mut View) { ViewBase(p); }
    }
    pub use self::lib_rr::*;
}

#[test]
fn glob_cycle_memo() {
    let mut v = core_root::View { x: 1 };
    editor::EditorPanel(&mut v);
    assert_eq!(v.x + core_root::adaptive() + editor::runtime_view::runtime(), 7);
}
