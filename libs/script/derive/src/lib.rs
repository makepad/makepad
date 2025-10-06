extern crate proc_macro;
use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser};

#[derive(Clone, Default, Eq, Hash, Copy, Ord, PartialOrd, PartialEq)]
struct Id(pub u64);

impl Id {
    pub const SEED:u64 = 0xd6e8_feb8_6659_fd93;
    pub const ARRAY:u64 = 0x0000_2000_0000_0000;
    // from https://nullprogram.com/blog/2018/07/31/
    // i have no idea what im doing with start value and finalisation.
    pub const fn from_bytes(seed:u64, id_bytes: &[u8], start: usize, end: usize, or:u64) -> Self {
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
        // truncate to 45 bits fitting in a NaN box
        Self ((x & 0x0000_1fff_ffff_ffff) | or)
    }
            
    pub const fn from_str(id_str: &str) -> Self {
        let bytes = id_str.as_bytes();
        if bytes.len() > 0 && bytes[0] == b'$'{
            Self::from_bytes(Self::SEED, bytes, 0, bytes.len(), Self::ARRAY)
        }
        else{
            Self::from_bytes(Self::SEED, bytes, 0, bytes.len(), 0)
        }
    }
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

#[proc_macro_derive(Script)]
pub fn derive_script(_input: TokenStream) -> TokenStream {
    Default::default()
}
