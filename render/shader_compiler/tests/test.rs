use makepad_shader_compiler::analyse;
use makepad_shader_compiler::ast::Shader;
use makepad_shader_compiler::generate::{self, ShaderKind};
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse;

const SOURCE: &str = r#"
    struct Foo {
        x: float,
        y: float
    }

    struct Bar {
        foo: Foo
    }

    const FOO: int = 10;
    const BAR: int = FOO * 2;
    
    attribute aPosition: vec2;
    attribute aBla: float;
    attribute aColor: vec3;

    instance iDisplacement: vec4;
    
    varying vColor: vec3;

    uniform uModelViewMatrix: A::B::C() in draw;

    fn vertex() -> vec4 {
        foo();
        max(vec2(1.0), vec2(2.0));
    }

    fn fragment() -> vec4 {
        bar();
        aBla;
    }

    fn foo() {
        qux();
        vColor = vec3(1.0);
        vec4(vec2(1.0), 1.0, 2.0);
    }

    fn bar() {
        qux();
        aPosition;
        mat4(mat3(1.0));
    }

    fn qux() {
        aColor;
        let x: float;
        x = 42.0;
        uModelViewMatrix * vec4(1.0);
    }
"#;

#[test]
fn test() {
    let mut shader = Shader::new();
    parse::parse(
        &lex::lex(SOURCE.chars())
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        &mut shader,
    )
    .unwrap();
    analyse::analyse(&mut shader).unwrap();
    println!("{}", generate::generate(ShaderKind::Vertex, &shader));
    println!("{}", generate::generate(ShaderKind::Fragment, &shader));
}
