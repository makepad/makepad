use makepad_shader_compiler::analyse;
use makepad_shader_compiler::ast::ShaderAst;
use makepad_shader_compiler::generate::{self, ShaderKind};
use makepad_shader_compiler::lex;
use makepad_shader_compiler::parse;

const SOURCE: &str = r#"
    struct Cx {}

    impl Cx {
        fn foo() {

        }

        fn bar(self) {

        }
    }

    fn vertex() -> vec4 {
        let cx: Cx;
        cx.bar();
    }

    fn fragment() -> vec4 {
        
    }
"#;

#[test]
fn test() {
    let mut shader = ShaderAst::new();
    parse::parse(
        &lex::lex(SOURCE.chars(), 0)
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        &mut shader,
    )
    .unwrap();
    analyse::analyse(&mut shader, &Vec::new()).unwrap();
    println!("{}", generate::generate(ShaderKind::Vertex, &shader));
    println!("{}", generate::generate(ShaderKind::Fragment, &shader));
}
