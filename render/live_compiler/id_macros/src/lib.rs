use proc_macro::{TokenStream};

#[path = "../../../micro_serde/derive/src/macro_lib.rs"]
mod macro_lib;
use crate::macro_lib::*;

use crate::live_id::*;
#[path = "../../../live_tokenizer/src/live_id.rs"]
mod live_id; 

#[proc_macro] 
pub fn id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 

    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        let id = LiveId::from_str_unchecked(&name);
        tb.add("LiveId (").suf_u64(id.0).add(")");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        let id = LiveId::from_str_unchecked(&punct);
        tb.add("LiveId (").suf_u64(id.0).add(")");
        tb.end()
    }
    else if let Some(v) = parser.eat_literal(){
        if let Ok(v) = v.to_string().parse::<u64>(){
            tb.add("LiveId (").suf_u64(v).add(")");
            return tb.end()
        }
        else{
            parser.unexpected()
        }
    }
    else{
        parser.unexpected()
    }
}

// absolutely a very bad idea but lets see if we can do this.
#[proc_macro]
pub fn id_num(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 

    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        if !parser.eat_punct_alone(','){
            return error("please add a number")
        }
        if let Some(v) = parser.eat_literal(){
            if let Ok(v) = v.to_string().parse::<u64>(){
                let id = LiveId::from_str_unchecked(&name);
                tb.add("LiveId (").suf_u64(id.0&0xffff_ffff_0000_0000 | (v&0xffff_ffff)).add(")");
                return tb.end()
            }
            else{
                return error("please add a number")
            }
        }
        else{
            let arg = parser.eat_level();
            let id = LiveId::from_str_unchecked(&name);
            tb.add("LiveId (").suf_u64(id.0&0xffff_ffff_0000_0000).add("|((").stream(Some(arg)).add(")&0xffff_ffff)").add(")");
            tb.end()
        }
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
        tb.add("LiveId::from_str(").string(&name).add(")");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        tb.add("LiveId::from_str(").string(&punct).add(")");
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



