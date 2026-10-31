// the runtime half of the Shader derive (anc-mac), as makepad_draw::rr: `meta!` defines a type's
// ancestry macro (a macro_rules made by an expansion, `#[macro_export]`, its `$` passed in),
// `chain!` is what a base's ancestry macro calls back with the depths so far.
extern crate self as anc_draw;

#[doc(hidden)]
#[macro_export]
macro_rules! meta {
    ($name:ident, $d:tt, [$($depth:tt)*]) => {
        #[doc(hidden)]
        #[macro_export]
        macro_rules! $name {
            ($d callback:path, $d($d args:tt)*) => {
                $d callback! { [$($depth)*] $d($d args)* }
            };
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! chain {
    ([$($depth:tt)*] $derived:ty, $metadata:ident, $d:tt) => {
        impl $crate::rr::Depth for $derived {
            const DEPTH: usize = 0 $(+ $crate::proj!($depth))*;
        }
        $crate::meta!($metadata, $d, [() $(($depth))*]);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! proj {
    ($depth:tt) => { 1 };
}

pub mod rr {
    pub use crate::{chain, meta, proj};
    pub trait Depth {
        const DEPTH: usize;
    }
    pub trait Layout {
        type Base;
        fn base(&self) -> &Self::Base;
    }
}

#[derive(anc_mac::Shader)]
#[repr(C)]
pub struct Quad {
    pub x: u32,
}

impl rr::Depth for Quad {
    const DEPTH: usize = 0;
}
