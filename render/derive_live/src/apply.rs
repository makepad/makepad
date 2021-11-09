use proc_macro::TokenStream;
use crate::macro_lib::*;
use crate::id::*;

pub fn gen_impl(input:TokenStream)->TokenStream{
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    fn parse_level(parser:&mut TokenParser, tb:&mut TokenBuilder)->Result<(),TokenStream>{
        // key values
        while !parser.eat_eot(){
            
            let label = parser.expect_any_ident()?;
            let label_id = Id::from_str(&label).unwrap().0;
            // ok 
            if parser.is_brace(){ // local class
                parser.open_group();
                tb.add("GenNode{id:Id(0),value:GenValue::ClassNamed{class:Id(");
                tb.suf_u64(label_id).add(")}},");
                parse_level(parser, tb)?;
                parser.eat_punct(',');
            }
            else {
                parser.expect_punct(':')?;
                // ok now for the value
                tb.add("GenNode{id:Id(").suf_u64(label_id).add("),value:");
                if parser.is_paren(){
                    parser.open_group();
                    let arg = parser.eat_level();
                    tb.add("(").stream(Some(arg)).add(").to_gen_value()},");
                    parser.eat_punct(',');
                }
                else if let Some(class) = parser.eat_any_ident(){
                    let class_id = Id::from_str(&class).unwrap().0;
                    // could be local class or enum
                    if parser.eat_double_colon_destruct(){
                        let variant = parser.expect_any_ident()?;
                        let variant_id = Id::from_str(&variant).unwrap().0;
                        // now check if we have a , eot or ( or {
                        if parser.is_punct(',') || parser.is_eot(){
                            tb.add("GenValue::EnumBare{class:Id(");
                            tb.suf_u64(class_id).add("), variant:Id(");
                            tb.suf_u64(variant_id).add(")}},");
                            parser.eat_punct(',');
                        }
                        else if parser.is_brace(){
                            tb.add("GenValue::EnumNamed{class:Id(");
                            tb.suf_u64(class_id).add("), variant:Id(");
                            tb.suf_u64(variant_id).add(")}},");
                            parser.open_group();
                            parse_level(parser,tb)?;
                            parser.eat_punct(',');
                        }
                        else if parser.is_paren(){
                            tb.add("GenValue::EnumTuple{class:Id(");
                            tb.suf_u64(class_id).add("), variant:Id(");
                            tb.suf_u64(variant_id).add(")}},");
                            parser.open_group();
                            parse_level(parser,tb)?;
                            parser.eat_punct(',');
                        }
                        else{
                            return Err(error("Not a valid enum type"));
                        }
                    }
                    else if parser.is_brace(){ 
                        tb.add("GenValue::ClassNamed{class:Id(");
                        tb.suf_u64(class_id).add(")}},");
                        parser.open_group();
                        parse_level(parser,tb)?;
                        parser.eat_punct(',');
                    }
                    else{
                        return Err(error("Expected {}"));
                    }
                }
                else if parser.is_brace(){ // add value types here
                    tb.add("GenValue::ClassBare},");
                    parser.open_group();
                    parse_level(parser,tb)?;
                    parser.eat_punct(',');
                }
                else if parser.is_punct('#'){ // coLor!
                    todo!()
                }
                else if let Some(lit) = parser.eat_literal(){
                    // ok so.. bool float string or int..
                    let s = lit.to_string();
                    let bytes = s.as_bytes();
                    if bytes[0] == '"' as u8{ // its a string
                        let val = std::str::from_utf8(&bytes[1..bytes.len()-1]).unwrap();
                        tb.add("GenValue::Str(").string(val).add(")},");
                    }
                    else if s == "true" || s == "false"{
                        tb.add("GenValue::Bool(").ident(&s).add(")},");
                    }
                    else{
                        if let Ok(value) = s.parse::<f64>(){
                            tb.add("GenValue::Float(").unsuf_f64(value).add(")},");
                        }
                        else if let Ok(value) = s.parse::<i64>(){
                            tb.add("GenValue::Int(").unsuf_i64(value).add(")},");
                        }
                        else{
                            return Err(error("Value cant be parsed"));
                        }
                        // has to be a number int or float
                    }
                    parser.eat_punct(',');
                }
                else{
                    return Err(error("Unexpected token"));
                }
            }
        }
        tb.add("GenNode{id:Id(0),value:GenValue::Close},");
        Ok(())
    }

    tb.add("&[GenNode{id:Id(0),value:GenValue::ClassBare},");
    if let Err(e) = parse_level(&mut parser, &mut tb){
        return e
    };
    tb.add("]");
    tb.end()
}