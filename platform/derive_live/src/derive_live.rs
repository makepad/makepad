use proc_macro::{TokenStream};

use crate::macro_lib::*;
use crate::live_id::*;


pub fn derive_live_hook_impl(input: TokenStream) -> TokenStream {
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
            tb.add("LiveHook for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("LiveHook for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_from_live_id_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            tb.add("impl");
            tb.add("From<LiveId> for").ident(&struct_name).add("{");
            tb.add("    fn from(live_id:LiveId)->").ident(&struct_name).add("{").ident(&struct_name).add("(live_id)}");
            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}


fn parse_live_type(parser: &mut TokenParser, tb: &mut TokenBuilder) -> Result<(), TokenStream> {
    
    let main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        let struct_name = parser.expect_any_ident() ?;
        let generic = parser.eat_generic();
        let types = parser.eat_all_types();
        let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
        
        let mut fields = if let Some(_types) = types {
            return error_result("Unexpected type form")
        }
        else if let Some(fields) = parser.eat_all_struct_fields() {
            fields
        }
        else {
            return error_result("Unexpected field form")
        };
        
        // alright now. we have a field
        for field in &mut fields {
            if field.attrs.len() == 1 && &field.attrs[0].name != "live" && field.attrs[0].name != "calc" && field.attrs[0].name != "rust" && field.attrs[0].name != "state" {
                return error_result(&format!("Field {} does not have a live, calc or rust attribute", field.name));
            }
            if field.attrs.len() == 0 { // insert a default
                field.attrs.push(Attribute {name: "live".to_string(), args: None});
            }
        }
        
        // special marker fields
        let deref_target = fields.iter().find( | field | field.name == "deref_target");
        let draw_vars = fields.iter().find( | field | field.name == "draw_vars");
        let geometry = fields.iter().find( | field | field.name == "geometry");
        let animator = fields.iter().find( | field | field.name == "animator");
        // ok we have to parse the animator args fields
        
        if deref_target.is_some() && draw_vars.is_some() {
            return error_result("Cannot dereive Live with more than one of: both draw_vars and deref_target");
        }
        
        if draw_vars.is_some() && !geometry.is_some() {
            return error_result("drawvars requires a geometry object to be present");
        }
        
        let animator_kv = if let Some(animator) = animator {
            let kv = if let Some(attr) = animator.attrs.iter().find( | attr | attr.name == "state") {
                if let Some(args) = &attr.args {
                    let mut parser = TokenParser::new(args.clone());
                    // ok its key:value comma
                    let mut kv = Vec::new();
                    while !parser.eat_eot() {
                        let def = parser.expect_any_ident() ?;
                        parser.eat_punct_alone(',');
                        kv.push(def);
                    }
                    kv
                }
                else {
                    return error_result("state attribute needs arguments");
                }
            }
            else {
                return error_result("Animator needs a state(state_default) attribute");
            };
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveAnimate for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn init_animator(&mut self, cx: &mut Cx) {");
            for def in &kv {
                tb.add("    self.animator.cut_to_live(cx,self.").ident(def).add(");");
            }
            tb.add("    }");
            
            tb.add("    fn animate_to(&mut self, cx: &mut Cx, state: Option<LivePtr>) {");
            tb.add("        if self.animator.state.is_none() {");
            tb.add("            self.init_animator(cx);");
            tb.add("         }");
            tb.add("         self.animator.animate_to_live(cx, state);");
            tb.add("    }");
            
            tb.add("    fn apply_animator(&mut self, cx: &mut Cx) {");
            tb.add("        let state = self.animator.swap_out_state();");
            tb.add("        self.apply(cx, ApplyFrom::Animate, state.child_by_name(0,id!(state)).unwrap(), &state);");
            tb.add("        self.animator.swap_in_state(state);");
            tb.add("    }");
            
            
            tb.add("    fn animate_cut(&mut self, cx: &mut Cx, state: Option<LivePtr>) {");
            tb.add("        if self.animator.state.is_none() {");
            tb.add("            self.init_animator(cx);");
            tb.add("         }");
            tb.add("         self.animator.cut_to_live(cx, state);");
            tb.add("         self.apply_animator(cx);");
            tb.add("    }");
            
            
            tb.add("    fn animator_is_in_state(&mut self, cx: &mut Cx, state: Option<LivePtr>)->bool{");
            tb.add("        if state.is_none() { return false }");
            tb.add("        if self.animator.state.is_none() {");
            for def in &kv {
                tb.add("         if state == self.").ident(def).add("{ return true }");
            }
            tb.add("             return false");
            tb.add("         }");
            tb.add("         else{");
            tb.add("             return self.animator.is_in_state(cx, state)");
            tb.add("         }");
            tb.add("    }");
            
            
            tb.add("    fn animator_handle_event(&mut self, cx: &mut Cx, event: &mut Event)->AnimatorAction{");
            tb.add("        let ret = self.animator.handle_event(cx, event);");
            tb.add("        if ret.is_animating(){self.apply_animator(cx);}");
            tb.add("        ret");
            tb.add("    }");
            tb.add("}");
            Some(kv)
        }
        else {
            None
        };
        
        if draw_vars.is_some() { // we have draw vars, make sure we are repr(C)6
            if main_attribs.iter().find( | attr | attr.name == "repr" && attr.args.as_ref().unwrap().to_string().to_lowercase() == "c").is_none() {
                return error_result("Any struct with draw_vars needs to be repr(c)")
            }
        }
        
        if let Some(deref_target) = deref_target {
            tb.add("impl").stream(generic.clone());
            tb.add("std::ops::Deref for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    type Target = ").stream(Some(deref_target.ty.clone())).add(";");
            tb.add("    fn deref(&self) -> &Self::Target {&self.deref_target}");
            tb.add("}");
            tb.add("impl").stream(generic.clone());
            
            tb.add("std::ops::DerefMut for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.deref_target}");
            tb.add("}");
        }
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveApplyValue for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn apply_value(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize{");
        tb.add("        match nodes[index].id {");
        for field in &fields {
            if field.attrs[0].name == "live" {
                tb.add("    LiveId(").suf_u64(LiveId::from_str(&field.name).unwrap().0).add(")=>self.").ident(&field.name).add(".apply(cx, apply_from, index, nodes),");
            }
        }
        // Unknown value handling
        if deref_target.is_some() {
            tb.add("        _=> self.deref_target.apply_value(cx, apply_from, index, nodes)");
        }
        else {
            if draw_vars.is_some() {
                tb.add("    _=> self.draw_vars.apply_value(cx, apply_from, index, nodes)");
            }
            else {
                tb.add("    _=> self.apply_value_unknown(cx, apply_from, index, nodes)");
            }
        }
        tb.add("        }");
        tb.add("    }");
        
        tb.add("}");
        
        // forward a potential deref_target
        if draw_vars.is_some() || deref_target.is_some() {
            tb.add("impl").stream(generic.clone()).ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    pub fn deref_target_before_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]){");
            tb.add("        self.before_apply(cx, apply_from, index, nodes);");
            if draw_vars.is_some() {
                tb.add("    self.draw_vars.before_apply(cx, apply_from, index, nodes, &self.geometry);");
            }
            else if deref_target.is_some() {
                tb.add("    self.deref_target.deref_target_before_apply(cx, apply_from, index, nodes);");
            }
            tb.add("    }");
            tb.add("    pub fn deref_target_after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]){");
            if draw_vars.is_some() {
                tb.add("    self.draw_vars.after_apply(cx, apply_from, index, nodes, &self.geometry);");
            }
            else if deref_target.is_some() {
                tb.add("    self.deref_target.deref_target_after_apply(cx, apply_from, index, nodes);");
            }
            tb.add("        self.after_apply(cx, apply_from, index, nodes);");
            tb.add("    }");
            tb.add("}");
        }
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveApply for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, start_index: usize, nodes: &[LiveNode])->usize {");
        
        tb.add("        self.before_apply(cx, apply_from, start_index, nodes);");
        if draw_vars.is_some() {
            tb.add("        self.draw_vars.before_apply(cx, apply_from, start_index, nodes, &self.geometry);");
        }
        else if deref_target.is_some() {
            tb.add("        self.deref_target.deref_target_before_apply(cx, apply_from, start_index, nodes);");
        }
        
        tb.add("        let struct_id = LiveId(").suf_u64(LiveId::from_str(&struct_name).unwrap().0).add(");");
        tb.add("        if !nodes[start_index].value.is_structy_type(){");
        tb.add("            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, struct_id);");
        tb.add("            self.after_apply(cx, apply_from, start_index, nodes);");
        tb.add("            return nodes.skip_node(start_index);");
        tb.add("        }");
        
        tb.add("        let mut index = start_index + 1;"); // skip the class
        tb.add("        loop{");
        tb.add("            if nodes[index].value.is_close(){");
        tb.add("                index += 1;");
        tb.add("                break;");
        tb.add("            }");
        tb.add("            index = self.apply_value(cx, apply_from, index, nodes);");
        tb.add("        }");
        
        if let Some(_) = draw_vars {
            tb.add("    self.draw_vars.after_apply(cx, apply_from, start_index, nodes, &self.geometry);");
        }
        else if let Some(_) = deref_target {
            tb.add("    self.deref_target.deref_target_after_apply(cx, apply_from, start_index, nodes);");
        }
        
        if let Some(_) = animator { // apply the default states
            tb.add("    if let Some(file_id) = apply_from.file_id() {");
            for def in &animator_kv.unwrap() {
                tb.add("    if let Some(index) = nodes.child_by_path(start_index, &[");
                tb.add("        self.animator.get_state_id_of(cx, self.").ident(def).add(",LiveId(").suf_u64(LiveId::from_str(def).unwrap().0).add(")),");
                tb.add("        id!(apply)");
                tb.add("        ]) {");
                tb.add("            self.apply(cx, ApplyFrom::Animate, index, nodes);");
                tb.add("    }");
            }
            tb.add("    }");
        }
        
        tb.add("        self.after_apply(cx, apply_from, start_index, nodes);");
        
        tb.add("        return index;");
        tb.add("    }");
        tb.add("}");
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveNew for").ident(&struct_name).stream(generic).stream(where_clause).add("{");
        
        tb.add("    fn live_type_info(cx:&mut Cx) -> LiveTypeInfo {");
        tb.add("        let mut fields = Vec::new();");
        
        for field in &fields {
            let attr = &field.attrs[0];
            if attr.name == "live" || attr.name == "calc" {
                tb.add("fields.push(LiveTypeField{id:LiveId::from_str(").string(&field.name).add(").unwrap(),");
                // ok so what do we do if we have an Option<..>
                // how about LiveOrCalc becomes LiveFieldType::Option
                match TokenParser::unwrap_option(field.ty.clone()) {
                    Ok(inside) => {
                        if attr.name != "live" {
                            return error_result("For option type only use of live is supported")
                        }
                        tb.add("live_type_info:").stream(Some(inside)).add("::live_type_info(cx),");
                        tb.add("live_field_kind: LiveFieldKind::LiveOption");
                    }
                    Err(not_option) => {
                        tb.add("live_type_info:").stream(Some(not_option)).add("::live_type_info(cx),");
                        if attr.name == "live" {
                            tb.add("live_field_kind: LiveFieldKind::Live");
                        }
                        else {
                            tb.add("live_field_kind: LiveFieldKind::Calc");
                        }
                    }
                }
                tb.add("});");
            }
        }
        tb.add("        LiveTypeInfo{");
        tb.add("            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),");
        tb.add("            live_type: LiveType::of::<Self>(),");
        tb.add("            fields,");
        
        tb.add("            type_name: LiveId::from_str(").string(&struct_name).add(").unwrap()");
        tb.add("        }");
        tb.add("    }");
        
        tb.add("    fn live_register(cx: &mut Cx) {");

        for attr in main_attribs.iter().filter(|attr| attr.name == "live_register"){
            if attr.args.is_none(){
                return error_result("live_register needs an argument")
            }
            tb.add("(").stream(attr.args.clone()).add(")(cx);");
        }
        
        // we need this here for shader enums to register without hassle
        for field in &fields {
            let attr = &field.attrs[0];
            if attr.name == "live" || attr.name == "calc" {
                match TokenParser::unwrap_option(field.ty.clone()) {
                    Ok(inside) => {
                        tb.stream(Some(inside)).add("::live_register(cx);");
                    }
                    Err(not_option) => {
                        tb.stream(Some(not_option)).add("::live_register(cx);");
                    }
                }
            }
        }
        
        tb.add("    }");
        
        tb.add("    fn new(cx: &mut Cx) -> Self {");
        tb.add("        let mut ret = Self {");
        for field in &fields {
            let attr = &field.attrs[0];
            tb.ident(&field.name).add(":");
            if field.name == "animator"{
                tb.add("Default::default()");
            }
            else if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty() {
                if attr.name == "live" {
                    tb.add("LiveNew::new(cx)");
                }
                else {
                    tb.add("Default::default()");
                }
            }
            else {
                tb.stream(attr.args.clone());
            }
            tb.add(",");
        }
        tb.add("        };");
        tb.add("        ret.after_new(cx);");
        tb.add("        ret");
        tb.add("    }");
        tb.add("}");
        
        Ok(())
    }
    else if parser.eat_ident("enum") {
        let enum_name = parser.expect_any_ident() ?;
        let generic = parser.eat_generic();
        let where_clause = parser.eat_where_clause(None);
        
        if !parser.open_brace() {
            return error_result("cant find open brace for enum")
        }
        
        struct EnumItem {
            name: String,
            attributes: Vec<Attribute>,
            kind: EnumKind
        }
        
        enum EnumKind {
            Bare,
            Named(Vec<StructField>),
            Tuple(Vec<TokenStream>)
        }
        let mut items = Vec::new();
        
        impl EnumItem {
            
            fn gen_new(&self, tb: &mut TokenBuilder) {
                tb.add("Self::").ident(&self.name);
                match &self.kind {
                    EnumKind::Bare => (),
                    EnumKind::Named(_) => {tb.add("{").stream(self.attributes[0].args.clone()).add("}");},
                    EnumKind::Tuple(_) => {tb.add("(").stream(self.attributes[0].args.clone()).add(")");}
                }
            }
        }
        
        let mut pick = None;
        while !parser.eat_eot() {
            let attributes = parser.eat_attributes();
            // check if we have a default attribute
            if let Some(name) = parser.eat_any_ident() {
                if attributes.len() > 0 && attributes[0].name == "pick" {
                    if pick.is_some() {
                        return error_result(&format!("Enum can only have a single field marked pick"));
                    }
                    pick = Some(items.len())
                }
                if let Some(types) = parser.eat_all_types() {
                    items.push(EnumItem {name, attributes, kind: EnumKind::Tuple(types)})
                }
                else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                    items.push(EnumItem {name, attributes, kind: EnumKind::Named(fields)})
                }
                else if parser.is_punct_alone(',') || parser.is_eot() { // bare variant
                    items.push(EnumItem {name, attributes, kind: EnumKind::Bare})
                }
                else {
                    return error_result("unexpected whilst parsing enum")
                }
            }
            //eprintln!("HERE2");
            parser.eat_punct_alone(',');
        }
        
        if pick.is_none() {
            return error_result(&format!("Enum needs atleast one field marked pick"));
        }
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveNew for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn new(cx:&mut Cx) -> Self {");
        tb.add("        let mut ret = ");
        items[pick.unwrap()].gen_new(tb);
        tb.add("        ;ret.after_new(cx);ret");
        tb.add("    }");
        
        tb.add("    fn live_type_info(cx:&mut Cx) -> LiveTypeInfo {");
        tb.add("        LiveTypeInfo{");
        tb.add("            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),");
        tb.add("            live_type: LiveType::of::<Self>(),");
        tb.add("            fields: Vec::new(),");
        tb.add("            type_name: LiveId::from_str(").string(&enum_name).add(").unwrap(),");
        /*tb.add("            kind: LiveTypeKind::Enum,");*/
        tb.add("        }");
        tb.add("    }");
        
        tb.add("    fn live_register(cx: &mut Cx) {");

        
        let is_u32_enum = main_attribs.iter().find( | attr | attr.name == "repr" && attr.args.as_ref().unwrap().to_string().to_lowercase() == "u32").is_some();
        if is_u32_enum {
            tb.add("        let mut variants = Vec::new();");
            for item in &items {
                match item.kind {
                    EnumKind::Bare => {
                        tb.add("variants.push(LiveId::from_str(").string(&item.name).add(").unwrap());");
                    },
                    EnumKind::Named(_) |
                    EnumKind::Tuple(_) => {
                        return error_result("For repr(u32) shader-accessible enums only bare values are supported");
                    }
                }
            }
            tb.add("        cx.shader_registry.register_enum(LiveType::of::<").ident(&enum_name).add(">(),ShaderEnum{enum_name:LiveId::from_str(").string(&enum_name).add(").unwrap(),variants});");
        }
        
        tb.add("    }");
        tb.add("}");
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveApply for").ident(&enum_name).stream(generic).stream(where_clause).add("{");
        //tb.add("    fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>() }");
        
        tb.add("    fn apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, start_index:usize, nodes: &[LiveNode]) -> usize {");
        tb.add("        self.before_apply(cx, apply_from, start_index, nodes);");
        tb.add("        let mut index = start_index;");
        tb.add("        let enum_id = LiveId(").suf_u64(LiveId::from_str(&enum_name).unwrap().0).add(");");
        tb.add("        match &nodes[start_index].value{");
        tb.add("            LiveValue::BareEnum{base,variant}=>{");
        tb.add("                if *base != enum_id{");
        tb.add("                    cx.apply_error_wrong_enum_base(live_error_origin!(), index, nodes, enum_id, *base);");
        tb.add("                    index = nodes.skip_node(index);");
        tb.add("                    self.after_apply(cx, apply_from, start_index, nodes);");
        tb.add("                    return index;");
        tb.add("                }");
        tb.add("                match variant{");
        for item in &items {
            if let EnumKind::Bare = item.kind {
                tb.add("            LiveId(").suf_u64(LiveId::from_str(&item.name).unwrap().0).add(")=>{index += 1;*self = Self::").ident(&item.name).add("},");
            }
        }
        tb.add("                    _=>{");
        tb.add("                        cx.apply_error_wrong_enum_variant(live_error_origin!(), index, nodes, enum_id, *variant);");
        tb.add("                        index = nodes.skip_node(index);");
        tb.add("                    }");
        tb.add("                }");
        tb.add("            },");
        
        tb.add("            LiveValue::NamedEnum{base, variant}=>{");
        tb.add("                if *base != enum_id{");
        tb.add("                    cx.apply_error_wrong_enum_base(live_error_origin!(), index, nodes, enum_id, *base);");
        tb.add("                    index = nodes.skip_node(index);");
        tb.add("                    self.after_apply(cx, apply_from, start_index, nodes);");
        tb.add("                    return index;");
        tb.add("                }");
        tb.add("                match variant{");
        for item in &items {
            if let EnumKind::Named(fields) = &item.kind {
                tb.add("            LiveId(").suf_u64(LiveId::from_str(&item.name).unwrap().0).add(")=>{");
                tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                tb.add("                else{*self = ");
                item.gen_new(tb);
                tb.add("                }");
                tb.add("                if let Self::").ident(&item.name).add("{");
                for field in fields {
                    tb.ident(&field.name).add(":").ident(&format!("prefix_{}", field.name)).add(",");
                }
                tb.add("                } = self {");
                tb.add("                    index += 1;"); // skip the class
                tb.add("                    loop{");
                tb.add("                        if nodes[index].value.is_close(){");
                tb.add("                            index += 1;");
                tb.add("                            break;");
                tb.add("                        }");
                tb.add("                        match nodes[index].id{");
                for field in fields {
                    tb.add("                        LiveId(").suf_u64(LiveId::from_str(&field.name).unwrap().0).add(")=>{index = (*");
                    tb.ident(&format!("prefix_{}", field.name)).add(").apply(cx, apply_from, index, nodes);},");
                }
                tb.add("                            _=>{");
                tb.add("                                cx.apply_error_named_enum_invalid_prop(live_error_origin!(), index, nodes, enum_id, *variant, nodes[index].id);");
                tb.add("                                index = nodes.skip_node(index);");
                tb.add("                            }");
                tb.add("                        }");
                tb.add("                    }");
                tb.add("                }");
                tb.add("            }");
            }
        }
        tb.add("                    _=>{");
        tb.add("                        cx.apply_error_wrong_enum_variant(live_error_origin!(), index, nodes, enum_id, *variant);");
        tb.add("                        index = nodes.skip_node(index);");
        tb.add("                    }");
        tb.add("                }");
        tb.add("            }");
        tb.add("            LiveValue::TupleEnum{base, variant}=>{");
        tb.add("                if *base != enum_id{");
        tb.add("                    cx.apply_error_wrong_enum_base(live_error_origin!(), index, nodes, enum_id, *base);");
        tb.add("                    index = nodes.skip_node(index);");
        tb.add("                    self.after_apply(cx, apply_from, start_index, nodes);");
        tb.add("                    return index;");
        tb.add("                }");
        tb.add("                match variant{");
        
        for item in &items {
            if let EnumKind::Tuple(args) = &item.kind {
                tb.add("            LiveId(").suf_u64(LiveId::from_str(&item.name).unwrap().0).add(")=>{");
                
                tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                tb.add("                else{*self = ");
                item.gen_new(tb);
                tb.add("                }");
                
                tb.add("                if let Self::").ident(&item.name).add("(");
                for i in 0..args.len() {
                    tb.ident(&format!("var{}", i)).add(",");
                }
                tb.add("                ) = self{");
                tb.add("                    index += 1;"); // skip the class
                tb.add("                    loop{");
                tb.add("                        if nodes[index].value.is_close(){");
                tb.add("                            index += 1;");
                tb.add("                            break;");
                tb.add("                        }");
                tb.add("                        let arg = index - start_index - 1;");
                tb.add("                        match arg{");
                for i in 0..args.len() {
                    tb.add("                        ").unsuf_usize(i).add("=>{index = (*").ident(&format!("var{}", i)).add(").apply(cx, apply_from, index, nodes); },");
                }
                tb.add("                            _=>{");
                tb.add("                                cx.apply_error_tuple_enum_arg_not_found(live_error_origin!(), index, nodes, enum_id, *variant, arg);");
                tb.add("                                index = nodes.skip_node(index);");
                tb.add("                            }");
                tb.add("                        }");
                tb.add("                    }");
                tb.add("                }");
                tb.add("            }");
            }
        }
        tb.add("                    _=>{");
        tb.add("                        cx.apply_error_wrong_enum_variant(live_error_origin!(), index, nodes, enum_id, *variant);");
        tb.add("                        index = nodes.skip_node(index);");
        tb.add("                    }");
        tb.add("                }");
        tb.add("            }");
        tb.add("            _=>{");
        tb.add("               cx.apply_error_expected_enum(live_error_origin!(), index, nodes);");
        tb.add("               index = nodes.skip_node(index);");
        tb.add("            }");
        tb.add("        }");
        tb.add("        self.after_apply(cx, apply_from, start_index, nodes);");
        tb.add("        index");
        tb.add("    }");
        
        tb.add("}");
        
        //tb.eprint();
        Ok(())
    }
    else {
        error_result("Not enum or struct")
    }
    
}

pub fn derive_live_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    if let Err(err) = parse_live_type(&mut parser, &mut tb) {
        return err
    }
    else {
        tb.end()
    }
    
}