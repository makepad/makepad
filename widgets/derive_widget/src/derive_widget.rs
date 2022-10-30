use proc_macro::{TokenStream};

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, error};

pub fn derive_widget_action_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl Into<Box<dyn WidgetAction>> for ").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone());
            tb.add("{");
            tb.add("    fn into(self)->Box<dyn WidgetAction>{");
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

pub fn derive_widget_impl(input: TokenStream) -> TokenStream {
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
            tb.add("Widget for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}");
            tb.add("    fn handle_widget_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {");
            tb.add("        let uid = self.widget_uid();");
            tb.add("        self.handle_event_fn(cx, event, &mut |cx, action|{");
            tb.add("            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))");
            tb.add("        });");
            tb.add("    }");
            tb.add("    fn redraw(&mut self, cx:&mut Cx) {");
            tb.add("        self.area().redraw(cx)");
            tb.add("    }");
            tb.add("    fn get_walk(&self) -> Walk {");
            tb.add("        self.walk");
            tb.add("    }");
            tb.add("    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {");
            tb.add("        let _= self.draw_walk(cx, walk);");
            tb.add("        WidgetDraw::done()");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn camel_case_to_snake_case(inp: &str) -> String {
    let mut out = String::new();
    let mut last_uc = true;
    for c in inp.chars() {
        if c.is_ascii_uppercase() {
            if !last_uc{
                out.push('_');
            }
            last_uc = true;
            out.push(c.to_ascii_lowercase())
        }
        else {
            last_uc = false;
            out.push(c)
        }
    }
    out
}

pub fn derive_widget_ref_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let clean_name = if let Some(sn) = struct_name.strip_suffix("Ref"){
                sn
            }
            else{
                return error("derive WidgetRef can only be done on a struct with ending name Ref")
            };
            let snake_name = camel_case_to_snake_case(&clean_name);
            
            tb.add("impl std::ops::Deref for ").ident(&struct_name).add("{");
            tb.add("    type Target = WidgetRef;");
            tb.add("    fn deref(&self)->&Self::Target{");
            tb.add("        &self.0");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl").ident(&struct_name).add("{");
            
            tb.add("   pub fn inner(&self) -> Option<std::cell::Ref<'_, ").ident(clean_name).add(" >> {");
            tb.add("       self.0.inner()");
            tb.add("   }");
            
            tb.add("   pub fn inner_mut(&self) -> Option<std::cell::RefMut<'_, ").ident(clean_name).add(" >> {");
            tb.add("       self.0.inner_mut()");
            tb.add("   }");
            tb.add("}");
            
            tb.add("impl LiveHook for ").ident(&struct_name).add("{}");
            tb.add("impl LiveApply for ").ident(&struct_name).add("{");
            tb.add("    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {");
            tb.add("        if let Some(mut inner) = self.inner_mut(){");
            tb.add("            return inner.apply(cx, from, index, nodes)");
            tb.add("        }");
            tb.add("        nodes.skip_node(index)");
            tb.add("    }");
            tb.add("}");

            tb.add("impl LiveNew for ").ident(&struct_name).add("{");
            tb.add("    fn new(cx: &mut Cx) -> Self {");
            tb.add("        Self (WidgetRef::new_with_inner(Box::new(").ident(clean_name).add("::new(cx))))");
            tb.add("    }");
            tb.add("    fn live_type_info(cx: &mut Cx) -> LiveTypeInfo {");
            tb.add("        ").ident(clean_name).add("::live_type_info(cx)");
            tb.add("    }");
            tb.add("}");

            let frame_ext = format!("{}FrameRefExt", clean_name);
            let widget_ext = format!("{}WidgetRefExt", clean_name);
            let get_fn = format!("get_{}", snake_name);
            let into_fn = format!("into_{}", snake_name);
            
            tb.add("pub trait").ident(&frame_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&struct_name).add(";");
            tb.add("}");

            tb.add("impl ").ident(&frame_ext).add(" for FrameRef{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&struct_name).add("{");
            tb.add("        ").ident(&struct_name).add("(self.get_widget(path))");
            tb.add("    }");
            tb.add("}");

            tb.add("pub trait").ident(&widget_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&struct_name).add(";");
            tb.add("    fn ").ident(&into_fn).add("(self) -> ").ident(&struct_name).add(";");
            tb.add("}");

            tb.add("impl ").ident(&widget_ext).add(" for WidgetRef{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&struct_name).add("{");
            tb.add("        ").ident(&struct_name).add("(self.get_widget(path))");
            tb.add("    }");
            tb.add("    fn ").ident(&into_fn).add("(self) -> ").ident(&struct_name).add("{");
            tb.add("        ").ident(&struct_name).add("(self)");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

