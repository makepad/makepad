use proc_macro::{TokenStream};
use crate::macro_lib::*;

pub enum DrawType {
    DrawQuad,
    DrawText
}

pub fn derive_draw_impl(input: TokenStream, draw_type: DrawType) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    
    if !parser.eat_punct('#')
        || !parser.open_bracket()
        || !parser.eat_ident("repr")
        || !parser.open_paren()
        || !parser.eat_ident("C")
        || !parser.eat_eot()
        || !parser.eat_eot() {
        return error("Expected #[repr(C)] after derive Quad because the structure is memory-aligned to instance data")
    }
    
    parser.eat_ident("pub");
    if parser.eat_ident("struct") {
        if let Some(struct_name) = parser.eat_any_ident() {
            // lets eat all the fields
            let fields = if let Some(fields) = parser.eat_all_struct_fields() {
                fields
            }
            else {
                return error("No fields in struct");
            };
            
            let mut with_shader = TokenBuilder::new();
            with_shader.add("pub fn with_shader ( cx : & mut Cx , shader : Shader , slots : usize ) -> Self {");
            with_shader.add("Self {");
            
            let mut debug = false;
            let mut custom_new = false;
            let mut base_type = None;
            let mut default_shader = None;
            let mut uniforms = Vec::new();
            let mut instances = Vec::new();
            let mut uni_insts = Vec::new();
            let mut type_slot_concat = TokenBuilder::new();
            for field in &fields {
                for attr in &field.attrs {
                    if attr.name == "default_shader" {
                        default_shader = Some(attr.args.clone());
                    }
                    if attr.name == "debug_draw_input" {
                        debug = true;
                    }
                    if attr.name == "custom_new" {
                        custom_new = true;
                    }
                }
                if field.name == "base" {
                    base_type = Some(field.ty.clone());
                }
                else { // only accept uniforms
                    // spit out attrib
                    if base_type.is_none() {
                        uniforms.push((field.name.clone(), field.ty.to_string()));
                        uni_insts.push((field.name.clone(), field.ty.to_string()));
                    }
                    else {
                        type_slot_concat.add("+");
                        type_slot_concat.stream(Some(field.ty.clone()));
                        type_slot_concat.add(":: slots ( )");
                        instances.push((field.name.clone(), field.ty.to_string()));
                        uni_insts.push((field.name.clone(), field.ty.to_string()));
                    }
                }
            }
            let base_type = if let Some(bt) = base_type {bt} else {
                return error("base field not defined!")
            };
            if default_shader.is_none() {
                return error("#[default_shader()] defined!")
            }
            // 'base' needs a shader field
            // and after can only accept instance fields
            
            // ok we have fields and attribs
            tb.add("impl").ident(&struct_name).add("{");
            
            tb.add("pub fn default_shader ( ) -> LiveItemId {");
            tb.add("live_item_id ! (").stream(default_shader.clone()).add(")");
            tb.add("}");
            
            tb.add("pub fn");
            if custom_new {
                tb.add("custom_new");
            }
            else {
                tb.add("new");
            }
            tb.add(" ( cx : & mut Cx , shader : Shader ) -> Self {");
            tb.add("Self :: with_slots ( cx , default_shader_overload ! ( cx , shader ,").stream(default_shader).add(") , 0 )");
            tb.add("}");
            
            tb.add("pub fn with_slots ( cx : & mut Cx , shader : Shader , slots : usize ) -> Self {");
            tb.add("Self {");
            tb.add("base :").ident(&base_type.to_string());
            tb.add(":: with_slots ( cx , shader , slots").stream(Some(type_slot_concat.end())).add(") ,");
            // now add initializers for all values
            for field in &fields {
                if field.name != "base" {
                    tb.ident(&field.name).add(": Default :: default ( ) ,");
                }
            }
            tb.add("}");
            tb.add("}");
            
            tb.add("pub fn live_draw_input ( ) -> LiveDrawInput {");
            tb.add("let mut def =").ident(&base_type.to_string()).add(" :: live_draw_input ( ) ;");
            
            for (name, ty) in &uniforms {
                tb.add("def . add_uniform ( module_path ! ( ) , ");
                tb.string(&struct_name).add(" , ");
                tb.string(name).add(" , ");
                tb.ident(ty).add(" :: ty_expr ( ) ) ;");
            }
            
            for (name, ty) in &instances {
                tb.add("def . add_instance ( module_path ! ( ) , ");
                tb.string(&struct_name).add(" , ");
                tb.string(name).add(" , ");
                tb.ident(ty).add(" :: ty_expr ( ) ) ;");
            }
            
            tb.add("return def ; }");
            
            tb.add("pub fn register_draw_input ( cx : & mut Cx ) {");
            tb.add("cx . live_styles . register_draw_input ( live_item_id ! (");
            tb.add("self ::").ident(&struct_name).add(") , Self :: live_draw_input ( ) ) ;");
            tb.add("}");
            
            tb.add("pub fn write_uniforms ( & self , cx : & mut Cx ) {");
            tb.add("if self . area ( ) . is_first_instance ( ) { ");
            
            for (name, _) in &uniforms {
                tb.add("self .").ident(&name).add(". write_draw_input ( cx , self . area ( ) ,");
                tb.add(" live_item_id ! ( self :: ").ident(&struct_name).add("::").ident(&name).add(")");
                tb.add(",").string(&format!("self::{}::{}", struct_name, name)).add(") ;");
            }
            
            tb.add("} }");
            
            for (name, ty) in &uni_insts {
                tb.add("pub fn").ident(&format!("set_{}", name)).add("( & mut self , cx : & mut Cx , v :").ident(ty).add(") {");
                tb.add("self .").ident(name).add("= v ;");
                tb.add("v . write_draw_input ( cx , self . area ( ) ,");
                tb.add("live_item_id ! ( self :: ").ident(&struct_name).add("::").ident(&name).add(")");
                tb.add(",").string(&format!("self::{}::{}", struct_name, name)).add(") ;");
                tb.add("}");
            }
            
            for (name, ty) in &uni_insts {
                tb.add("pub fn").ident(&format!("with_{}", name)).add("( self ,").ident(name).add(":").ident(ty).add(") -> Self {");
                tb.add("Self {").ident(name).add(", .. self }");
                tb.add("}");
            }
            
            tb.add("pub fn animate ( & mut self , cx : & mut Cx , a : & mut Animator , t : f64 ) {");
            tb.add("self . base . animate ( cx , a , t ) ;");
            for (name, ty) in &uni_insts {
                tb.add("if let Some ( v ) = ").ident(ty).add(":: animate ( cx , a , t ,");
                tb.add("live_item_id ! ( self :: ").ident(&struct_name).add("::").ident(&name).add(")");
                tb.add(") {");
                tb.add("self .").ident(&format!("set_{}", name)).add("( cx , v ) ;");
                tb.add("}");
            }
            
            tb.add("}");
            
            tb.add("pub fn last_animate ( & mut self , a : & Animator ) {");
            tb.add("self . base . last_animate ( a ) ;");
            for (name, ty) in &uni_insts {
                tb.add("if let Some ( v ) = ").ident(ty).add(":: last_animate ( a ,");
                tb.add("live_item_id ! ( self :: ").ident(&struct_name).add("::").ident(&name).add(")");
                tb.add(") {");
                tb.add("self .").ident(name).add(" = v ;");
                tb.add("}");
            }
            
            tb.add("}");
            
            tb.add("pub fn shader ( & self ) -> Shader { self . base . shader ( ) }");
            tb.add("pub fn set_shader ( & mut self , shader : Shader ) { self . base . set_shader ( shader ) }");
            tb.add("pub fn area ( & self ) -> Area { self . base . area ( ) }");
            tb.add("pub fn set_area ( & mut self , area : Area ) { self . base . set_area ( area ) }");
            tb.add("pub fn with_draw_depth ( self , depth : f32 ) -> Self { Self { base : self . base . with_draw_depth ( depth ) , .. self } }");

            tb.add("pub fn begin_many ( & mut self , cx : & mut Cx ) { self . base . begin_many ( cx ) }");
            tb.add("pub fn end_many ( & mut self , cx : & mut Cx ) { self . base . end_many ( cx ) ; self . write_uniforms ( cx ) }");
            
            match draw_type {
                DrawType::DrawText => {
                    
                    tb.add("pub fn get_monospace_base ( & self , cx : & mut Cx ) -> Vec2 { self . base . get_monospace_base ( cx ) }");
                    tb.add("pub fn closest_text_offset ( & self , cx : & Cx , pos : Vec2 ) -> Option < usize > { self . base . closest_text_offset ( cx , pos ) }");
                    
                    tb.add("pub fn buf_truncate ( & mut self , len : usize ) { self . base . buf_truncate ( len ) ; }");
                    tb.add("pub fn buf_push_char ( & mut self , c : char ) { self . base . buf_push_char ( c ) ; }");
                    tb.add("pub fn buf_push_str ( & mut self , val : & str ) { self . base . buf_push_str ( val ) ; }");
                    
                    tb.add("pub fn draw_text ( & mut self , cx : & mut Cx , pos : Vec2 ) { self . base . draw_text ( cx , pos ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_text_walk ( & mut self , cx : & mut Cx , text : & str ) { self . base . draw_text_walk ( cx , text ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_text_rel ( & mut self , cx : & mut Cx , pos : Vec2 , text : & str ) { self . base . draw_text_rel ( cx , pos , text ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_text_abs ( & mut self , cx : & mut Cx , pos : Vec2 , text : & str ) { self . base . draw_text_abs ( cx , pos , text ) ; self . write_uniforms ( cx ) }");
                    
                    tb.add("pub fn draw_text_chunk < F > (  & mut self , cx : & mut Cx , pos : Vec2 , char_offset : usize , chunk : & [ char ] , mut char_callback : F )");
                    tb.add("where F : FnMut ( char , usize , f32 , f32 ) -> f32 { self . base . draw_text_chunk ( cx , pos , char_offset , chunk , char_callback ) }");
                },
                
                DrawType::DrawQuad => {
                    // quad forward implementation
                    tb.add("pub fn begin_quad ( & mut self , cx : & mut Cx , layout : Layout ) { self . base . begin_quad ( cx , layout ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn end_quad ( & mut self , cx : & mut Cx )  { self . base . end_quad ( cx ) }");
                    
                    tb.add("pub fn draw_quad_walk ( & mut self , cx : & mut Cx , walk : Walk )  { self . base . draw_quad_walk ( cx , walk ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad_rel ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_rel ( cx , rect ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad_abs ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_abs ( cx , rect ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad ( & mut self , cx : & mut Cx ) { self . base . draw_quad ( cx ) ; self . write_uniforms ( cx ) }");
                }
            }
            
            tb.add("}");
            if debug {
                tb.eprint();
            }
            return tb.end();
        }
    }
    parser.unexpected()
}
