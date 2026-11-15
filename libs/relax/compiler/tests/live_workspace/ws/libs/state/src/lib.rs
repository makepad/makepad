//! A trait implemented for arrays whose lengths are the upstream crate's numbers.
use lw_nums::{sizes, Unsigned};

pub trait SerializableState {
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
        )*
    };
}

impl_state!(U1, U2, U3, U4);

pub fn state() -> usize {
    7
}
