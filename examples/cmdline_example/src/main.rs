const SOURCE: &'static str = r#"
    DrawQuad: DrawShader {
        draw_input self::DrawQuad;
        uniform uni1: float
        const cnst1: float = 1.0
        live: 1.0
        
        Struct2: Struct {
            field c: float
            fn struct_2_set(inout self, x: float) {self.c = x;}
        }
        
        Struct1: Struct {
            field a: float
            field b: Struct2
            fn struct_1_closure(inout self, c: fn(x: float) -> float) -> float {
                return c(self.a);
            }
            fn struct_1_set(inout self) {self.a = 2.0; self.b.struct_2_set(self.struct_1_closure( | x | x + 1.0));}
        }
        
        fn pixel(self) -> vec4 {
            let x = Struct1 {a: 1.0, b: Struct2 {c: 1.0 + self.dinst}};
            x.struct_1_set();
            let t = self.dmat;
            return #f;
        }
        
        fn override(self) {}
        
        fn vertex(self) -> vec4 {
            let x = Struct2 {c: self.uni1 + cnst1 + live};
            x.struct_2_set(3.0);
            self.override();
            let y = vec4(1.0);
            return #f;
        }
    }
    DrawQuad2: DrawQuad {
        fn override(self) {1 + 1;}
    }
"#;

use makepad_live_parser::*;
use makepad_shader_compiler::shaderregistry::ShaderRegistry;
use makepad_shader_compiler::shaderregistry::DrawShaderInput;
use makepad_shader_compiler::shaderast::Ty;

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
    di.add_uniform("duni", Ty::Float);
    di.add_instance("dinst", Ty::Float);
    di.add_instance("dmat", Ty::Mat4);
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

    let result = sr.generate_hlsl_shader(id!(main), id!(test), &[id!(DrawQuad)], None); //Some(FileId(0)));
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
