extern crate proc_macro;
use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, Id, error};

#[proc_macro] 
pub fn script_id(item: TokenStream) -> TokenStream {
    id(item)
}

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
pub fn script_ids(item: TokenStream) -> TokenStream {
    ids(item)
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
pub fn script_ids_array(item: TokenStream) -> TokenStream {
    ids_array(item)
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
pub fn script_id_lut(item: TokenStream) -> TokenStream {
    id_lut(item)
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

#[proc_macro_derive(FromLiveId)]
pub fn derive_from_live_id_impl(input: TokenStream) -> TokenStream {
    derive_from_id_impl(input)
}

#[proc_macro_derive(FromId)]
pub fn derive_from_id_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            tb.add("impl");
            tb.add("From<Id> for").ident(&struct_name).add("{");
            tb.add("    fn from(live_id:Id)->").ident(&struct_name).add("{").ident(&struct_name).add("(live_id)}");
            tb.add("}");
                        
            tb.add("impl");
            tb.add("From<&[Id;1]> for").ident(&struct_name).add("{");
            tb.add("    fn from(live_id:&[LiveId;1])->").ident(&struct_name).add("{").ident(&struct_name).add("(live_id[0])}");
            tb.add("}");
            
            tb.add("impl");
            tb.add("From<u64> for").ident(&struct_name).add("{");
            tb.add("    fn from(live_id:u64)->").ident(&struct_name).add("{").ident(&struct_name).add("(LiveId(live_id))}");
            tb.add("}");
            
            
            return tb.end();
        }
    }
    parser.unexpected()
}

// absolutely a very bad idea but lets see if we can do this.
#[proc_macro]
pub fn live_id_num(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    
    let mut parser = TokenParser::new(item);
    if let Some(name) = parser.eat_any_ident() {
        if !parser.eat_punct_alone(','){
            return error("please add a number")
        }
        // then eat the next bit
        let arg = parser.eat_level();
        let id = Id::from_str(&name);
        tb.add("Id::from_num(").suf_u64(id.0).add(",").stream(Some(arg)).add(")");
        tb.end()
    }
    else{
        parser.unexpected()
    }
}

#[proc_macro] 
pub fn some_id(item: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new(); 
    let v = item.to_string();
    let id = Id::from_str(&v);
    tb.add("Some(Id (").suf_u64(id.0).add("))");
    tb.end()
}