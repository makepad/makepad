use proc_macro::{TokenStream};

mod debug_opt;
use crate::debug_opt::*;

#[proc_macro_derive(DebugOpt)]
pub fn derive_debug_opt(input: TokenStream) -> TokenStream {
    derive_debug_opt_impl(input)
}
