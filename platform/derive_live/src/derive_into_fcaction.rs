use proc_macro::{TokenStream};

use crate::macro_lib::*;

pub fn derive_into_frame_component_action_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let _types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            tb.add("impl").stream(generic.clone());
            tb.add("Into<OptionFrameComponentAction> for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn into(self)->Option<Box<dyn FrameComponentAction>>{");
            tb.add("        match &self{");
            tb.add("            Self::None=>None,");
            tb.add("            _=>Some(Box::new(self))");
            tb.add("        }");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("Into<OptionFrameComponentAction> for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn into(self)->Option<Box<dyn FrameComponentAction>>{");
            tb.add("        match &self{");
            tb.add("            Self::None=>None,");
            tb.add("            _=>Some(Box::new(self))");
            tb.add("        }");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

