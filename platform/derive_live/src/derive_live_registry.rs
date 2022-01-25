use proc_macro::{TokenStream};

use crate::macro_lib::*;

pub fn derive_live_registry_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);
    let main_attribs = parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            
            let attr = main_attribs.iter().find( | attr | attr.name == "generate_registry");
            if attr.is_none() || attr.unwrap().args.is_none() {
                return error("need generate_history attribute")
            }
            let mut parser = TokenParser::new(attr.unwrap().args.clone().unwrap());
            let registry = parser.eat_any_ident();
            parser.eat_punct_alone(',');
            let component = parser.eat_any_ident();
            parser.eat_punct_alone(',');
            let factory = parser.eat_any_ident();
            parser.eat_punct_alone(',');
            if registry.is_none() || component.is_none() || factory.is_none() {
                return error("generate_history needs (registry,component,factory) args")
            }
            let registry = registry.unwrap();
            let component = component.unwrap();
            let factory = factory.unwrap();
            
            tb.add("pub struct RegItem {");
            tb.add("    live_ptr: Option<LivePtr>,");
            tb.add("    factory: Box<dyn ").ident(&factory).add(" >,");
            tb.add("    id: LiveId,");
            tb.add("    live_type_info: LiveTypeInfo");
            tb.add("}");
            tb.add("pub struct ").ident(&registry).add(" {");
            tb.add("    items: std::collections::HashMap<TypeId, RegItem>");
            tb.add("}");
            tb.add("impl CxRegistryNew for ").ident(&registry).add("{");
            tb.add("    fn new() -> Self {");
            tb.add("        Self {");
            tb.add("            items: std::collections::HashMap::new()");
            tb.add("        }");
            tb.add("    }");
            tb.add("}");
            tb.add("impl ").ident(&registry).add(" {");
            tb.add("    pub fn register(&mut self, live_type_info: LiveTypeInfo, factory: Box<dyn ").ident(&factory).add(">, id: LiveId) {");
            tb.add("        self.items.insert(live_type_info.live_type, RegItem {");
            tb.add("            live_ptr: None,");
            tb.add("            factory,");
            tb.add("            live_type_info,");
            tb.add("            id,");
            tb.add("        });");
            tb.add("    }");

            tb.add("    pub fn apply(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode], obj:&mut (dyn ").ident(&component).add(" + 'static)){");
            tb.add("         if let Some(reg_item) = self.items.get(&obj.type_id()){");
            tb.add("            live_traits::from_ptr_impl(cx, reg_item.live_ptr.unwrap(),|cx, file_id, index, nodes| obj.apply(cx, ApplyFrom::UpdateFromDoc {file_id}, index, nodes));");
            tb.add("         }");
            tb.add("    }");
            
            tb.add("    pub fn new(&self, cx: &mut Cx, live_type: LiveType) -> Option<Box<dyn ").ident(&component).add(" >> {");
            tb.add("        self.items.get(&live_type)");
            tb.add("            .and_then( | reg_item | Some({");
            tb.add("                let mut ret = reg_item.factory.new(cx);if reg_item.live_ptr.is_none(){panic!(\"Component liveptr is none, did you include the registry in the main DSL flow?\");};");
            tb.add("                live_traits::from_ptr_impl(cx, reg_item.live_ptr.unwrap(),|cx, file_id, index, nodes| ret.apply(cx, ApplyFrom::NewFromDoc {file_id}, index, nodes));");
            tb.add("                ret");
            tb.add("             }))");
            tb.add("    }");
            tb.add("}");
            
            
            tb.add("impl LiveNew for").ident(&struct_name).add("{");
            tb.add("    fn new(_cx: &mut Cx) -> Self {");
            tb.add("        return Self ()");
            tb.add("    }");
            tb.add("    fn live_register(_cx: &mut Cx) {}");
            tb.add("    fn live_type_info(cx: &mut Cx) -> LiveTypeInfo {");
            tb.add("        let registry = cx.registries.get_or_create::<").ident(&registry).add(">();");
            tb.add("        let mut fields = Vec::new();");
            tb.add("        for item in registry.items.values() {");
            tb.add("            fields.push(LiveTypeField {");
            tb.add("                id: item.id,");
            tb.add("                live_type_info: item.live_type_info.clone(),");
            tb.add("                live_field_kind: LiveFieldKind::Live");
            tb.add("            });");
            tb.add("        }");
            tb.add("        LiveTypeInfo {");
            tb.add("            live_type: LiveType::of::<Self>(),");
            tb.add("            type_name: LiveId::from_str(").string(&struct_name).add(").unwrap(),");
            tb.add("            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),");
            tb.add("            fields");
            tb.add("        }");
            tb.add("    }");
            tb.add("}");
            tb.add(" impl LiveApply for ").ident(&struct_name).add(" {");
            tb.add("     fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {");
            tb.add("         if let Some(file_id) = apply_from.file_id() {");
            tb.add("             let mut registry = cx.registries.get_or_create::<").ident(&registry).add(">();");
            tb.add("             let generation = cx.live_registry.borrow().file_id_to_file(file_id).generation;");
            tb.add("             for item in registry.items.values_mut() {");
            tb.add("                 let index = nodes.child_by_name(index, item.id).expect(\"Registry item found, but child node not. Make sure the registry is live_register called last otherwise the DSL nodes aren't populated\");");
            tb.add("                 item.live_ptr = Some(LivePtr {file_id, index: index as u32, generation})");
            tb.add("             }");
            tb.add("         }");
            tb.add("         nodes.skip_node(index)");
            tb.add("     }");
            tb.add(" }");
            return tb.end();
        }
    }
    return parser.unexpected()
}

