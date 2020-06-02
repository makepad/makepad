use mpsc::ast::*;
use mpsc::emit::Emitter;
use mpsc::lex;
use mpsc::parse::Parser;
use std::error::Error;

const SOURCE: &str = r#"
    fn vertex() {}

    fn fragment() -> vec4 {
        foo();
        return vec4(1.0);
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
