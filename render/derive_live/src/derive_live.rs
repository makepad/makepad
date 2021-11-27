use proc_macro::{TokenStream};

use crate::macro_lib::*;
use crate::id::*;

pub fn derive_live_animate_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            
            let fields = if let Some(_types) = types {
                return parser.unexpected();
            }
            else if let Some(fields) = parser.eat_all_struct_fields() {
                fields
            }
            else {
                return parser.unexpected();
            };
            
            let animator = fields.iter().find( | field | field.name == "animator");
            let state_default = fields.iter().find( | field | field.name == "state_default");
            if !animator.is_some() {
                // no can do
                return error("Cannot generate LiveAnimate without animator");
            }
            if !state_default.is_some() {
                // no can do
                return error("Cannot generate LiveAnimate without state_default");
            }
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveAnimate for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn animate_to(&mut self, cx: &mut Cx, state: LivePtr) {");
            tb.add("        if self.animator.state.is_none() {");
            tb.add("            self.animator.cut_to_live(cx, self.state_default.unwrap());");
            tb.add("         }");
            tb.add("        self.animator.animate_to_live(cx, state);");
            tb.add("    }");
            tb.add("    fn handle_animation(&mut self, cx: &mut Cx, event: &mut Event) {");
            tb.add("        if self.animator.do_animation(cx, event) {");
            tb.add("            let state = self.animator.swap_out_state();");
            tb.add("            self.apply(cx, ApplyFrom::Animate, 0, &state);");
            tb.add("            self.animator.swap_in_state(state);");
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
            tb.add("LiveComponentHooks for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    return parser.unexpected()
}


pub fn derive_live_cast_impl(input: TokenStream) -> TokenStream {
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
            tb.add("LiveCast for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("LiveCast for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_into_any_action_impl(input: TokenStream) -> TokenStream {
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
            tb.add("Into<OptionAnyAction> for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn into(self)->Option<Box<dyn AnyAction>>{");
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
            tb.add("Into<OptionAnyAction> for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn into(self)->Option<Box<dyn AnyAction>>{");
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


pub fn derive_live_apply_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            
            let fields = if let Some(_types) = types {
                return parser.unexpected();
            }
            else if let Some(fields) = parser.eat_all_struct_fields() {
                fields
            }
            else {
                return parser.unexpected();
            };
            
            let deref_target = fields.iter().find( | field | field.name == "deref_target");
            let draw_vars = fields.iter().find( | field | field.name == "draw_vars");
            let animator = fields.iter().find( | field | field.name == "animator");
            let live_ptr = fields.iter().find( | field | field.name == "live_ptr");
            
            if deref_target.is_some() && draw_vars.is_some() ||
            deref_target.is_some() && animator.is_some() ||
            animator.is_some() && draw_vars.is_some() {
                // no can do
                return error("Cannot generate LiveApply with more than one of: deref_target/draw_call_vars/animator");
            }
            
            if let Some(_) = draw_vars {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveApply for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize {");
                tb.add("        self.draw_vars.apply_value(cx, apply_from, index, nodes)");
                tb.add("    }");
                tb.add("    fn before_apply(&mut self, cx:&mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]){");
                tb.add("        self.draw_vars.before_apply(cx, apply_from, index, nodes, &self.geometry);");
                tb.add("    }");
                tb.add("    fn after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]) {");
                tb.add("        self.draw_vars.after_apply(cx, apply_from, index, nodes);");
                if live_ptr.is_some(){
                    tb.add("    if let Some(file_id) = apply_from.file_id() {self.live_ptr = Some(LivePtr::from_index(file_id, index));}");
                }
                tb.add("    }");
                tb.add("}");
            }
            else if let Some(_) = deref_target {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveApply for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize{");
                tb.add("        self.deref_target.apply_value_unknown(cx, apply_from, index, nodes)");
                tb.add("    }");
                tb.add("    fn before_apply(&mut self, cx:&mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]){");
                tb.add("        self.deref_target.before_apply(cx, apply_from, index, nodes);");
                tb.add("    }");
                tb.add("    fn after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]) {");
                tb.add("        self.deref_target.after_apply(cx, apply_from, index, nodes);");
                if live_ptr.is_some(){
                    tb.add("    if let Some(file_id) = apply_from.file_id() {self.live_ptr = Some(LivePtr::from_index(file_id, index));}");
                }
                tb.add("    }");
                tb.add("}");
            }
            else if let Some(_) = animator {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveApply for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, nodes: &[LiveNode]) {");
                tb.add("        if let Some(file_id) = apply_from.file_id() {");
                if live_ptr.is_some(){
                    tb.add("        self.live_ptr = Some(LivePtr::from_index(file_id, index));");
                }
                tb.add("            if let Ok(index) = nodes.child_by_name(index, id!(state_default)) {");
                tb.add("                self.apply(cx, ApplyFrom::Animate, index, nodes);");
                tb.add("            }");
                tb.add("        }");
                tb.add("    }");
                tb.add("}");
            }
            else {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveApply for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                if live_ptr.is_some(){
                    tb.add("fn after_apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index: usize, _nodes: &[LiveNode]) {");
                    tb.add("    if let Some(file_id) = apply_from.file_id() {self.live_ptr = Some(LivePtr::from_index(file_id, index));}");
                    tb.add("}");
                }
                tb.add("}");
                
            }
            
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("LiveApply for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_live_component_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    let main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            
            let fields = if let Some(_types) = types {
                return parser.unexpected();
            }
            else if let Some(fields) = parser.eat_all_struct_fields() {
                fields
            }
            else {
                return parser.unexpected();
            };
            
            let deref_target = fields.iter().find( | field | field.name == "deref_target");
            
            let draw_vars = fields.iter().find( | field | field.name == "draw_vars");
            if draw_vars.is_some() { // we have draw vars, make sure we are repr(C)6
                if main_attribs.iter().find( | attr | attr.name == "repr" && attr.args.as_ref().unwrap().to_string().to_lowercase() == "c").is_none() {
                    return error("Any struct with draw_vars needs to be repr(c)")
                }
            }
            
            // alright now. we have a field
            for field in &fields {
                if field.attrs.len() != 1 || field.attrs[0].name != "live" && field.attrs[0].name != "calc" && field.attrs[0].name != "rust" {
                    return error(&format!("Field {} does not have a live, calc or hide attribute", field.name));
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
            tb.add("LiveComponentValue for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            
            tb.add("    fn apply_value(&mut self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize{");
            tb.add("        match nodes[index].id {");
            for field in &fields {
                if field.attrs[0].name == "live" {
                    tb.add("    Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>self.").ident(&field.name).add(".apply(cx, apply_from, index, nodes),");
                }
            }
            if deref_target.is_some() {
                tb.add("        _=> self.deref_target.apply_value(cx, apply_from, index, nodes)");
            }
            else {
                tb.add("        _=> self.apply_value_unknown(cx, apply_from, index, nodes)");
            }
            tb.add("        }");
            tb.add("    }");
            
            tb.add("}");
            
            
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponent for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            
            tb.add("    fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>() }");
            
            tb.add("    fn apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, start_index: usize, nodes: &[LiveNode])->usize {");
            //tb.add("    cx.profile_start(start_index as u64);");

            tb.add("        self.before_apply(cx, apply_from, start_index, nodes);");
            
            tb.add("        let struct_id = Id(").suf_u64(Id::from_str(&struct_name).unwrap().0).add(");");
            tb.add("        if !nodes[start_index].value.is_structy_type(){");
            tb.add("            cx.apply_error_wrong_type_for_struct(apply_from, start_index, nodes, struct_id);");
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
            tb.add("        self.after_apply(cx, apply_from, start_index, nodes);");
            //tb.add("    cx.profile_end(start_index as u64);");
            tb.add("        return index;");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveNew for").ident(&struct_name).stream(generic).stream(where_clause).add("{");
            
            tb.add("    fn live_type_info() -> LiveTypeInfo {");
            tb.add("        let mut fields = Vec::new();");
            
            for field in &fields {
                let attr = &field.attrs[0];
                if attr.name == "live" || attr.name == "calc" {
                    tb.add("fields.push(LiveTypeField{id:Id::from_str(").string(&field.name).add(").unwrap(),");
                    // ok so what do we do if we have an Option<..>
                    // how about LiveOrCalc becomes LiveFieldType::Option
                    match TokenParser::unwrap_option(field.ty.clone()) {
                        Ok(inside) => {
                            if attr.name != "live" {
                                return error("For option type only use of live is supported")
                            }
                            tb.add("live_type_info:").stream(Some(inside)).add("::live_type_info(),");
                            tb.add("live_field_kind: LiveFieldKind::LiveOption");
                        }
                        Err(not_option) => {
                            tb.add("live_type_info:").stream(Some(not_option)).add("::live_type_info(),");
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
            tb.add("            module_path: ModulePath::from_str2(&module_path!()).unwrap(),");
            tb.add("            live_type: Self::live_type(),");
            tb.add("            fields,");
            // we have to decide Class or Object
            let live_type_kind = if main_attribs.len()>0{
                if main_attribs[0].name == "live_type_kind"{
                    main_attribs[0].args.clone()
                }
                else {None}
            }else{None};
            
            if let Some(live_type_kind) = live_type_kind{
                tb.add("            kind: LiveTypeKind::").stream(Some(live_type_kind)).add(",");
            }
            else{
                tb.add("            kind: LiveTypeKind::Class,");
            }
            
            tb.add("            type_name: Id::from_str(").string(&struct_name).add(").unwrap()");
            tb.add("        }");
            tb.add("    }");
            
            
            tb.add("    fn live_register(cx: &mut Cx) {");
            tb.add("        struct Factory();");
            tb.add("        impl LiveFactory for Factory {");
            
            tb.add("            fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveComponent> {");
            tb.add("                Box::new(").ident(&struct_name).add(" ::new(cx))");
            tb.add("            }");
            
            
            tb.add("        }");
            tb.add("        cx.register_factory(").ident(&struct_name).add("::live_type(), Box::new(Factory()));");
            // lets register all our components
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
            tb.add("        Self {");
            for field in &fields {
                let attr = &field.attrs[0];
                tb.ident(&field.name).add(":");
                if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty() {
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
            
            if !parser.open_brace() {
                return parser.unexpected()
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
                    if attributes.len() != 1 {
                        return error(&format!("Field {} does not have a live or pick attribute", name));
                    }
                    if attributes[0].name == "pick" {
                        if pick.is_some() {
                            return error(&format!("Enum can only have a single field marked pick"));
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
                        return parser.unexpected();
                    }
                }
                //eprintln!("HERE2");
                parser.eat_punct_alone(',');
            }
            
            if pick.is_none() {
                return error(&format!("Enum needs atleast one field marked pick"));
            }
            
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveNew for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            
            tb.add("    fn new(cx:&mut Cx) -> Self {");
            items[pick.unwrap()].gen_new(&mut tb);
            tb.add("    }");
            
            tb.add("    fn live_type_info() -> LiveTypeInfo {");
            tb.add("        LiveTypeInfo{");
            tb.add("            module_path: ModulePath::from_str(&module_path!()).unwrap(),");
            tb.add("            live_type: Self::live_type(),");
            tb.add("            fields: Vec::new(),");
            tb.add("            type_name: Id::from_str(").string(&enum_name).add(").unwrap(),");
            tb.add("            kind: LiveTypeKind::Enum,");
            tb.add("        }");
            tb.add("    }");
            
            tb.add("    fn live_register(cx: &mut Cx) {");
            
            let is_u32_enum = main_attribs.iter().find( | attr | attr.name == "repr" && attr.args.as_ref().unwrap().to_string().to_lowercase() == "u32").is_some();
            if is_u32_enum {
                tb.add("        let mut variants = Vec::new();");
                for item in &items {
                    match item.kind {
                        EnumKind::Bare => {
                            tb.add("variants.push(Id::from_str(").string(&item.name).add(").unwrap());");
                        },
                        EnumKind::Named(_) |
                        EnumKind::Tuple(_) => {
                            return error("For repr(u32) shader-accessible enums only bare values are supported");
                        }
                    }
                }
                tb.add("        cx.shader_registry.register_enum(").ident(&enum_name).add("::live_type(),ShaderEnum{enum_name:Id::from_str(").string(&enum_name).add(").unwrap(),variants});");
            }
            
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponent for").ident(&enum_name).stream(generic).stream(where_clause).add("{");
            tb.add("    fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>() }");
            
            tb.add("    fn apply(&mut self, cx: &mut Cx, apply_from:ApplyFrom, start_index:usize, nodes: &[LiveNode]) -> usize {");
            tb.add("        self.before_apply(cx, apply_from, start_index, nodes);");
            tb.add("        let mut index = start_index;");
            tb.add("        let enum_id = Id(").suf_u64(Id::from_str(&enum_name).unwrap().0).add(");");
            tb.add("        match &nodes[start_index].value{");
            tb.add("            LiveValue::BareEnum{base,variant}=>{");
            tb.add("                if *base != enum_id{");
            tb.add("                    cx.apply_error_wrong_enum_base(apply_from, index, nodes, enum_id, *base);");
            tb.add("                    index = nodes.skip_node(index);");
            tb.add("                    self.after_apply(cx, apply_from, start_index, nodes);");
            tb.add("                    return index;");
            tb.add("                }");
            tb.add("                match variant{");
            for item in &items {
                if let EnumKind::Bare = item.kind {
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{index += 1;*self = Self::").ident(&item.name).add("},");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        cx.apply_error_wrong_enum_variant(apply_from, index, nodes, enum_id, *variant);");
            tb.add("                        index = nodes.skip_node(index);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            },");
            
            tb.add("            LiveValue::NamedEnum{base, variant}=>{");
            tb.add("                if *base != enum_id{");
            tb.add("                    cx.apply_error_wrong_enum_base(apply_from, index, nodes, enum_id, *base);");
            tb.add("                    index = nodes.skip_node(index);");
            tb.add("                    self.after_apply(cx, apply_from, start_index, nodes);");
            tb.add("                    return index;");
            tb.add("                }");
            tb.add("                match variant{");
            for item in &items {
                if let EnumKind::Named(fields) = &item.kind {
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
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
                        tb.add("                        Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>{index = (*");
                        tb.ident(&format!("prefix_{}", field.name)).add(").apply(cx, apply_from, index, nodes);},");
                    }
                    tb.add("                            _=>{");
                    tb.add("                                cx.apply_error_named_enum_invalid_prop(apply_from, index, nodes, enum_id, *variant, nodes[index].id);");
                    tb.add("                                index = nodes.skip_node(index);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        cx.apply_error_wrong_enum_variant(apply_from, index, nodes, enum_id, *variant);");
            tb.add("                        index = nodes.skip_node(index);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            tb.add("            LiveValue::TupleEnum{base, variant}=>{");
            tb.add("                if *base != enum_id{");
            tb.add("                    cx.apply_error_wrong_enum_base(apply_from, index, nodes, enum_id, *base);");
            tb.add("                    index = nodes.skip_node(index);");
            tb.add("                    self.after_apply(cx, apply_from, start_index, nodes);");
            tb.add("                    return index;");
            tb.add("                }");
            tb.add("                match variant{");
            
            for item in &items {
                if let EnumKind::Tuple(args) = &item.kind {
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
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
                    tb.add("                                cx.apply_error_tuple_enum_arg_not_found(apply_from, index, nodes, enum_id, *variant, arg);");
                    tb.add("                                index = nodes.skip_node(index);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        cx.apply_error_wrong_enum_variant(apply_from, index, nodes, enum_id, *variant);");
            tb.add("                        index = nodes.skip_node(index);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            tb.add("            _=>{");
            tb.add("               cx.apply_error_expected_enum(apply_from, index, nodes);");
            tb.add("               index = nodes.skip_node(index);");
            tb.add("            }");
            tb.add("        }");
            tb.add("        self.after_apply(cx, apply_from, start_index, nodes);");
            tb.add("        index");
            tb.add("    }");
            
            tb.add("}");
            
            //tb.eprint();
            return tb.end();
        }
    }
    return parser.unexpected()
}