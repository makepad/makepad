use proc_macro::{TokenStream};

use makepad_macro_lib::{TokenBuilder, TokenParser, type_to_static_callable};

pub fn derive_from_wasm_impl(input: TokenStream) -> TokenStream {
    
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    
    parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(name) = parser.eat_any_ident() {
            
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("FromWasm"));
            let fields = if types.is_none() {
                parser.eat_all_struct_fields()
            }
            else {None};
            
            tb.add("impl").stream(generic.clone());
            tb.add("FromWasm for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{");
            
            tb.add("    fn type_name()->&'static str{").string(&name).add("}");
            tb.add("    fn live_id()->LiveId{id!(").ident(&name).add(")}");
            
            tb.add("    fn from_wasm_inner(&self ,out:&mut FromWasmMsg){");
            
            let mut js = TokenBuilder::new();
            
            js.add("    fn from_wasm_js_body(out:&mut String, prop:&str){");
            if let Some(types) = &types {
                js.add("    out.push_str(&format!(").string("if({0} == undefined){0} = [];\n").add(",prop));");
                for (index, ty) in types.iter().enumerate() {
                    tb.add("self.").unsuf_usize(index).add(".from_wasm_inner(out);");
                    let ty = type_to_static_callable(ty.clone());
                    js.stream(Some(ty)).add("::from_wasm_js_body(out, &format!(").string(&format!("{{}}.{}", index)).add(",prop));");
                }
            }
            else if let Some(fields) = &fields {
                js.add("    out.push_str(&format!(").string("if({0} == undefined){0} = {{}};\n").add(",prop));");
                for field in fields {
                    tb.add("self.").ident(&field.name).add(".from_wasm_inner(out);");
                    let ty = type_to_static_callable(field.ty.clone());
                    js.stream(Some(ty)).add("::from_wasm_js_body(out, &format!(").string(&format!("{{}}.{}", field.name)).add(",prop));");
                }
            }
            else {
                return parser.unexpected()
            }
            tb.add("   }");
            js.add("   }");
            
            tb.stream(Some(js.end()));
            
            tb.add("};");
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("SerBin"));
            
            tb.add("impl").stream(generic.clone());
            tb.add("FromWasm for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{");
            
            tb.add("    fn type_name()->&'static str{").string(&name).add("}");
            tb.add("    fn live_id()->LiveId{id!(").ident(&name).add(")}");
            
            tb.add("    fn from_wasm_inner(&self ,out:&mut FromWasmMsg){");
            tb.add("        match self {");
            
            if !parser.open_brace() {
                return parser.unexpected()
            }
            
            let mut js = TokenBuilder::new();
            
            js.add("    fn from_wasm_js_body(out:&mut String, prop:&str){");
            
            // ok so the JS
            js.add("        out.push_str(&format!(").string("{0} = {{}};\n").add(",prop));");
            js.add("        out.push_str(").string("switch (app.u32[this.u32_offset++]){\n").add(");");
            
            let mut index = 0;
            while !parser.eat_eot() {
                parser.eat_attributes();
                // parse ident
                if let Some(variant) = parser.eat_any_ident() {
                    js.add("out.push_str(").string(&format!("case {}:\n", index)).add(");");
                    js.add("out.push_str(&format!(").string(&format!("{{}}.type=\"{}\"\n;", &variant)).add(",prop));");
                    
                    if let Some(types) = parser.eat_all_types() {
                        
                        tb.add("Self ::").ident(&variant).add("(");
                        for (index, ty) in types.iter().enumerate() {
                            let ty = type_to_static_callable(ty.clone());
                            js.stream(Some(ty)).add("::from_wasm_js_body(out, &format!(").string(&format!("{{}}[{}]", index)).add(",prop));");
                            tb.ident(&format!("n{}", index)).add(",");
                        }
                        tb.add(") => {").suf_u32(index).add(".from_wasm_inner(out);");
                        for i in 0..types.len() {
                            tb.ident(&format!("n{}", i)).add(".from_wasm_inner(out);");
                        }
                        tb.add("}");
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                        tb.add("Self ::").ident(&variant).add("{");
                        for field in fields.iter() {
                            tb.ident(&field.name).add(",");
                            
                            let ty = type_to_static_callable(field.ty.clone());
                            js.stream(Some(ty)).add("::from_wasm_js_body(out, &format!(").string(&format!("{{}}.{}", field.name)).add(",prop));");
                        }
                        tb.add("} => {").suf_u32(index).add(".from_wasm_inner(out);");
                        for field in fields {
                            tb.ident(&field.name).add(".from_wasm_inner(out);");
                        }
                        tb.add("}");
                    }
                    else if parser.is_punct_alone(',') || parser.is_eot() { // bare variant
                        tb.add("Self ::").ident(&variant).add("=> {");
                        tb.suf_u32(index).add(".from_wasm_inner(out); }");
                    }
                    else {
                        return parser.unexpected();
                    }
                    js.add("out.push_str(").string("break;").add(");");
                    
                    index += 1;
                    parser.eat_punct_alone(',');
                }
                else {
                    return parser.unexpected()
                }
            }
            tb.add("} }");
            
            js.add("out.push_str(").string("}").add(");");
            js.add("}");
            
            tb.stream(Some(js.end()));
            
            tb.add("};");
            
            return tb.end();
        }
    }
    return parser.unexpected()
}


