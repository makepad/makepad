#![allow(dead_code)]

pub mod util;
pub mod math;
pub mod span;
pub mod token;
pub mod lex;
pub mod liveerror;
pub mod liveparser;
pub mod livenode;
pub mod livedocument;
pub mod liveregistry;
pub mod liveexpander;
pub mod liveid;

pub use makepad_id_macros::*;
pub use {
    crate::{
        math::*,
        liveid::{LiveId, LivePtr, LiveFileId},
        liveregistry::{LiveRegistry, LiveDocNodes},
        liveid::LiveModuleId,
        livenode::{
            LiveValue,
            LiveNode,
            LiveType,
            LiveNodeSlice,
            LiveNodeVec,
            LiveTypeInfo,
            LiveTypeField,
            LiveFieldKind,
            LiveNodeReader,
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
        util::PrettyPrintedF32,
        livedocument::{LiveScopeItem, LiveDocument, LiveScopeTarget}
    }
};