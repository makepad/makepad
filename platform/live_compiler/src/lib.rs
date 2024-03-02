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
pub mod live_component;
pub mod live_node_cbor;
//pub mod live_node_cbor;
pub mod live_node_reader;

pub use makepad_math;
pub use makepad_derive_live;
pub use makepad_live_tokenizer;
pub use makepad_live_tokenizer::makepad_micro_serde;
pub use makepad_live_tokenizer::makepad_live_id;
//pub use makepad_live_id::makepad_error_log;

pub use {
    makepad_live_tokenizer::{
        LiveId,
        LiveIdMap
    },
    makepad_live_tokenizer::vec4_ext,
    crate::{
        live_component::{
            LiveComponentInfo,
            LiveComponentRegistry
        },
        live_eval::{
            live_eval,
            LiveEval
        },
        live_registry::{
            LiveFileChange,
            LiveRegistry,
            //LiveDocNodes,
        },
        live_ptr::{
            LiveModuleId,
            LivePtr,
            LiveRef,            
            LiveFileGeneration,
            LiveFileId,
        },
        live_node_vec::{
            LiveNodeSlice,
            LiveNodeVec,
            LiveNodeSliceApi,
            LiveNodeVecApi,
        },
       live_node_cbor::{
            LiveNodeSliceToCbor,
            LiveNodeVecFromCbor
        },/*
        live_node_msgpack::{
            LiveNodeSliceToMsgPack,
            L*iveNodeVecFromMsgPack
        },*/
        live_node_reader::{
            LiveNodeReader,
        },
        live_node::{
            LiveProp,
            LiveIdAsProp,
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
            LivePropType,
            LiveBinding,
            LiveIdPath,
            //LiveTypeKind,
        },
        live_token::{TokenWithSpan, LiveToken, LiveTokenId},
        span::{
            TextSpan,
            TokenSpan,
            TextPos
        },
        makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
        live_error::{
            LiveError,
            LiveFileError
        },
        live_document::{LiveOriginal, LiveExpanded}
    }
};
