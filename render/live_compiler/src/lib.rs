#![allow(dead_code)]

pub mod util;
//pub mod math;
pub mod span;
pub mod token;
pub mod lex;
pub mod liveerror;
pub mod liveparser;
pub mod livenode;
pub mod livenodevec;
pub mod livedocument;
pub mod liveregistry;
pub mod liveexpander;

pub use makepad_id_macros::*;
pub use makepad_math::*;
pub use {
    crate::{
        liveregistry::{
            LiveRegistry,
            LiveDocNodes,
        },
        liveid::LiveModuleId,
        livenodevec::{
            LiveNodeSlice,
            LiveNodeVec,
            LiveNodeReader,
        },
        livenode::{
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
        span::Span,
        liveerror::{
            LiveError,
            LiveErrorOrigin,
            LiveFileError
        },
        livedocument::{LiveDocument}
    }
};
