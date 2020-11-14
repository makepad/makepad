use proc_macro::{TokenStream};
use crate::macro_lib::*;

pub enum DrawType {
    DrawQuad,
    DrawText
}

#[derive(PartialEq)]
enum ShaderType {
    Float,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
    Texture2D
}

impl ShaderType {
    fn parse(ts: &str) -> Result<ShaderType, TokenStream> {
        match ts{
            "f32" => Ok(ShaderType::Float),
            "Vec2" => Ok(ShaderType::Vec2),
            "Vec3" => Ok(ShaderType::Vec3),
            "Vec4" => Ok(ShaderType::Vec4),
            "Mat2" => Ok(ShaderType::Mat2),
            "Mat3" => Ok(ShaderType::Mat3),
            "Mat4" => Ok(ShaderType::Mat4),
            "Texture2D" => Ok(ShaderType::Texture2D),
            _ => Err(error("Unexpected type, only f32, Vec2, Vec3, Vec4, Mat2, Mat3, Mat4 supported"))
        }
    }

    fn slots(self) -> usize {
        match self {
            ShaderType::Texture2D => 0,
            ShaderType::Float => 1,
            ShaderType::Vec2 => 2,
            ShaderType::Vec3 => 3,
            ShaderType::Vec4 => 4,
            ShaderType::Mat2 => 4,
            ShaderType::Mat3 => 9,
            ShaderType::Mat4 => 16
        }
    }
    
