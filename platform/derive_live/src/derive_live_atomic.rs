use proc_macro::{TokenStream};

use makepad_macro_lib::{
    TokenBuilder,
    TokenParser,
    error_result,
    Attribute
};
use makepad_live_id::*;

pub fn derive_live_atomic_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    if let Err(err) = derive_live_atomic_impl_inner(&mut parser, &mut tb) {
        return err
    }
    else {
        tb.end()
    }
    
}
fn derive_live_atomic_impl_inner(parser: &mut TokenParser, tb: &mut TokenBuilder) -> Result<(), TokenStream> {
    
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
        
        for field in &mut fields {
            if field.attrs.len() == 0 { // insert a default
                field.attrs.push(Attribute {name: "live".to_string(), args: None});
            }
        }
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveAtomicValue for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn apply_value_atomic(&self, cx: &mut Cx, apply_from:ApplyFrom, index:usize, nodes:&[LiveNode]) -> usize{");
        tb.add("        match nodes[index].id {");
        
        for field in &fields {
            if field.attrs[0].name == "live" {
                tb.add("    LiveId(").suf_u64(LiveId::from_str(&field.name).unwrap().0).add(")=>self.").ident(&field.name).add(".apply_atomic(cx, apply_from, index, nodes),");
            }
        }
        tb.add("            _=> {");
        tb.add("                cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);");
        tb.add("                nodes.skip_node(index)");
        tb.add("             }");
        tb.add("        }");
        tb.add("    }");
        
        tb.add("}");

        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveAtomic for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");

        tb.add("    fn apply_atomic(&self, cx: &mut Cx, apply_from:ApplyFrom, start_index: usize, nodes: &[LiveNode])->usize {");
        tb.add("        let index = start_index;");
        tb.add("        let struct_id = LiveId(").suf_u64(LiveId::from_str(&struct_name).unwrap().0).add(");");
        tb.add("        if !nodes[start_index].value.is_structy_type(){");
        tb.add("            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, struct_id);");
        tb.add("            return nodes.skip_node(start_index);");
        tb.add("        }");
        
        tb.add("        let mut index = start_index + 1;"); // skip the class
        tb.add("        loop{");
        tb.add("            if nodes[index].value.is_close(){");
        tb.add("                index += 1;");
        tb.add("                break;");
        tb.add("            }");
        tb.add("            index = self.apply_value_atomic(cx, apply_from, index, nodes);");
        tb.add("        }");
        tb.add("        index");
        tb.add("    }");
        tb.add("}");
        if main_attribs.iter().any( | attr | attr.name == "live_debug") {
            tb.eprint();
        }
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
        }
        
        let mut items = Vec::new();
        /*
        let is_u32_enum = main_attribs.iter().find( | attr | attr.name == "repr" && attr.args.as_ref().unwrap().to_string().to_lowercase() == "u32").is_some();
        
        if !is_u32_enum{
            return error_result("LiveAtomic enums must always be repr(u32)")
        }
        */
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
                if let Some(_) = parser.eat_all_types() {
                    return error_result("For atomic enums only bare values are supported");
                }
                else if let Some(_) = parser.eat_all_struct_fields() { // named variant
                    return error_result("For atomic enums only bare values are supported");
                }
                else if parser.is_punct_alone(',') || parser.is_eot() { // bare variant
                    items.push(EnumItem {name})
                }
                else {
                    return error_result("unexpected whilst parsing enum")
                }
            }
            parser.eat_punct_alone(',');
        }
        
        if pick.is_none() {
            return error_result(&format!("Enum needs atleast one field marked pick"));
        }
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveAtomicU32Enum for").ident(&enum_name).stream(generic).stream(where_clause).add("{");
        //tb.add("    fn type_id(&self)->std::any::TypeId{ std::any::TypeId::of::<Self>() }");
        tb.add("    fn as_u32(&self) -> u32 {");
        
        
        tb.add("        match(self){");
        for (index,item) in items.iter().enumerate() {
            tb.add("        Self::").ident(&item.name).add("=>").unsuf_usize(index).add(",");
        }
        tb.add("        }");
        tb.add("    }");
        
        tb.add("    fn from_u32(val:u32) -> Self {");
        tb.add("        match(val){");
        for (index,item) in items.iter().enumerate() {
            tb.add("        ").unsuf_usize(index).add("=>Self::").ident(&item.name).add(",");
        }
        tb.add("        _=>panic!(").string("Invalid u32 for enum, should be impossible").add(")");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        
        Ok(())
    }
    else {
        error_result("Not enum or struct")
    }
    
}


