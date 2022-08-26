mod ast;
mod char_class;
mod compiler;
mod cursor;
mod dfa;
mod nfa;
mod parser;
mod program;
mod range;
mod regex;
mod sparse_set;
mod str_cursor;
mod utf8;

pub use self::regex::Regex;

use self::{
    ast::Ast, char_class::CharClass, compiler::Compiler, cursor::Cursor, dfa::Dfa, nfa::Nfa,
    parser::Parser, program::Program, range::Range, sparse_set::SparseSet, str_cursor::StrCursor,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let regex = Regex::new("abc");
        println!("{:?}", regex.find("xxxabcyyy"));
    }
}
