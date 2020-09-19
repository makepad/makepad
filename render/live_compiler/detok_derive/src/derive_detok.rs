use proc_macro::{TokenStream};

use crate::macro_lib::*;

pub fn derive_de_tok_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("DeTok"));

            tb.add("impl").stream(generic.clone());
            tb.add("DeTok for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_tok ( p : & mut DeTokParser )");
            tb.add("-> std :: result :: Result < Self , LiveError > { ");
             
            // if we use an uppercase, its really rust syntax
            // if we use lowercase with _ we use props from default() if not defined
            tb.add("p . accept_token ( Token :: Ident ( Ident :: new (").string(&name).add(") ) ) ;");
            tb.add("p . accept_token ( Token :: PathSep ) ;");
            
            if let Some(types) = types {

                tb.add("p . expect_token ( Token :: LeftParen )  ? ;");
                tb.add("let r = Self (");
                for _ in 0..types.len() {
                    tb.add("{ let r = DeTok :: de_tok ( p ) ? ; p . accept_token ( Token :: Comma ) ; r } ,");
                }
                tb.add(") ;");
                tb.add("p . expect_token ( Token :: RightParen )  ? ;");
                tb.add("std :: result :: Result :: Ok ( r )");
            }
            else if let Some(fields) = parser.eat_all_struct_fields() { // if all our fields are f32's
                // we can use a special all() function for f32 only structs
                let mut all_f32 = true;
                for (_field, ty) in &fields{
                    let ty_str = ty.to_string();
                    if ty_str != "f32"{
                        all_f32 = false;
                        break;
                    }
                }
                if all_f32{
                    tb.add("if p . accept_token ( Token :: Ident ( Ident :: new (").string("all").add(") ) ) {");
                    tb.add("p . expect_token ( Token :: LeftParen ) ? ;");
                    tb.add("let f = f32 :: de_tok ( p ) ? ;");
                    tb.add("p . expect_token ( Token :: RightParen ) ? ;");
                    tb.add("return std :: result :: Result :: Ok ( Self {");
                    for (field, _ty) in &fields {
                        tb.ident(&field).add(": f ,");
                    }
                    tb.add("} ) }");
                }
                
                tb.add("let mut default = Self :: default ( ) ;");
                tb.add("p . expect_token ( Token :: LeftBrace )  ? ;");
                for (field, _ty) in &fields {
                    tb.add("let mut").ident(&format!("_{}", field)).add("= None ;");
                }
                tb.add("while let Ok ( ident ) = p . parse_ident ( ) {");

                for (index, (field, _ty)) in fields.iter().enumerate() {
                    if index != 0{
                        tb.add("else");
                    }
                    tb.add("if ident == Ident :: new (").string(&field).add(") {");
                    tb.add("p . expect_token ( Token :: Colon ) ? ;");
                    tb.ident(&format!("_{}", field)).add("= Some ( DeTok :: de_tok ( p  ) ? ) ;");
                    tb.add("p . accept_token ( Token :: Comma ) ;");
                    tb.add("}");
                }
                tb.add("}");
                tb.add("if p . accept_token ( Token :: Splat ) {");
                tb.add("default = DeTokSplat :: de_tok_splat ( p ) ? ;");
                tb.add("}");
                tb.add("p . expect_token ( Token :: RightBrace )  ? ;");
                tb.add("std :: result :: Result :: Ok ( Self {");
                for (field, ty) in fields {
                    tb.ident(&field).add(":");
                    if ty.into_iter().next().unwrap().to_string() == "Option" {
                        tb.add("if let Some ( t ) =").ident(&format!("_{}", field));
                        tb.add("{ t } else { None } ,");
                    }
                    else {
                        tb.add("if let Some ( t ) =").ident(&format!("_{}", field));
                        tb.add("{ t } else { default .").ident(&field).add("} ,");
                    }
                }
                tb.add("} )");
            }
            else {
                return parser.unexpected()
            }
            tb.add("} } ;");
            //tb.eprint();
            return tb.end();
        }
    }
    else if parser.eat_ident("enum") {
        
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let where_clause = parser.eat_where_clause(Some("DeTok"));
            
            tb.add("impl").stream(generic.clone());
            tb.add("DeTok for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_tok ( p : & mut DeTokParser )");
            tb.add("-> std :: result :: Result < Self , LiveError > { ");
            tb.add("if p . accept_token ( Token :: Ident ( Ident :: new (").string(&name).add(") ) ) {");
            tb.add("p . expect_token ( Token :: PathSep ) ? ;");
            tb.add("}");
            tb.add("let ident = p . parse_ident ( ) ? ;");
            
            if !parser.open_brace() {
                return parser.unexpected()
            }
            while !parser.eat_eot() {
                // parse ident
                if let Some(variant) = parser.eat_any_ident() {
                    tb.add("if ident == Ident :: new (").string(&variant).add(") {");
                    if let Some(types) = parser.eat_all_types() {
                        tb.add("p . expect_token ( Token :: LeftParen )  ? ;");
                        tb.add("let r = Self ::").ident(&variant).add("(");
                        for _ in 0..types.len() {
                            tb.add("{ let r = DeTok :: de_tok ( p ) ? ; p . accept_token ( Token :: Comma ) ; r } ,");
                        }
                        tb.add(") ;"); 
                        tb.add("p . expect_token ( Token :: RightParen ) ? ;");
                        tb.add("return Ok ( r ) ;");
                    }
                    else if let Some(fields) = parser.eat_all_struct_fields() { // named variant
                        tb.add("p . expect_token ( Token :: LeftBrace )  ? ;");
                        for (field, _ty) in &fields {
                            tb.add("let mut").ident(&format!("_{}", field)).add("= None ;");
                        }
                        tb.add("while let Ok ( ident ) = p . parse_ident ( ) {");
                        for (index, (field, _ty)) in fields.iter().enumerate() {
                            if index != 0{
                                tb.add("else");
                            }
                            tb.add("if ident == Ident :: new (").string(&field).add(") {");
                            tb.add("p . expect_token ( Token :: Colon ) ? ;");
                            tb.ident(&format!("_{}", field)).add("= Some ( DeTok :: de_tok ( p  ) ? ) ;");
                            tb.add("p . accept_token ( Token :: Comma ) ;");
                            tb.add("}");
                        }
                        tb.add("}");
                        tb.add("p . expect_token ( Token :: RightBrace )  ? ;");
                        tb.add("return Ok ( Self ::").ident(&variant).add("{");
                        for (field, ty) in fields {
                            tb.ident(&field).add(":");
                            if ty.into_iter().next().unwrap().to_string() == "Option" {
                                tb.add("if let Some ( t ) =").ident(&format!("_{}", field));
                                tb.add("{ t } else { None } ,");
                            }
                            else {
                                tb.add("if let Some ( t ) =").ident(&format!("_{}", field));
                                tb.add("{ t } else { return Err ( p . error_missing_prop (");
                                tb.string(&field).add(") ) } ,");
                            }
                        }
                        tb.add("} )");
                    }
                    else if parser.is_punct(',') || parser.is_eot() { // bare variant
                        tb.add("return Ok ( Self ::").ident(&variant).add(")");
                    }
                    else {
                        return parser.unexpected();
                    }
                    
                    tb.add("}");
                    parser.eat_punct(',');
                }
                else {
                    return parser.unexpected()
                }
            } 
            tb.add("return Err ( p . error_enum ( ident , ").string(&name).add(") )");
            tb.add("} }");
            //tb.eprint();
            return tb.end();
        }
    }
    return parser.unexpected()
}

pub fn derive_de_tok_splat_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(name) = parser.eat_any_ident() {
            let generic = parser.eat_generic();
            let _types = parser.eat_all_types();
            let where_clause = parser.eat_where_clause(Some("DeTok"));

            tb.add("impl").stream(generic.clone());
            tb.add("DeTokSplat for").ident(&name).stream(generic).stream(where_clause);
            tb.add("{ fn de_tok_splat ( p : & mut DeTokParser )");
            tb.add("-> std :: result :: Result < Self , LiveError > { ");
            tb.add("return Err ( p . error_not_splattable (").string(&name).add(") ) ;");
            tb.add("} }");
            return tb.end();
        }
    }
    return parser.unexpected()
}

