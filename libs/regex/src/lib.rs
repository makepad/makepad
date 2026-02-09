#![allow(dead_code)]

mod ast;
mod case_fold;
mod char;
mod char_class;
mod compile;
mod dfa;
mod error;
mod input;
mod leb128;
mod nfa;
mod parse;
mod prog;
mod range;
mod regex;
mod sparse_set;
mod str;
mod unicode;
mod utf8;

pub use self::{
    error::{Error, Result},
    input::{Cursor, Input},
    parse::Options as ParseOptions,
    regex::Regex,
};

#[cfg(test)]
mod tests;
