#![allow(dead_code)]

pub mod util;
//pub mod math;
pub mod span;
pub mod token;
pub mod lex;
pub mod live_error;
pub mod live_parser;
pub mod live_node;
pub mod live_node_vec;
pub mod live_document;
pub mod live_registry; 
pub mod live_expander;
pub mod live_id;

pub use makepad_id_macros::*;
pub use makepad_math::*;
pub use {
    crate::{
        live_registry::{
            LiveRegistry,
            LiveDocNodes,
        },
        live_id::{
            LiveModuleId,
            LiveId,
            LivePtr,
            LiveFileId,
        },
        live_node_vec::{
            LiveNodeSlice,
            LiveNodeVec,
            LiveNodeReader,
        },
        live_node::{
            LiveEditInfo,
            LiveValue,
            LiveNode,
            LiveType,
            LiveTypeInfo,
            LiveTypeField,
            LiveFieldKind,
            LiveBinOp,
            LiveUnOp,
            LiveNodeOrigin,
            InlineString,
            FittedString,
            LiveTypeKind,
        },
        token::{TokenWithSpan, Token, TokenId},
        span::{
            Span,
            TextPos
        },
        live_error::{
            LiveError,
            LiveErrorOrigin,
            LiveFileError
        },
        live_document::{LiveDocument}
    }
};
