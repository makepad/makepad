use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, error};

pub fn derive_widget_impl(input: TokenStream) ->  TokenStream {
    let mut out = TokenStream::new();
    out.extend(derive_widget_node_impl(input.clone()));
    out.extend(derive_widget_register_impl(input.clone()));
    out.extend(derive_widget_ref_impl(input.clone()));
    out.extend(derive_widget_set_impl(input.clone()));
    out
}

pub fn derive_widget_node_impl(input: TokenStream) ->  TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let main_attribs = parser.eat_attributes();
    let designable =  main_attribs.iter().any(|v| v.name == "designable");
    
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        let struct_name = parser.expect_any_ident().unwrap();
        let generic = parser.eat_generic();
        let types = parser.eat_all_types();
        let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
                
        let mut fields = if let Some(_types) = types {
            return error("Unexpected type form")
        }
        else if let Some(fields) = parser.eat_all_struct_fields() {
            fields
        }
        else {
            return error("Unexpected field form")
        };
                
        // alright now. we have a field
        let mut walk_field = None;
        let mut area_field = None;
        let mut deref_field = None;
        let mut wrap_field = None;
        let mut action_data_field = None;
        let mut find_fields = Vec::new();
        let mut redraw_fields = Vec::new();
        for field in &mut fields {
            if field.attrs.iter().any(|v| v.name == "walk"){
                walk_field = Some(field.name.clone());
            }
            if field.attrs.iter().any(|v| v.name == "deref"){
                deref_field = Some(field.name.clone());
            }
            if field.attrs.iter().any(|v| v.name == "redraw"){
                redraw_fields.push(field.name.clone());
            }
            if field.attrs.iter().any(|v| v.name == "find"){
                find_fields.push(field.name.clone());
            }
            if field.attrs.iter().any(|v| v.name == "wrap"){
                wrap_field = Some(field.name.clone());
            }
            if field.attrs.iter().any(|v| v.name == "area"){
                area_field = Some(field.name.clone());
            }
            if field.attrs.iter().any(|v| v.name == "action_data"){
                action_data_field = Some(field.name.clone());
            }
        }
        tb.add("impl").stream(generic.clone());
        tb.add("WidgetNode for").ident(&struct_name).stream(generic).stream(where_clause).add("{");
        if designable{
            tb.add("    fn widget_design(&mut self) -> Option<&mut dyn WidgetDesign>{return Some(self)}");
        }
        
        if let Some(action_data_field) = &action_data_field{
            tb.add("    fn set_action_data(&mut self, action_data:std::sync::Arc<dyn WidgetActionTrait>) { self.").ident(action_data_field).add(".set_box(action_data)}");
            tb.add("    fn action_data(&mut self)->Option<std::sync::Arc<dyn WidgetActionTrait>> { self.").ident(action_data_field).add(".clone_data()}");
        }
        
        if let Some(wrap_field) = &wrap_field{
            tb.add("    fn walk(&mut self, cx:&mut Cx) -> Walk { self.").ident(wrap_field).add(".walk(cx)}");            
            tb.add("    fn redraw(&mut self, cx:&mut Cx) { self.").ident(wrap_field).add(".redraw(cx)}");
            tb.add("    fn area(&self)->Area{ self.").ident(wrap_field).add(".area()}");
            tb.add("    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet){self.").ident(wrap_field).add(".find_widgets(path, cached, results)}");
            tb.add("   fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{");
            tb.add("       self.").ident(wrap_field).add(".uid_to_widget(uid)");
            tb.add("   }"); 
        }
        else{
            if let Some(area_field) = &area_field{
                tb.add("    fn area(&self)->Area{ self.").ident(area_field).add(".area()}");
            }
            else if let Some(deref_field) = &deref_field{
                tb.add("    fn area(&self)->Area{ self.").ident(deref_field).add(".area()}");
            }
            else if !redraw_fields.is_empty(){
                tb.add("    fn area(&self)->Area{ self.").ident(&redraw_fields[0]).add(".area()}");
            }
            
            if let Some(walk_field) = &walk_field{
                tb.add("    fn walk(&mut self, _cx:&mut Cx) -> Walk { self.").ident(walk_field).add("}");
            }
            else if let Some(deref_field) = &deref_field{
                tb.add("    fn walk(&mut self, cx:&mut Cx) -> Walk { self.").ident(deref_field).add(".walk(cx)}");
            }
            else{
                tb.add("    fn walk(&mut self, cx:&mut Cx) -> Walk { Walk::default()}");
            }
            if !redraw_fields.is_empty(){
                tb.add("    fn redraw(&mut self, cx:&mut Cx) {");
                for redraw_field in redraw_fields{
                    tb.add("    self.").ident(&redraw_field).add(".redraw(cx);");
                }
                tb.add("    }");
            }
            else if let Some(deref_field) = &deref_field{
                tb.add("    fn redraw(&mut self, cx:&mut Cx) { self.").ident(deref_field).add(".redraw(cx)}");
            }
            else{
                return error("Need either a field marked redraw or deref or wrap to find redraw method")
            }
            if !find_fields.is_empty(){
                tb.add("    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet){");
                for find_field in &find_fields{
                    tb.add("    self.").ident(find_field).add(".find_widgets(path, cached, results);");
                }
                tb.add("    }");
                tb.add("    fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{");
                for find_field in &find_fields{
                    tb.add("    let x = self.").ident(find_field).add(".uid_to_widget(uid);");
                    tb.add("    if !x.is_empty(){return x;}");
                }
                tb.add("        WidgetRef::empty()");
                tb.add("    }");
            }
            else if let Some(deref_field) = &deref_field{
                tb.add("    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet){self.").ident(deref_field).add(".find_widgets(path, cached, results)}");
                tb.add("   fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{");
                tb.add("       self.").ident(deref_field).add(".uid_to_widget(uid)");
                tb.add("   }");
            }
            else{
                tb.add("    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet){}");
                tb.add("    fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{");
                tb.add("       WidgetRef::empty()");
                tb.add("   }");            
            }
        }
        tb.add("}");
        return tb.end();
    }
    parser.unexpected()
}

 
pub fn derive_widget_register_impl(input: TokenStream) -> TokenStream {
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
            tb.add("LiveRegister for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            //tb.add("    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}");
            tb.add("    fn live_register(cx: &mut Cx) {");
            tb.add("        register_widget!(cx, ").ident(&struct_name).add(");");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    parser.unexpected() 
}

