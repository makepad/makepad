//! relax-lex: Rust lexer and token model for Relax and native procedural macros.
//! `lexer` is Relax's lexer (source bytes -> flat `Tok` list); `tokens` is Relax's proc-macro token model
//! (TokenStream/Group/Ident/Punct/Literal/Span). Std only; this directory is copied verbatim into Relax.

pub mod lexer;
pub mod packed;
pub mod tokens;
pub mod validate;
