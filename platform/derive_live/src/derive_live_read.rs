use proc_macro::{TokenStream};

use makepad_macro_lib::{
    TokenBuilder,
    TokenParser,
    error_result,
    Attribute,
    StructField
};
use makepad_live_id::*;

pub fn derive_live_read_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    if let Err(err) = derive_live_read_impl_inner(&mut parser, &mut tb) {
        return err
    }
    else {
        tb.end()
    }
    
}

fn derive_live_read_impl_inner(parser: &mut TokenParser, tb: &mut TokenBuilder) -> Result<(), TokenStream> {
    
    let _main_attribs = parser.eat_attributes();
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
            if field.attrs.len() == 1 && field.attrs[0].name != "live" && field.attrs[0].name != "calc" && field.attrs[0].name != "rust" {
                return error_result(&format!("Field {} does not have a live, calc into or rust attribute", field.name));
            }
            if field.attrs.len() == 0 { // insert a default
                field.attrs.push(Attribute {name: "live".to_string(), args: None});
            }
        }
        
        tb.add("impl").stream(generic.clone());
        tb.add("LiveRead for").ident(&struct_name).stream(generic.clone()).stream(where_clause.clone()).add("{");
        
        tb.add("    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){");
        tb.add("        out.push(LiveNode::from_id_value(id, LiveValue::Object));");
        for field in &fields {
            if field.attrs[0].name == "live"{
                tb.add("self.").ident(&field.name).add(".live_read_to(LiveId(").suf_u64(LiveId::from_str(&field.name).unwrap().0).add("), out);");
            }
        }
        tb.add("        out.push(LiveNode::from_id_value(id, LiveValue::Close));");
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
            _attributes: Vec<Attribute>,
            kind: EnumKind
        }
        
        enum EnumKind {
            Bare,
            Named(Vec<StructField>),
            Tuple(Vec<TokenStream>)
        }
        let mut items = Vec::new();
        
        
        while !parser.eat_eot() {
            let _attributes = parser.eat_attributes();
            // check if we have a default attribute
            if let Some(name) = parser.eat_any_ident() {
                if let Some(types) = parser.eat_all_types() {
                    items.push(EnumItem {name, _attributes, kind: EnumKind::Tuple(types)})
                }
                else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                    items.push(EnumItem {name, _attributes, kind: EnumKind::Named(fields)})
                }
                else if parser.is_punct_alone(',') || parser.is_eot() { // bare variant
                    items.push(EnumItem {name, _attributes, kind: EnumKind::Bare})
                }
                else {
                    return error_result("unexpected whilst parsing enum")
                }
            }
            parser.eat_punct_alone(',');
        }
        
        tb.add("impl LiveRead for").ident(&enum_name).stream(generic).stream(where_clause).add("{");
        tb.add("    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){");
        tb.add("        match self{");
        for item in &items {
            match &item.kind{
                EnumKind::Bare=>{
                    tb.add("    Self::").ident(&item.name).add("=>{");
                    tb.add("        out.push(LiveNode::from_id_value(id, LiveValue::BareEnum{");
                    tb.add("            base: LiveId(").suf_u64(LiveId::from_str(&enum_name).unwrap().0).add("),");
                    tb.add("            variant: LiveId(").suf_u64(LiveId::from_str(&item.name).unwrap().0).add(")");
                    tb.add("        }));");
                    tb.add("    },");
                }
                EnumKind::Named(fields)=>{
                    tb.add("    Self::").ident(&item.name).add("{");
                    for field in fields {
                        tb.ident(&field.name).add(":").ident(&format!("prefix_{}", field.name)).add(",");
                    }
                    tb.add("    }=>{");
                    tb.add("        out.push(LiveNode::from_id_value(id, LiveValue::NamedEnum{");
                    tb.add("            base: LiveId(").suf_u64(LiveId::from_str(&enum_name).unwrap().0).add("),");
                    tb.add("            variant: LiveId(").suf_u64(LiveId::from_str(&item.name).unwrap().0).add(")");
                    tb.add("        }));");
                    for field in fields {
                        tb.ident(&format!("prefix_{}", field.name)).add(".live_read_to(LiveId(").suf_u64(LiveId::from_str(&field.name).unwrap().0).add(", out);");
                    }
                    tb.add("        out.push(LiveNode::from_id_value(id, LiveValue::Close));");
                    tb.add("    },");
                }
                EnumKind::Tuple(args) =>{
                    tb.add("      Self::").ident(&item.name).add("(");
                    for i in 0..args.len() {
                        tb.ident(&format!("var{}", i)).add(",");
                    }
                    tb.add("    ) =>{");
                    tb.add("        out.push(LiveNode::from_id_value(id, LiveValue::TupleEnum{");
                    tb.add("            base: LiveId(").suf_u64(LiveId::from_str(&enum_name).unwrap().0).add("),");
                    tb.add("            variant: LiveId(").suf_u64(LiveId::from_str(&item.name).unwrap().0).add(")");
                    tb.add("        }));");
                    for i in 0..args.len() {
                        tb.ident(&format!("var{}", i)).add(".live_read_to(LiveId(0), out);");
                    }
                    tb.add("        out.push(LiveNode::from_id_value(id, LiveValue::Close));");
                    tb.add("    },");
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

