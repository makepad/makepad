use proc_macro::{TokenStream};

use makepad_macro_lib::{TokenBuilder, TokenParser};

pub fn derive_from_wasm_impl(input: TokenStream) -> TokenStream {

    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    
    parser.eat_attributes();
    parser.eat_ident("pub");
    if parser.eat_ident("struct"){
        if let Some(name) = parser.eat_any_ident(){

            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("FromWasm"));
            let fields = if types.is_none(){
                parser.eat_all_struct_fields()
            }
            else {None};            
            // implement from_wasm creating the exact same structure
            // as the to wasm does
            
            tb.add("impl").stream(generic.clone());
            tb.add("FromWasm for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{");

            tb.add("    fn type_name()->&'static str{").string(&name).add("}");
            tb.add("    fn live_id()->LiveId{id!(").ident(&name).add(")}");

            tb.add("    fn from_wasm_inner(&self ,out:&mut FromWasmMsg){");

            if let Some(types) = &types{
                for i in 0..types.len(){
                     tb.add("self.").unsuf_usize(i).add(".from_wasm_inner(out);");
                }
            }
            else if let Some(fields) = &fields{
                for field in fields{
                    tb.add("self.").ident(&field.name).add(".from_wasm_inner(out);");
                }
            }
            else{
                return parser.unexpected()
            }
            tb.add("   }"); 
            
            tb.add("    fn from_wasm_js_body(out:&mut String, prop:&str){");
            if let Some(types) = &types{
                tb.add("    out.push_str(&format!(").string("if({0} == undefined){0} = [];\n").add(",prop));");
                for (index,ty) in types.iter().enumerate(){
                    tb.stream(Some(ty.clone())).add("::from_wasm_js_body(out, &format!(").string(&format!("{{}}.{}",index)).add(",prop));");
                }
            }
            else if let Some(fields) = &fields{ 
                tb.add("    out.push_str(&format!(").string("if({0} == undefined){0} = {{}};\n").add(",prop));");
                for field in fields{
                    tb.stream(Some(field.ty.clone())).add("::from_wasm_js_body(out, &format!(").string(&format!("{{}}.{}",field.name)).add(",prop));");
                }
            }
            else{
                return parser.unexpected()
            }
            
            tb.add("}");
            
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
    if parser.eat_ident("struct"){
        if let Some(name) = parser.eat_any_ident(){

            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("ToWasm"));
            let fields = if types.is_none(){
                parser.eat_all_struct_fields()
            }
            else {None};
            
            tb.add("impl").stream(generic.clone());
            tb.add("ToWasm for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{");

            tb.add("    fn type_name()->&'static str{").string(&name).add("}");
            tb.add("    fn live_id()->LiveId{id!(").ident(&name).add(")}");

            tb.add("    fn to_wasm(inp:&mut ToWasmMsg)->Self{");
            if let Some(types) = &types{
                tb.add("Self(");
                for i in 0..types.len(){
                     tb.add("ToWasm").unsuf_usize(i).add("::to_wasm(inp),");
                }
                tb.add(")");
            }
            else if let Some(fields) = &fields{ 
                tb.add("Self{");
                for field in fields{
                    tb.ident(&field.name).add(":ToWasm::to_wasm(inp),");
                }
                tb.add("}");
            }
            else{
                return parser.unexpected()
            }
            tb.add("}"); 
            
            tb.add("    fn u32_size()->usize{");
            
            if let Some(types) = &types{
                for (index,ty) in types.iter().enumerate(){
                    if index > 0{
                        tb.add("+");
                    }
                    tb.stream(Some(ty.clone())).add("::u32_size()");
                }
            }
            else if let Some(fields) = &fields{ 
                for (index, field) in fields.iter().enumerate(){
                    if index > 0{
                        tb.add("+");
                    }
                    tb.stream(Some(field.ty.clone())).add("::u32_size()");
                }
            }
            else{
                return parser.unexpected()
            }
            tb.add("}"); 
            
            tb.add("    fn to_wasm_js_body(out:&mut String, prop:&str){");
            if let Some(types) = &types{
                for (index,ty) in types.iter().enumerate(){
                    tb.stream(Some(ty.clone())).add("::to_wasm_js_body(out, &format!(").string(&format!("{{}}.{}",index)).add(",prop));");
                }
            }
            else if let Some(fields) = &fields{ 
                for field in fields{
                    tb.stream(Some(field.ty.clone())).add("::to_wasm_js_body(out, &format!(").string(&format!("{{}}.{}",field.name)).add(",prop));");
                }
            }
            else{
                return parser.unexpected()
            }
            tb.add("}"); 
            
            tb.add("};"); 
            return tb.end();
            
        }
    }
    
    return parser.unexpected()
} 
