use proc_macro::{TokenStream};

use makepad_macro_lib::{TokenBuilder, TokenParser};

pub fn derive_frame_action_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
   if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl Into<Box<dyn FrameAction>> for ").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone());
            tb.add("{");
            tb.add("    fn into(self)->Box<dyn FrameAction>{");
            tb.add("        Box::new(self)");
            tb.add("    }");
            tb.add("}");
            tb.add("impl").stream(generic.clone());
            tb.add("Default for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn default()->Self{Self::None}");
            tb.add("}");
            
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_frame_component_impl(input: TokenStream) -> TokenStream {
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
            tb.add("FrameComponent for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, FramePath, Box<dyn FrameAction>)) {");
            tb.add("        self.handle_event(cx, event, &mut |cx, action|{");
            tb.add("            dispatch_action(cx, FramePath::empty(), action.into())");
            tb.add("        });");
            tb.add("    }");
            tb.add("    fn get_walk(&self) -> Walk {");
            tb.add("        self.walk");
            tb.add("    }");
            tb.add("    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, _self_uid:FrameUid) -> DrawResult {");
            tb.add("        self.draw_walk(cx, walk);");
            tb.add("        DrawResult::Done");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

