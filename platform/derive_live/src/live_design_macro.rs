#![allow(unstable_name_collisions)]

use {
    proc_macro::{
        TokenStream,
        Span,
        Delimiter,
    },
    makepad_micro_proc_macro::{TokenBuilder, TokenParser},
    std::fmt::Write
};

pub fn live_design_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    if let Some(span) = parser.span() {
        let (s, live_types) = token_parser_to_whitespace_matching_string(&mut parser, span);
        //tb.ident(&cx);
        tb.add("pub fn live_design(cx:&mut Cx) {");
        tb.add("    let live_body = LiveBody {");
        tb.add("        cargo_manifest_path: env!(").string("CARGO_MANIFEST_DIR").add(").to_string(),");
        tb.add("        module_path :").ident_with_span("module_path", span).add("!().to_string(),");
        tb.add("        file:").ident_with_span("file", span).add("!().to_string().replace(").string("\\").add(",").string("/").add("),");
        tb.add("        line:line!() as usize,");
        tb.add("        column:column!() as usize,");
        tb.add("        live_type_infos:{");
        tb.add("            let mut v = Vec::new();");
        for live_type in &live_types {
            tb.stream(Some(live_type.clone())).add("::live_design_with(cx);");
            tb.add("        v.push(").stream(Some(live_type.clone())).add("::live_type_info(cx));");
        }
        tb.add("            v");
        tb.add("        },");
        tb.add("        code:").string(&s).add(".to_string()");
        tb.add("    };");
        tb.add("    cx.register_live_body(live_body);");
        tb.add("}");
        tb.end()
    }
    else {
        tb.add("pub fn live_design(cx:&mut Cx) {");
        tb.add("}");
        tb.end()
    }
}
 
// this function parses tokens into a source-equal whitespaced output string
fn token_parser_to_whitespace_matching_string(parser: &mut TokenParser, span: Span) -> (String, Vec<TokenStream>) {
    
    let mut s = String::new();
    let mut live_types = Vec::new();
    
    tp_to_str(parser, span, &mut s, &mut live_types, &mut None);
    
    return (s, live_types);
    
    #[derive(Clone, Copy)]
    struct Lc {
        line: usize,
        column: usize
    }
    
    impl Lc {
        fn _next_char(self) -> Self {
            Self {line: self.line, column: self.column + 1}
        }
    }
    
    fn delim_to_pair(delim: Delimiter) -> (char, char) {
        match delim {
            Delimiter::Brace => ('{', '}'),
            Delimiter::Parenthesis => ('(', ')'),
            Delimiter::Bracket => ('[', ']'),
            Delimiter::None => (' ', ' '),
        }
    }
    
    
    fn parse_type_ident(in_delim: Delimiter, parser: &mut TokenParser, out: &mut String, live_types: &mut Vec<TokenStream>) -> bool {
        if in_delim == Delimiter::Brace && parser.is_group_with_delim(Delimiter::Brace) {
            parser.open_group();
            write!(out, "{{{{dummy}}}}").unwrap();
            live_types.push(parser.eat_level());
            true
        }
        else {
            false
        }
    }
    
    fn tp_to_str(parser: &mut TokenParser, span: Span, out: &mut String, live_types: &mut Vec<TokenStream>, last_end: &mut Option<Lc>) {
        fn lc_from_start(span: Span) -> Lc {
            Lc {
                line: span.start().line(),
                column: span.start().column()
            }
        }
        
        fn lc_from_end(span: Span) -> Lc {
            Lc {
                line: span.end().line(),
                column: span.end().column()
            }
        }
        
        #[cfg(not(lines))]
        #[allow(clippy::ptr_arg)]
        fn delta_whitespace(_now: Lc, _needed: Lc, _out: &mut String) {
        }
        
        #[cfg(lines)]
        fn delta_whitespace(now: Lc, needed: Lc, out: &mut String) {
            if now.line == needed.line {
                for _ in now.column..needed.column {
                    out.push(' ');
                }
            }
            else {
                for _ in now.line..needed.line {
                    out.push('\n');
                }
                for _ in 1..needed.column {
                    out.push(' ');
                }
            }
        }
        
        if last_end.is_none() {
            *last_end = Some(lc_from_start(span));
        }
        
        #[cfg(not(lines))]
        let mut last_tt = None;
        
        while !parser.eat_eot() {
            let span = parser.span().unwrap();
            if let Some(delim) = parser.open_group() {
                // if delim is { and the next one is also { write out a type index
                if parse_type_ident(delim, parser, out, live_types) {
                    parser.eat_eot();
                    continue;
                }
                
                let (gs, ge) = delim_to_pair(delim);
                let start = lc_from_start(span);
                let end = lc_from_end(span);
                delta_whitespace(last_end.unwrap(), start, out);
                out.push(gs);
                *last_end = Some(start._next_char());
                tp_to_str(parser, span, out, live_types, last_end);
                delta_whitespace(last_end.unwrap(), Lc {line: end.line, column: end.column - 1}, out);
                *last_end = Some(end);
                out.push(ge);
            }
            else {
                if let Some(tt) = &parser.current {
                    #[cfg(not(lines))]
                    {
                        fn is_ident(tt: &TokenTree) -> bool {
                            if let TokenTree::Ident(_) = tt {
                                return true
                            }
                            false
                        }
                        
                        fn is_string_lit(tt: &TokenTree) -> bool {
                            if let TokenTree::Literal(lit) = tt {
                                if let Some('"') = lit.to_string().chars().next() {
                                    return true
                                }
                            }
                            false
                        }
                        
                        fn is_punct(tt: &TokenTree) -> bool {
                            if let TokenTree::Punct(_) = tt {
                                return true
                            }
                            false
                        }
                        if let Some(last_tt) = &last_tt {
                            if !((is_ident(last_tt) && is_string_lit(tt)) || is_punct(last_tt)) {
                                out.push(' ');
                            }
                        }
                        last_tt = Some(tt.clone());
                    };
                    #[cfg(lines)]
                    {
                        let start = lc_from_start(span);
                        delta_whitespace(last_end.unwrap(), start, out);
                    }
                    out.push_str(&tt.to_string());
                    
                    *last_end = Some(lc_from_end(span));
                }
                parser.advance();
            }
        }
    }
}

// Span fallback API

#[cfg(not(lines))]
use proc_macro::TokenTree;

#[cfg(not(lines))]
struct SpanFallbackApiInfo {
    line: usize,
    column: usize
}

#[cfg(not(lines))]
impl SpanFallbackApiInfo{
    fn line(&self)->usize{self.line}
    fn column(&self)->usize{self.column}
}

#[cfg(not(lines))]
trait SpanFallbackApi {
    fn start(&self) -> SpanFallbackApiInfo {
        SpanFallbackApiInfo {line: 1, column: 1}
    }
    fn end(&self) -> SpanFallbackApiInfo {
        SpanFallbackApiInfo {line: 1, column: 1}
    }
}

#[cfg(not(lines))]
impl SpanFallbackApi for Span {}
