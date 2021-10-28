use proc_macro::{TokenStream};

use crate::macro_lib::*;
use crate::id::*;

pub fn derive_live_update_hooks_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            tb.add("impl LiveUpdateHooks for").ident(&struct_name).add(" {");
            tb.add("    fn live_update_value_unknown(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {");
            tb.add("    }");
            tb.add("    fn before_live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {");
            tb.add("    }");
            tb.add("    fn after_live_update(&mut self, cx: &mut Cx, _live_ptr: LivePtr) {");
            tb.add("    }");
            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_live_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    let _main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None);//Some("LiveUpdateHooks"));
            
            let mut ln = TokenBuilder::new();
            let mut lf = TokenBuilder::new();
            let mut lu = TokenBuilder::new();
            if let Some(_types) = types {
                return parser.unexpected();
            }
            else if let Some(fields) = parser.eat_all_struct_fields() {
                let deref_target =  fields.iter().find(|field| field.name == "deref_target");
                
                lf.add("fn live_fields(&self, fields: &mut Vec<LiveField>) {");
                lu.add("fn live_update_value(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {");
                lu.add("match id {");
                
                ln.add("fn live_new(cx: &mut Cx) -> Self {");
                ln.add("    Self {");
                // alright now. we have a field
                for field in &fields{
                    ln.ident(&field.name).add(":");
                    let mut attr_found = false;
                    
                    // ok check what our options are.
                    for attr in &field.attrs{
                        if attr.name == "hidden"{
                            if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty(){
                                ln.add("Default::default()");
                            }
                            else{
                                ln.stream(attr.args.clone());
                            }
                            ln.add(",");
                            attr_found = true;
                            break;
                        }
                        else if attr.name == "live" || attr.name == "local"{

                            lf.add("fields.push(LiveField{id:Id::from_str(").string(&field.name).add(").unwrap()");
                            lf.add(", live_type:").stream(Some(field.ty.clone())).add("::live_type()");
                            
                            if attr.name == "live"{
                                lf.add(", field_type: LiveFieldType::Live");
                                lu.add("Id(").suf_u64(Id::from_str(&field.name).unwrap().0).add(")=>self.").ident(&field.name).add(".live_update(cx, ptr),");
                            }
                            else if attr.name == "local"{
                                lf.add(", field_type: LiveFieldType::Local");
                            }
                            
                            lf.add("});");
                            
                            if attr.args.is_none () || attr.args.as_ref().unwrap().is_empty(){
                                if attr.name == "live"{
                                    ln.add("LiveNew::live_new(cx)");
                                }
                                else{
                                    ln.add("Default::default()");
                                }
                            }
                            else{
                                ln.stream(attr.args.clone());
                            }
                            ln.add(",");
                            attr_found = true;
                            break;
                        }
                    }
                    if !attr_found{
                        return error(&format!("Field {} does not have a live or local attribute", field.name));
                    }
                }
                
                if let Some(deref_target) = deref_target{
                    lu.add(" _=> self.deref_target.live_update_value(cx, id, ptr)");
                    
                    // just forward the hooks
                    tb.add("impl").stream(generic.clone());
                    tb.add("LiveUpdateHooks for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
                    tb.add("    fn live_update_value_unknown(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {");
                    tb.add("        self.deref_target.live_update_value_unknown(cx, id, ptr);");
                    tb.add("    }");
                    
                    tb.add("    fn before_live_update(&mut self, cx:&mut Cx, live_ptr: LivePtr){");
                    tb.add("        self.deref_target.before_live_update(cx, live_ptr);");
                    tb.add("    }");
                    
                    tb.add("    fn after_live_update(&mut self, cx: &mut Cx, live_ptr:LivePtr) {");
                    tb.add("        self.deref_target.after_live_update(cx, live_ptr);");
                    tb.add("    }");
                    tb.add("}");

                    // just forward the hooks
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
                else{
                    lu.add(" _=> self.live_update_value_unknown(cx, id, ptr)");
                }
                
                lu.add("} }");
                ln.add("    }");
                ln.add("}");
                lf.add("}");
            }
            else{
                return parser.unexpected();
            }

            tb.add("impl").stream(generic.clone());
            tb.add("LiveUpdateValue for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.stream(Some(lu.end()));
            tb.add("}");

            tb.add("impl").stream(generic.clone());
            tb.add("LiveUpdate for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {");
            tb.add("        self.before_live_update(cx, live_ptr);");
            tb.add("        if let Some(mut iter) = cx.shader_registry.live_registry.live_class_iterator(live_ptr) {");
            tb.add("            while let Some((id, live_ptr)) = iter.next(&cx.shader_registry.live_registry) {");
            tb.add("                if id == id!(rust_type) && !cx.verify_type_signature(live_ptr, Self::live_type()) {");
            tb.add("                    return;");
            tb.add("                 }");
            tb.add("                self.live_update_value(cx, id, live_ptr)");
            tb.add("            }");
            tb.add("        }");
            tb.add("        self.after_live_update(cx, live_ptr);");
            tb.add("    }");
            tb.add("    fn _live_type(&self) -> LiveType {");
            tb.add("        Self::live_type()");
            tb.add("    }");
            tb.add("}");

            tb.add("impl").stream(generic.clone());
            tb.add("LiveNew for").ident(&struct_name).stream(generic).stream(where_clause).add("{");
            tb.add("    fn live_type() -> LiveType {");
            tb.add("        LiveType(std::any::TypeId::of::<").ident(&struct_name).add(">())");
            tb.add("    }");
            tb.add("    fn live_register(cx: &mut Cx) {");
            tb.add("        cx.register_live_body(live_body());");
            tb.add("        struct Factory();");
            tb.add("        impl LiveFactory for Factory {");
            tb.add("            fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveUpdate> {");
            tb.add("                Box::new(").ident(&struct_name).add(" ::live_new(cx))");
            tb.add("            }");
            tb.add("            fn live_type(&self) -> LiveType {");
            tb.add("                ").ident(&struct_name).add(" ::live_type()");
            tb.add("            }");
            tb.stream(Some(lf.end()));
            tb.add("        }");
            tb.add("        cx.register_factory(").ident(&struct_name).add("::live_type(), Box::new(Factory()));");
            tb.add("    }");
            tb.stream(Some(ln.end()));
            tb.add("}");

            return tb.end();
        }
    }
    /*
    else if parser.eat_ident("enum") {
        
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("DeLive"));
            
            tb.add("impl").stream(generic.clone());
            tb.add("makepad_live_parser :: DeLive for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_live ( lr : & makepad_live_parser :: LiveRegistry , file : usize , level : usize , index : usize )");
            tb.add("-> std :: result :: Result < Self , makepad_live_parser :: DeLiveErr > { ");
            
            if !parser.open_brace() {
                return parser.unexpected()
            }
            let mut named = Vec::new();
            let mut bare = Vec::new();
            let mut unnamed = Vec::new();
            while !parser.eat_eot() {
                
                if let Some(variant) = parser.eat_any_ident() {
                    if let Some(types) = parser.eat_all_types() {
                        unnamed.push((variant, types));
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                        named.push((variant, fields))
                    }
                    else if parser.is_punct(',') || parser.is_eot() { // bare variant
                        bare.push(variant)
                    }
                    else {
                        return parser.unexpected();
                    }
                }
                
                parser.eat_punct(',');
            }
            // alright lets write out our matcher
            tb.add("let doc = & lr . expanded [ file ] ;");
            tb.add("let cn = & doc . nodes [ level ] [ index ]  ;");
            
            tb.add("match cn . value {");
            
            if bare.len()>0 {
                tb.add("makepad_live_parser :: LiveValue :: IdPack ( id ) => {");
                tb.add("let orig_id = lr . find_enum_origin ( id , id ) ;");
                tb.add("match orig_id {");
                for variant in bare {
                    let id = Id::from_str(&variant).unwrap();
                    tb.add("IdPack (").suf_u64(id.0).add(") =>");
                    tb.add("return Ok ( Self ::").ident(&variant).add(") ,");
                }
                tb.add("_ => return Err ( makepad_live_parser :: DeLiveErr :: enum_notfound ( orig_id , cn . id_pack , file , level , index ) )");
                tb.add("}");
                tb.add("}");
            }
            
            if unnamed.len()>0 {
                tb.add("makepad_live_parser :: LiveValue :: Call { target , node_start , node_count } => {");
                tb.add("let orig_id = lr . find_enum_origin ( target , target ) ;");
                tb.add("match orig_id {");
                for (variant, types) in unnamed {
                    let id = Id::from_str(&variant).unwrap();
                    tb.add("IdPack (").suf_u64(id.0).add(") => {");
                    // ok now we need to parse the arguments
                    
                    tb.add("if node_count < ").unsuf_usize(types.len()).add("{");
                    tb.add("return Err ( makepad_live_parser :: DeLiveErr :: arg_count ( cn . id_pack , node_count as usize ,");
                    tb.unsuf_usize(types.len()).add(", file , level , index ) ) ;");
                    tb.add("}");
                    tb.add("let ln = level + 1 ;");
                    tb.add("let ns = node_start as usize ;");
                    tb.add("return std :: result :: Result :: Ok ( Self ::").ident(&variant).add("(");
                    for i in 0..types.len() {
                        tb.add("makepad_live_parser :: DeLive :: de_live ( lr , file , ln , ns +").unsuf_usize(i).add(") ? ,");
                    }
                    tb.add(") ) } ,");
                }
                tb.add("_ => return Err ( makepad_live_parser :: DeLiveErr :: enum_notfound ( orig_id , cn . id_pack , file , level , index ) )");
                tb.add("}");
                tb.add("}");
            }
            
            if named.len()>0 {
                tb.add("makepad_live_parser :: LiveValue :: Class { class , node_start , node_count } => {");
                tb.add("let orig_id = lr . find_enum_origin ( class , class ) ;");
                tb.add("match orig_id {");
                for (variant, fields) in named {
                    let id = Id::from_str(&variant).unwrap();
                    tb.add("IdPack (").suf_u64(id.0).add(") => {");
                    
                    tb.add("let ln = level + 1 ;");
                    
                    for field in &fields {
                        tb.add("let mut").ident(&format!("_{}", field.name)).add("= None ;");
                    }
                    tb.add("for i in 0 .. ( node_count as usize ) {");
                    tb.add("let si = i + ( node_start as usize ) ;");
                    tb.add("let n = & doc . nodes [ ln ] [ si ] ;");
                    tb.add("match n . id_pack {");
                    for field in &fields {
                        // lets id it
                        let id = Id::from_str(&field.name).unwrap();
                        tb.add("IdPack (").suf_u64(id.0).add(") =>");
                        tb.ident(&format!("_{}", field.name));
                        tb.add("= Some ( makepad_live_parser :: DeLive :: de_live ( lr , file , ln , si ) ? ) ,");
                    }
                    tb.add("_ => ( )");
                    tb.add("} }");
                    
                    tb.add("return std :: result :: Result :: Ok ( Self ::").ident(&variant).add("{");
                    for field in fields {
                        tb.ident(&field.name).add(":");
                        if field.ty.into_iter().next().unwrap().to_string() == "Option" {
                            tb.add("if let Some ( t ) = ").ident(&format!("_{}", field.name));
                            tb.add("{ Some ( t ) } else { None } ,");
                        }
                        else {
                            tb.add("if let Some ( t ) =").ident(&format!("_{}", field.name));
                            tb.add("{ t } else { return Err ( makepad_live_parser :: DeLiveErr :: miss_prop ( cn . id_pack ,");
                            tb.string(&field.name).add(", file , level , index ) ) } ,");
                        }
                    }
                    tb.add("} ) } ,");
                }
                tb.add("_ => return Err ( makepad_live_parser :: DeLiveErr :: enum_notfound ( orig_id , cn . id_pack , file , level , index ) )");
                tb.add("}");
                tb.add("}");
            }
            
            tb.add("_ => ( )");
            tb.add("}");
            
            tb.add("return Err ( makepad_live_parser :: DeLiveErr :: enum_notfound ( IdPack :: empty ( ) , cn . id_pack , file , level , index ) )");
            tb.add("} }");
            return tb.end();
        }
    }*/
    return parser.unexpected()
}