use proc_macro::{TokenStream};

use makepad_micro_proc_macro::{
    TokenBuilder,
    TokenParser,
    Attribute,
    StructField,
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
                return error_result("Please annotate the field type with #[rust] for rust-only fields, and #[live] for scriptable mapped fields and #[deref] for a base class and #[script] to call script_new");
            }
        }
        
        
        // Deref
        
        
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
        
        // marker         
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptDeriveMarker for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
                
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptHookDeref for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        tb.add("    fn on_deref_before_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, scope:&mut Scope, value:ScriptValue){");
        tb.add("         <Self as ScriptHook>::on_before_apply(self, vm, apply, scope, value);");
        tb.add("         <Self as ScriptHook>::on_before_dispatch(self, vm, apply, scope, value);");
        tb.add("    }");
        
        tb.add("    fn on_deref_after_apply(&mut self,vm: &mut ScriptVm, apply:&Apply, scope:&mut Scope, value:ScriptValue){");
        
        tb.add("        <Self as ScriptHook>::on_after_apply(self, vm, apply, scope, value);");
        tb.add("        <Self as ScriptHook>::on_after_dispatch(self, vm, apply, scope, value);");
        tb.add("    }");
        tb.add("}");
                
        
        // ScriptApply
        
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptApply for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn script_type_id(&self)->ScriptTypeId{ ScriptTypeId::of::<Self>()}");
        
        tb.add("    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, scope:&mut Scope, value:ScriptValue) {");
        tb.add("       if <Self as ScriptHook>::on_custom_apply(self, vm, apply, scope, value) || value.is_nil(){return};");
        
        tb.add("        <Self as ScriptHookDeref>::on_deref_before_apply(self, vm, apply, scope, value);");
        
        
        for field in &fields {
            if field.attrs.iter().any( | a | a.name == "live" ){
                tb.add("if let Some(v) = vm.heap.value_apply_if_dirty(value, id!(")
                    .ident(&field.name).add(").into()){");
                tb.add("<").stream(Some(field.ty.clone())).add(" as ScriptApply>::script_apply(&mut self.").ident(&field.name).add(",vm, apply, scope, v);");
                tb.add("}");
            }
            if field.attrs.iter().any( | a | a.name =="deref" || a.name == "splat" || a.name =="walk" || a.name=="layout"){
                tb.add("<").stream(Some(field.ty.clone())).add(" as ScriptApply>::script_apply(&mut self.").ident(&field.name).add(", vm, apply, scope, value);");
            }
        }
        tb.add("        if let Some(o) = value.as_object(){vm.heap.set_first_applied_and_clean(o);}");
        tb.add("        <Self as ScriptHookDeref>::on_deref_after_apply(self, vm, apply, scope, value);");
        tb.add("    }");
        
        
        tb.add("    fn script_to_value(&self, vm: &mut ScriptVm)->ScriptValue {");
        
        tb.add("        let proto = Self::script_proto(vm).into();");
        tb.add("        let obj = vm.heap.new_with_proto(proto);");
        tb.add("        self.script_to_value_props(vm, obj);");
        tb.add("        obj.into()");
        tb.add("     }");
                                        
        tb.add("    fn script_to_value_props(&self, vm: &mut ScriptVm, obj:ScriptObject) {");
        
        for field in &fields {
            if field.attrs.iter().find(|a| a.name == "deref").is_some(){
                tb.add("self.").ident(&field.name).add(".script_to_value_props(vm, obj);");
            }
            if let Some(_) = field.attrs.iter().find(|a| a.name == "live"){
                tb.add("let value:ScriptValue = <").stream(Some(field.ty.clone())).add(" as ScriptApply>::script_to_value( &self.").ident(&field.name).add(", vm); ");
                tb.add("vm.heap.set_value(obj, ScriptValue::from_id(id_lut!(")
                .ident(&field.name).add(")), value, &vm.thread.trap);");
            }
        }
                
        tb.add("    }");
        tb.add("}");
                
        
         
        // ScriptNew
        
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptNew for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");

        tb.add("    fn script_type_id_static()->ScriptTypeId{ ScriptTypeId::of::<Self>()}");
        
        tb.add("    fn script_new(vm: &mut ScriptVm) -> Self {");
        tb.add("        Self {");
        for field in &fields {
            tb.ident(&field.name).add(":");
            
            if let Some(attr) = field.attrs.iter().find(|a| a.name == "new" || a.name == "live" ||a.name == "deref" || a.name == "rust"){
                if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty() {
                    if attr.name == "live" || attr.name =="new" || attr.name == "deref" {
                        tb.add("ScriptNew::script_new_with_default(vm)");
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
        tb.add("        }");
        tb.add("    }");
        
         
        tb.add("    fn script_proto_props(vm: &mut ScriptVm, obj:ScriptObject, props:&mut ScriptTypeProps) {");
        
        // Find the index of the deref field (if any)
        let deref_index = fields.iter().position(|f| f.attrs.iter().any(|a| a.name == "deref"));
        
        for (idx, field) in fields.iter().enumerate() {
            // Process deref field - recursively adds props from the base type
            if field.attrs.iter().find(|a| a.name == "deref").is_some(){
                tb.add("<").stream(Some(field.ty.clone())).add(" as ScriptNew>::script_proto_props(vm, obj, props);");
                // Mark where Rust instance fields begin (fields AFTER the deref)
                tb.add("props.mark_rust_instance_start();");
            }
            
            // Process live fields
            if let Some(_attr) = field.attrs.iter().find(|a| a.name == "live"){
                // Skip live fields that come BEFORE the deref field - they are not instance data
                if let Some(deref_idx) = deref_index {
                    if idx < deref_idx {
                        // Still register prototype for scripting, but don't add to props
                        // (props is used for shader instance field collection)
                        tb.add("<").stream(Some(field.ty.clone())).add(" as ScriptNew>::script_proto(vm);");
                        // Note: NOT adding to props - these are config fields, not instance data
                        continue;
                    }
                }
                
                // This is either a field after deref, or there's no deref field
                tb.add("<").stream(Some(field.ty.clone())).add(" as ScriptNew>::script_proto(vm);");
                tb.add("props.insert(id_lut!(").ident(&field.name).add("),<").stream(Some(field.ty.clone())).add(" as ScriptNew>::script_type_id_static());");
            }
        }
        
        tb.add("    }");
        tb.add("}");
        
        if main_attribs.iter().any( | attr | attr.name == "debug_print") {
            tb.eprint();
        }
        
        return Ok(())
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
                if attributes.iter().any(|a| a.name == "pick" || a.name == "default"){
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
            parser.eat_level_or_punct(',');
        }
        
        if pick.is_none() {
            return error_result("Enum needs atleast one field marked pick");
        }
        
        
        // marker         
        
        
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptDeriveMarker for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
        
        
        
        
        
        // ScriptNew
        
        
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptNew for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}");
        tb.add("    fn script_new(vm:&mut ScriptVm)->Self{");
        tb.add("       ");
        items[pick.unwrap()].gen_new(tb) ?;
        tb.add("       ");
        tb.add("    }");
        
        tb.add("    fn script_default(vm:&mut ScriptVm)->ScriptValue{");
        tb.add("        Self::script_proto(vm);");
        tb.add("        Self::script_new(vm).script_to_value(vm)");
        tb.add("    }");
        
        tb.add("    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{");
        tb.add("        if let Some(o) = value.as_object(){");
        tb.add("            let root_proto = heap.root_proto(o);");
        tb.add("            if let Some(id) = root_proto.as_id(){");
        tb.add("                return match id{");
        for item in &items {
            tb.add("                 id!(").ident(&item.name).add(")=>true,");
        }
        tb.add("                     _=>false");
        tb.add("                 }");
        tb.add("            }");
        tb.add("        }");
        tb.add("        false");
        tb.add("    }");
        
        tb.add("    fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{");
        tb.add("        let enum_object = vm.heap.new_object();");

        for item in &items {
            match &item.kind {
                EnumKind::Bare => {
                    tb.add("let bare = vm.heap.new_with_proto(id_lut!(").ident(&item.name).add(").into());");
                    tb.add("vm.heap.set_value(enum_object, id!(").ident(&item.name).add(").into(), bare.into(), &vm.thread.trap);");
                    tb.add("vm.heap.freeze(bare);");
                },
                EnumKind::Tuple(args) => {
                    tb.add("vm.add_method(enum_object, id_lut!(").ident(&item.name).add("), &[], |vm, args|{");
                    tb.add("    let tuple = vm.heap.new_with_proto(id!(").ident(&item.name).add(").into());");
                    tb.add("    if vm.heap.vec_len(args) != ").unsuf_usize(args.len()).add("{");
                    tb.add("        vm.thread.trap.err_invalid_arg_count();");
                    tb.add("    }");
                    for (i, arg) in args.iter().enumerate(){
                        tb.add("if let Some(a) = vm.heap.vec_value_if_exist(args, ").unsuf_usize(i).add("){");
                        tb.add("    if!<").stream(Some(arg.clone())).add(" as ScriptNew>::script_type_check(vm.heap, a){");
                        tb.add("        vm.thread.trap.err_invalid_arg_type();");
                        tb.add("    }");
                        tb.add("}");
                    }
                    tb.add("    vm.heap.vec_push_vec(tuple, args, &vm.thread.trap);");
                    tb.add("    tuple.into()");
                    tb.add("});");
                }
                EnumKind::Named(fields) =>{
                    tb.add("let def =");
                    item.gen_new(tb) ?;
                    tb.add(";");
                    tb.add("let named = vm.heap.new_with_proto(id_lut!(").ident(&item.name).add(").into());");
                    tb.add("let mut props = ScriptTypeProps::default();");
                    tb.add("if let Self::").ident(&item.name).add("{");
                    for (i, field) in fields.iter().enumerate(){
                        tb.ident(&field.name).add(":").ident(&format!("v{i}")).add(",");
                    }
                    tb.add("} = def{");
                    for (i, field) in fields.iter().enumerate(){
                        tb.add("let value = ").ident(&format!("v{i}")).add(".script_to_value(vm);");
                        tb.add("props.insert(id_lut!(").ident(&field.name).add("), <").stream(Some(field.ty.clone())).add(" as ScriptNew>::script_type_id_static());");
                        tb.add(" vm.heap.set_value(named, id!(").ident(&field.name).add(").into(), value, &vm.thread.trap);");
                    }
                    tb.add("}");
                    tb.add("let ty_check = ScriptTypeCheck{props, object: None};");
                    tb.add("let ty_index = vm.heap.register_type(None, ty_check);");
                    tb.add("vm.heap.set_type(named, ty_index);");
                    tb.add("vm.heap.freeze_component(named);");
                    tb.add("vm.heap.set_value(enum_object, id!(").ident(&item.name).add(").into(), named.into(), &vm.thread.trap);");
                    // uh oh crap. we need to get the default value out of the unparsed defaults
                }
            }
        }
        tb.add("    enum_object.into()");
        tb.add("    }");
        tb.add("}");
        
                        
                
        // ScriptApply
        
        
        tb.add("impl").stream(generic.clone());
        tb.add("ScriptApply for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                
        tb.add("    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}");
        tb.add("    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, scope:&mut Scope, value:ScriptValue){");
        tb.add("        if self.on_custom_apply(vm, apply, scope, value){");
        tb.add("            return");
        tb.add("        }");
        tb.add("        if let Some(object) = value.as_object(){");
        tb.add("            let root_proto = vm.heap.root_proto(object);");
        tb.add("            if let Some(id) = root_proto.as_id(){");
        tb.add("                match id{");
        for item in &items {
            match &item.kind {
                EnumKind::Bare => {
                    tb.add("        id!(").ident(&item.name).add(")=>{");
                    tb.add("            *self = Self::").ident(&item.name).add(";");
                    tb.add("            return;");
                    tb.add("        }");
                }
                EnumKind::Tuple(args) => {
                    tb.add("        id!(").ident(&item.name).add(")=>{");
                    tb.add("            if let Self::").ident(&item.name).add("(..)  = self{}else{");
                    tb.add("               *self = ");
                    item.gen_new(tb) ?;
                    tb.add(";");
                    tb.add("            }");
                    tb.add("            if let Self::").ident(&item.name).add("(");
                    for i in 0..args.len(){
                        tb.ident(&format!("v{i}")).add(",");
                    }
                    tb.add(") = self{");
                    for (i, arg) in args.iter().enumerate(){
                        tb.add("            if let Some(v) = vm.heap.vec_value_if_exist(object, ").unsuf_usize(i).add("){");
                        tb.add("                 <").stream(Some(arg.clone())).add(" as ScriptApply>::script_apply(").ident(&format!("v{i}")).add(", vm, apply, scope, v);");
                        tb.add("            }");
                    }
                    tb.add("            }");
                    tb.add("            return;");
                    tb.add("        }");
                }
                EnumKind::Named(fields) =>{
                    tb.add("        id!(").ident(&item.name).add(")=>{");
                    tb.add("            if let Self::").ident(&item.name).add("{..}  = self{}else{");
                    tb.add("               *self = ");
                    item.gen_new(tb) ?;
                    tb.add(";");
                    tb.add("            }");
                    tb.add("            if let Self::").ident(&item.name).add("{");
                    for (i, field) in fields.iter().enumerate(){
                        tb.ident(&field.name).add(":").ident(&format!("v{i}")).add(",");
                    }
                    tb.add("} = self{");
                    for (i, field) in fields.iter().enumerate(){
                        tb.add("if let Some(v) = vm.heap.value_apply_if_dirty(value, ScriptValue::from_id(id!(").ident(&field.name).add("))){");
                        tb.add("    <").stream(Some(field.ty.clone())).add(" as ScriptApply>::script_apply(").ident(&format!("v{i}")).add(", vm, apply, scope, v);");
                        tb.add("}");
                    }
                    tb.add("            }");
                    tb.add("            return;");
                    tb.add("        }");
                }
            }
        }
        tb.add("                    _=>{}");
        tb.add("                }");
        tb.add("            }");
        tb.add("        }");
        tb.add("        vm.thread.trap.err_enum_unknown_variant();");
        tb.add("    }");
        
                
        tb.add("    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{");
        tb.add("        match self{");
        for item in &items {
            match &item.kind {
                EnumKind::Bare => {
                    tb.add("Self::").ident(&item.name).add("=>{");
                    tb.add("    Self::script_enum_lookup_variant(vm,id!(").ident(&item.name).add("))");
                    tb.add("}");
                }
                EnumKind::Tuple(args) => {
                    tb.add("Self::").ident(&item.name).add("(");
                    for i in 0..args.len(){
                        tb.ident(&format!("v{i}")).add(",");
                    }
                    tb.add(")=>{");
                    tb.add("    let tuple = vm.heap.new_with_proto(id!(").ident(&item.name).add(").into());");
                    for (i, arg) in args.iter().enumerate(){
                        tb.add("let value = <").stream(Some(arg.clone())).add(" as ScriptApply>::script_to_value(").ident(&format!("v{i}")).add(",vm);");
                        tb.add("vm.heap.vec_push(tuple, NIL, value, &vm.thread.trap);");
                    }
                    tb.add("    tuple.into()");
                    tb.add("}");
                }
                EnumKind::Named(fields) =>{
                    tb.add("Self::").ident(&item.name).add("{");
                    for (i, field) in fields.iter().enumerate(){
                        tb.ident(&field.name).add(":").ident(&format!("v{i}")).add(",");
                    }
                    tb.add("}=>{");
                    tb.add("    let proto = Self::script_enum_lookup_variant(vm,id!(").ident(&item.name).add("));");
                    tb.add("    let named = vm.heap.new_with_proto(proto);");
                    for (i, field) in fields.iter().enumerate(){
                        tb.add("let value = <").stream(Some(field.ty.clone())).add(" as ScriptApply>::script_to_value(").ident(&format!("v{i}")).add(", vm);");
                        tb.add("vm.heap.set_value(named, id!(").ident(&field.name).add(").into(), value, &vm.thread.trap);");
                    }
                    tb.add("    named.into()");
                    tb.add("}");
                }
            }
        }
        tb.add("        }");
        tb.add("    }");
        
        tb.add("}");
                            
        Ok(())
    }
    else {
        error_result("Not enum or struct")
    }    
}

pub fn derive_script_hook_impl(input: TokenStream) -> TokenStream {
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
            tb.add("ScriptHook for").ident(&struct_name).stream(generic).stream(where_clause).add("{}");
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(enum_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(None);
            tb.add("impl").stream(generic.clone());
            tb.add("ScriptHook for").ident(&enum_name).stream(generic).stream(where_clause).add("{}");
            return tb.end();
        }
    }
    parser.unexpected()
}
/*    
        
//#[derive(Script)]
#[allow(unused)]
pub enum EnumTest{
    //  #[pick]
    Bare,
    Tuple(f64),
    Named{named_field:f64}
}
    
impl ScriptHook for EnumTest{
}
    
impl ScriptNew for EnumTest{
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_new(vm:&mut ScriptVm)->Self{let mut ret = Self::Bare; ret.on_new(vm);ret}
            
    fn script_default(vm:&mut ScriptVm)->ScriptValue{
        Self::script_proto(vm);
        Self::script_new(vm).script_to_value(vm)
    }
            
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if Self::on_type_check(heap, value){
            return true
        }
        if let Some(o) = value.as_object(){
            let root_proto = heap.root_proto(o);
            if let Some(id) = root_proto.as_id(){
                return match id{
                    id!(Bare)=>true,
                    id!(Tuple)=>true,
                    id!(Named)=>true,
                    _=>false
                }
            }
        }
        false
    }
            
    fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{
        let enum_object = vm.heap.new();
                    
        // how do we typecheck an enum type eh
        let bare = vm.heap.new_with_proto(id!(Bare).into());
        vm.heap.set_value(enum_object, id_lut!(Bare).into(), bare.into(), &vm.thread.trap);
        vm.heap.freeze(bare);
                    
        // alright next one the tuple
        vm.add_method(enum_object, id!(Tuple), &[], |vm, args|{
            let tuple = vm.heap.new_with_proto(id!(Tuple).into());
            if vm.heap.vec_len(args) != 1 {
                vm.thread.trap.err_invalid_arg_count();
            }
            if let Some(a) = vm.heap.vec_value_if_exist(args, 0){
                if !f64::script_type_check(vm.heap, a){
                    vm.thread.trap.err_invalid_arg_type();
                }
            }
            vm.heap.vec_push_vec(tuple, args, &vm.thread.trap);
            tuple.into()
        });
                    
        // we can make a type index prop check for sself thing
        let def = Self::Named{named_field: 1.0};
        let named = vm.heap.new_with_proto(id_lut!(Named).into());
        let mut props = ScriptTypeProps::default();
        if let Self::Named{named_field:v0} = def{
                            
            let value = v0.script_to_value(vm);
            props.insert(id_lut!(named_field), f64::script_type_id_static());
            vm.heap.set_value(named, id!(named_field).into(), value, &vm.thread.trap);
                            
        }
                    
        let ty_check = ScriptTypeCheck{props, object: None};
        let ty_index = vm.heap.register_type(None, ty_check);
        vm.heap.freeze_with_type(named, ty_index);
        vm.heap.set_value(enum_object, id!(Named).into(), named.into(), &vm.thread.trap);
                    
        enum_object.into()
    }
}
    
impl ScriptToValue for EnumTest{
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        match self{
            Self::Bare=>{
                Self::script_enum_lookup_variant(vm, id!(Bare))
            }
            Self::Tuple(x)=>{
                let tuple = vm.heap.new_with_proto(id!(Tuple).into());
                let value = x.script_to_value(vm);
                vm.heap.vec_push(tuple, NIL, value, &vm.thread.trap);
                tuple.into()
            }
            Self::Named{named_field}=>{
                let proto = Self::script_enum_lookup_variant(vm, id!(Named));
                let named = vm.heap.new_with_proto(proto);
                let value = named_field.script_to_value(vm);
                vm.heap.set_value(named, id_lut!(named_field).into(), value, &vm.thread.trap);
                named.into()
            }
        }
    }
}
    
impl ScriptApply for EnumTest{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&Apply, value:ScriptValue){
        if self.on_skip_apply(vm, apply, value){
            return
        }
        if let Some(object) = value.as_object(){
            let root_proto = vm.heap.root_proto(object);
            // we now have to fetch the proto LiveId of the object
            if let Some(id) = root_proto.as_id(){
                match id{
                    id!(Bare)=>{
                        *self = Self::Bare;
                        return;
                    }
                    id!(Tuple)=>{
                        if let Self::Tuple(..) = self{} else {*self = Self::Tuple(1.0)};
                        if let Self::Tuple(a1) = self{
                            if let Some(v) = vm.heap.vec_value_if_exist(object, 0){
                                a1.script_apply(vm, apply, v);
                            }
                            return
                        }
                        return
                    }
                    id!(Named)=>{
                        if let Self::Named{..} = self{} else { *self = Self::Named{named_field:1.0}};
                        if let Self::Named{named_field} = self{
                            if let Some(v) = vm.heap.value_apply_if_dirty(value, ScriptValue::from_id(id!(named_field))){
                                named_field.script_apply(vm, apply, v);
                            }
                            return
                        }
                    }
                    _=>{
                    }
                }
            }
        }
        vm.thread.trap.err_enum_unknown_variant();
    }
}*/