use proc_macro::{TokenStream};

use makepad_macro_lib::{TokenBuilder, TokenParser, error};

use crate::live_id::*;
#[path = "../../src/live_id.rs"]
mod live_id; 

mod derive_from_live_id;
use crate::derive_from_live_id::*;

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

#[proc_macro] 
pub fn ids(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let mut parser = TokenParser::new(item);
    fn parse(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
        tb.add("&[");
        loop{
            let ident = parser.expect_any_ident()?;
            let id = LiveId::from_str_unchecked(&ident);
            tb.add("LiveId (").suf_u64(id.0).add("),");
            if parser.eat_eot(){
                tb.add("]");
                return Ok(())
            }
            parser.expect_punct_alone('.')?
        }
    }
    if let Err(e) = parse(&mut parser, &mut tb){
        return e
    };
    tb.end()
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
        // then eat the next bit
        let arg = parser.eat_level();
        let id = LiveId::from_str_unchecked(&name);
        tb.add("LiveId::from_num_unchecked(").suf_u64(id.0).add(",").stream(Some(arg)).add(")");
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

#[proc_macro_derive(FromLiveId)]
pub fn derive_from_live_id(input: TokenStream) -> TokenStream {
    derive_from_live_id_impl(input)
}
