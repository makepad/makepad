use proc_macro::{TokenStream};
use crate::macro_lib::*;

pub fn derive_ser_bin_impl(input: TokenStream) -> TokenStream {

    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    
    parser.eat_ident("pub");
    if parser.eat_ident("struct"){
        if let Some(name) = parser.eat_any_ident(){
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("SerBin"));

            tb.add("impl").stream(generic.clone());
            tb.add("SerBin for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn ser_bin ( & self , s : & mut Vec < u8 > ) {");

            if let Some(types) = types{
                for i in 0..types.len(){
                     tb.add("self .").unsuf_usize(i).add(". ser_bin ( s ) ;");
                }
            }
            else if let Some(fields) = parser.eat_all_struct_fields(){ 
                for field in fields{
                    tb.add("self .").ident(&field.name).add(". ser_bin ( s ) ;");
                }
            }
            else{
                return parser.unexpected()
            }
            tb.add("} } ;"); 
            return tb.end();
        }
    }
    else if parser.eat_ident("enum"){
        if let Some(name) = parser.eat_any_ident(){
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("SerBin"));
            
            tb.add("impl").stream(generic.clone());
            tb.add("SerBin for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn ser_bin ( & self , s : & mut Vec < u8 > ) {");
            tb.add("match self {");
            
            if !parser.open_brace(){
                return parser.unexpected()
            }
            let mut index = 0;
            while !parser.eat_eot(){
                // parse ident
                if let Some(variant) = parser.eat_any_ident(){
                    if let Some(types) = parser.eat_all_types(){
                        tb.add("Self ::").ident(&variant).add("(");
                        for i in 0..types.len(){
                            tb.ident(&format!("n{}", i)).add(",");
                        }
                        tb.add(") => {").suf_u16(index).add(". ser_bin ( s ) ;");
                        for i in 0..types.len(){
                            tb.ident(&format!("n{}", i)).add(". ser_bin ( s ) ;");
                        }
                        tb.add("}");
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields(){ // named variant
                        tb.add("Self ::").ident(&variant).add("{");
                        for field in fields.iter(){
                            tb.ident(&field.name).add(",");
                        }
                        tb.add("} => {").suf_u16(index).add(". ser_bin ( s ) ;");
                        for field in fields{
                            tb.ident(&field.name).add(". ser_bin ( s ) ;");
                        }
                        tb.add("}");
                    }
                    else if parser.is_punct_alone(',') || parser.is_eot(){ // bare variant
                        tb.add("Self ::").ident(&variant).add("=> {");
                        tb.suf_u16(index).add(". ser_bin ( s ) ; }");
                    }
                    else{
                        return parser.unexpected();
                    }
                    index += 1;
                    parser.eat_punct_alone(',');
                }
                else{
                    return parser.unexpected()
                }
            }
            tb.add("} } } ;");
            return tb.end();
        }
    }
    return parser.unexpected()
} 

pub fn derive_de_bin_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    
    parser.eat_ident("pub");
    if parser.eat_ident("struct"){
        if let Some(name) = parser.eat_any_ident(){
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("DeBin"));

            tb.add("impl").stream(generic.clone());
            tb.add("DeBin for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_bin ( o : & mut usize , d : & [ u8 ] )");
            tb.add("-> std :: result :: Result < Self , DeBinErr > { ");
            tb.add("std :: result :: Result :: Ok ( Self");

            if let Some(types) = types{
                tb.add("(");
                for _ in 0..types.len(){
                     tb.add("DeBin :: de_bin ( o , d ) ? ,");
                }
                tb.add(")");
            }
            else if let Some(fields) = parser.eat_all_struct_fields(){ 
                tb.add("{");
                for field in fields{
                    tb.ident(&field.name).add(": DeBin :: de_bin ( o , d ) ? ,");
                }
                tb.add("}");
            }
            else{
                return parser.unexpected()
            }
            tb.add(") } } ;"); 
            return tb.end();
        }
    }
    else if parser.eat_ident("enum"){
        if let Some(name) = parser.eat_any_ident(){
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("DeBin"));
            
            tb.add("impl").stream(generic.clone());
            tb.add("DeBin for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_bin ( o : & mut usize , d : & [ u8 ] )");
            tb.add("-> std :: result :: Result < Self , DeBinErr > {");
            tb.add("let id : u16 = DeBin :: de_bin ( o , d ) ? ;");
            tb.add("match id {");
            
            if !parser.open_brace(){
                return parser.unexpected()
            }
            let mut index = 0;
            while !parser.eat_eot(){
                // parse ident
                if let Some(variant) = parser.eat_any_ident(){
                    tb.suf_u16(index as u16).add("=> {");
                    tb.add("std :: result :: Result :: Ok ( Self ::");
                    if let Some(types) = parser.eat_all_types(){
                        tb.ident(&variant).add("(");
                        for _ in 0..types.len(){
                            tb.add("DeBin :: de_bin ( o , d ) ? ,");
                        }
                        tb.add(")");
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields(){ // named variant
                        tb.ident(&variant).add("{");
                        for field in fields.iter(){
                            tb.ident(&field.name).add(": DeBin :: de_bin ( o , d ) ? ,");
                        }
                        tb.add("}");
                    }
                    else if parser.is_punct_alone(',') || parser.is_eot(){ // bare variant
                        tb.ident(&variant);
                    }
                    else{
                        return parser.unexpected();
                    }
                    
                    tb.add(") }");
                    index += 1;
                    parser.eat_punct_alone(',');
                }
                else{
                    return parser.unexpected()
                }
            } 
            tb.add("_ => std :: result :: Result :: Err ( DeBinErr { o : * o , l :");
            tb.unsuf_usize(1).add(", s : d . len ( ) , msg : ").string(&name).add(". to_string ( ) } )");
            tb.add("} } } ;");
            return tb.end();
        }
    }
    return parser.unexpected()
}
