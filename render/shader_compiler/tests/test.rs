use makepad_shader_compiler::ast::*;
use makepad_shader_compiler::emit::Emitter;
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse::Parser;
use std::error::Error;

const SOURCE: &str = r#"
    attribute aPosition: vec2;
    
    varying vColor: vec3;
    varying vTexCoord: vec4;

    fn vertex() -> vec4 {
        vColor = vec3(aPosition, 1.0);
        return vec4(aPosition, 0.0, 1.0);
    }

    fn fragment() -> vec4 {
        return vec4(vColor, 1.0);
    }

    fn foo() {
        fragment();
    }
"#;

#[test]
fn test() {
    if let Err(ref error) = (|| -> Result<(), Box<dyn Error>> {
        let tokens = lex::lex(SOURCE.chars()).collect::<Result<Vec<_>, _>>()?;
        let shader = Shader::parse(&mut Parser::new(tokens.iter().cloned()))?;
        println!("{}", shader.emit(&mut Emitter::new())?.string);
        Ok(())
    })() {
        println!("{}", error);
    }
}
