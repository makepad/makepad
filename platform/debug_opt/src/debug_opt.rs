use proc_macro::{TokenStream};

use makepad_macro_lib::{TokenBuilder, TokenParser};

pub fn derive_debug_opt_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("std::fmt::Debug for ").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone());
            tb.add("{");
            tb.add("    fn fmt(&self, _f: &mut std::fmt::Formatter)->Result<(),std::fmt::Error>{");
            tb.add("        Ok(())");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    else if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let _types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            tb.add("#[derive(Debug)]");
            tb.eprint();
            tb.end();
            
/*            tb.add("impl").stream(generic.clone());
            tb.add("std::fmt::Debug for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("{");
            tb.add("    fn fmt(&self, _f: &mut std::fmt::Formatter)->Result<(),std::fmt::Error>{");
            tb.add("        Ok(())");
            tb.add("    }");
            tb.add("}");
            return tb.end();*/
        }
    }
    return parser.unexpected()
}
