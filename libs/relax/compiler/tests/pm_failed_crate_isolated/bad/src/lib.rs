// A macro crate that does not load in Relax's proc-macro host (whose crates have `cfg(relax)`) and
// builds under rustc: the stand-in for a crate the host cannot load (a matcher bug, say).
use proc_macro::TokenStream;

#[cfg(relax)]
macro_rules! only_a {
    (a) => {};
}
#[cfg(relax)]
only_a!(b);

/// `seven!()`: 7.
#[proc_macro]
pub fn seven(_: TokenStream) -> TokenStream {
    "7".parse().unwrap()
}
