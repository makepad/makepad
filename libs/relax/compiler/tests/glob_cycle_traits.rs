// Traits in scope through a glob whose target was first reached inside a glob cycle in the same
// walk: `rr_prelude` (`pub use super::*` of `core_root`) is reached while `core_root` is still being
// walked (core_root -> adaptive -> rr_prelude -> core_root), where it can only contribute an empty
// set; that partial set must not stand for the later visit from `lib_rr`, whose method call needs
// the trait `Bump` through `rr_prelude`. rustc resolves `p.bump()`.
pub mod core_root {
    pub mod view {
        pub struct View { pub x: u32 }
        pub trait Bump { fn bump(&mut self); }
        impl Bump for View { fn bump(&mut self) { self.x += 1; } }
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
        pub fn panel(p: &mut View) { p.bump(); }
    }
    pub use self::lib_rr::*;
}

#[test]
fn glob_cycle_traits() {
    let mut v = core_root::View { x: 1 };
    editor::panel(&mut v);
    assert_eq!(v.x + core_root::adaptive() + editor::runtime_view::runtime(), 7);
}
