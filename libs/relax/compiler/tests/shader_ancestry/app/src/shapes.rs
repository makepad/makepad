// derived shader types in a module of their own: the base comes in by a glob import of another
// crate (re-exported by a third) (its macro-namespace binding too), the grandchild's base is defined in this module
use anc_wid::*;

#[derive(anc_mac::Shader)]
#[repr(C)]
pub struct Gauss {
    #[deref]
    pub base: Quad,
    pub r: u32,
}

#[derive(anc_mac::Shader)]
#[repr(C)]
pub struct GaussWide {
    #[deref]
    pub base: Gauss,
}
