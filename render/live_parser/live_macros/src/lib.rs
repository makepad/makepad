use proc_macro::{TokenStream};

#[path = "../../../microserde/derive/src/macro_lib.rs"]
mod macro_lib;
use crate::macro_lib::*;

use crate::id::*;
#[path = "../../../live_parser/src/id.rs"]
mod id; 


#[proc_macro]
pub fn id_check(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let item_str = item.to_string();
    
    let id = Id::from_str(&item_str);
    tb.add("Id (").suf_u64(id.0).add(") . panic_collision (").string(&item_str).add(")");
    tb.end()
}


#[proc_macro]
pub fn id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    // item HAS to be an identifier.
    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        let id = Id::from_str(&name.to_string());
        tb.add("Id (").suf_u64(id.0).add(")");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        let id = Id::from_str(&punct.to_string());
        tb.add("Id (").suf_u64(id.0).add(")");
        tb.end()
    }
    else{
        parser.unexpected()
    }
}

#[proc_macro]
pub fn id_pack(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let id = Id::from_str(&item.to_string());
    tb.add("IdPack (").suf_u64(id.0).add(")");
    tb.end()
}


#[proc_macro]
pub fn live_error_origin(_item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    tb.add("LiveErrorOrigin { filename : file ! ( ) . to_string ( ) , line : line ! ( ) as usize }");
    tb.end()
}



