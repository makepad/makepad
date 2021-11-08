use proc_macro::{TokenStream};

use crate::macro_lib::*;
use crate::id::*;

pub fn derive_live_component_hooks_impl(input: TokenStream) -> TokenStream {
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
            else{
                return parser.unexpected();
            };
            
            let deref_target = fields.iter().find( | field | field.name == "deref_target");
            let draw_call_vars = fields.iter().find( | field | field.name == "draw_call_vars");
            
            if let Some(_) = draw_call_vars{
                tb.add("impl").stream(generic.clone());
                tb.add("LiveComponentHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn live_update_value_unknown(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {");
                tb.add("        self.draw_call_vars.update_value(cx, ptr, id);");
                tb.add("    }");
                tb.add("    fn live_apply_value_unknown(&mut self, cx: &mut Cx, index:&mut usize, nodes:&[ApplyNode]) {");
                tb.add("        self.draw_call_vars.apply_value(cx, index, nodes);");
                tb.add("    }");
                tb.add("    fn before_live_update(&mut self, cx:&mut Cx, live_ptr: LivePtr){");
                tb.add("        self.draw_call_vars.init_shader(cx, DrawShaderPtr(live_ptr), &self.geometry);");
                tb.add("    }");
                tb.add("    fn after_live_update(&mut self, cx: &mut Cx, live_ptr:LivePtr) {");
                tb.add("        self.draw_call_vars.init_slicer(cx);");
                tb.add("    }");
                tb.add("}");                
            }
            else if let Some(_) = deref_target {
                tb.add("impl").stream(generic.clone());
                tb.add("LiveComponentHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                tb.add("    fn live_update_value_unknown(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {");
                tb.add("        self.deref_target.live_update_value_unknown(cx, id, ptr);");
                tb.add("    }");
                tb.add("    fn live_apply_value_unknown(&mut self, cx: &mut Cx, index:&mut usize, nodes:&[ApplyNode]) {");
                tb.add("        self.deref_target.live_apply_value_unknown(cx, index, nodes);");
                tb.add("    }");
                tb.add("    fn before_live_update(&mut self, cx:&mut Cx, live_ptr: LivePtr){");
                tb.add("        self.deref_target.before_live_update(cx, live_ptr);");
                tb.add("    }");
                tb.add("    fn after_live_update(&mut self, cx: &mut Cx, live_ptr:LivePtr) {");
                tb.add("        self.deref_target.after_live_update(cx, live_ptr);");
                tb.add("    }");
                tb.add("}");
            }
            else{
                tb.add("impl").stream(generic.clone());
                tb.add("LiveComponentHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{}");
            }
            
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

pub fn derive_live_component_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
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
            else{
                return parser.unexpected();
            };
            
            let deref_target = fields.iter().find( | field | field.name == "deref_target");

            // alright now. we have a field
            for field in &fields {
                if field.attrs.len() != 1 || field.attrs[0].name != "live" && field.attrs[0].name != "local"  && field.attrs[0].name != "hidden" {
                    return error(&format!("Field {} does not have a live, local or hidden attribute", field.name));
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
            tb.add("    fn live_update_value(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {");
            tb.add("        match id {");
            for field in &fields{
                if field.attrs[0].name == "live"{
                    tb.add("    Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>self.").ident(&field.name).add(".live_update(cx, ptr),");
                }
            }
            if deref_target.is_some(){
                tb.add("        _=> self.deref_target.live_update_value(cx, id, ptr)");
            }
            else{
                tb.add("        _=> self.live_update_value_unknown(cx, id, ptr)");
            }
            tb.add("        }");
            tb.add("    }");
            
            tb.add("    fn live_apply_value(&mut self, cx: &mut Cx, index:&mut usize, nodes:&[ApplyNode]) {");
            tb.add("        match nodes[*index].id {");
            for field in &fields{
                if field.attrs[0].name == "live"{
                    tb.add("    Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>self.").ident(&field.name).add(".live_apply(cx, index, nodes),");
                }
            }
            if deref_target.is_some(){
                tb.add("        _=> self.deref_target.live_apply_value(cx, index, nodes)");
            }
            else{
                tb.add("        _=> self.live_apply_value_unknown(cx, index, nodes)");
            }
            tb.add("        }");
            tb.add("    }");
            
            tb.add("}");
            
            
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponent for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {");
            tb.add("        self.before_live_update(cx, live_ptr);");
            tb.add("        if let Some(mut iter) = cx.shader_registry.live_registry.live_class_iterator(live_ptr) {");
            tb.add("            while let Some((id, live_ptr)) = iter.next_id(&cx.shader_registry.live_registry) {");
            tb.add("                if id == id!(rust_type) && !cx.verify_type_signature(live_ptr, Self::live_type()) {");
            tb.add("                    return;");
            tb.add("                 }");
            tb.add("                self.live_update_value(cx, id, live_ptr)");
            tb.add("            }");
            tb.add("        }");
            tb.add("        self.after_live_update(cx, live_ptr);");
            tb.add("    }");
            tb.add("    fn live_apply(&mut self, cx: &mut Cx, index:&mut usize, nodes: &[ApplyNode]) {");
            tb.add("        let start_index = *index;");
            tb.add("        self.before_live_apply(cx, start_index, nodes);");
            tb.add("        *index += 1;"); // skip the class
            tb.add("        loop{");
            tb.add("            if nodes[*index].value.is_close(){");
            tb.add("                *index += 1;"); 
            tb.add("                break;");
            tb.add("            }");
            tb.add("            self.live_apply_value(cx, index, nodes);");
            tb.add("        }");
            tb.add("        self.after_live_apply(cx, start_index, nodes);");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveNew for").ident(&struct_name).stream(generic).stream(where_clause).add("{");
            tb.add("    fn live_type() -> LiveType {");
            tb.add("        LiveType(std::any::TypeId::of::<").ident(&struct_name).add(">())");
            tb.add("    }");
            
            tb.add("    fn live_register(cx: &mut Cx) {");
            tb.add("        struct Factory();");
            tb.add("        impl LiveFactory for Factory {");
            
            tb.add("            fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveComponent> {");
            tb.add("                Box::new(").ident(&struct_name).add(" ::live_new(cx))");
            tb.add("            }");
            
            tb.add("            fn live_fields(&self, fields: &mut Vec<LiveField>) {");
            for field in &fields{
                let attr = &field.attrs[0];
                if attr.name == "live" || attr.name  == "local"{
                    tb.add("fields.push(LiveField{id:Id::from_str(").string(&field.name).add(").unwrap(),");
                    tb.add("live_type:Some(").stream(Some(field.ty.clone())).add("::live_type()),");
                    if attr.name == "live"{
                        tb.add("kind: LiveFieldKind::Live");
                    }
                    else{
                        tb.add("kind: LiveFieldKind::Local");
                    }
                    tb.add("});");
                }
            }
            tb.add("            }");
            
            tb.add("        }");
            tb.add("        cx.register_factory(").ident(&struct_name).add("::live_type(), Box::new(Factory()));");
            tb.add("    }");
            tb.add("    fn live_new(cx: &mut Cx) -> Self {");
            tb.add("        Self {");
            for field in &fields{
                let attr = &field.attrs[0];
                tb.ident(&field.name).add(":");
                if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty() {
                    if attr.name == "live" {
                        tb.add("LiveNew::live_new(cx)");
                    }
                    else {
                        tb.add("Default::default()");
                    }
                }
                else{
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
            
            let mut default = None;
            while !parser.eat_eot() {
                let attributes = parser.eat_attributes();
                // check if we have a default attribute
                if let Some(name) = parser.eat_any_ident() {
                    if attributes.len() != 1{
                        return error(&format!("Field {} does not have a live or default attribute", name));
                    }
                    if attributes[0].name == "default"{
                        if default.is_some(){
                            return error(&format!("Enum can only have a single field marked default"));                            
                        }
                        default = Some(items.len())
                    }
                    if let Some(types) = parser.eat_all_types() {
                        items.push(EnumItem {name, attributes, kind: EnumKind::Tuple(types)})
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                        items.push(EnumItem {name, attributes, kind: EnumKind::Named(fields)})
                    }
                    else if parser.is_punct(',') || parser.is_eot() { // bare variant
                        items.push(EnumItem {name, attributes, kind: EnumKind::Bare})
                    }
                    else {
                        return parser.unexpected();
                    }
                }
                
                parser.eat_punct(',');
            }
            if default.is_none(){
                return error(&format!("Enum needs atleast one field marked default"));
            }
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveNew for").ident(&enum_name).stream(generic.clone()).stream(where_clause.clone()).add("{");

            tb.add("    fn live_new(_cx:&mut Cx) -> Self {");
            items[default.unwrap()].gen_new(&mut tb);
            tb.add("    }");
            tb.add("    fn live_type() -> LiveType {");
            tb.add("        LiveType(std::any::TypeId::of::<").ident(&enum_name).add(">())");
            tb.add("    }");
            tb.add("    fn live_register(cx: &mut Cx) {");
            tb.add("        let mut bare = Vec::new();");
            tb.add("        let mut named = Vec::new();");
            tb.add("        let mut tuple = Vec::new();");
            for item in &items{
                match item.kind{
                    EnumKind::Bare => tb.add("bare.push(Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add("));"),
                    EnumKind::Named(_) => tb.add("named.push(Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add("));"),
                    EnumKind::Tuple(_) => tb.add("tuple.push(Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add("));")
                };
            }
            tb.add("        cx.register_enum(").ident(&enum_name).add("::live_type(),LiveEnumInfo{bare, named, tuple});");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl").stream(generic.clone());
            tb.add("LiveComponent for").ident(&enum_name).stream(generic).stream(where_clause).add("{");

            tb.add("    fn live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {");
            tb.add("        self.before_live_update(cx, live_ptr);");
            tb.add("        let node = cx.shader_registry.live_registry.resolve_ptr(live_ptr);");
            tb.add("        match &node.value{");
            tb.add("            LiveValue::MultiPack(pack)=>{");
            tb.add("                let id = cx.shader_registry.live_registry.find_enum_origin(*pack, node.id);");
            tb.add("                match id{");
            for item in &items{
                if let EnumKind::Bare = item.kind{
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>*self = Self::").ident(&item.name).add(",");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum Wrapping cannot find id {}").add(", id);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            },");
            
            tb.add("            LiveValue::Class{class, node_start, node_count}=>{");
            tb.add("                let id = cx.shader_registry.live_registry.find_enum_origin(*class, node.id);");
            tb.add("                match id{");
            for item in &items{
                if let EnumKind::Named(fields) = &item.kind{
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
                    tb.add("                }");
                    tb.add("                if let Self::").ident(&item.name).add("{");
                    for field in fields{
                        tb.ident(&field.name).add(":").ident(&format!("prefix_{}",field.name)).add(",");
                    }
                    tb.add("                } = self {");
                    tb.add("                    let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start, *node_count);");
                    tb.add("                    while let Some((prop_id, live_ptr)) = iter.next_id(&cx.shader_registry.live_registry) {");
                    tb.add("                        match prop_id{");
                    for field in fields{
                        tb.add("                        Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>(*");
                        tb.ident(&format!("prefix_{}",field.name)).add(").live_update(cx, live_ptr),");
                    }
                    tb.add("                            _=>{");
                    tb.add("                                println!(").string("Enum Wrapping cannot find named struct {} property {}").add(", id, prop_id);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum Wrapping cannot find named struct {}").add(", id);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            
            
            tb.add("            LiveValue::Call{target, node_start, node_count}=>{");
            tb.add("                let id = cx.shader_registry.live_registry.find_enum_origin(*target, node.id);");
            tb.add("                match id{");
            
            for item in &items{
                if let EnumKind::Tuple(args) = &item.kind{
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
                    tb.add("                }");
                    
                    tb.add("                if let Self::").ident(&item.name).add("(");
                    for i in 0..args.len(){
                        tb.ident(&format!("var{}",i)).add(",");
                    }
                    tb.add("                    ) = self {");
                    tb.add("                    let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start, *node_count);");
                    tb.add("                    while let Some((count, live_ptr)) = iter.next_prop() {");
                    tb.add("                        match count{");
                    for i in 0..args.len(){
                        tb.add("                        ").unsuf_usize(i).add("=>(*").ident(&format!("var{}",i)).add(").live_update(cx, live_ptr),");
                    }
                    tb.add("                            _=>{");
                    tb.add("                                println!(").string("Enum Wrapping cannot find tuple struct {} arg {}").add(", id, count);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum Wrapping cannot find tuple struct {}").add(", id);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            
            
            tb.add("            _=>()");
            tb.add("        }");
            tb.add("        self.after_live_update(cx, live_ptr);");
            tb.add("    }");
            
            tb.add("    fn live_apply(&mut self, cx: &mut Cx, index:&mut usize, nodes: &[ApplyNode]) {");
            tb.add("        let start_index = *index;");
            tb.add("        self.before_live_apply(cx, start_index, nodes);");
            tb.add("        match &nodes[start_index].value{");
            tb.add("            ApplyValue::Id(id)=>{");
            tb.add("                match id{");
            for item in &items{
                if let EnumKind::Bare = item.kind{
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>*self = Self::").ident(&item.name).add(",");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum Wrapping cannot find id {}").add(", id);");
            tb.add("                        ApplyValue::skip_value(index, nodes);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            },");

            tb.add("            ApplyValue::Class{class, ..}=>{");
            tb.add("                match class{");
            for item in &items{
                if let EnumKind::Named(fields) = &item.kind{
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
                    tb.add("                }");
                    tb.add("                if let Self::").ident(&item.name).add("{");
                    for field in fields{
                        tb.ident(&field.name).add(":").ident(&format!("prefix_{}",field.name)).add(",");
                    }
                    tb.add("                } = self {");
                    tb.add("                    *index += 1;"); // skip the class
                    tb.add("                    loop{");
                    tb.add("                        if nodes[*index].value.is_close(){");
                    tb.add("                            *index += 1;"); 
                    tb.add("                            break;");
                    tb.add("                        }");
                    tb.add("                        match nodes[*index].id{");
                    for field in fields{
                        tb.add("                        Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>(*");
                        tb.ident(&format!("prefix_{}",field.name)).add(").live_apply(cx, index, nodes),");
                    }
                    tb.add("                            _=>{");
                    tb.add("                                println!(").string("Enum Wrapping cannot find named struct {} property {}").add(", nodes[start_index].id, nodes[*index].id);");
                    tb.add("                                ApplyValue::skip_value(index, nodes);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum Wrapping cannot find named struct {}").add(", nodes[start_index].id);");
            tb.add("                        ApplyValue::skip_value(index, nodes);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            tb.add("            ApplyValue::Call{target, ..}=>{");
            tb.add("                match target{");
            
            for item in &items{
                if let EnumKind::Tuple(args) = &item.kind{
                    tb.add("            Id(").suf_u64(Id::from_str(&item.name).unwrap().0).add(")=>{");
                    
                    tb.add("                if let Self::").ident(&item.name).add("{..} = self{}");
                    tb.add("                else{*self = ");
                    item.gen_new(&mut tb);
                    tb.add("                }");
                    
                    tb.add("                if let Self::").ident(&item.name).add("(");
                    for i in 0..args.len(){
                        tb.ident(&format!("var{}",i)).add(",");
                    }
                    tb.add("                ) = self{");
                    tb.add("                    *index += 1;"); // skip the class
                    tb.add("                    loop{");
                    tb.add("                        if nodes[*index].value.is_close(){");
                    tb.add("                            *index += 1;"); 
                    tb.add("                            break;");
                    tb.add("                        }");
                    tb.add("                        let arg = *index - start_index - 1;");
                    tb.add("                        match arg{");
                    for i in 0..args.len(){
                        tb.add("                        ").unsuf_usize(i).add("=>(*").ident(&format!("var{}",i)).add(").live_apply(cx, index, nodes),");
                    }
                    tb.add("                            _=>{");
                    tb.add("                                println!(").string("Enum Wrapping cannot find tuple struct {} arg {}").add(", nodes[start_index].id, arg);");
                    tb.add("                                ApplyValue::skip_value(index, nodes);");
                    tb.add("                            }");
                    tb.add("                        }");
                    tb.add("                    }");
                    tb.add("                }");
                    tb.add("            }");
                }
            }
            tb.add("                    _=>{");
            tb.add("                        println!(").string("Enum Wrapping cannot find tuple struct {}").add(", nodes[start_index].id);");
            tb.add("                        ApplyValue::skip_value(index, nodes);");
            tb.add("                    }");
            tb.add("                }");
            tb.add("            }");
            tb.add("            _=>{");
            tb.add("               ApplyValue::skip_value(index, nodes);");
            tb.add("            }");
            tb.add("        }");
            tb.add("        self.after_live_apply(cx, start_index, nodes);");
            tb.add("    }");
            
            tb.add("}");
            
            //tb.eprint();
            return tb.end();
        }
    }
    return parser.unexpected()
}