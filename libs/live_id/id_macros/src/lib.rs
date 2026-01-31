use proc_macro::{TokenStream, TokenTree};

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, error, LiveId};

mod derive_from_live_id;
use crate::derive_from_live_id::*;

// Helper to parse an identifier that may be prefixed with $
fn parse_maybe_prefixed_ident(parser: &mut TokenParser) -> Result<String, TokenStream> {
    if parser.eat_punct_alone('$') {
        // $ followed by ident
        let ident = parser.expect_any_ident()?;
        Ok(format!("${}", ident))
    } else {
        parser.expect_any_ident()
    }
}

// Helper to eat an identifier that may be prefixed with $ (returns None if not found)
fn eat_maybe_prefixed_ident(parser: &mut TokenParser) -> Option<String> {
    if parser.eat_punct_alone('$') {
        // $ followed by ident
        parser.eat_any_ident().map(|ident| format!("${}", ident))
    } else {
        parser.eat_any_ident()
    }
}

// Helper to parse a single token from TokenStream, handling $-prefixed identifiers specially
// Falls back to to_string() for punctuation like *, /, +, -, <<, >> etc.
fn parse_single_token(item: TokenStream) -> String {
    let mut iter = item.clone().into_iter();
    // Check if first token is $ followed by an ident (regardless of spacing)
    if let Some(TokenTree::Punct(p)) = iter.next() {
        if p.as_char() == '$' {
            if let Some(TokenTree::Ident(ident)) = iter.next() {
                return format!("${}", ident);
            }
        }
    }
    // Fallback: use to_string() for everything else (handles *, /, +, -, <<, >> etc.)
    item.to_string()
}

#[proc_macro] 
pub fn live_id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let v = parse_single_token(item);
    let id = LiveId::from_str(&v);
    tb.add("LiveId (").suf_u64(id.0).add(")");
    tb.end()
}

#[proc_macro] 
pub fn some_id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let v = parse_single_token(item);
    let id = LiveId::from_str(&v);
    tb.add("Some(LiveId (").suf_u64(id.0).add("))");
    tb.end()
}

#[proc_macro] 
pub fn id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let v = parse_single_token(item);
    if !v.is_empty() {
        let id = LiveId::from_str(&v);
        tb.add("LiveId (").suf_u64(id.0).add(")");
    }
    else {
        tb.add("LiveId (0)");
    }
    tb.end()
}

#[proc_macro] 
pub fn ids(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let mut parser = TokenParser::new(item);
    fn parse(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
        tb.add("&[");
        loop{
            // if its a {} insert it as code
            if parser.open_paren(){
                tb.stream(Some(parser.eat_level()));
                tb.add(",");
            }
            else{
                let ident = parse_maybe_prefixed_ident(parser)?;
                let id = LiveId::from_str(&ident);
                tb.add("LiveId (").suf_u64(id.0).add("),");
            }
                
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

fn ids_array_impl(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let mut parser = TokenParser::new(item);
    fn parse(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
        tb.add("&[");
        'outer: loop{
            tb.add("&[");
            loop{
                let ident = parse_maybe_prefixed_ident(parser)?;
                let id = LiveId::from_str(&ident);
                tb.add("LiveId (").suf_u64(id.0).add("),");
                if parser.eat_eot(){
                    tb.add("]");
                    break 'outer
                }
                if parser.eat_punct_alone(','){
                    tb.add("]");
                    break
                }
                parser.expect_punct_alone('.')?
            }
            tb.add(",");
            if parser.eat_eot(){
                break;
            }
        }
        tb.add("]");
        Ok(())
    }
    if let Err(e) = parse(&mut parser, &mut tb){
        return e
    };
    tb.end()
}

#[proc_macro] 
pub fn ids_array(item: TokenStream) -> TokenStream {
    ids_array_impl(item)
}

#[proc_macro] 
pub fn ids_list(item: TokenStream) -> TokenStream {
    ids_array_impl(item)
}


// absolutely a very bad idea but lets see if we can do this.
#[proc_macro]
pub fn live_id_num(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 

    let mut parser = TokenParser::new(item);
    if let Some(name) = eat_maybe_prefixed_ident(&mut parser) {
        if !parser.eat_punct_alone(','){
            return error("please add a number")
        }
        // then eat the next bit
        let arg = parser.eat_level();
        let id = LiveId::from_str(&name);
        tb.add("LiveId::from_num(").suf_u64(id.0).add(",").stream(Some(arg)).add(")");
        tb.end()
    }
    else{
        parser.unexpected()
    }
}

#[proc_macro]
pub fn id_lut(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 

    let mut parser = TokenParser::new(item);
    if let Some(name) = eat_maybe_prefixed_ident(&mut parser) {
        tb.add("LiveId::from_str_with_lut(").string(&name).add(").unwrap()");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        tb.add("LiveId::from_str_with_lut(").string(&punct).add(").unwrap()");
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