    fn to_uniform_write(self) -> &'static str {
        match self {
            ShaderType::Texture2D => panic!("invalid"),
            ShaderType::Float => "write_uniform_float",
            ShaderType::Vec2 => "write_uniform_vec2",
            ShaderType::Vec3 => "write_uniform_vec3",
            ShaderType::Vec4 => "write_uniform_vec4",
            ShaderType::Mat2 => "write_uniform_mat2",
            ShaderType::Mat3 => "write_uniform_mat3",
            ShaderType::Mat4 => "write_uniform_mat4",
        }
    }
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
             
            let mut total_slots = 0;
            let mut base_type = None;
            let mut default_shader = None;
            let mut uniforms = Vec::new();
            let mut instances = Vec::new();
            let mut textures = Vec::new();
            
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
                    let _st = match ShaderType::parse(&field.ty.to_string()){
                        Err(e)=>return e,
                        Ok(st)=>{
                            if st == ShaderType::Texture2D{
                                if !base_type.is_none(){
                                    return error("Texture2D not allowed in instance position, please move before 'base'");
                                }
                                textures.push((field.name.clone(), field.ty.to_string()));
                            }
                            else{
                                if base_type.is_none(){
                                    uniforms.push((field.name.clone(), field.ty.to_string()));
                                }
                                else{
                                    total_slots += st.slots();
                                    instances.push((field.name.clone(), field.ty.to_string()));
                                }
                            }
                        }
                    };
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
            tb.add("Self :: with_slot_count ( cx , default_shader_overload ! ( cx , shader ,").stream(default_shader).add(") , 0 )");
            tb.add("}");
            
            tb.add("pub fn with_slot_count ( cx : & mut Cx , shader : Shader , slots : usize ) -> Self {");
            tb.add("Self {");
            tb.add("base :").ident(&base_type.to_string());
            tb.add(":: with_slot_count ( cx , shader , slots +").unsuf_usize(total_slots).add(") ,");
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
                tb.string(name).add(" , ");
                tb.string(ty).add(" ) ;");
            }

            for (name, ty) in &instances {
                tb.add("def . add_instance ( module_path ! ( ) , ");
                tb.string(name).add(" , ");
                tb.string(ty).add(" ) ;");
            }

            for (name, ty) in &textures {
                tb.add("def . add_texture ( module_path ! ( ) , ");
                tb.string(name).add(" , ");
                tb.string(ty).add(" ) ;");
            }

            tb.add("return def ; }");
            
            tb.add("pub fn register_draw_input ( cx : & mut Cx ) {");
            tb.add("cx . live_styles . register_draw_input ( live_item_id ! (");
            tb.add("self ::").ident(&struct_name).add(") , Self :: live_draw_input ( ) ) ;");
            tb.add("}");
            
            tb.add("pub fn write_uniforms ( & self , cx : & mut Cx ) {");
            tb.add("if self . area ( ) . need_uniforms_now ( ) {");
            for (name, ty) in &uniforms {
                let st = ShaderType::parse(&ty).unwrap();
                tb.add("self . area ( ) .").ident(st.to_uniform_write()).add("( cx , live_item_id ! (");
                tb.add("self :: ").ident(&base_type.to_string()).add("::").ident(&name);
                tb.add(") , self .").ident(&name).add(") ;");
            }
            
            tb.add("} }");
            
            
            match draw_type {
                DrawType::DrawText => {
                    tb.add("pub fn area ( & self ) -> Area { self . base . area ( ) }");
                    tb.add("pub fn get_monospace_base ( & self , cx : & mut Cx ) -> Vec2 { self . base . get_monospace_base ( cx ) }");
                    tb.add("pub fn find_closest_offset ( & self , cx : & Cx , pos : Vec2 ) -> usize { self . base . find_closest_offset ( cx , pos ) }");
                    tb.add("pub fn draw_text ( & mut self , cx : & mut Cx , text : & str ) -> bool { if self . base . draw_text ( cx , text ) { self . write_uniforms ( cx ) } else { false } }");
                    tb.add("pub fn lock_aligned_text ( & mut self , cx : & mut Cx ) { self . base . lock_aligned_text ( cx ) }");
                    tb.add("pub fn lock_text ( & mut self , cx : & mut Cx ) { self . base . lock_text ( cx ) }");
                    tb.add("pub fn unlock_text ( & mut self , cx : & mut Cx ) { self . base . unlock_text ( cx ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn add_text ( & mut self , cx : & mut Cx , geom_x : f32 , geom_y : f32 , text : & str ) { self . base . add_text ( cx , geom_x , geom_y , text ) }");
                    tb.add("pub fn add_text_chunk < F > (  & mut self , cx : & mut Cx , geom_x : f32 , geom_y : f32 , char_offset : usize , chunk : & [ char ] , mut char_callback : F )");
                    tb.add("where F : FnMut ( char , usize , f32 , f32 ) -> f32 { self . base . add_text_chunk ( cx , geom_x , geom_y , char_offset , chunk , char_callback ) }");
                },
                DrawType::DrawQuad => {
                    // quad forward implementation
                    tb.add("pub fn area ( & self ) -> Area { self . base . area ( ) }");
                    tb.add("pub fn begin_quad ( & mut self , cx : & mut Cx , layout : Layout ) { self . base . begin_quad ( cx , layout ) }");
                    tb.add("pub fn end_quad ( & mut self , cx : & mut Cx )  { self . base . end_quad ( cx ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad ( & mut self , cx : & mut Cx , walk : Walk )  { self . base . draw_quad ( cx , walk ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad_rel ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_rel ( cx , rect ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn draw_quad_abs ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_abs ( cx , rect ) ; self . write_uniforms ( cx ) }");
                    tb.add("pub fn lock_aligned_quad ( & mut self , cx : & mut Cx ) { self . base . lock_aligned_quad ( cx ) }");
                    tb.add("pub fn lock_quad ( & mut self , cx : & mut Cx ) { self . base . lock_quad ( cx ) }");
                    tb.add("pub fn add_quad ( & mut self , rect : Rect ) { self . base . add_quad ( rect ) }");
                    tb.add("pub fn unlock_quad ( & mut self , cx : & mut Cx ) { self . base . unlock_quad ( cx ) ; self . write_uniforms ( cx ) }");
                }
            }
            
            tb.add("}");
            tb.eprint();
            return tb.end();
        }
    }
    parser.unexpected()
}
