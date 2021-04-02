use proc_macro::{TokenStream};
mod live_derive; 
use crate::live_derive::*;
use crate::macro_lib::*;
#[path = "../../../microserde/derive/src/macro_lib.rs"]
mod macro_lib; 

use crate::id::*;
#[path = "../../src/id.rs"]
mod id; 

#[proc_macro_derive(DeTok)]
pub fn live_derive(input: TokenStream) -> TokenStream {
    live_derive_impl(input)
}

#[proc_macro]
pub fn id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let id = Id::from_str(&item.to_string());
    tb.add("Id (").suf_u64(id.0).add(")");
    tb.end()
}

#[proc_macro]
pub fn token_ident(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let id = Id::from_str(&item.to_string());
    tb.add("Token :: Ident ( Id (").suf_u64(id.0).add(") )");
    tb.end()
}

#[proc_macro]
pub fn token_punct(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let id = Id::from_str(&item.to_string());
    tb.add("Token :: Punct ( Id (").suf_u64(id.0).add(") )");
    tb.end()
}
