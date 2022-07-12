mod ast;
mod input;
mod generator;
mod nfa;
mod parser;
mod program;
mod sparse_set;

use {
    self::{ast::Ast, nfa::Nfa, program::Program, sparse_set::SparseSet},
    std::{cell::RefCell, result, sync::Arc},
};

pub struct Regex {
    unique: RefCell<Unique>,
    shared: Arc<Shared>,
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Self> {
        let ast = parser::parse(pattern)?;
        let nfa_program = generator::generate(&ast);
        Ok(Self {
            unique: RefCell::new(Unique {
                nfa: Nfa::new(&nfa_program),
            }),
            shared: Arc::new(Shared { nfa_program }),
        })
    }

    pub fn run<I: Input>(&self, input: I) -> bool {
        
    }
}

pub type Result<T> = result::Result<T, Error>;

pub enum Error {
    Parser(parser::Error),
}

impl From<parser::Error> for Error {
    fn from(error: parser::Error) -> Self {
        Self::Parser(error)
    }
}

struct Unique {
    nfa: Nfa,
}

struct Shared {
    nfa_program: Program,
}
