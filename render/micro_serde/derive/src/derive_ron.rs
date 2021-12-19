 
use proc_macro::{TokenStream};
use crate::macro_lib::*;

pub fn derive_ser_ron_impl(input: TokenStream) -> TokenStream {

    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    
    parser.eat_ident("pub");
    if parser.eat_ident("struct"){
        if let Some(name) = parser.eat_any_ident(){
            
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("SerRon"));

            tb.add("impl").stream(generic.clone());
            tb.add("SerRon for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn ser_ron ( & self , d : usize , s : & mut SerRonState ) {");
            
            if let Some(types) = types{
                tb.add("s . out . push (").chr('(').add(") ;");
                for i in 0..types.len(){
                     tb.add("self .").unsuf_usize(i).add(". ser_ron ( d , s ) ;");
                     if i != types.len() - 1{
                         tb.add("s . out . push_str (").string(", ").add(") ;");
                     }
                }
                tb.add("s . out . push (").chr(')').add(") ;");
            }
            else if let Some(fields) = parser.eat_all_struct_fields(){ 
                tb.add("s . st_pre ( ) ;");
                // named struct
                for field in fields{
                    if field.ty.into_iter().next().unwrap().to_string() == "Option"{
                        tb.add("if let Some ( t ) = ").add("& self .").ident(&field.name).add("{");
                        tb.add("s . field ( d + 1 ,").string(&field.name).add(") ;");
                        tb.add("t . ser_ron ( d + 1 , s ) ; s . conl ( ) ; } ;");
                    }
                    else{
                        tb.add("s . field ( d + 1 ,").string(&field.name).add(" ) ;");
                        tb.add("self .").ident(&field.name).add(". ser_ron ( d + 1 , s ) ; s . conl ( ) ;");
                    }
                }
                tb.add("s . st_post ( d ) ;");
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
            let where_clause = parser.eat_where_clause(Some("SerRon"));

            tb.add("impl").stream(generic.clone());
            tb.add("SerRon for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn ser_ron ( & self , d : usize , s : & mut  SerRonState ) {");
            tb.add("match self {");
            
            if !parser.open_brace(){
                return parser.unexpected()
            }

            while !parser.eat_eot(){
                // parse ident
                if let Some(variant) = parser.eat_any_ident(){
                    if let Some(types) = parser.eat_all_types(){
                        
                        tb.add("Self ::").ident(&variant).add("(");
                        for i in 0..types.len(){
                            tb.ident(&format!("n{}", i)).add(",");
                        }
                        tb.add(") => {");
                        tb.add("s . out . push_str (").string(&variant).add(") ;");
                        tb.add("s . out . push (").chr('(').add(") ;");
                        
                        for i in 0..types.len(){
                            tb.ident(&format!("n{}", i)).add(". ser_ron ( d , s ) ;");
                            if i != types.len() - 1{
                                tb.add("s . out . push_str (").string(", ").add(") ;");
                            }
                        }
                        tb.add("s . out . push (").chr(')').add(") ;");
                        tb.add("}");
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields(){ // named variant
                        tb.add("Self ::").ident(&variant).add("{");
                        for field in fields.iter(){
                            tb.ident(&field.name).add(",");
                        }
                        tb.add("} => {");
                        
                        tb.add("s . out . push_str (").string(&variant).add(") ;");
                        tb.add("s . st_pre ( ) ;");
                        
                        for field in fields{
                            if field.ty.into_iter().next().unwrap().to_string() == "Option"{
                                tb.add("if ").ident(&field.name).add(". is_some ( ) {");
                                tb.add("s . field ( d + 1 ,").string(&field.name).add(") ;");
                                tb.ident(&field.name).add(" . ser_ron ( d + 1 , s ) ; s . conl ( ) ; } ;");
                            }
                            else{
                                tb.add("s . field ( d + 1 ,").string(&field.name).add(" ) ;");
                                tb.ident(&field.name).add(". ser_ron ( d + 1 , s ) ; s . conl ( ) ;");
                            }
                        }
                        tb.add("s . st_post ( d ) ; }");
                    }
                    else if parser.is_punct_alone(',') || parser.is_eot(){ // bare variant
                        tb.add("Self ::").ident(&variant).add("=> {");
                        tb.add("s . out . push_str (").string(&variant).add(") ; }");
                    }
                    else{
                        return parser.unexpected();
                    }
                    parser.eat_punct_alone(',');
                }
                else{
                    return parser.unexpected()
                }
            }
            tb.add("}");
            tb.add("} } ;");
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_de_ron_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    parser.eat_ident("pub");
    if parser.eat_ident("struct"){
        if let Some(name) = parser.eat_any_ident(){
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("DeRon"));

            tb.add("impl").stream(generic.clone());
            tb.add("DeRon for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_ron ( s : &  mut DeRonState , i : & mut std :: str :: Chars )");
            tb.add("-> std :: result :: Result < Self , DeRonErr > { ");

            if let Some(types) = types{
                tb.add("s . paren_open ( i ) ? ;");
                tb.add("let r = Self");
                tb.add("(");
                for _ in 0..types.len(){
                     tb.add("{ let r = DeRon :: de_ron ( s , i ) ? ; s . eat_comma_paren ( i ) ? ; r } ,");
                }
                tb.add(") ;");
                tb.add("s . paren_close ( i ) ? ;");
                tb.add("std :: result :: Result :: Ok ( r ) ");
            }
            else if let Some(fields) = parser.eat_all_struct_fields(){ 
                tb.add("s . paren_open ( i ) ? ;");
                for field in &fields{
                    tb.add("let mut").ident(&format!("_{}",field.name)).add("= None ;");
                }
                tb.add("while let Some ( _ ) = s . next_ident ( ) {");
                tb.add("match s . identbuf . as_ref ( ) {");
                for field in &fields{
                    tb.string(&field.name).add("=> { s . next_colon ( i ) ? ;");
                    tb.ident(&format!("_{}",field.name)).add("= Some ( DeRon :: de_ron ( s , i ) ? ) ; } ,");
                }
                tb.add("_ => return std :: result :: Result :: Err ( s . err_exp ( & s . identbuf ) )");
                tb.add("} ; s . eat_comma_paren ( i ) ? ;");
                tb.add("} ; s . paren_close ( i ) ? ;");
                
                tb.add("std :: result :: Result :: Ok ( Self {");
                for field in fields{
                    tb.ident(&field.name).add(":");
                    if field.ty.into_iter().next().unwrap().to_string() == "Option"{
                        tb.add("if let Some ( t ) =").ident(&format!("_{}",field.name));
                        tb.add("{ t } else { None } ,");
                    }
                    else{
                        tb.add("if let Some ( t ) =").ident(&format!("_{}",field.name));
                        tb.add("{ t } else { return Err ( s . err_nf (");
                        tb.string(&field.name).add(") ) } ,");
                    }
                }
                tb.add("} )");
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
            let where_clause = parser.eat_where_clause(Some("DeRon"));

            tb.add("impl").stream(generic.clone());
            tb.add("DeRon for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_ron ( s : & mut  DeRonState , i : & mut std :: str :: Chars )");
            tb.add("-> std :: result :: Result < Self , makepad_microserde :: DeRonErr > { ");
            tb.add("s . ident ( i ) ? ;");
            tb.add("std :: result :: Result :: Ok ( match s . identbuf . as_ref ( ) {");
            
            if !parser.open_brace(){
                return parser.unexpected()
            }
            while !parser.eat_eot(){
                // parse ident
                if let Some(variant) = parser.eat_any_ident(){
                    tb.string(&variant).add("=> {");
                    if let Some(types) = parser.eat_all_types(){
                        
                        tb.add("s . paren_open ( i ) ? ;");
                        tb.add("let r = Self ::").ident(&variant).add("(");
                        for _ in 0..types.len(){
                            tb.add("{ let r = DeRon :: de_ron ( s , i ) ? ; s . eat_comma_paren ( i ) ? ; r } ,");
                        }
                        tb.add(") ;");
                        tb.add("s . paren_close ( i ) ? ; r");
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields(){ // named variant
                        tb.add("s . paren_open ( i ) ? ;");
                        for field in &fields{
                            tb.add("let mut").ident(&format!("_{}",field.name)).add("= None ;");
                        }
                        tb.add("while let Some ( _ ) = s . next_ident ( ) {");
                        tb.add("match s . identbuf . as_ref ( ) {");
                        for field in &fields{
                            tb.string(&field.name).add("=> { s . next_colon ( i ) ? ;");
                            tb.ident(&format!("_{}",field.name)).add("= Some ( DeRon :: de_ron ( s , i ) ? ) ; } ,");
                        }
                        tb.add("_ => return std :: result :: Result :: Err ( s . err_exp ( & s . strbuf ) )");
                        tb.add("} ; s . eat_comma_paren ( i ) ? ;");
                        tb.add("} ; s . paren_close ( i ) ? ;");
                        
                        tb.add("Self ::").ident(&variant).add("{");
                        for field in fields{
                            tb.ident(&field.name).add(":");
                            if field.ty.into_iter().next().unwrap().to_string() == "Option"{
                                tb.add("if let Some ( t ) =").ident(&format!("_{}",field.name));
                                tb.add("{ t } else { None } ,");
                            }
                            else{
                                tb.add("if let Some ( t ) =").ident(&format!("_{}",field.name));
                                tb.add("{ t } else { return Err ( s . err_nf (");
                                tb.string(&field.name).add(") ) } ,");
                            }
                        }
                        tb.add("}");
                    }
                    else if parser.is_punct_alone(',') || parser.is_eot(){ // bare variant
                        tb.add("Self ::").ident(&variant);
                    }
                    else{
                        return parser.unexpected();
                    }
                    
                    tb.add("}");
                    parser.eat_punct_alone(',');
                }
                else{
                    return parser.unexpected()
                }
            } 
            tb.add("_ => return std :: result :: Result :: Err ( s . err_enum ( & s . identbuf ) )");
            tb.add("} ) } }");
           return tb.end();
        }
    }
    return parser.unexpected()
}
