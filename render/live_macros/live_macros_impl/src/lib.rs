use proc_macro_hack::proc_macro_hack;
use proc_macro::{TokenTree, TokenStream, Span};
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse;
use makepad_shader_compiler::ast::{ShaderAst, Decl, TyExprKind};
use makepad_shader_compiler::colors::Color;
use makepad_shader_compiler::shader::ShaderGen;

#[path = "../../../microserde/derive/src/macro_lib.rs"]
mod macro_lib; 
use crate::macro_lib::*;


fn live_loc(tb:&mut TokenBuilder, span:Span){
    tb.add("LiveLoc {");
    tb.add("file :").ident_with_span("file", span).add("! ( )").add(",");
    tb.add("line :").ident_with_span("line", span).add("! ( ) as usize").add(",");
    tb.add("column : ").ident_with_span("column", span).add("! ( ) as usize").add("}");
}

// The actual macro
#[proc_macro_hack]
pub fn shader(input: TokenStream) -> TokenStream {
    let mut input_iter = input.into_iter();
    // get the first, error if more
    if let Some(TokenTree::Literal(lit)) = input_iter.next() {
        // alright. lets get the string out
        // dump it in the shader parser
        let source_quoted = &lit.to_string();
        if source_quoted.len() <= 2 {
            return error("shader macro needs single string with shader code as argument");
        }
        
        let source = &source_quoted[1..source_quoted.len() - 1];
        let tokens = lex::lex(source.chars(), 0).collect::<Result<Vec<_>, _>>();
        if let Err(err) = tokens {
            let start = ShaderGen::byte_to_row_col(err.span.start, source);
            return error_span(&format!("Shader lex error relative line:{} col:{} len:{} - {}", start.0, start.1 + 1, err.span.end - err.span.start, err), lit.span());
        }
        let tokens = tokens.unwrap();
        

        let mut shader = ShaderAst::new();
        if let Err(err) = parse::parse(&tokens, &mut shader) {
            // lets find the span info
            let start = ShaderGen::byte_to_row_col(err.span.start, source);
            return error_span(&format!("Shader parse error relative line:{} col:{} len:{} - {}", start.0, start.1 + 1, err.span.end - err.span.start, err), lit.span());
        }
        
        let mut tb = TokenBuilder::new();
        tb.add("ShaderSub {");
        tb.add("loc :");
        live_loc(&mut tb, lit.span());
        tb.add(", code :").string(source).add(". to_string ( ) ,");
        
        fn prop_def(tb: &mut TokenBuilder, decl_ident: String, call_ident: String) {
            tb.add("PropDef { name :").string(&decl_ident).add(". to_string ( )");
            tb.add(", ident :").string(&call_ident).add(". to_string ( ) ,");
            tb.add("prop_id :");
            for (last, part) in call_ident.split("::").identify_last() {
                tb.ident(part);
                if !last {
                    tb.add("::");
                }
            }
            tb.add("( ) . prop_id ( )");
            tb.add("} ,");
        }
        tb.add("attribute_props : vec ! [");
        for decl in &shader.decls {
            match decl {
                Decl::Attribute(decl) => {
                    match decl.ty_expr.kind {
                        TyExprKind::Var {ident, ..} => {
                            prop_def(&mut tb, decl.ident.to_string(), ident.to_string());
                        },
                        _ => {
                            return error(&format!("Type expression for attribute {}", decl.ident));
                        }
                    }
                },
                _ => ()
            }
        }
        tb.add("] , instance_props : vec ! [");
        for decl in &shader.decls {
            match decl {
                Decl::Instance(decl) => {
                    match decl.ty_expr.kind {
                        TyExprKind::Var {ident, ..} => {
                            prop_def(&mut tb, decl.ident.to_string(), ident.to_string());
                        },
                        _ => {
                            return error(&format!("Type expression for instance {}", decl.ident));
                        }
                    }
                },
                _ => ()
            }
        }
        tb.add("] , uniform_props : vec ! [");
        for decl in &shader.decls {
            match decl {
                Decl::Uniform(decl) => {
                    match decl.ty_expr.kind {
                        TyExprKind::Var {ident, ..} => {
                            prop_def(&mut tb, decl.ident.to_string(), ident.to_string());
                        },
                        _ => {
                            return error(&format!("Type expression for uniform {}", decl.ident));
                        }
                    }
                },
                _ => ()
            }
        }
        tb.add("] }");
        
        if input_iter.next().is_some() {
            // return error
            return error("shader macro needs single string as argument");
        }
        
        let output = tb.end();
        return output
    }
    else {
        // return error
        return error("shader macro needs single string as argument");
    }
    
}

// The actual macro
#[proc_macro_hack]
pub fn color(input: TokenStream) -> TokenStream {

    fn parse_color_channel(tt:&TokenTree)->f32{
        if let TokenTree::Literal(c) = tt{
            let c = c.to_string();
            let parsed = c.parse();
            if !parsed.is_ok(){
                return 1.0;
            }
            let parsed = parsed.unwrap();
            if c.contains("."){
                return parsed;
            }else{
                return parsed / 255.0;
            }
        }
        return 1.0;
    }
    
    let items = input.into_iter().collect::<Vec<TokenTree>>();
    // get the first, error if more
    let color = if items.len() == 1{
        if let TokenTree::Ident(ident) = &items[0]{
            Color::parse(&format!("#{}", ident.to_string()))
        }
        else{
            return error("color macro argument error");
        }
    }
    else if items.len() == 2{
        if let TokenTree::Punct(pct) = &items[0]{
            if pct.as_char() != '#'{
                return error_span("color macro argument error", pct.span());
            }
            if let TokenTree::Ident(ident) = &items[1] {
                Color::parse(&format!("#{}", ident.to_string()))
            }
            else if let TokenTree::Literal(lit) = &items[1]{
                Color::parse(&format!("#{}", lit.to_string()))
            }
            else{
                return error("color macro argument error");
            }
        }
        else{
            return error("color macro argument error");
        }
    }
    else if items.len() == 5{ // its rgb
        Color{
            r:parse_color_channel(&items[0]),
            g:parse_color_channel(&items[2]),
            b:parse_color_channel(&items[4]),
            a:1.0
        }
    }
    else if items.len() == 7{
        Color{
            r:parse_color_channel(&items[0]),
            g:parse_color_channel(&items[2]),
            b:parse_color_channel(&items[4]),
            a:parse_color_channel(&items[6]),
        }
    }else{
        return error("color macro argument error");
    };
    
    let mut tb = TokenBuilder::new();
    tb.add("LiveColor {");
    tb.add("loc :");
    live_loc(&mut tb, items[0].span());
    // now the color
    tb.add(", color : Color {");
    tb.add("r :").unsuf_f32(color.r).add(",");
    tb.add("g :").unsuf_f32(color.g).add(",");
    tb.add("b :").unsuf_f32(color.b).add(",");
    tb.add("a :").unsuf_f32(color.a);
    tb.add("} }");
    tb.end()
}

