use proc_macro::{TokenStream};

#[path = "../../../microserde/derive/src/macro_lib.rs"]
mod macro_lib;
use crate::macro_lib::*;

use crate::id::*;
#[path = "../../../live_compiler/src/id.rs"]
mod id; 

#[proc_macro]
pub fn id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 

    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        let id = Id::from_str_unchecked(&name);
        tb.add("Id (").suf_u64(id.0).add(")");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        let id = Id::from_str_unchecked(&punct);
        tb.add("Id (").suf_u64(id.0).add(")");
        tb.end()
    }
    else{
        parser.unexpected()
    }
}
#[proc_macro]
pub fn id_from_str(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 

    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        tb.add("Id::from_str(").string(&name).add(")");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        tb.add("Id::from_str(").string(&punct).add(")");
        tb.end()
    }
    else{
        parser.unexpected()
    }
}

#[proc_macro]
pub fn live_error_origin(_item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    tb.add("LiveErrorOrigin { filename : file ! ( ) . to_string ( ) , line : line ! ( ) as usize }");
    tb.end()
}



