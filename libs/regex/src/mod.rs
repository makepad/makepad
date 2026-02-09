#![allow(dead_code)]

mod ast;
mod case_fold;
mod char;
mod char_class;
mod compile;
mod input;
mod dfa;
mod error;
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

pub use self::{error::{Error, Result}, input::{Cursor, Input}, regex::Regex};

#[cfg(test)]
mod tests;