pub fn derive_to_wasm_impl(input: TokenStream) -> TokenStream {
    
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    
    parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(name) = parser.eat_any_ident() {
            
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("ToWasm"));
            let fields = if types.is_none() {
                parser.eat_all_struct_fields()
            }
            else {None};
            
            tb.add("impl").stream(generic.clone());
            tb.add("ToWasm for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{");
            
            tb.add("    fn type_name()->&'static str{").string(&name).add("}");
            tb.add("    fn live_id()->LiveId{id!(").ident(&name).add(")}");
            
            let mut js = TokenBuilder::new();
            let mut sz = TokenBuilder::new();
            js.add("    fn to_wasm_js_body(out:&mut String, prop:&str){");
            
            tb.add("    fn to_wasm(inp:&mut ToWasmMsg)->Self{");
            sz.add("    fn u32_size()->usize{0");
            
            if let Some(types) = &types {
                tb.add("Self(");
                for (index, ty) in types.iter().enumerate() {
                    let ty = type_to_static_callable(ty.clone());
                    js.stream(Some(ty.clone())).add("::to_wasm_js_body(out, &format!(").string(&format!("{{}}.{}", index)).add(",prop));");
                    
                    tb.add("ToWasm::to_wasm(inp),");
                    sz.add("+").stream(Some(ty)).add("::u32_size()");
                }
                tb.add(")");
            }
            else if let Some(fields) = &fields {
                tb.add("Self{");
           
                for field in fields {
                    
                    let ty = type_to_static_callable(field.ty.clone());
                    
                    js.stream(Some(ty.clone())).add("::to_wasm_js_body(out, &format!(").string(&format!("{{}}.{}", field.name)).add(",prop));");
                    
                    tb.ident(&field.name).add(":ToWasm::to_wasm(inp),");
                    sz.add("+").stream(Some(ty)).add("::u32_size()");
                }
                tb.add("}");
            }
            else {
                return parser.unexpected()
            }
            tb.add("}");
            js.add("}");
            sz.add("}");
            
            tb.stream(Some(js.end()));
            tb.stream(Some(sz.end()));
            tb.add("};");
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("ToWasm"));
            
            tb.add("impl").stream(generic.clone());
            tb.add("ToWasm for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{");
            
            tb.add("    fn type_name()->&'static str{").string(&name).add("}");
            tb.add("    fn live_id()->LiveId{id!(").ident(&name).add(")}");
            
            let mut tw = TokenBuilder::new();
            let mut js = TokenBuilder::new();
            let mut sz = TokenBuilder::new();

            tw.add("    fn to_wasm(inp:&mut ToWasmMsg)->Self{");
            tw.add("         match inp.read_u32(){");

            js.add("    fn to_wasm_js_body(out:&mut String, prop:&str){");
            js.add("        out.push_str(&format!(").string("switch ({}.type){{\n").add(",prop));");

            sz.add("    fn u32_size()->usize{ 0");
            
            if !parser.open_brace() {
                return parser.unexpected()
            }
            let mut index = 0;
            while !parser.eat_eot() {
                // parse ident
                parser.eat_attributes();
                if let Some(variant) = parser.eat_any_ident() {
                    js.add("out.push_str(").string(&format!("case \"{}\":\n", variant)).add(");");
                    js.add("out.push_str(").string(&format!("app.u32[this.u32_offset++] = {}\n;", index)).add(");");

                    sz.add(".max(1");
                    tw.unsuf_usize(index as usize).add("=>");
                    tw.add("Self::");
                    if let Some(types) = parser.eat_all_types() {
                        
                        tw.ident(&variant).add("(");
                        for (index, ty) in types.iter().enumerate() {
                            let ty = type_to_static_callable(ty.clone());
                            js.stream(Some(ty.clone())).add("::to_wasm_js_body(out, &format!(").string(&format!("{{}}[{}]", index)).add(",prop));");

                            tw.add("ToWasm::to_wasm(inp),");
                            sz.add("+").stream(Some(ty)).add("::u32_size()");
                        }
                        tw.add(")");
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                        tw.ident(&variant).add("{");
                        for field in fields {
                            let ty = type_to_static_callable(field.ty.clone());
                            js.stream(Some(ty.clone())).add("::to_wasm_js_body(out, &format!(").string(&format!("{{}}.{}", field.name)).add(",prop));");
                            
                            tw.ident(&field.name).add(":ToWasm::to_wasm(inp),");
                            
                            sz.add("+").stream(Some(ty)).add("::u32_size()");
                        }
                        tw.add("}");
                    }
                    else if parser.is_punct_alone(',') || parser.is_eot() { // bare variant
                        tw.ident(&variant);
                    }
                    else {
                        return parser.unexpected();
                    }
                    js.add("out.push_str(").string("break;\n").add(");");
                    sz.add(")");
                    
                    tw.add(",");
                    index += 1;
                    parser.eat_punct_alone(',');
                }
                else {
                    return parser.unexpected()
                }
            }
            tw.add("_ => panic!(").string("enum variant invalid").add(")}");
            js.add("out.push_str(").string("}").add(");");
            js.add("}");
            sz.add("}");
            tw.add("}");
            tb.stream(Some(tw.end()));
            tb.stream(Some(js.end()));
            tb.stream(Some(sz.end()));
            tb.add("}");
            return tb.end();
        }
    }
    // ok so. JS.
    /*
    match prop.type{
        case "Variant":
            app.u32[this.u32_offset++] = 0
            this.reserve_u32() // summed size of all the types
            .. the rest
            break
        default:{
            throw new Error("Invalid variant for type")
        }
    }
    */
    return parser.unexpected()
}
