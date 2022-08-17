mod ast;
mod char_class;
mod compiler;
mod cursor;
mod nfa;
mod parser;
mod program;
mod range;
mod sparse_set;
mod str;

use self::{
    ast::Ast, char_class::CharClass, compiler::Compiler, cursor::Cursor, nfa::Nfa, parser::Parser,
    program::Program, range::Range, sparse_set::SparseSet,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let ast = Parser::new().parse("a[cd]|[ab]c");
        println!("{:?}", ast);
        let program = Compiler::new().compile(&ast);
        println!("{:?}", program);
        let mut nfa = Nfa::new();
        let cursor = str::StrCursor::new("xyxxxdacz");
        let mut slots = [None; 2];
        println!("{:?}", nfa.run(&program, cursor, &mut slots));
        println!("{:?}", slots);
    }
}
