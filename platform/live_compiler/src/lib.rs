#![allow(dead_code)]

pub mod util;
pub mod span;
pub mod live_token;
pub mod live_error;
pub mod live_parser;
pub mod live_node;
pub mod live_node_vec;
pub mod live_document;
pub mod live_registry; 
pub mod live_expander;
pub mod live_ptr;
pub mod live_eval;

pub use makepad_id_macros::*;
pub use makepad_math;
pub use makepad_derive_live;
pub use makepad_live_tokenizer;
pub use makepad_id_macros;
pub use makepad_live_tokenizer::makepad_micro_serde;

pub use {
    makepad_live_tokenizer::{
        LiveId,
        LiveIdMap
    },
    makepad_live_tokenizer::vec4_ext,
    crate::{
        live_eval::{
            live_eval,
            LiveEval
        },
        live_registry::{
            LiveEditEvent,
            LiveRegistry,
            LiveDocNodes,
        },
        live_ptr::{
            LiveModuleId,
            LivePtr,
            LiveFileGeneration,
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
            //LiveTypeKind,
        },
        live_token::{TokenWithSpan, LiveToken, LiveTokenId},
        span::{
            TextSpan,
            TokenSpan,
            TextPos
        },
        live_error::{
            LiveError,
            LiveErrorOrigin,
            LiveFileError
        },
        live_document::{LiveOriginal, LiveExpanded}
    }
};
