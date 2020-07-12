use proc_macro_hack::proc_macro_hack;
use proc_macro::{TokenTree, TokenStream};
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse;
use makepad_shader_compiler::ast::{ShaderAst, Decl, TyExprKind};

#[path = "../../../microserde/derive/src/macro_lib.rs"]
mod macro_lib; 
use crate::macro_lib::*;

fn byte_to_row_col(byte:usize, source:&str)->(usize,usize){
    let lines = source.split("\n");
    let mut o = 0;
    for (index,line) in lines.enumerate(){
        if byte >= o && byte < o+line.len(){
            return (index, byte - o)
        }
        o += line.len() + 1;
    }
    return (0,0)
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
        let tokens = lex::lex(source.chars()).collect::<Result<Vec<_>, _>>();
        if let Err(err) = tokens {
            let start = byte_to_row_col(err.span.start, source);
            return error_span(&format!("Shader error relative line:{} col:{} len:{} - {}", start.0, start.1 + 1, err.span.end - err.span.start, err), lit.span());
        }
        let tokens = tokens.unwrap();
        

        let mut shader = ShaderAst::new();
        if let Err(err) = parse::parse(&tokens, &mut shader) {
            // lets find the span info
            let start = byte_to_row_col(err.span.start, source);
            return error_span(&format!("Shader error relative line:{} col:{} len:{} - {}", start.0, start.1 + 1, err.span.end - err.span.start, err), lit.span());
        }
        
        let mut tb = TokenBuilder::new();
        tb.add("ShaderSub {");
        tb.add("file : ").file_macro(lit.span()).add(",");
        tb.add("line : ").line_macro(lit.span()).add(",");
        tb.add("code :").string(source).add(". to_string ( ) , instance_props : vec ! [");
        
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
    
    let mut input_iter = input.into_iter();
    // get the first, error if more
    if let Some(item) = input_iter.next() {
        eprintln!("{:?}", item);
        
    }
    else{
        return error("color macro is empty");
    }
        
    
    TokenStream::new()
}

