extern crate proc_macro;
use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, Id};

#[proc_macro] 
pub fn id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let v = item.to_string();
    if v != ""{
        let id = Id::from_str(&v);
        tb.add("Id (").suf_u64(id.0).add(")");
    }
    else {
        tb.add("Id (0)");
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
                let ident = parser.expect_any_ident()?;
                let id = Id::from_str(&ident);
                tb.add("Id (").suf_u64(id.0).add("),");
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

#[proc_macro] 
pub fn ids_array(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let mut parser = TokenParser::new(item);
    fn parse(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
        tb.add("&[");
        'outer: loop{
            tb.add("&[");
            loop{
                let ident = parser.expect_any_ident()?;
                let id = Id::from_str(&ident);
                tb.add("Id (").suf_u64(id.0).add("),");
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
pub fn id_lut(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
        
    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        tb.add("Id::from_str_with_lut(").string(&name).add(").unwrap()");
        tb.end()
    }
    else if let Some(punct) = parser.eat_any_punct(){
        tb.add("Id::from_str_with_lut(").string(&punct).add(").unwrap()");
        tb.end()
    }
    else{
        parser.unexpected()
    }
}