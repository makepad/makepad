use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, error};
use makepad_live_id::*;


fn parse_array(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
    while !parser.eat_eot(){
        let mut prop = TokenBuilder::new();
        prop.add("origin:LiveNodeOrigin::empty(), id:LiveId(0)");
        parse_value(prop.end(), parser, tb)?;
        parser.eat_punct_alone(',');
    }
    Ok(())
}

fn parse_object(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
    while !parser.eat_eot(){
        
        let live_id = if parser.is_bracket(){ // computed id
            parser.open_group();
            parser.eat_level()
        }
        else{
            let mut prop = TokenBuilder::new();
            let prop_id = LiveId::from_str(&parser.expect_any_ident()?).unwrap();
            prop.add("LiveId(").suf_u64(prop_id.0).add(")");
            prop.end()
        };
        
        if parser.eat_punct_alone(':'){ // value is following
            let mut prop = TokenBuilder::new();
            prop.add("origin:LiveNodeOrigin::field(), id:");
            prop.stream(Some(live_id));
            // if we have a = its an instance assign
            parse_value(prop.end(), parser, tb)?;
        }
        else if parser.eat_punct_alone('='){
            let mut prop = TokenBuilder::new();
            prop.add("origin:LiveNodeOrigin::instance(), id:");
            prop.stream(Some(live_id));
            // if we have a = its an instance assign
            parse_value(prop.end(), parser, tb)?;
        }
        /*
        else if parser.is_brace(){  // its an inline class
            parser.open_group();
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:LiveId(0),value:LiveValue::Class{");
            tb.add("class:").stream(Some(prop_id_ts)).add("}},");
            parse_object(parser,tb)?;
            tb.add("LiveNode{origin:LiveNodeOrigin::empty(), id:LiveId(0),value:LiveValue::Close},");
        }*/
        else{
            return Err(parser.unexpected());
        }
        parser.eat_punct_alone(',');
    }
    Ok(())
}

