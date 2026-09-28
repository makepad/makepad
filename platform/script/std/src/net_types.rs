//! Script-facing mirrors of the makepad-network request/response types.
//!
//! makepad-network stays free of makepad-script (so it compiles in parallel
//! with the script crate); the `net` script module converts between these
//! mirrors and the network types at the boundary. The mirrors keep the type
//! and field names scripts see (`net.HttpRequest{...}`, `res.status_code`).

use crate::makepad_network as network;
use makepad_script::*;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WebSocketTransport {
    #[pick]
    #[default]
    Auto,
    PlainTcp,
    Platform,
}

impl From<WebSocketTransport> for network::WebSocketTransport {
    fn from(v: WebSocketTransport) -> Self {
        match v {
            WebSocketTransport::Auto => Self::Auto,
            WebSocketTransport::PlainTcp => Self::PlainTcp,
            WebSocketTransport::Platform => Self::Platform,
        }
    }
}

#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(clippy::upper_case_acronyms)]
pub enum HttpMethod {
    #[pick]
    #[default]
    GET,
    HEAD,
    POST,
    PUT,
    DELETE,
    CONNECT,
    OPTIONS,
    TRACE,
    PATCH,
}

impl From<HttpMethod> for network::HttpMethod {
    fn from(v: HttpMethod) -> Self {
        match v {
            HttpMethod::GET => Self::GET,
            HttpMethod::HEAD => Self::HEAD,
            HttpMethod::POST => Self::POST,
            HttpMethod::PUT => Self::PUT,
            HttpMethod::DELETE => Self::DELETE,
            HttpMethod::CONNECT => Self::CONNECT,
            HttpMethod::OPTIONS => Self::OPTIONS,
            HttpMethod::TRACE => Self::TRACE,
            HttpMethod::PATCH => Self::PATCH,
        }
    }
}

#[derive(Script, ScriptHook, Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    #[live]
    pub metadata_id: LiveId,
    #[live]
    pub url: String,
    #[live]
    pub method: HttpMethod,
    #[live]
    pub headers: BTreeMap<String, Vec<String>>,
    #[live]
    pub ignore_ssl_cert: bool,
    #[live]
    pub is_streaming: bool,
    #[live]
    pub max_response_body_bytes: u64,
    #[live]
    pub body: Option<Vec<u8>>,
    #[live]
    pub websocket_transport: WebSocketTransport,
}

/// Same defaults as `network::HttpRequest::default()` (no response size cap).
impl Default for HttpRequest {
    fn default() -> Self {
        Self {
            metadata_id: LiveId::empty(),
            url: String::new(),
            method: HttpMethod::GET,
            headers: BTreeMap::new(),
            ignore_ssl_cert: false,
            is_streaming: false,
            max_response_body_bytes: u64::MAX,
            body: None,
            websocket_transport: WebSocketTransport::Auto,
        }
    }
}

impl From<HttpRequest> for network::HttpRequest {
    fn from(v: HttpRequest) -> Self {
        Self {
            metadata_id: v.metadata_id,
            url: v.url,
            method: v.method.into(),
            headers: v.headers,
            ignore_ssl_cert: v.ignore_ssl_cert,
            is_streaming: v.is_streaming,
            max_response_body_bytes: v.max_response_body_bytes,
            body: v.body,
            websocket_transport: v.websocket_transport.into(),
        }
    }
}

#[derive(Script, ScriptHook, Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    #[live]
    pub metadata_id: LiveId,
    #[live]
    pub status_code: u16,
    #[live]
    pub headers: BTreeMap<String, Vec<String>>,
    #[live]
    pub body: Option<Arc<[u8]>>,
}

impl From<&network::HttpResponse> for HttpResponse {
    fn from(v: &network::HttpResponse) -> Self {
        Self {
            metadata_id: v.metadata_id,
            status_code: v.status_code,
            headers: v.headers.clone(),
            body: v.body.clone(),
        }
    }
}

#[derive(Script, ScriptHook, Clone, Debug, PartialEq, Eq)]
pub struct HttpError {
    #[live]
    pub message: String,
    #[live]
    pub metadata_id: LiveId,
}

impl From<&network::HttpError> for HttpError {
    fn from(v: &network::HttpError) -> Self {
        Self {
            message: v.message.clone(),
            metadata_id: v.metadata_id,
        }
    }
}

#[derive(Script, ScriptHook, Clone, Debug)]
pub struct HttpServerHeaders {
    #[live]
    pub addr_text: String,
    #[live]
    pub lines: Vec<String>,
    #[live]
    pub verb: String,
    #[live]
    pub path: String,
    #[live]
    pub path_no_slash: String,
    #[live]
    pub search: Option<String>,
    #[live]
    pub content_length: Option<u64>,
    #[live]
    pub accept_encoding: Option<String>,
    #[live]
    pub sec_websocket_key: Option<String>,
}

impl From<&network::utils::HttpServerHeaders> for HttpServerHeaders {
    fn from(v: &network::utils::HttpServerHeaders) -> Self {
        Self {
            addr_text: v.addr_text.clone(),
            lines: v.lines.clone(),
            verb: v.verb.clone(),
            path: v.path.clone(),
            path_no_slash: v.path_no_slash.clone(),
            search: v.search.clone(),
            content_length: v.content_length,
            accept_encoding: v.accept_encoding.clone(),
            sec_websocket_key: v.sec_websocket_key.clone(),
        }
    }
}

#[derive(Script, ScriptHook, Clone)]
pub struct HttpServerResponse {
    #[live]
    pub header: String,
    #[live]
    pub body: Vec<u8>,
}

impl From<HttpServerResponse> for network::http_server::HttpServerResponse {
    fn from(v: HttpServerResponse) -> Self {
        Self::new(v.header, v.body)
    }
}
