use proc_macro::TokenStream;
use crate::macro_lib::*;
use crate::live_id::*;
use crate::colorhex::*;

fn parse_array(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
    while !parser.eat_eot(){
        let mut prop_id_ts = TokenBuilder::new();
        prop_id_ts.add("LiveId(0)");
        parse_value(prop_id_ts.end(), parser, tb)?;
        parser.eat_punct_alone(',');
    }
    Ok(())
}

fn parse_object(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
    while !parser.eat_eot(){
        let prop_id_ts = if parser.is_bracket(){ // computed id
            parser.open_group();
            parser.eat_level()
        }
        else{
            let prop = parser.expect_any_ident()?;
            let prop_id = LiveId::from_str(&prop).unwrap();
            let mut prop_id_ts = TokenBuilder::new();
            prop_id_ts.add("LiveId(").suf_u64(prop_id.0).add(")");
            prop_id_ts.end()
        };
        if parser.eat_punct_alone(':'){ // value is following
            parse_value(prop_id_ts, parser, tb)?;
        }
        else if parser.is_brace(){  // its an inline class
            parser.open_group();
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:LiveId(0),value:LiveValue::Class{");
            tb.add("class:").stream(Some(prop_id_ts)).add("}},");
            parse_object(parser,tb)?;
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:LiveId(0),value:LiveValue::Close},");
        }
        else{
            return Err(parser.unexpected());
        }
        parser.eat_punct_alone(',');
    }
    Ok(())
}

fn parse_value(prop_id:TokenStream, parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{

    if parser.is_paren(){ // its a Rust injection
        parser.open_group();
        let arg = parser.eat_level();
        tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:(").stream(Some(arg)).add(").to_live_value()},");
    }
    else if parser.is_bracket(){ // its an array
        tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::Array},");
        parser.open_group();
        parse_array(parser,tb)?;
        tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Close},");
    }
    else if parser.is_brace(){ // its a bare class
        tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::Object},");
        parser.open_group();
        parse_object(parser, tb)?;
        tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Close},");
    }  
    // key values
    else if let Some(class) = parser.eat_any_ident(){
        let class_id = LiveId::from_str(&class).unwrap().0;
        // could be local class or enum
        if parser.eat_double_colon_destruct(){
            let variant = parser.expect_any_ident()?;
            let variant_id = LiveId::from_str(&variant).unwrap().0;
            // now check if we have a , eot or ( or {
            if parser.is_punct_alone(',') || parser.is_eot(){
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::BareEnum{");
                tb.add("base:LiveId(").suf_u64(class_id).add("), variant:LiveId(").suf_u64(variant_id).add(")}},");
            }
            else if parser.is_brace(){
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::NamedEnum{");
                tb.add("base:LiveId(").suf_u64(class_id).add("), variant:LiveId(").suf_u64(variant_id).add(")}},");
                parser.open_group();
                while !parser.eat_eot(){
                    let prop = parser.expect_any_ident()?;
                    let prop_id = LiveId::from_str(&prop).unwrap();
                    let mut prop_id_ts = TokenBuilder::new();
                    prop_id_ts.add("LiveId(").suf_u64(prop_id.0).add(")");
                    parser.expect_punct_alone(':')?;
                    parse_value(prop_id_ts.end(), parser, tb)?;
                    parser.eat_punct_alone(',');
                }
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Close},");
            }
            else if parser.is_paren(){
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::TupleEnum{");
                tb.add("base:LiveId(").suf_u64(class_id).add("), variant:LiveId(").suf_u64(variant_id).add(")}},");
                parser.open_group();
                while !parser.eat_eot(){
                    let mut prop_id_ts = TokenBuilder::new();
                    prop_id_ts.add("LiveId(0)");
                    parse_value(prop_id_ts.end(), parser, tb)?;
                    parser.eat_punct_alone(',');
                }
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Close},");
            }
            else{
                return Err(error("Not a valid enum type"));
            }
        }
        else if parser.is_brace(){ 
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::Clone(");
            tb.add("LiveId(").suf_u64(class_id).add("))},");
            parser.open_group();
            parse_object(parser,tb)?;
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Close},");
        }
        else{
            if class == "true"{
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::Bool(true)},");
            }
            else if class == "false"{
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::Bool(false)},");
            }
            else{
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id.clone())).add(",value:LiveValue::Id(");
                tb.add("LiveId(").suf_u64(class_id).add("))},");
            }
        }
    }
    else if parser.eat_punct_alone('#'){ // coLor!
        // ok we now eat an ident
        let color = parser.expect_any_ident()?;
        let bytes = color.as_bytes();
        let val = if bytes[0] == 'x' as u8{
            hex_bytes_to_u32(&bytes[1..])
        }
        else{
            hex_bytes_to_u32(bytes)
        };
        if let Ok(val) = val{
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Color(").suf_u32(val).add(")},");
        }
        else{
            return Err(error(&format!("Can't parse color {}", color)));
        }
    }
    else if let Some(lit) = parser.eat_literal(){
        // ok so.. bool float string or int..
        let s = lit.to_string();
        let bytes = s.as_bytes();
        if bytes[0] == '"' as u8{ // its a string
            let val = std::str::from_utf8(&bytes[1..bytes.len()-1]).unwrap();
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Str(").string(val).add(")},");
        }
        else if s == "true" || s == "false"{
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Bool(").ident(&s).add(")},");
        }
        else{
            if s.chars().position(|c| c == '.').is_some(){
                if let Ok(value) = s.parse::<f64>(){
                    tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Float(").unsuf_f64(value).add(")},");
                }
                else{
                    return Err(error("Value cant be parsed"));
                }
            }
            else if let Ok(value) = s.parse::<i64>(){
                tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:").stream(Some(prop_id)).add(",value:LiveValue::Int(").unsuf_i64(value).add(")},");
            }
            else{
                return Err(error("Value cant be parsed"));
            }
            // has to be a number int or float
        }  
    }
    Ok(())
}

