//! Store-facing HTTP transport shim.
//!
//! The bounded byte transport is shared with other servers; only the JSON
//! response conveniences remain store-specific.

pub use makepad_bounded_http::{
    etag_matches, if_range_matches, parse_head, parse_range, reason, BodyError, BodyState,
    ChunkPhase, Conn, Head, HeadError, Method, RangeSpec, MAX_DRAIN_BYTES, MAX_HEADERS,
    MAX_HEADER_LINE, MAX_HEAD_BYTES, MAX_QUERY_PAIRS, MAX_TARGET_BYTES,
};

use super::json;
use std::ops::{Deref, DerefMut};

/// A bounded byte response with the store's JSON conveniences.
pub struct Resp(makepad_bounded_http::Resp);

impl Resp {
    pub fn empty(status: u16) -> Resp {
        Resp(makepad_bounded_http::Resp::empty(status))
    }

    pub fn json(status: u16, value: &json::Value) -> Resp {
        Resp(makepad_bounded_http::Resp {
            status,
            headers: vec![
                ("Content-Type", "application/json".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            body: value.to_json().into_bytes(),
            close: false,
        })
    }

    pub fn error(status: u16, msg: &str) -> Resp {
        Resp::json(status, &json::obj(vec![("error", json::s(msg))]))
    }

    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Resp {
        Resp(makepad_bounded_http::Resp::bytes(status, content_type, body))
    }

    pub fn with_header(mut self, key: &'static str, value: String) -> Resp {
        self.headers.push((key, value));
        self
    }

    pub fn closing(mut self) -> Resp {
        self.close = true;
        self
    }
}

impl Deref for Resp {
    type Target = makepad_bounded_http::Resp;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Resp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
