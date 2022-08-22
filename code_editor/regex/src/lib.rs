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

use self::{
    ast::Ast, char_class::CharClass, compiler::Compiler, cursor::Cursor, dfa::Dfa, nfa::Nfa, parser::Parser,
    program::Program, range::Range, sparse_set::SparseSet,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let ast = Parser::new().parse("ab|bc");
        println!("{:?}", ast);
        let program = Compiler::new().compile(
            &ast,
            compiler::Options {
                bytewise: true,
                ..compiler::Options::default()
            },
        );
        println!("{:?}", program);
        let mut dfa = Dfa::new();
        let cursor = str::StrCursor::new("abcz");
        println!("{:?}", dfa.run(&program, cursor));
    }
}
