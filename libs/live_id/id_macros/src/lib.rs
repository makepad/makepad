use proc_macro::{TokenStream};

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, error};

const LIVE_ID_SEED:u64 = 0xd6e8_feb8_6659_fd93;

const fn from_bytes(seed:u64, id_bytes: &[u8], start: usize, end: usize) -> u64 {
    let mut x = seed;
    let mut i = start;
    while i < end {
        x = x.overflowing_add(id_bytes[i] as u64).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        i += 1;
    }
    // mark high bit as meaning that this is a hash id
    return (x & 0x7fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000
}

const fn from_str_unchecked(id_str: &str) -> u64 {
    let bytes = id_str.as_bytes();
    from_bytes(LIVE_ID_SEED, bytes, 0, bytes.len())
}

mod derive_from_live_id;
use crate::derive_from_live_id::*;

#[proc_macro] 
pub fn live_id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 

    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        let id = from_str_unchecked(&name);
        tb.add("LiveId (").suf_u64(id).add(")");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        let id = from_str_unchecked(&punct);
        tb.add("LiveId (").suf_u64(id).add(")");
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
pub fn id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let mut parser = TokenParser::new(item);
    fn parse(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
        tb.add("&[");
        loop{
            let ident = parser.expect_any_ident()?;
            let id = from_str_unchecked(&ident);
            tb.add("LiveId (").suf_u64(id).add("),");
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
        let id = from_str_unchecked(&name);
        tb.add("LiveId::from_num_unchecked(").suf_u64(id).add(",").stream(Some(arg)).add(")");
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