/*
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
            //tb.add("    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}");
            tb.add("    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {");
            tb.add("        let uid = self.widget_uid();");
            tb.add("        self.handle_event_with(cx, event, &mut |cx, action|{");
            tb.add("            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))");
            tb.add("        });");
            tb.add("    }");
            tb.add("    fn redraw(&mut self, cx:&mut Cx) {");
            tb.add("        self.area().redraw(cx)");
            tb.add("    }");
            tb.add("    fn walk(&self) -> Walk {");
            tb.add("        self.get_widwalk()");
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
}*/

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
        if let Some(clean_name) = parser.eat_any_ident() {
            let ref_name = format!("{}Ref", clean_name);
            let snake_name = camel_case_to_snake_case(&clean_name);
                         
            tb.add("#[derive(Clone, Debug)]");
            tb.add("pub struct ").ident(&ref_name).add("(WidgetRef);");
            tb.add("impl std::ops::Deref for ").ident(&ref_name).add("{");
            tb.add("    type Target = WidgetRef;");
            tb.add("    fn deref(&self)->&Self::Target{");
            tb.add("        &self.0");
            tb.add("    }");
            tb.add("}");
            tb.add("impl std::ops::DerefMut for ").ident(&ref_name).add("{");
            tb.add("    fn deref_mut(&mut self)->&mut Self::Target{");
            tb.add("        &mut self.0");
            tb.add("    }");
            tb.add("}");
            tb.add("impl").ident(&ref_name).add("{");
            
            tb.add("    pub fn has_widget(&self, widget:&WidgetRef)->").ident(&ref_name).add("{");
            tb.add("        if self.0 == *widget{");
            tb.add("             ").add(&ref_name).add("(widget.clone())");
            tb.add("        } else {");
            tb.add("             ").add(&ref_name).add("(WidgetRef::default())");
            tb.add("        }");
            tb.add("    }");

            tb.add("   pub fn borrow(&self) -> Option<std::cell::Ref<'_, ").ident(&clean_name).add(" >> {");
            tb.add("       self.0.borrow()");
            tb.add("   }");
            
            tb.add("   pub fn borrow_mut(&self) -> Option<std::cell::RefMut<'_, ").ident(&clean_name).add(" >> {");
            tb.add("       self.0.borrow_mut()");
            tb.add("   }");

            tb.add("   pub fn borrow_if_eq(&self, widget:&WidgetRef) -> Option<std::cell::Ref<'_, ").ident(&clean_name).add(" >> {");
            tb.add("        if self.0 == *widget{");
            tb.add("             self.0.borrow()");
            tb.add("        } else {");
            tb.add("             None");
            tb.add("        }");
            tb.add("   }");
            
            tb.add("   pub fn borrow_mut_if_eq(&self, widget:&WidgetRef) -> Option<std::cell::RefMut<'_, ").ident(&clean_name).add(" >> {");
            tb.add("        if self.0 == *widget{");
            tb.add("             self.0.borrow_mut()");
            tb.add("        } else {");
            tb.add("             None");
            tb.add("        }");
            tb.add("   }");
            tb.add("}");

            //let frame_ext = format!("{}ViewRefExt", clean_name);
            let widget_ref_ext = format!("{}WidgetRefExt", clean_name);
            let actions_ext = format!("{}WidgetActionsExt", clean_name);
            let widget_ext = format!("{}WidgetExt", clean_name);
            let get_fn = snake_name.to_string();
            let as_fn = format!("as_{}", snake_name);

            tb.add("pub trait").ident(&widget_ref_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&ref_name).add(";");
            tb.add("    fn ").ident(&as_fn).add("(&self) -> ").ident(&ref_name).add(";");
            tb.add("}");
            
            tb.add("pub trait").ident(&actions_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&ref_name).add(";");
            tb.add("}");
            
            tb.add("impl ").ident(&widget_ref_ext).add(" for WidgetRef{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&ref_name).add("{");
            tb.add("        ").ident(&ref_name).add("(self.widget(path))");
            tb.add("    }");
            tb.add("    fn ").ident(&as_fn).add("(&self) -> ").ident(&ref_name).add("{");
            tb.add("        ").ident(&ref_name).add("(self.clone())");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl ").ident(&actions_ext).add(" for Actions{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&ref_name).add("{");
            tb.add("        ").ident(&ref_name).add("(self.widget(path))");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl Default for ").ident(&ref_name).add("{");
            tb.add("    fn default()->Self{Self(Default::default())}");
            tb.add("}");
/*
            tb.add("impl ").ident(&widget_ref_ext).add(" for Option<WidgetRef>{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&ref_name).add("{");
            tb.add("        if let Some(s) = &self{").ident(&ref_name).add("(s.widget(path))}else{").ident(&ref_name).add("(WidgetRef::empty())}");
            tb.add("    }");
            tb.add("    fn ").ident(&as_fn).add("(&self) -> ").ident(&ref_name).add("{");
            tb.add("        if let Some(s) = &self{").ident(&ref_name).add("(s.clone())}else{").ident(&ref_name).add("(WidgetRef::empty())}");
            tb.add("    }");
            tb.add("}"); */
                        
            tb.add("pub trait").ident(&widget_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&ref_name).add(";");
            tb.add("}");
            
            tb.add("impl<T> ").ident(&widget_ext).add(" for T where T: Widget{");
            tb.add("    fn ").ident(&get_fn).add("(&self, path: &[LiveId]) -> ").ident(&ref_name).add("{");
            tb.add("        ").ident(&ref_name).add("(self.widget(path))");
            tb.add("    }");
            tb.add("}");
            
            return tb.end();
        }
    }

    parser.unexpected()
}


pub fn derive_widget_set_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(clean_name) = parser.eat_any_ident() {
            let set_name = format!("{}Set", clean_name);
            let snake_name = camel_case_to_snake_case(&clean_name);
            
            tb.add("#[derive(Clone, Debug)]");
            tb.add("pub struct ").ident(&set_name).add("(WidgetSet);");
            tb.add("impl std::ops::Deref for ").ident(&set_name).add("{");
            tb.add("    type Target = WidgetSet;");
            tb.add("    fn deref(&self)->&Self::Target{");
            tb.add("        &self.0");
            tb.add("    }");
            tb.add("}");
            tb.add("impl std::ops::DerefMut for ").ident(&set_name).add("{");
            tb.add("    fn deref_mut(&mut self)->&mut Self::Target{");
            tb.add("        &mut self.0");
            tb.add("    }");
            tb.add("}");

            let set_ext = format!("{}SetWidgetSetExt", clean_name);
            let ref_ext = format!("{}SetWidgetRefExt", clean_name);
            let widget_ext = format!("{}SetWidgetExt", clean_name);
            let get_fn = format!("{}_set", snake_name);
            let as_fn = format!("as_{}_set", snake_name);
            let ref_name = format!("{}Ref", clean_name);
            
            tb.add("pub trait").ident(&set_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&self, paths: &[&[LiveId]]) -> ").ident(&set_name).add(";");
            tb.add("    fn ").ident(&as_fn).add("(&self) -> ").ident(&set_name).add(";");
            tb.add("}");

            tb.add("impl ").ident(&set_name).add("{");
            tb.add("    pub fn has_widget(&self, widget:&WidgetRef)->").ident(&ref_name).add("{");
            tb.add("        if self.contains(widget){");
            tb.add("             ").add(&ref_name).add("(widget.clone())");
            tb.add("        } else {");
            tb.add("             ").add(&ref_name).add("(WidgetRef::default())");
            tb.add("        }");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl ").ident(&set_ext).add(" for WidgetSet{");
            tb.add("    fn ").ident(&get_fn).add("(&self, paths: &[&[LiveId]]) -> ").ident(&set_name).add("{");
            tb.add("        ").ident(&set_name).add("(self.widgets(paths))");
            tb.add("    }");
            tb.add("    fn ").ident(&as_fn).add("(&self) -> ").ident(&set_name).add("{");
            tb.add("        ").ident(&set_name).add("(self.clone())");
            tb.add("    }");
            tb.add("}");

            tb.add("pub trait").ident(&ref_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&self, paths: &[&[LiveId]]) -> ").ident(&set_name).add(";");
            tb.add("}");

            tb.add("impl ").ident(&ref_ext).add(" for WidgetRef{");
            tb.add("    fn ").ident(&get_fn).add("(&self, paths: &[&[LiveId]]) -> ").ident(&set_name).add("{");
            tb.add("        ").ident(&set_name).add("(self.widgets(paths))");
            tb.add("    }");
            tb.add("}");
            
            
            tb.add("pub trait").ident(&widget_ext).add("{");
            tb.add("    fn ").ident(&get_fn).add("(&mut self, paths: &[&[LiveId]]) -> ").ident(&set_name).add(";");
            tb.add("}");

            tb.add("impl<T> ").ident(&widget_ext).add(" for T where T: Widget{");
            tb.add("    fn ").ident(&get_fn).add("(&mut self, paths: &[&[LiveId]]) -> ").ident(&set_name).add("{");
            tb.add("        ").ident(&set_name).add("(self.widgets(paths))");
            tb.add("    }");
            tb.add("}");
            
            let iter_name = format!("{}SetIterator", clean_name);
            
            tb.add("impl").ident(&set_name).add("{");
            tb.add("    pub fn iter(&self)-> ").ident(&iter_name).add("{");
            tb.add("         ").ident(&iter_name).add("{");
            tb.add("            iter:self.0.iter()");
            tb.add("        }");
            tb.add("    }");
            tb.add("}");
            
            tb.add("pub struct ").ident(&iter_name).add("<'a>{");
            tb.add("    iter: WidgetSetIterator<'a>,");
            tb.add("}");
            
            tb.add("impl<'a> Iterator for ").ident(&iter_name).add("<'a> {");
            tb.add("    type Item = ").ident(&ref_name).add(";");
            tb.add("    fn next(&mut self)->Option<Self::Item>{");
            tb.add("        if let Some(next) = self.iter.next(){");
            tb.add("            return Some(").ident(&ref_name).add("(next.clone()))");
            tb.add("        }");
            tb.add("        None");
            tb.add("    }");
            tb.add("}");
            
            
            return tb.end();
        }
    }

    parser.unexpected()
}

