mod ast;
mod compiler;
mod cursor;
mod nfa;
mod parser;
mod program;
mod sparse_set;
mod str;

use self::{
    ast::Ast, compiler::Compiler, nfa::Nfa, parser::Parser, program::Program,
    sparse_set::SparseSet,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let ast = Parser::new().parse("ac|bc");
        println!("{:?}", ast);
        let program = Compiler::new().compile(&ast);
        println!("{:?}", program);
        let mut nfa = Nfa::new();
        let cursor = str::StrCursor::new("xyabcz");
        let mut slots = [None; 2];
        println!("{:?}", nfa.run(&program, cursor, &mut slots));
        println!("{:?}", slots);
    }
}
