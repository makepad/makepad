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
             
            let mut base_type = None;
            let mut default_shader = None;
            let mut uniforms = Vec::new();
            let mut instances = Vec::new();
            let mut type_slot_concat = TokenBuilder::new();
            for field in &fields {
                for attr in &field.attrs{
                    if attr.name == "default_shader"{
                        default_shader = Some(attr.args.clone());
                    }
                }
                if field.name == "base" {
                    base_type = Some(field.ty.clone());
                }
                else { // only accept uniforms
                    // spit out attrib
                    if base_type.is_none(){
                        uniforms.push((field.name.clone(), field.ty.to_string()));
                    }
                    else{
                        type_slot_concat.add("+");
                        type_slot_concat.stream(Some(field.ty.clone()));
                        type_slot_concat.add(":: slots ( )");
                        instances.push((field.name.clone(), field.ty.to_string()));
                    }
                }
            }
            let base_type = if let Some(bt) = base_type {bt} else {
                return error("base field not defined!")
            };
            if default_shader.is_none(){
                return error("#[default_shader()] defined!")
            }
            // 'base' needs a shader field
            // and after can only accept instance fields
            
            // ok we have fields and attribs
            tb.add("impl").ident(&struct_name).add("{");
            tb.add("pub fn new ( cx : & mut Cx , shader : Shader ) -> Self {");
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
                tb.string(&base_type.to_string()).add(" , ");
                tb.string(name).add(" , ");
                tb.ident(ty).add(" :: ty_expr ( ) ) ;");
            }

            for (name, ty) in &instances {
                tb.add("def . add_instance ( module_path ! ( ) , ");
                tb.string(&base_type.to_string()).add(" , ");
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
                tb.add(" live_item_id ! ( self :: ").ident(&base_type.to_string()).add("::").ident(&name).add(")");
                tb.add(",").string(&format!("self::{}::{}", base_type.to_string(), name)).add(") ;");
            }
            
            tb.add("} }");
            
            for (name, ty) in &uniforms {
                tb.add("pub fn").ident(&format!("set_{}", name)).add("( & mut self , cx : & mut Cx , v :").ident(ty).add(") {");
                tb.add("self .").ident(name).add("= v ;");
                tb.add("v . write_draw_input ( cx , self . area ( ) ,");
                tb.add("live_item_id ! ( self :: ").ident(&base_type.to_string()).add("::").ident(&name).add(")");
                tb.add(",").string(&format!("self::{}::{}", base_type.to_string(), name)).add(") ;");
                tb.add("}");
            }

            for (name, ty) in &instances {
                tb.add("pub fn").ident(&format!("set_{}", name)).add("( & mut self , cx : & mut Cx , v :").ident(ty).add(") {");
                tb.add("self .").ident(name).add("= v ;");
                tb.add("v . write_draw_input ( cx , self . area ( ) ,");
                tb.add("live_item_id ! ( self :: ").ident(&base_type.to_string()).add("::").ident(&name).add(")");
                tb.add(",").string(&format!("self::{}::{}", base_type.to_string(), name)).add(") ;");
                tb.add("}");
            }
            
            for (name, ty) in &uniforms {
                tb.add("pub fn").ident(&format!("with_{}", name)).add("( self ,").ident(name).add(":").ident(ty).add(") -> Self {");
                tb.add("Self {").ident(name).add(", .. self }");
                tb.add("}");
            }

            for (name, ty) in &instances {
                tb.add("pub fn").ident(&format!("with_{}", name)).add("( self ,").ident(name).add(":").ident(ty).add(") -> Self {");
                tb.add("Self {").ident(name).add(", .. self }");
                tb.add("}");
            }
            
            match draw_type {
                DrawType::DrawText => {
                    tb.add("pub fn with_draw_depth ( self , depth : f32 ) -> Self { Self { base : self . base . with_draw_depth ( depth ) , .. self } }");
                    tb.add("pub fn area ( & self ) -> Area { self . base . area ( ) }");
                    tb.add("pub fn set_area ( & mut self , area : Area ) { self . base . set_area ( area ) }");
                    tb.add("pub fn get_monospace_base ( & self , cx : & mut Cx ) -> Vec2 { self . base . get_monospace_base ( cx ) }");
                    tb.add("pub fn find_closest_offset ( & self , cx : & Cx , pos : Vec2 ) -> usize { self . base . find_closest_offset ( cx , pos ) }");
                    tb.add("pub fn draw_text ( & mut self , cx : & mut Cx , text : & str ) { self . base . draw_text ( cx , text ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn lock_aligned_text ( & mut self , cx : & mut Cx ) { self . base . lock_aligned_text ( cx ) }");
                    tb.add("pub fn lock_text ( & mut self , cx : & mut Cx ) { self . base . lock_text ( cx ) }");
                    tb.add("pub fn unlock_text ( & mut self , cx : & mut Cx ) { self . base . unlock_text ( cx ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn add_text ( & mut self , cx : & mut Cx , pos : Vec2 , text : & str ) { self . base . add_text ( cx , pos , text ) }");
                    tb.add("pub fn add_text_chunk < F > (  & mut self , cx : & mut Cx , pos : Vec2 , char_offset : usize , chunk : & [ char ] , mut char_callback : F )");
                    tb.add("where F : FnMut ( char , usize , f32 , f32 ) -> f32 { self . base . add_text_chunk ( cx , pos , char_offset , chunk , char_callback ) }");
                },
                
                DrawType::DrawQuad => {
                    // quad forward implementation
                    tb.add("pub fn with_draw_depth ( self , depth : f32 ) -> Self { Self { base : self . base . with_draw_depth ( depth ) , .. self } }");
                    tb.add("pub fn area ( & self ) -> Area { self . base . area ( ) }");
                    tb.add("pub fn set_area ( & mut self , area : Area ) { self . base . set_area ( area ) }");
                    tb.add("pub fn begin_quad ( & mut self , cx : & mut Cx , layout : Layout ) { self . base . begin_quad ( cx , layout ) }");
                    tb.add("pub fn end_quad ( & mut self , cx : & mut Cx )  { self . base . end_quad ( cx ) ; self . write_uniforms ( cx ) }");

                    tb.add("pub fn draw_quad_walk ( & mut self , cx : & mut Cx , walk : Walk )  { self . base . draw_quad_walk ( cx , walk ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad_aligned ( & mut self , cx : & mut Cx ) { self . base . draw_quad_aligned ( cx ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad ( & mut self , cx : & mut Cx ) { self . base . draw_quad ( cx ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad_rel ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_rel ( cx , rect ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad_abs ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_abs ( cx , rect ) ; self . write_uniforms ( cx ) }");

                    tb.add("pub fn lock_aligned_quad ( & mut self , cx : & mut Cx ) { self . base . lock_aligned_quad ( cx ) }");
                    tb.add("pub fn lock_quad ( & mut self , cx : & mut Cx ) { self . base . lock_quad ( cx ) }");
                    tb.add("pub fn add_quad ( & mut self ) { self . base . add_quad ( ) }");
                    tb.add("pub fn unlock_quad ( & mut self , cx : & mut Cx ) { self . base . unlock_quad ( cx ) ; self . write_uniforms ( cx ) }");
                    
                }
            }
            
            tb.add("}");
            //tb.eprint();
            return tb.end(); 
        }
    }
    parser.unexpected()
}
