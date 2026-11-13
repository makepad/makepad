//! A trait with a supertrait bound, implemented for arrays whose lengths are the upstream crate's
//! type-level numbers, and for one whose length a const fn computes.
use ccl_nums::{sizes, Unsigned};

pub trait SerializableState
where
    Self: Sized,
{
    fn bytes() -> usize;
}

macro_rules! impl_state {
    ($($n:ident),*) => {
        $(
            impl SerializableState for [u8; sizes::$n::USIZE] {
                fn bytes() -> usize { sizes::$n::USIZE }
            }
            impl SerializableState for [u16; sizes::$n::USIZE] {
                fn bytes() -> usize { 2 * sizes::$n::USIZE }
            }
            impl SerializableState for [u32; sizes::$n::USIZE] {
                fn bytes() -> usize { 4 * sizes::$n::USIZE }
            }
        )*
    };
}

impl_state!(U1, U2, U3, U4, U5, U6, U7, U8);

const fn twice(n: usize) -> usize {
    2 * n
}

impl SerializableState for [u64; twice(1)] {
    fn bytes() -> usize {
        16
    }
}

pub fn total() -> usize {
    <[u8; 8] as SerializableState>::bytes() + <[u16; 4] as SerializableState>::bytes() + <[u32; 3] as SerializableState>::bytes()
        + <[u64; 2] as SerializableState>::bytes()
}
