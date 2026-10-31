use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser};

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
            } else {
                None
            };

            tb.add("impl").stream(generic.clone());
            tb.add("FromWasm for")
                .ident(&name)
                .stream(generic)
                .stream(where_clause);
            tb.add("{");

            tb.add("    fn type_name()->&'static str{")
                .string(&name)
                .add("}");
            tb.add("    fn live_id()->LiveId{live_id!(")
                .ident(&name)
                .add(")}");

            tb.add("    fn from_wasm_inner(self ,out:&mut FromWasmMsg){");

            let mut js = TokenBuilder::new();

            js.add("    fn from_wasm_js_body(out: &mut WasmJSOutput, slot:usize, is_recur: bool, prop:&str, temp:usize){");

            if let Some(types) = &types {
                js.add("    out.push_ln(slot, &format!(")
                    .string("{{if({0} === undefined){0} = [];")
                    .add(",prop));");
                js.add("    let slot = out.check_slot(slot, is_recur, prop, temp, ")
                    .string(&name)
                    .add(");if slot.is_none(){return}; let slot = slot.unwrap();");

                for (index, ty) in types.iter().enumerate() {
                    tb.add("self.")
                        .unsuf_usize(index)
                        .add(".from_wasm_inner(out);");
                    js.add("let new_temp = out.alloc_temp();");
                    js.add("<")
                        .stream(Some(ty.clone()))
                        .add(" as FromWasm>::from_wasm_js_body(out, slot, false, &format!(")
                        .string(&format!("t{{}}.{}", index))
                        .add(",temp), new_temp);");
                }
            } else if let Some(fields) = &fields {
                js.add("    out.push_ln(slot, &format!(")
                    .string("if({0} === undefined){0} = {{}};")
                    .add(",prop));");
                js.add("    let slot = out.check_slot(slot, is_recur, prop, temp, ")
                    .string(&name)
                    .add(");if slot.is_none(){return}; let slot = slot.unwrap();");
                for field in fields {
                    tb.add("self.")
                        .ident(&field.name)
                        .add(".from_wasm_inner(out);");
                    js.add("let new_temp = out.alloc_temp();");
                    js.add("<")
                        .stream(Some(field.ty.clone()))
                        .add(" as FromWasm>::from_wasm_js_body(out, slot, false, &format!(")
                        .string(&format!("t{{}}.{}", field.name))
                        .add(",temp), new_temp);");
                }
            } else {
                return parser.unexpected();
            }
            tb.add("   }");
            js.add("   }");

            tb.stream(Some(js.end()));

            tb.add("}");

            // the page's reader (host.rs FromWasmRead): the fields in writing order
            tb.add("impl FromWasmRead for").ident(&name).add("{");
            tb.add("    fn read_from_wasm(inp:&mut ToWasmMsgRef)->Self{");
            if let Some(types) = &types {
                tb.add("Self(");
                for _ in types.iter() {
                    tb.add("FromWasmRead::read_from_wasm(inp),");
                }
                tb.add(")");
            } else if let Some(fields) = &fields {
                tb.add("Self{");
                for field in fields {
                    tb.ident(&field.name).add(":FromWasmRead::read_from_wasm(inp),");
                }
                tb.add("}");
            }
            tb.add("}}");
            return tb.end();
        }
    } else if parser.eat_ident("enum") {
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("FromWasm"));
            let mut rd = TokenBuilder::new();
            rd.add("impl FromWasmRead for").ident(&name).add("{");
            rd.add("    fn read_from_wasm(inp:&mut ToWasmMsgRef)->Self{");
            rd.add("        match inp.read_u32() {");

            tb.add("impl").stream(generic.clone());
            tb.add("FromWasm for")
                .ident(&name)
                .stream(generic)
                .stream(where_clause);
            tb.add("{");

            tb.add("    fn type_name()->&'static str{")
                .string(&name)
                .add("}");
            tb.add("    fn live_id()->LiveId{LiveId::from_str_with_lut(")
                .string(&name)
                .add(")}");

            tb.add("    fn from_wasm_inner(self ,out:&mut FromWasmMsg){");
            tb.add("        match self {");

            if !parser.open_brace() {
                return parser.unexpected();
            }

            let mut js = TokenBuilder::new();

            js.add("    fn from_wasm_js_body(out: &mut WasmJSOutput, slot:usize, is_recur: bool, prop:&str, temp:usize){");
            js.add("        out.push_ln(slot, &format!(")
                .string("{0} = {{}};")
                .add(",prop));");

            js.add("        let slot = out.check_slot(slot, true, prop, temp, ")
                .string(&name)
                .add(");if slot.is_none(){return}; let slot = slot.unwrap();");
            // ok so the JS
            js.add("        out.push_ln(slot, ")
                .string("switch (app.u32[this.u32_offset++]){")
                .add(");");

            let mut index = 0;
            while !parser.eat_eot() {
                parser.eat_attributes();
                // parse ident
                if let Some(variant) = parser.eat_any_ident() {
                    js.add("out.push_ln(slot, ")
                        .string(&format!("case {}:", index))
                        .add(");");
                    js.add("out.push_ln(slot, &format!(")
                        .string(&format!("t{{}}.type=\"{}\";", &variant))
                        .add(",temp));");

                    if let Some(types) = parser.eat_all_types() {
                        tb.add("Self ::").ident(&variant).add("(");
                        for (index, ty) in types.iter().enumerate() {
                            js.add("let new_temp = out.alloc_temp();");
                            js.add("<")
                                .stream(Some(ty.clone()))
                                .add("as FromWasm>::from_wasm_js_body(out, slot, false, &format!(")
                                .string(&format!("t{{}}[{}]", index))
                                .add(",temp), new_temp);");
                            tb.ident(&format!("n{}", index)).add(",");
                        }
                        tb.add(") => {")
                            .suf_u32(index)
                            .add(".from_wasm_inner(out);");
                        for i in 0..types.len() {
                            tb.ident(&format!("n{}", i)).add(".from_wasm_inner(out);");
                        }
                        tb.add("}");
                        rd.unsuf_usize(index as usize).add("=> Self::").ident(&variant).add("(");
                        for _ in types.iter() {
                            rd.add("FromWasmRead::read_from_wasm(inp),");
                        }
                        rd.add("),");
                    } else if let Some(fields) = parser.eat_all_struct_fields() {
                        // named variant
                        tb.add("Self ::").ident(&variant).add("{");
                        for field in fields.iter() {
                            tb.ident(&field.name).add(",");

                            js.add("let new_temp = out.alloc_temp();");
                            js.add("<")
                                .stream(Some(field.ty.clone()))
                                .add("as FromWasm>::from_wasm_js_body(out, slot, false, &format!(")
                                .string(&format!("t{{}}.{}", field.name))
                                .add(",temp), new_temp);");
                        }
                        tb.add("} => {")
                            .suf_u32(index)
                            .add(".from_wasm_inner(out);");
                        rd.unsuf_usize(index as usize).add("=> Self::").ident(&variant).add("{");
                        for field in &fields {
                            tb.ident(&field.name).add(".from_wasm_inner(out);");
                            rd.ident(&field.name).add(":FromWasmRead::read_from_wasm(inp),");
                        }
                        rd.add("},");
                        tb.add("}");
                    } else if parser.is_punct_alone(',') || parser.is_eot() {
                        // bare variant
                        tb.add("Self ::").ident(&variant).add("=> {");
                        tb.suf_u32(index).add(".from_wasm_inner(out); }");
                        rd.unsuf_usize(index as usize).add("=> Self::").ident(&variant).add(",");
                    } else {
                        return parser.unexpected();
                    }
                    js.add("out.push_ln(slot, ").string("break;").add(");");

                    index += 1;
                    parser.eat_punct_alone(',');
                } else {
                    return parser.unexpected();
                }
            }
            tb.add("} }");

            js.add("out.push_ln(slot, ").string("}").add(");");
            js.add("}");

            tb.stream(Some(js.end()));

            tb.add("}");
            rd.add("_ => panic!(").string("enum variant invalid").add(")}}}");
            tb.stream(Some(rd.end()));

            return tb.end();
        }
    }
    parser.unexpected()
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
            } else {
                None
            };

            tb.add("impl").stream(generic.clone());
            tb.add("ToWasm for")
                .ident(&name)
                .stream(generic)
                .stream(where_clause);
            tb.add("{");

            tb.add("    fn type_name()->&'static str{")
                .string(&name)
                .add("}");
            tb.add("    fn live_id()->LiveId{LiveId::from_str_with_lut(")
                .string(&name)
                .add(").unwrap()}");

            let mut js = TokenBuilder::new();
            let mut sz = TokenBuilder::new();
            js.add("    fn to_wasm_js_body(out: &mut WasmJSOutput, slot:usize, is_recur: bool, prop:&str, temp:usize){");
            js.add("        let slot = out.check_slot(slot, is_recur, prop, temp, ")
                .string(&name)
                .add(");if slot.is_none(){return}; let slot = slot.unwrap();");

            tb.add("    fn read_to_wasm(inp:&mut ToWasmMsgRef)->Self{");
            sz.add("    fn u32_size()->usize{0");

            if let Some(types) = &types {
                tb.add("Self(");
                for (index, ty) in types.iter().enumerate() {
                    js.add("let new_temp = out.alloc_temp();");
                    js.stream(Some(ty.clone()))
                        .add("::to_wasm_js_body(out, slot, false, &format!(")
                        .string(&format!("t{{}}.{}", index))
                        .add(",temp), new_temp);");

                    tb.add("ToWasm::read_to_wasm(inp),");
                    sz.add("+")
                        .stream(Some(ty.clone()))
                        .add(" as ToWasm>::u32_size()");
                }
                tb.add(")");
            } else if let Some(fields) = &fields {
                tb.add("Self{");

                for field in fields {
                    js.add("let new_temp = out.alloc_temp();");
                    js.add("<")
                        .stream(Some(field.ty.clone()))
                        .add(" as ToWasm>::to_wasm_js_body(out, slot, false, &format!(")
                        .string(&format!("t{{}}.{}", field.name))
                        .add(",temp), new_temp);");

                    tb.ident(&field.name).add(":ToWasm::read_to_wasm(inp),");
                    sz.add("+ <")
                        .stream(Some(field.ty.clone()))
                        .add(" as ToWasm>::u32_size()");
                }
                tb.add("}");
            } else {
                return parser.unexpected();
            }
            tb.add("}");
            js.add("}");
            sz.add("}");

            tb.stream(Some(js.end()));
            tb.stream(Some(sz.end()));
            tb.add("}");

            // the page's writer (host.rs ToWasmWrite): the fields in reading order
            tb.add("impl ToWasmWrite for").ident(&name).add("{");
            tb.add("    fn write_to_wasm(&self, out:&mut ToWasmWriter){");
            if let Some(types) = &types {
                for index in 0..types.len() {
                    tb.add("ToWasmWrite::write_to_wasm(&self.").unsuf_usize(index).add(", out);");
                }
            } else if let Some(fields) = &fields {
                for field in fields {
                    tb.add("ToWasmWrite::write_to_wasm(&self.").ident(&field.name).add(", out);");
                }
            }
            tb.add("}}");
            return tb.end();
        }
    } else if parser.eat_ident("enum") {
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("ToWasm"));

            tb.add("impl").stream(generic.clone());
            tb.add("ToWasm for")
                .ident(&name)
                .stream(generic)
                .stream(where_clause);
            tb.add("{");

            tb.add("    fn type_name()->&'static str{")
                .string(&name)
                .add("}");
            tb.add("    fn live_id()->LiveId{live_id!(")
                .ident(&name)
                .add(")}");

            let mut js = TokenBuilder::new();
            let mut sz = TokenBuilder::new();

            tb.add("    fn read_to_wasm(inp:&mut ToWasmMsg)->Self{");
            tb.add("         match inp.read_u32(){");

            js.add("    fn to_wasm_js_body(out: &mut WasmJSOutput, slot:usize, is_recur: bool, prop:&str, temp:usize){");
            js.add("        let slot = out.check_slot(slot, true, prop, temp, ")
                .string(&name)
                .add(");if slot.is_none(){return}; let slot = slot.unwrap();");
            js.add("        out.push_ln(slot, &format!(")
                .string("switch (t{}.type){{")
                .add(",temp));");

            sz.add("    fn u32_size()->usize{ 0");
            let mut wr = TokenBuilder::new();
            wr.add("impl ToWasmWrite for").ident(&name).add("{");
            wr.add("    fn write_to_wasm(&self, out:&mut ToWasmWriter){");
            wr.add("        match self {");

            if !parser.open_brace() {
                return parser.unexpected();
            }
            let mut index = 0;
            while !parser.eat_eot() {
                // parse ident
                parser.eat_attributes();
                if let Some(variant) = parser.eat_any_ident() {
                    js.add("out.push_ln(slot,")
                        .string(&format!("case \"{}\":", variant))
                        .add(");");
                    js.add("out.push_ln(slot,")
                        .string(&format!("app.u32[this.u32_offset++] = {};", index))
                        .add(");");

                    sz.add(".max(1");
                    tb.unsuf_usize(index as usize).add("=>");
                    tb.add("Self::");
                    if let Some(types) = parser.eat_all_types() {
                        tb.ident(&variant).add("(");
                        wr.add("Self::").ident(&variant).add("(");
                        for i in 0..types.len() {
                            wr.ident(&format!("n{}", i)).add(",");
                        }
                        wr.add(") => { out.push_u32(").suf_u32(index).add(");");
                        for i in 0..types.len() {
                            wr.add("ToWasmWrite::write_to_wasm(").ident(&format!("n{}", i)).add(", out);");
                        }
                        wr.add("}");
                        for (index, ty) in types.iter().enumerate() {
                            js.add("let new_temp = out.alloc_temp();");
                            js.add("<")
                                .stream(Some(ty.clone()))
                                .add(" as ToWasm>::to_wasm_js_body(out, slot, false, &format!(")
                                .string(&format!("t{{}}[{}]", index))
                                .add(",temp), new_temp);");

                            tb.add("ToWasm::read_to_wasm(inp),");
                            sz.add("+<")
                                .stream(Some(ty.clone()))
                                .add("as ToWasm>::u32_size()");
                        }
                        tb.add(")");
                    } else if let Some(fields) = parser.eat_all_struct_fields() {
                        // named variant
                        tb.ident(&variant).add("{");
                        wr.add("Self::").ident(&variant).add("{");
                        for field in &fields {
                            wr.ident(&field.name).add(",");
                        }
                        wr.add("} => { out.push_u32(").suf_u32(index).add(");");
                        for field in &fields {
                            wr.add("ToWasmWrite::write_to_wasm(").ident(&field.name).add(", out);");
                        }
                        wr.add("}");
                        for field in fields {
                            js.add("let new_temp = out.alloc_temp();");
                            js.add("<")
                                .stream(Some(field.ty.clone()))
                                .add(" as ToWasm>::to_wasm_js_body(out, slot, false, &format!(")
                                .string(&format!("t{{}}.{}", field.name))
                                .add(",temp), new_temp);");

                            tb.ident(&field.name).add(":ToWasm::read_to_wasm(inp),");

                            sz.add("+<")
                                .stream(Some(field.ty.clone()))
                                .add(" as ToWasm>::u32_size()");
                        }
                        tb.add("}");
                    } else if parser.is_punct_alone(',') || parser.is_eot() {
                        // bare variant
                        tb.ident(&variant);
                        wr.add("Self::").ident(&variant).add("=> out.push_u32(").suf_u32(index).add("),");
                    } else {
                        return parser.unexpected();
                    }
                    js.add("out.push_ln(slot,").string("break;").add(");");
                    sz.add(")");

                    tb.add(",");
                    index += 1;
                    parser.eat_punct_alone(',');
                } else {
                    return parser.unexpected();
                }
            }
            tb.add("_ => panic!(")
                .string("enum variant invalid")
                .add(")}");
            js.add("out.push_ln(slot, ").string("}").add(");");
            js.add("}");
            sz.add("}");
            tb.add("}");
            tb.stream(Some(js.end()));
            tb.stream(Some(sz.end()));
            tb.add("}");
            wr.add("}}}");
            tb.stream(Some(wr.end()));
            return tb.end();
        }
    }
    parser.unexpected()
}
