#![feature(proc_macro_span)]
use proc_macro::{TokenStream, Span, Delimiter};

mod derive_live; 
use crate::derive_live::*;

#[path = "../../microserde/derive/src/macro_lib.rs"]
mod macro_lib;
use crate::macro_lib::*;
use std::fmt::Write;

//use crate::id::*;
#[path = "../../live_parser/src/id.rs"]
mod id; 

#[proc_macro_derive(Live, attributes(local, live, align_64))] 
pub fn derive_live(input: TokenStream) -> TokenStream {
    derive_live_impl(input)
}


#[proc_macro]
pub fn live_body(input: TokenStream) -> TokenStream {

    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    if let Some(span) = parser.span(){
        let mut s = String::new();
        let mut live_types = Vec::new();
        tokenparser_to_string(&mut parser, span, &mut s, &mut live_types, &mut None);
        
        //tb.ident(&cx);
        tb.add("fn live_body ( ) -> LiveBody { LiveBody {");
        tb.add("module_path :").ident_with_span("module_path",span).add("! ( ) . to_string ( ) ,");
        tb.add("file :").ident_with_span("file",span).add("! ( ) . to_string ( ) . replace ( ").string("\\").add(",").string("/").add(") ,");
        tb.add("line :").ident_with_span("line",span).add("! ( ) as usize ,");
        tb.add("column :").ident_with_span("column",span).add("! ( ) as usize ,");
        tb.add("live_types : { let mut v = Vec :: new ( ) ;");
        for live_type in live_types{
            tb.add("v . push (").stream(Some(live_type)).add(" :: live_type ( ) ) ;");
        }
        tb.add(" v } ,");
        tb.add("code :").string(&s).add(" . to_string ( ) } }");
        return tb.end();
    }
    else{
        return parser.unexpected();
    } 
}

fn parse_type_ident(parser: &mut TokenParser, out: &mut String,  live_types: &mut Vec<TokenStream>)->bool{
     if parser.is_group_with_delim(Delimiter::Brace){
        parser.open_group();
        write!(out, "{{{{{0}}}}}", live_types.len()).unwrap();
        live_types.push(parser.eat_level());
        true
    }
    else{
        false
    }
}

#[cfg(feature = "nightly")]
fn tokenparser_to_string(parser: &mut TokenParser, span:Span, out: &mut String, live_types:&mut Vec<TokenStream>, last_end:&mut Option<Lc>){
    fn lc_from_start(span:Span)->Lc{
        Lc{
            line:span.start().line,
            column:span.start().column
        }
    }
    
    fn lc_from_end(span:Span)->Lc{
        Lc{
            line:span.end().line,
            column:span.end().column
        }
    }
    
    fn delta_whitespace(now:Lc, needed:Lc, out: &mut String){
        
        if now.line == needed.line{
            for _ in now.column..needed.column{
                out.push(' ');
            }
        }
        else{
            for _ in now.line..needed.line{
                out.push('\n');
            }
            for _ in 0..needed.column{
                out.push(' ');
            }
        }
    }
    
    if last_end.is_none(){
        *last_end = Some(lc_from_start(span));
    }

    while !parser.eat_eot(){
        let span = parser.span().unwrap();
        
        
        if let Some(delim) = parser.open_group(){
            // if delim is { and the next one is also { write out a type index
            if parse_type_ident(parser, out, live_types){
                parser.eat_eot();
                continue;
            }
            
            let (gs,ge) = delim_to_pair(delim);
            let start = lc_from_start(span);
            let end = lc_from_end(span);
            delta_whitespace(last_end.unwrap(), start, out);
            out.push(gs);
            *last_end = Some(start._next_char());
            tokenparser_to_string(parser, span, out, live_types, last_end);
            delta_whitespace(last_end.unwrap(), end, out);
            *last_end = Some(end);
            out.push(ge);
        }
        else{
            if let Some(tt) = &parser.current{
                let start = lc_from_start(span);
                delta_whitespace(last_end.unwrap(), start, out);
                out.push_str(&tt.to_string());
                *last_end = Some(lc_from_end(span));
            }
            parser.advance();
        } 
    }
}

#[cfg(not(feature = "nightly"))]
fn tokenparser_to_string(parser: &mut TokenParser, _span:Span, out: &mut String, live_types:&mut Vec<TokenStream>, last_end:&mut Option<Lc>){
    while !parser.eat_eot(){
        let span = parser.span().unwrap();
        if let Some(delim) = parser.open_group(){
            // if delim is { and the next one is also { write out a type index
            parse_type_ident(parser, out, live_types);
            let (s,e) = delim_to_pair(delim);
            out.push(s);
            tokenparser_to_string(parser, span, out, live_types, last_end);
            out.push(e);
        }
        else{
            if let Some(tt) = &parser.current{
                out.push_str(&tt.to_string());
            }
            parser.advance();
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Lc {
    line: usize,
    column: usize
}

impl Lc{
    fn _next_char(self)->Self{
        Self{line:self.line, column:self.column+1}
    }
}

fn delim_to_pair(delim:Delimiter)->(char, char){
    match delim{
        Delimiter::Brace=>('{','}'),
        Delimiter::Parenthesis=>('(',')'),
        Delimiter::Bracket=>('[',']'),
        Delimiter::None=>(' ',' '),
    }
}
