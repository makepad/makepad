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
    struct Cx {
        pos: vec2
    }

    impl Cx {
        fn foo() -> Cx {
            let cx: Cx;
            uUniform1;
            return cx;
        }

        fn bar(inout self) {
            aPosition;
            vColor;
            -self.pos;
        }

        fn qux(self, x: float) -> float {
            aPosition;
            vColor;
            tTextureFoo;
            return 2.0 * x;
        }

        fn translate(inout self, x: float, y: float) -> vec2 {
            self.pos -= vec2(x, y);
            return self.pos;
        }
    }

    const FOO: float = 1.0;

    uniform uUniform1: Self::my_uniform();
    uniform uUniform2: Self::my_uniform() in draw;

    texture tTextureFoo: Self::my_texture();
    texture tTextureBar: Self::my_texture();

    geometry aPosition: Self::my_geometry();
    geometry aColor: Self::my_geometry();

    instance iRotation: Self::my_instance();

    varying vColor: vec3;

    fn vertex() -> vec4 {
        pick!(red);
        let cx = Cx::foo();
        for i from 0 to 10 step 2 {
            vec4(1.0, vec2(2.0, 3.0), 4.0);
        }
        return vec4(1.0);
    }

    fn pixel() -> vec4 {
        let cx = Cx::foo();
        cx.qux(10.0 + 20.0);
        return vec4(1.0);
    }
"#;

#[test]
fn test() {
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
        ],
        false,
    )
    .unwrap();
    //println!("GLSL VERTEX");
    //println!("{}", generate_glsl::generate_vertex_shader(&shader, true));
    //println!("GLSL FRAGMENT");
    //println!("{}", generate_glsl::generate_fragment_shader(&shader, true));
    //println!("METAL");
    //println!("{}", generate_metal::generate_shader(&shader, false));
    println!("HLSL");
    println!("{}", generate_hlsl::generate_shader(&shader, false));
}
