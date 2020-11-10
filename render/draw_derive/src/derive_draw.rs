use proc_macro::{TokenStream};

pub enum DrawType{
    DrawQuad,
    DrawText
}

use crate::macro_lib::*;
#[derive(Clone, Debug, PartialEq)]
enum AttribType {
    Instance,
    Uniform,
    Shader,
}

struct Attrib{
    ty: AttribType,
    path: TokenStream
}

impl Attrib {
    fn parse(parser: &mut TokenParser) -> Result<Attrib, TokenStream> {
        fn parse_live_item_id(parser: &mut TokenParser) -> Result<TokenStream, TokenStream> {
            if !parser.open_paren() {
                return Err(error("expected ("))
            }
            if let Some(ret) = parser.eat_ident_path() {
                if !parser.eat_eot() {
                    return Err(error("expected )"));
                }
                return Ok(ret)
            }
            return Err(error("expected live_item_id"))
        }
        if parser.eat_punct('#') { // parse our attribute
            if !parser.open_bracket() {
                return Err(error("Expected ["))
            }
            let ret = if parser.eat_ident("instance") {
                Attrib{ty:AttribType::Instance, path: parse_live_item_id(parser) ?}
            }
            else if parser.eat_ident("uniform") {
                Attrib{ty:AttribType::Uniform, path: parse_live_item_id(parser) ?}
            }
            else if parser.eat_ident("shader") {
                Attrib{ty:AttribType::Shader, path: parse_live_item_id(parser) ?}
            }
            else {
                return Err(error("Expected instance, uniform or "))
            };
             if !parser.eat_eot() {
                return Err(error("expected ]"));
            }
            return Ok(ret)
        }
        return Err(error("Expected #[shader()] #[uniform()] or #[instance()] attribute"))
    }
}

enum ShaderType {
    Float,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4
}