fn parse_value(node_start:TokenStream,  parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{

    if parser.is_paren(){ // its a Rust injection
        parser.open_group();
        let arg = parser.eat_level();
        tb.add("LiveNode{").stream(Some(node_start)).add(",value:(").stream(Some(arg)).add(").to_live_value()},");
    }
    else if parser.is_bracket(){ // its an array
        tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::Array},");
        parser.open_group();
        parse_array(parser,tb)?;
        tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Close},");
    }
    else if parser.is_brace(){ // its a bare class
        tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::Object},");
        parser.open_group();
        parse_object(parser, tb)?;
        tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Close},");
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
                tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::BareEnum{");
                tb.add("base:LiveId(").suf_u64(class_id).add("), variant:LiveId(").suf_u64(variant_id).add(")}},");
            }
            else if parser.is_brace(){
                tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::NamedEnum{");
                tb.add("base:LiveId(").suf_u64(class_id).add("), variant:LiveId(").suf_u64(variant_id).add(")}},");
                parser.open_group();
                while !parser.eat_eot(){
                    let prop = parser.expect_any_ident()?;
                    let prop_id = LiveId::from_str(&prop).unwrap();
                    let mut start = TokenBuilder::new();
                    start.add("origin:LiveNodeOrigin::empty(), id:LiveId(").suf_u64(prop_id.0).add(")");
                    parser.expect_punct_alone(':')?;
                    parse_value(start.end(), parser, tb)?;
                    parser.eat_punct_alone(',');
                }
                tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Close},");
            }
            else if parser.is_paren(){
                tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::TupleEnum{");
                tb.add("base:LiveId(").suf_u64(class_id).add("), variant:LiveId(").suf_u64(variant_id).add(")}},");
                parser.open_group();
                while !parser.eat_eot(){
                    let mut start = TokenBuilder::new();
                    start.add("origin:LiveNodeOrigin::empty(), id:LiveId(0)");
                    parse_value(start.end(), parser, tb)?;
                    parser.eat_punct_alone(',');
                }
                tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Close},");
            }
            else{
                return Err(error("Not a valid enum type"));
            }
        }
        else if parser.is_brace(){ 
            tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::Clone(");
            tb.add("LiveId(").suf_u64(class_id).add("))},");
            parser.open_group();
            parse_object(parser,tb)?;
            tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Close},");
        }
        else{
            if class == "true"{
                tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::Bool(true)},");
            }
            else if class == "false"{
                tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::Bool(false)},");
            }
            else{
                tb.add("LiveNode{").stream(Some(node_start.clone())).add(",value:LiveValue::Id(");
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
            tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Color(").suf_u32(val).add(")},");
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
            tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Str(").string(val).add(")},");
        }
        else if s == "true" || s == "false"{
            tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Bool(").ident(&s).add(")},");
        }
        else{
            if s.chars().position(|c| c == '.').is_some(){
                if let Ok(value) = s.parse::<f64>(){
                    tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Float64(").unsuf_f64(value).add(")},");
                }
                else{
                    return Err(error("Value cant be parsed"));
                }
            }
            else if let Ok(value) = s.parse::<i64>(){
                tb.add("LiveNode{").stream(Some(node_start)).add(",value:LiveValue::Int64(").unsuf_i64(value).add(")},");
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


pub fn hex_bytes_to_u32(buf: &[u8]) -> Result<u32, ()> {
    fn hex_to_int(c: u8) -> Result<u32, ()> {
        if c >= 48 && c <= 57 {
            return Ok((c - 48) as u32);
        }
        if c >= 65 && c <= 70 {
            return Ok((c - 65 + 10) as u32);
        }
        if c >= 97 && c <= 102 {
            return Ok((c - 97 + 10) as u32);
        }
        return Err(());
    }
    
    match buf.len() {
        1 => {
            // #w
            let val = hex_to_int(buf[0]) ?;
            return Ok((val << 28) | (val << 24) | (val << 20) | (val << 16) | (val << 12) | (val << 8) | 0xff);
        }
        2 => { //#ww
            let val = (hex_to_int(buf[0]) ? << 4) + hex_to_int(buf[1]) ?;
            return Ok((val << 24) | (val << 16) | (val << 8) | 0xff)
        },
        3 => {
            // #rgb
            let r = hex_to_int(buf[0]) ?;
            let g = hex_to_int(buf[1]) ?;
            let b = hex_to_int(buf[2]) ?;
            return Ok((r << 28) | (r << 24) | (g << 20) | (g << 16) | (b << 12) | (b << 8) | 0xff);
        }
        4 => {
            // #rgba
            let r = hex_to_int(buf[0]) ?;
            let g = hex_to_int(buf[1]) ?;
            let b = hex_to_int(buf[2]) ?;
            let a = hex_to_int(buf[3]) ?;
            return Ok((r << 28) | (r << 24) | (g << 20) | (g << 16) | (b << 12) | (b << 8) | (a << 4) | a);
        }
        6 => {
            // #rrggbb
            let r = (hex_to_int(buf[0]) ? << 4) + hex_to_int(buf[1]) ?;
            let g = (hex_to_int(buf[2]) ? << 4) + hex_to_int(buf[3]) ?;
            let b = (hex_to_int(buf[4]) ? << 4) + hex_to_int(buf[5]) ?;
            return Ok((r << 24) | (g << 16) | (b << 8) | 0xff)
        }
        8 => {
            // #rrggbbaa
            let r = (hex_to_int(buf[0]) ? << 4) + hex_to_int(buf[1]) ?;
            let g = (hex_to_int(buf[2]) ? << 4) + hex_to_int(buf[3]) ?;
            let b = (hex_to_int(buf[4]) ? << 4) + hex_to_int(buf[5]) ?;
            let a = (hex_to_int(buf[6]) ? << 4) + hex_to_int(buf[7]) ?;
            return Ok((r << 24) | (g << 16) | (b << 8) | a)
        }
        _ => (),
    }
    return Err(());
} 
