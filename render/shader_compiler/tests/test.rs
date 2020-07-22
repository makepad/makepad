use makepad_shader_compiler::analyse;
use makepad_shader_compiler::ast::ShaderAst;
use makepad_shader_compiler::generate_metal;
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse;
use makepad_shader_compiler::shadergen::*;
use makepad_shader_compiler::uid;

const SOURCE: &str = r#"
    struct Cx {
        prop: float
    }

    impl Cx {
        fn foo() -> Cx {
            let cx: Cx;
            return cx;
        }

        fn bar(self) {
            tTexture;
            -self.prop;
        }

        fn qux(self, x: float) -> float {
            aPosition;
            return 2.0 * x;
            self.bar();
        }
    }

    texture tTexture: Self::my_texture();

    attribute aPosition: Self::my_attribute();
    attribute aColor: Self::my_attribute();

    instance iRotation: Self::my_instance();

    varying vColor: vec3;

    fn vertex() -> vec4 {
        let cx = Cx::foo();
        cx.bar();
        for i from 0 to 10 step 2 {
            vec4(1.0, vec2(2.0, 3.0), 4.0);
        }
        return vec4(1.0);
    }

    fn pixel() -> vec4 {
        let cx = Cx::foo();
        cx.qux(10.0);
        return vec4(1.0);
    }
"#;

#[test]
fn test() {
    fn my_texture() -> Texture2dId { uid!() }
    fn my_attribute() -> Vec3Id { uid!() }
    fn my_instance() -> Vec3Id { uid!() }

    let mut shader = ShaderAst::new();
    parse::parse(
        &lex::lex(SOURCE.chars(), 0)
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        &mut shader,
    )
    .unwrap();
    analyse::analyse(&mut shader, &[
        &PropDef {
            name: String::from("my_texture"),
            ident: String::from("Self::my_texture"),
            prop_id: my_texture().into()
        },
        &PropDef {
            name: String::from("my_attribute"),
            ident: String::from("Self::my_attribute"),
            prop_id: my_attribute().into()
        },
        &PropDef {
            name: String::from("my_instance"),
            ident: String::from("Self::my_instance"),
            prop_id: my_instance().into()
        }
    ]).unwrap();
    println!("{}", generate_metal::generate_shader(&shader));
}
