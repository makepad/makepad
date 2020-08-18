/*
use std::io::{Read, Write};
use std::fs::File;
use image_formats::jpeg;
use image_formats::bmp;

fn load(name: &str) {
    println!("loading {}...",name);
    let mut infile = File::open(&name).unwrap();
    let mut buffer = Vec::new();
    infile.read_to_end(&mut buffer).unwrap();
    match jpeg::decode(&buffer) {
        Ok(image) => {
            let outname = (&name[0 .. name.len() - 4]).to_string() + ".bmp";
            match bmp::encode(&image) {
                Ok(value) => {
                    let mut outfile = File::create(&outname).unwrap();
                    outfile.write_all(&value).unwrap();
                },
                Err(msg) => {
                    println!("    Error: {}",msg);
                }
            };
        },
        Err(msg) => {
            println!("    Error: {}",msg);
        }
    }
}

fn main() {
   load("./examples/cmdline_example/test.jpg");
}
*/
use makepad_shader_compiler::analyse;
use makepad_shader_compiler::ast::ShaderAst;
use makepad_shader_compiler::generate_glsl;
use makepad_shader_compiler::generate_metal;
use makepad_shader_compiler::generate_hlsl;
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse;
use makepad_shader_compiler::shadergen::*;
use makepad_shader_compiler::uid;

const SOURCE: &str = r#"
    geometry aPosition: Self::my_geometry();
    geometry aColor: Self::my_geometry();

    instance iRotation: Self::my_instance();
    instance iMat: Self::my_mat4();

    varying vColor: vec3;

    fn vertex() -> vec4 {
        return vec4(1.0);
    }

    fn pixel() -> vec4 {
        let x = iMat;
        return vec4(1.0);
    }
"#;

fn main() {
    fn my_uniform() -> Mat4Id {
        uid!()
    }
    fn my_texture() -> Texture2dId {
        uid!()
    }
    fn my_geometry() -> Vec3Id {
        uid!()
    }
    fn my_instance() -> Vec3Id {
        uid!()
    }
    fn my_mat4() -> Mat4Id {
        uid!()
    } 
    let mut shader = ShaderAst::new();
    parse::parse(
        &lex::lex(SOURCE.chars(), 0)
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        &mut shader,
    )
    .unwrap();
    analyse::analyse(
        &mut shader,
        &[
            &PropDef {
                name: String::from("my_uniform"),
                ident: String::from("Self::my_uniform"),
                prop_id: my_uniform().into(),
                block: None,
            },
            &PropDef {
                name: String::from("my_texture"),
                ident: String::from("Self::my_texture"),
                prop_id: my_texture().into(),
                block: None,
            },
            &PropDef {
                name: String::from("my_geometry"),
                ident: String::from("Self::my_geometry"),
                prop_id: my_geometry().into(),
                block: None,
            },
            &PropDef {
                name: String::from("my_instance"),
                ident: String::from("Self::my_instance"),
                prop_id: my_instance().into(),
                block: None,
            },
            &PropDef {
                name: String::from("my_mat4"),
                ident: String::from("Self::my_mat4"),
                prop_id: my_mat4().into(),
                block: None,
            },
        ],
        false,
    )
    .unwrap();
    println!("GLSL VERTEX");
    println!("{}", generate_glsl::generate_vertex_shader(&shader, true));
    println!("GLSL FRAGMENT");
    println!("{}", generate_glsl::generate_fragment_shader(&shader, true));
    //println!("METAL");
    //println!("{}", generate_metal::generate_shader(&shader, false));
    //println!("HLSL");
    //println!("{}", generate_hlsl::generate_shader(&shader, false));
}
