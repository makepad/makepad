use proc_macro::{TokenStream};

use makepad_micro_proc_macro::{
    Id,
    TokenBuilder,
    TokenParser,
    error_result,
};

pub fn derive_script_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    if let Err(err) = derive_script_impl_inner(&mut parser, &mut tb) {
        err
    }
    else {
        tb.end()
    }
}

fn derive_script_impl_inner(parser: &mut TokenParser, tb: &mut TokenBuilder) -> Result<(), TokenStream> {
    
    let main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        let struct_name = parser.expect_any_ident() ?;
        let generic = parser.eat_generic();
        let types = parser.eat_all_types();
        let where_clause = parser.eat_where_clause(None); 
        
        let mut fields = if let Some(_types) = types {
            return error_result("Unexpected type form")
        }
        else if let Some(fields) = parser.eat_all_struct_fields() {
            fields
        }
        else {
            return error_result("Unexpected field form")
        };
        
        for field in &mut fields {
            if field.attrs.is_empty() { // need field def
                return error_result("Please annotate the field type with #[rust] for rust-only fields, and #[script] for scriptable mapped fields and #[deref] for a base class");
            }
        }
        
        let deref_field = fields.iter().find( | field | field.attrs.iter().any( | a | a.name == "deref"));
        
        if let Some(deref_field) = deref_field {
            tb.add("impl").stream(generic.clone());
            tb.add("std::ops::Deref for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    type Target = ").stream(Some(deref_field.ty.clone())).add(";");
            tb.add("    fn deref(&self) -> &Self::Target {&self.").ident(&deref_field.name).add("}");
            tb.add("}");
            tb.add("impl").stream(generic.clone());
            
            tb.add("std::ops::DerefMut for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.").ident(&deref_field.name).add("}");
            tb.add("}");
        }
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptHookDeref for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        tb.add("    fn on_deref_before_apply(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value){");
        tb.add("        <Self as ScriptHook>::on_before_apply(self, vm, apply, value);");
        
        if let Some(deref_field) = deref_field {
            tb.add("    <Self as ScriptHookDeref>::on_deref_before_apply(&mut self.").ident(&deref_field.name).add(",vm, apply, value);");
        }
        tb.add("    }");
        
        tb.add("    fn on_deref_after_apply(&mut self,vm: &mut Vm, apply:&mut ApplyScope, value:Value){");
        tb.add("        <Self as ScriptHook>::on_after_apply(self, vm, apply, value);");
        
        if let Some(deref_field) = deref_field {
            tb.add("    <Self as ScriptHookDeref>::on_deref_after_apply(&mut self.").ident(&deref_field.name).add(", vm, apply, value);");
        }
        tb.add("        <Self as ScriptHook>::on_after_apply(self, vm, apply, value);");
        tb.add("    }");
        tb.add("}");
                
        
        // Script
        
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptApply for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn script_apply(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value) {");
        tb.add("       if <Self as ScriptHook>::on_skip_apply(self, vm, apply, value) || value.is_nil(){return};");
        
        tb.add("        <Self as ScriptHookDeref>::on_deref_before_apply(self, vm, apply, value);");
        
        for field in &fields {
            if field.attrs.iter().any( | a | a.name == "live" || a.name =="script"){
                tb.add("if let Some(v) = vm.heap.value_apply_if_dirty(value, Value::from_id(id!(")
                    .ident(&field.name).add("))){");
                tb.add("self.").ident(&field.name).add(".script_apply(vm, apply, v);");
                tb.add("}");
            }
            if field.attrs.iter().any( | a | a.name =="deref" || a.name == "splat" || a.name =="walk" || a.name=="layout"){
                tb.add("self.").ident(&field.name).add(".script_apply(vm, apply, value);");
            }
        }
        tb.add("        if let Some(o) = value.as_object(){vm.heap.set_first_applied_and_clean(o);}");
        tb.add("        <Self as ScriptHookDeref>::on_deref_after_apply(self, vm, apply, value);");
        tb.add("    }");
        tb.add("}");
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptNew for").ident(&struct_name).stream(generic).stream(where_clause).add("{");
        
        tb.add("    fn script_new(vm: &mut Vm) -> Self {");
        tb.add("        let mut ret = Self {");
        for field in &fields {
            tb.ident(&field.name).add(":");
            
            if let Some(attr) = field.attrs.iter().find(|a| a.name == "script" || a.name == "live" ||a.name == "deref" || a.name == "rust"){
                if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty() {
                    if attr.name == "live" || attr.name =="script" || attr.name == "deref" {
                        tb.add("ScriptNew::script_new(vm)");
                    }
                    else {
                        tb.add("Default::default()");
                    }
                }
                else {
                    tb.add("(").stream(attr.args.clone()).add(").into()");
                }
            }
            else{
                tb.add("Default::default()");
            }
            tb.add(",");
        }
        tb.add("        };");
        tb.add("        <Self as ScriptHook>::on_new(&mut ret, vm);");
        tb.add("        ret");
        tb.add("    }");
         
        tb.add("    fn script_def_props(vm: &mut Vm, obj:Object) {");
        for field in &fields {
            
            if field.attrs.iter().find(|a| a.name == "deref").is_some(){
                tb.add("self.").ident(&field.name).add(".script_def_props(vm, obj)");
            }
            if let Some(attr) = field.attrs.iter().find(|a| a.name == "script" || a.name == "live"){
                tb.add("let value:Value = ");
                if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty() {
                    tb.add("").stream(Some(field.ty.clone())).add("::script_def(vm);");
                }
                else {
                    tb.add("(").stream(attr.args.clone()).add(").into();");
                }  
                tb.add("vm.heap.set_value(obj, Value::from_id(id_lut!(")
                    .ident(&field.name).add(")), value,&vm.thread.trap);");
            }
        }
        tb.add("    }");
        tb.add("}");
        
        if main_attribs.iter().any( | attr | attr.name == "debug_print") {
            tb.eprint();
        }
        
        return Ok(())
    }
    Ok(())
    /*
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
            
            fn gen_new(&self, tb: &mut TokenBuilder) -> Result<(), TokenStream> {
                tb.add("Self::").ident(&self.name);
                match &self.kind {
                    EnumKind::Bare => (),
                    EnumKind::Named(_) => {
                        if self.attributes.len() != 1 {
                            return error_result("For named and typle enums please provide default values");
                        }
                        tb.add("{").stream(self.attributes[0].args.clone()).add("}");
                    },
                    EnumKind::Tuple(_) => {
                        if self.attributes.len() != 1 {
                            return error_result("For named and typle enums please provide default values");
                        }
                        tb.add("(").stream(self.attributes[0].args.clone()).add(")");
                    }
                }
                Ok(())
            }
        }
        
        let mut pick = None;
        while !parser.eat_eot() {
            let attributes = parser.eat_attributes();
            // check if we have a default attribute
            if let Some(name) = parser.eat_any_ident() {
                if !attributes.is_empty() && attributes[0].name == "pick" {
                    if pick.is_some() {
                        return error_result("Enum can only have a single field marked pick");
                    }
                    pick = Some(items.len())
                }
                if let Some(types) = parser.eat_all_types() {
                    items.push(EnumItem {name, attributes, kind: EnumKind::Tuple(types)})
                }
                else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                    items.push(EnumItem {name, attributes, kind: EnumKind::Named(fields)})
                }
                else {
                    items.push(EnumItem {name, attributes, kind: EnumKind::Bare})
                }
            }
            //eprintln!("HERE2");
            parser.eat_level_or_punct(',');
        }
        
        if pick.is_none() {
            return error_result("Enum needs atleast one field marked pick");
        }
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveNew for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn new(cx:&mut Cx) -> Self {");
        tb.add("        let mut ret = ");
        items[pick.unwrap()].gen_new(tb) ?;
        tb.add("        ;ret.after_new_before_apply(cx);ret");
        tb.add("    }");
        
        tb.add("    fn live_type_info(cx:&mut Cx) -> LiveTypeInfo {");
        tb.add("        LiveTypeInfo{");
        tb.add("            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),");
        tb.add("            live_type: LiveType::of::<Self>(),");
        tb.add("            fields: Vec::new(),");
        let live_ignore = main_attribs.iter().any( | attr | attr.name == "live_ignore");
        tb.add("            live_ignore: ").ident(if live_ignore {"true"} else {"false"}).add(",");
        tb.add("            type_name: LiveId::from_str_with_lut(").string(&enum_name).add(").unwrap(),");
        /*tb.add("            kind: LiveTypeKind::Enum,");*/
        tb.add("        }");
        tb.add("    }");
        
        tb.add("    fn live_design_with(cx: &mut Cx) {");
        
        
        let is_u32_enum = main_attribs.iter().any( | attr | attr.name == "repr" && attr.args.as_ref().unwrap().to_string().to_lowercase() == "u32");
        if is_u32_enum {
            tb.add("        let mut variants = Vec::new();");
            for item in &items {
                match item.kind {
                    EnumKind::Bare => {
                        tb.add("variants.push(LiveId::from_str_with_lut(").string(&item.name).add(").unwrap());");
                    },
                    EnumKind::Named(_) |
                    EnumKind::Tuple(_) => {
                        return error_result("For repr(u32) shader-accessible enums only bare values are supported");
                    }
                }
            }
            tb.add("        cx.shader_registry.register_enum(LiveType::of::<").ident(&enum_name).add(">(),ShaderEnum{enum_name:LiveId::from_str_with_lut(").string(&enum_name).add(").unwrap(),variants});");
        }
        
        tb.add("    }");
        tb.add("}");
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveApply for").ident(&enum_name).stream(generic).stream(where_clause).add("{");
        //tb.add("    fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>() }");
        
        tb.add("    fn apply(&mut self, cx: &mut Cx, apply:&mut Apply, start_index:usize, nodes: &[LiveNode]) -> usize {");
        tb.add("        self.before_apply(cx, apply, start_index, nodes);");
        tb.add("        if let Some(index) = self.skip_apply(cx, apply, start_index, nodes){");
        tb.add("            self.after_apply(cx, apply, start_index, nodes);");
        tb.add("            return index");
        tb.add("        }");
        tb.add("        let mut index = start_index;");
        //tb.add("        let enum_id = LiveId(").suf_u64(LiveId::from_str(&enum_name).0).add(");");
        tb.add("        match &nodes[start_index].value{");
        
        tb.add("            LiveValue::Id(variant) | LiveValue::BareEnum(variant)=>{");
        tb.add("                match variant{");
        for item in &items {
            if let EnumKind::Bare = item.kind {
                //tb.add("            LiveId(").suf_u64(LiveId::from_str(&item.name).0).add(")=>{index += 1;*self = Self::").ident(&item.name).add("},");
            }
        }
        tb.add("                    _=>{");
        tb.add("                        cx.apply_error_wrong_enum_variant(live_error_origin!(), index, nodes, enum_id, *variant);");
        tb.add("                        index = nodes.skip_node(index);");
        tb.add("                    }");
        tb.add("                }");
        tb.add("            },");
        
        tb.add("            LiveValue::NamedEnum(variant)=>{");
        tb.add("                match variant{");
        for item in &items {
            if let EnumKind::Named(fields) = &item.kind {
                //tb.add("            LiveId(").suf_u64(LiveId::from_str(&item.name).0).add(")=>{");
                tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                tb.add("                else{*self = ");
                item.gen_new(tb) ?;
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
                    //tb.add("                        LiveId(").suf_u64(LiveId::from_str(&field.name).0).add(")=>{index = (*");
                    tb.ident(&format!("prefix_{}", field.name)).add(").apply(cx, apply, index, nodes);},");
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
        tb.add("            LiveValue::TupleEnum(variant)=>{");
        tb.add("                match variant{");
        
        for item in &items {
            if let EnumKind::Tuple(args) = &item.kind {
                //tb.add("            LiveId(").suf_u64(LiveId::from_str(&item.name).0).add(")=>{");
                
                tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                tb.add("                else{*self = ");
                item.gen_new(tb) ?;
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
                    tb.add("                        ").unsuf_usize(i).add("=>{index = (*").ident(&format!("var{}", i)).add(").apply(cx, apply, index, nodes); },");
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
        tb.add("        self.after_apply(cx, apply, start_index, nodes);");
        tb.add("        index");
        tb.add("    }");
        
        tb.add("}");
        
        //tb.eprint();
        Ok(())
    }
    else {
        error_result("Not enum or struct")
    }*/
    
}