impl ShaderType {
    fn parse(ts: &TokenStream) -> Result<ShaderType, TokenStream> {
        match ts.to_string().as_ref() {
            "f32" => Ok(ShaderType::Float),
            "Vec2" => Ok(ShaderType::Vec2),
            "Vec3" => Ok(ShaderType::Vec3),
            "Vec4" => Ok(ShaderType::Vec4),
            "Mat2" => Ok(ShaderType::Mat2),
            "Mat3" => Ok(ShaderType::Mat3),
            "Mat4" => Ok(ShaderType::Mat4),
            _ => Err(error("Unexpected type, only f32, Vec2, Vec3, Vec4, Mat2, Mat3, Mat4 supported"))
        }
    }
    fn slots(self) -> usize {
        match self {
            ShaderType::Float => 1,
            ShaderType::Vec2 => 2,
            ShaderType::Vec3 => 3,
            ShaderType::Vec4 => 4,
            ShaderType::Mat2 => 4,
            ShaderType::Mat3 => 9,
            ShaderType::Mat4 => 16
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
        if let Some(name) = parser.eat_any_ident() {
            // lets eat all the fields
            if !parser.open_brace() {
                return error("Expected {")
            }
            let mut fields = Vec::new();
            while !parser.eat_eot() {
                // lets eat an attrib
                let attrib = match Attrib::parse(&mut parser) {
                    Err(err) => return err,
                    Ok(attrib) => attrib
                };
                if let Some((field, ty)) = parser.eat_struct_field() {
                    fields.push((field, ty, attrib));
                    parser.eat_punct(',');
                }
            }
            // before 'base' we can only accept uniform fields
            let mut base_shader = None;
            let mut base_type = None;
            
            let mut with_shader = TokenBuilder::new();
            with_shader.add("pub fn with_shader ( cx : & mut Cx , shader : Shader , slots : usize ) -> Self {");
            with_shader.add("Self {");
            let mut total_slots = 0;
            for (field, ty, attrib) in &fields {
                if field == "base" {
                    base_type = Some(ty);
                    // only accept shader
                    if attrib.ty == AttribType::Shader {
                        base_shader = Some(attrib.path.clone());
                    }
                    else {
                        return error("base field requires a #[shader(self::shader)] attribute")
                    }
                }
                else if base_shader.is_none() { // only accept uniforms
                    if attrib.ty == AttribType::Uniform{
                        // output uniform code
                        let _shader_ty = match ShaderType::parse(&ty) {
                            Err(err) => return err,
                            Ok(ty) => ty
                        };
                        return error("Implement uniforms")
                    }
                    else {
                        return error("Before base field #[uniform(self::shader::uniformprop)] attributes are required")
                    }
                }
                else {
                    if attrib.ty == AttribType::Instance{
                        // output instance code
                        let shader_ty = match ShaderType::parse(&ty) {
                            Err(err) => return err,
                            Ok(ty) => ty
                        };
                        total_slots += shader_ty.slots();
                    }
                    else {
                        eprintln!("{:?}", attrib.ty);
                        return error("After base field using #[instance(self::shader::instanceprop)] attributes are required")
                    }
                }
            }
            let base_shader = if let Some(bs) = base_shader {bs} else {
                return error("base field not defined!")
            };
            // 'base' needs a shader field
            // and after can only accept instance fields
            
            // ok we have fields and attribs
            tb.add("impl").ident(&name).add("{");
            tb.add("pub fn new ( cx : & mut Cx ) -> Self {");
            tb.add("Self :: with_shader ( cx , live_shader ! ( cx ,").stream(Some(base_shader)).add(") , 0 )");
            tb.add("}");
            
            tb.add("pub fn with_shader ( cx : & mut Cx , shader : Shader , slots : usize ) -> Self {");
            tb.add("Self {");
            tb.add("base :").ident(&base_type.unwrap().to_string());
            tb.add(":: with_shader ( cx , shader , slots +").unsuf_usize(total_slots).add(") ,");
            // now add initializers for all values
            for (field, _ty, _attrib) in &fields {
                if field != "base"{
                    tb.ident(&field).add(": Default :: default ( ) ,");
                }
            }
            tb.add("}");
            tb.add("}");
            

            match draw_type{
                DrawType::DrawText=>{
                    tb.add("pub fn get_monospace_base ( & self , cx : & mut Cx ) -> Vec2 { self . base . get_monospace_base ( cx ) }");
                    tb.add("pub fn find_closest_offset ( & self , cx : & Cx , pos : Vec2 ) -> usize { self . base . find_closest_offset ( cx , pos ) }");
                    tb.add("pub fn draw_text ( & mut self , cx : & mut Cx , text : & str ) { self . base . draw_text ( cx , text ) }");
                    tb.add("pub fn lock_aligned_text ( & mut self , cx : & mut Cx ) { self . base . lock_aligned_text ( cx ) }");
                    tb.add("pub fn lock_text ( & mut self , cx : & mut Cx ) { self . base . lock_text ( cx ) }");
                    tb.add("pub fn unlock_text ( & mut self , cx : & mut Cx ) { self . base . unlock_text ( cx ) }");
                    tb.add("pub fn add_text ( & mut self , cx : & mut Cx , geom_x : f32 , geom_y : f32 , text : & str ) { self . base . add_text ( cx , geom_x , geom_y , text ) }");
                    tb.add("pub fn add_text_chunk < F > (  & mut self , cx : & mut Cx , geom_x : f32 , geom_y : f32 , char_offset : usize , chunk : & [ char ] , mut char_callback : F )");
                    tb.add("where F : FnMut ( char , usize , f32 , f32 ) -> f32 { self . base . add_text_chunk ( cx , geom_x , geom_y , char_offset , chunk , char_callback ) }");
                },
                DrawType::DrawQuad=>{
                    // quad forward implementation
                    tb.add("pub fn begin_quad ( & mut self , cx : & mut Cx , layout : Layout ) { self . base . begin_quad ( cx , layout ) }");
                    tb.add("pub fn end_quad ( & mut self , cx : & mut Cx ) { self . base . end_quad ( cx ) }");
                    tb.add("pub fn draw_quad ( & mut self , cx : & mut Cx , walk : Walk ) { self . base . draw_quad ( cx , walk ) }");
                    tb.add("pub fn draw_quad_rel ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_rel ( cx , rect ) }");
                    tb.add("pub fn draw_quad_abs ( & mut self , cx : & mut Cx , rect : Rect ) { self . base . draw_quad_abs ( cx , rect ) }");
                    tb.add("pub fn lock_aligned_quad ( & mut self , cx : & mut Cx ) { self . base . lock_aligned_quad ( cx ) }");
                    tb.add("pub fn lock_quad ( & mut self , cx : & mut Cx ) { self . base . lock_quad ( cx ) }");
                    tb.add("pub fn add_quad ( & mut self , rect : Rect ) { self . base . add_quad ( rect ) }");
                    tb.add("pub fn unlock_quad ( & mut self , cx : & mut Cx ) { self . base . unlock_quad ( cx ) }");
                }
            }
            
            tb.add("}");
            //tb.eprint();
            return tb.end();
        }
    }
    parser.unexpected()
}
