// zerovec's layout: a private module named like the crate, and exported macros, one of them
// named like the crate and the module too
mod xvec;
pub mod ule {
    pub trait AsULE: Copy {}
    impl AsULE for u32 {}
    pub struct Potential;
    impl Potential {
        pub const fn to_unaligned(x: u32) -> u32 {
            x
        }
    }
}
pub use crate::xvec::{XSlice, XVec};
