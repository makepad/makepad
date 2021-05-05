#![allow(unused_variables)]
#![allow(dead_code)]
use makepad_live_parser::*;
use makepad_live_parser::liveregistry::LiveFactoryTest;
use makepad_live_parser::liveregistry::LiveFactoriesTest;
use std::any::Any;

#[derive(Clone, Debug)]
struct DrawQuad{
}

impl DeLive for DrawQuad{
    fn de_live(lr: &LiveRegistry, file: usize, level: usize, index: usize) -> Result<Self,
    DeLiveErr>{
        // ok lets parse this
        
        
        return Ok(DrawQuad{})
    }
}

struct MyShaderFactory {}
impl LiveFactoryTest for MyShaderFactory {
    fn de_live_any(&self, lr: &LiveRegistry, file: usize, level: usize, index: usize) -> Result<Box<dyn Any>,
    DeLiveErr> {
        // lets give the shader compiler out component.
        // alright so.. lets parse the shader
        let mv = DrawQuad::de_live(lr, file, level, index) ?;
        Ok(Box::new(mv))
    }
}

fn main() {
    println!("{}", std::mem::size_of::<LiveNode>());
    // ok lets do a deserialize
    let mut lr = LiveFactoriesTest::default();
    let source = r#"
        ViewShader: Shader{
            uniform camera_projection: mat4 in pass;
            uniform draw_scroll: vec4 in draw;
            instance y: float
        }
        
        // what does this thing factory?
        DrawQuad: ViewShader{
            // these point to things in Rust
            draw_input self::DrawQuad;
            default_geometry makepad_render::shader_std::quad_2d;
            
            geometry geom: vec2;
            instance x: float
            instance y: float
            uniform z: float
            varying w: float

            myStruct:Struct{
                field x:float,
                field y:float
                fn bla(self){}
            }
            
            fn pixel()->vec4{
                return #f00
            }
            
            fn vertex()->vec4{
                return vec4(1.0)
            }
        }
    "#;
    match lr.registry.parse_live_file("test.live", id_check!(main), id_check!(test), source.to_string()) {
        Err(why) => panic!("Couldnt parse file {}", why),
        _ => ()
    }
    
    let mut errors = Vec::new();
    lr.registry.expand_all_documents(&mut errors);
    
    println!("{}", lr.registry.expanded[0]);
    
    for msg in errors {
        println!("{}\n", msg.to_live_file_error("", source));
    }
    
    lr.register_component(id!(main), id!(test), id!(DrawQuad), Box::new(MyShaderFactory {}));
    let val = lr.create_component(id!(main), id!(test), &[id!(DrawQuad)]);
    
    match val.unwrap().downcast_mut::<DrawQuad>() {
        Some(comp) => {
            println!("{:?}", comp);
        }
        None => {
            println!("No Value");
        }
    }
    
    // ok now we should deserialize MyObj
    // we might wanna plug the shader-compiler in some kind of deserializer as well
}
