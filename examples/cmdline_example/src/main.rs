const SOURCE: &'static str = r#"

    DrawQuad: DrawShader {
        uniform uni1: float
        instance inst1: float
        instance inst2: float
        fn shared(self)->vec4{
            return self.uni1 + self.inst1 + vec4(1.0);
        }
        
        fn pixel(self) -> vec4 {
            self.shared() ;
            return #f00;
        }
        
        fn vertex(self) -> vec4 {
            self.shared();
            return vec4(1.0+ self.inst2);
        }
    }
"#;

use makepad_live_parser::*;
use makepad_shader_compiler::shaderregistry::ShaderRegistry;
use makepad_shader_compiler::shaderregistry::DrawShaderInput;
use makepad_shader_compiler::shaderast::TyLit;

fn main() {
    let mut sr = ShaderRegistry::new();
    
    match sr.live_registry.parse_live_file("test.live", id_check!(main), id_check!(test), SOURCE.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    sr.live_registry.expand_all_documents(&mut errors);
    
    //println!("{}", lr.registry.expanded[0]);
    
    for msg in errors {
        println!("{}\n", msg.to_live_file_error("", SOURCE));
    }
    
    let mut di = DrawShaderInput::default();
    di.add_uniform("duni", TyLit::Float.to_ty_expr());
    di.add_instance("dinst", TyLit::Float.to_ty_expr());
    sr.register_draw_input("main::test", "DrawQuad", di);
    
    // lets just call the shader compiler on this thing
    let result = sr.analyse_draw_shader(id!(main), id!(test), &[id!(DrawQuad)]);
    match result {
        Err(e) => {
            println!("Error {}", e.to_live_file_error("", SOURCE));
        }
        Ok(_) => {
            println!("OK!");
        }
    }
    // ok the shader is analysed.
    // now we will generate the glsl shader.
    /*
    let result = sr.generate_glsl_shader(id!(main), id!(test), &[id!(DrawQuad)], None); //Some(FileId(0)));
    match result {
        Err(e) => {
            println!("Error {}", e.to_live_file_error("", SOURCE));
        }
        Ok((_vertex, pixel)) => {
            //println!("Vertex shader:\n{}\n\nPixel shader:\n{}", vertex,pixel);
            println!("{}", pixel);
        }
    }*/

    let result = sr.generate_metal_shader(id!(main), id!(test), &[id!(DrawQuad)], None); //Some(FileId(0)));
    match result {
        Err(e) => {
            println!("Error {}", e.to_live_file_error("", SOURCE));
        }
        Ok(shader) => {
            //println!("Vertex shader:\n{}\n\nPixel shader:\n{}", vertex,pixel);
            println!("{}", shader);
        }
    }
    
    /*
    lr.register_component(id!(main), id!(test), id!(DrawQuad), Box::new(MyShaderFactory {}));
    
    let val = lr.create_component(id!(main), id!(test), &[id!(DrawQuad)]);
    
    match val.unwrap().downcast_mut::<DrawQuad>() {
        Some(comp) => {
            println!("{:?}", comp);
        }
        None => {
            println!("No Value");
        }
    }*/
    
    // ok now we should deserialize MyObj
    // we might wanna plug the shader-compiler in some kind of deserializer as well
}
