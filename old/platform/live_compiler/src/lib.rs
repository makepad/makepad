#![allow(dead_code)]

pub mod live_component;
pub mod live_document;
pub mod live_error;
pub mod live_eval;
pub mod live_expander;
pub mod live_node;
pub mod live_node_cbor;
pub mod live_node_vec;
pub mod live_parser;
pub mod live_ptr;
pub mod live_registry;
pub mod live_token;
pub mod span;
pub mod util;
//pub mod live_node_cbor;
pub mod live_node_reader;

pub use makepad_derive_live;
pub use makepad_live_tokenizer;
pub use makepad_live_tokenizer::makepad_live_id;
pub use makepad_live_tokenizer::makepad_micro_serde;
pub use makepad_math;
//pub use makepad_live_id::makepad_error_log;

pub use {
    crate::{
        live_component::{LiveComponentInfo, LiveComponentRegistry},
        live_document::{LiveExpanded, LiveOriginal},
        live_error::{LiveError, LiveFileError},
        live_eval::live_eval_value,
        live_node::{
            InlineString,
            LiveBinOp,
            LiveBinding,
            LiveEditInfo,
            LiveFieldKind,
            LiveFont,
            LiveIdAsProp,
            LiveIdPath,
            //LiveTypeKind,
            LiveImport,
            LiveNode,
            LiveNodeOrigin,
            LiveProp,
            LivePropType,
            LiveType,
            LiveTypeField,
            LiveTypeInfo,
            LiveUnOp,
            LiveValue,
        },
        live_node_cbor::{LiveNodeSliceToCbor, LiveNodeVecFromCbor}, /*
                                                                    live_node_msgpack::{
                                                                        LiveNodeSliceToMsgPack,
                                                                        L*iveNodeVecFromMsgPack
                                                                    },*/
        live_node_reader::LiveNodeReader,
        live_node_vec::{LiveNodeSlice, LiveNodeSliceApi, LiveNodeVec, LiveNodeVecApi},
        live_ptr::{LiveFileGeneration, LiveFileId, LiveModuleId, LivePtr, LiveRef},
        live_registry::{
            LiveFileChange,
            LiveRegistry,
            LiveScopeTarget,
            //LiveDocNodes,
        },
        live_token::{LiveToken, LiveTokenId, TokenWithSpan},
        makepad_live_tokenizer::{live_error_origin, LiveErrorOrigin},
        span::{TextPos, TextSpan, TokenSpan},
    },
    makepad_live_tokenizer::vec4_ext,
    makepad_live_tokenizer::{LiveId, LiveIdMap},
};