pub fn live_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    tb.add("&[");
    tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:LiveId(0),value:LiveValue::Object},");
    if let Err(e) = parse_object(&mut parser, &mut tb){
        return e
    };
    tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:LiveId(0),value:LiveValue::Close},");
    tb.add("]");
    tb.end()
}

pub fn live_object_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    tb.add("&[");
    if let Err(e) = parse_object(&mut parser, &mut tb){
        return e
    };
    tb.add("]");
    tb.end()
}

pub fn live_array_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    tb.add("&[");
    if let Err(e) = parse_array(&mut parser, &mut tb){
        return e
    };
    tb.add("]");
    tb.end()
}


pub fn generate_ref_cast_api_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    if let Some(ident) = parser.eat_any_ident(){
        let mut tb = TokenBuilder::new();
        tb.add("impl dyn ").ident(&ident).add(" {");
        tb.add("    pub fn is<T: ").ident(&ident).add(" + 'static >(&self) -> bool {");
        tb.add("        let t = TypeId::of::<T>();");
        tb.add("        let concrete = self.type_id();");
        tb.add("        t == concrete");
        tb.add("    }");
        tb.add("    pub fn cast<T: ").ident(&ident).add(" + 'static >(&self) -> Option<&T> {");
        tb.add("        if self.is::<T>() {");
        tb.add("            Some(unsafe {&*(self as *const dyn ").ident(&ident).add(" as *const T)})");
        tb.add("        } else {");
        tb.add("            None");
        tb.add("        }");
        tb.add("    }");
        tb.add("    pub fn cast_mut<T: ").ident(&ident).add(" + 'static >(&mut self) -> Option<&mut T> {");
        tb.add("        if self.is::<T>() {");
        tb.add("            Some(unsafe {&mut *(self as *const dyn ").ident(&ident).add(" as *mut T)})");
        tb.add("        } else {");
        tb.add("            None");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        tb.end()
    }
    else{
        error("Expected identifier")
    }
}

pub fn generate_clone_cast_api_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    if let Some(ident) = parser.eat_any_ident(){
        let mut tb = TokenBuilder::new();
        tb.add("impl dyn ").ident(&ident).add(" {");
        tb.add("    pub fn is<T: ").ident(&ident).add(" + 'static >(&self) -> bool {");
        tb.add("        let t = TypeId::of::<T>();");
        tb.add("        let concrete = self.type_id();");
        tb.add("        t == concrete");
        tb.add("    }");
        tb.add("    pub fn cast<T: ").ident(&ident).add(" + 'static + Default + Clone>(&self) -> T {");
        tb.add("        if self.is::<T>() {");
        tb.add("            unsafe {&*(self as *const dyn ").ident(&ident).add(" as *const T)}.clone()");
        tb.add("        } else {");
        tb.add("            T::default()");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        tb.end()
    }
    else{
        error("Expected identifier")
    }
}
