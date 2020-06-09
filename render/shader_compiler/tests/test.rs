use makepad_shader_compiler::ast::*;
use makepad_shader_compiler::emit::Emitter;
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse::Parser;
use std::error::Error;

const SOURCE: &str = r#"
    uniform uModelViewMatrix: mat4;
    uniform uPerspectiveMatrix: mat2 block draw;

    attribute aPosition: vec4;
    attribute aColor: vec3;
    attribute aTexCoord: vec2;

    varying vColor: vec3;
    varying vTexCoord: vec2;
    varying vMatrix: mat4;

    fn vertex() -> vec4 {
        foo();
        vColor = aColor;
        return vec4(uModelViewMatrix * aPosition);
    }

    fn fragment() -> vec4 {
        bar();
        return vec4(vColor, 1.0);
    }

    fn foo() {
        uPerspectiveMatrix;
        qux();
    }

    fn bar() {
        qux();
    }

    fn qux() {}
"#;

#[test]
fn test() {
    if let Err(ref error) = (|| -> Result<(), Box<dyn Error>> {
        let tokens = lex::lex(SOURCE.chars()).collect::<Result<Vec<_>, _>>()?;
        let shader = ParsedShader::parse(&mut Parser::new(tokens.iter().cloned()))?;
        let shader_attrs = shader.emit(&mut Emitter::new())?;
        println!("VERTEX SHADER:");
        println!("{}", shader_attrs.vertex_string);
        println!("FRAGMENT SHADER:");
        println!("{}", shader_attrs.fragment_string);
        Ok(())
    })() {
        println!("{}", error);
    }
}
