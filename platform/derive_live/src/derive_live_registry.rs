use proc_macro::{TokenStream};

use crate::macro_lib::*;

pub fn derive_live_component_registry_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let _types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(None); //Some("LiveUpdateHooks"));
            if !struct_name.ends_with("Registry"){
                return error("Please use ComponentTraitRegistry as a naming scheme")
            }
            let trait_name = struct_name.replace("Registry","");

            tb.add("impl LiveComponentRegistry for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    fn type_id(&self) -> LiveType {LiveType::of::<").ident(&struct_name).add(">()}");
            
            tb.add("    fn component_type(&self) -> LiveId {id!(").ident(&trait_name).add(")}");
            tb.add("    fn get_module_set(&self, set: &mut std::collections::BTreeSet<LiveModuleId>){");
            tb.add("        self.map.values().for_each( | (info, _) | {set.insert(info.module_id);});");
            tb.add("    }");
            tb.add("    fn get_component_info(&self, name: LiveId) -> Option<LiveComponentInfo> {");
            tb.add("        self.map.values().find( | (info, _) | info.name == name).map( | (info, _) | info.clone())");
            tb.add("    }");
            tb.add("}");
            
            tb.add("impl ").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
            tb.add("    pub fn new(&self, cx: &mut Cx, ty: LiveType) -> Option<Box<dyn ").ident(&trait_name).add(" >> {");
            tb.add("        self.map.get(&ty).map( | (_, fac) | fac.new(cx))");
            tb.add("    }");
            tb.add("    pub fn new_and_apply_origin(&self, cx: &mut Cx, ty: LiveType) -> Option<Box<dyn ").ident(&trait_name).add(" >> {");
            tb.add("        self.map.get(&ty).map( | (info, fac) | {");
            tb.add("            let mut ret = fac.new(cx);");
            tb.add("            let live_ptr = cx.live_registry.borrow().module_id_and_name_to_ptr(info.module_id, info.name).unwrap();");
            tb.add("            live_traits::from_ptr_impl(cx, live_ptr, |cx, file_id, index, nodes|{");
            tb.add("                ret.apply(cx, ApplyFrom::NewFromDoc {file_id}, index, nodes)");
            tb.add("            });");
           tb.add("             ret");
            tb.add("        })");
            tb.add("    }");

            tb.add("}");
            return tb.end();
        }
    }
    return parser.unexpected()
}

