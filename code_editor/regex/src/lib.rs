mod ast;
mod char_class;
mod compiler;
mod cursor;
mod dfa;
mod nfa;
mod parser;
mod program;
mod range;
mod sparse_set;
mod str;
mod utf8;

use self::{
    ast::Ast, char_class::CharClass, compiler::Compiler, cursor::Cursor, dfa::Dfa, nfa::Nfa,
    parser::Parser, program::Program, range::Range, sparse_set::SparseSet,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let ast = Parser::new().parse("(a|b)*bbxxx");
        println!("{:?}", ast);
        let program = Compiler::new().compile(
            &ast,
            compiler::Options {
                dot_star: true,
                reverse: false,
                bytes: true,
                ..compiler::Options::default()
            },
        );
        println!("{:?}", program);
        let mut dfa = Dfa::new();
        let cursor = str::StrCursor::new("ababababbxxx");
        println!("{:?}", dfa.run(&program, cursor));
    }
}
