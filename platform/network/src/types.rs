use std::collections::BTreeMap;
use std::fmt;
use std::str;

use makepad_live_id::LiveId;
use makepad_micro_serde::{DeJson, DeJsonErr, SerJson};

#[cfg(feature = "script")]
use makepad_script::*;

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WebSocketTransport {
    #[cfg_attr(feature = "script", pick)]
    #[default]
    Auto,
    PlainTcp,
    Platform,
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HttpMethod {
    #[cfg_attr(feature = "script", pick)]
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

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GET => "GET",
            Self::HEAD => "HEAD",
            Self::POST => "POST",
            Self::PUT => "PUT",
            Self::DELETE => "DELETE",
            Self::CONNECT => "CONNECT",
            Self::OPTIONS => "OPTIONS",
            Self::TRACE => "TRACE",
            Self::PATCH => "PATCH",
        }
    }

    pub fn to_string(&self) -> &str {
        self.as_str()
    }
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpRequest {
    #[cfg_attr(feature = "script", live)]
    pub metadata_id: LiveId,
    #[cfg_attr(feature = "script", live)]
    pub url: String,
    #[cfg_attr(feature = "script", live)]
    pub method: HttpMethod,
    #[cfg_attr(feature = "script", live)]
    pub headers: BTreeMap<String, Vec<String>>,
    #[cfg_attr(feature = "script", live)]
    pub ignore_ssl_cert: bool,
    #[cfg_attr(feature = "script", live)]
    pub is_streaming: bool,
    #[cfg_attr(feature = "script", live)]
    pub body: Option<Vec<u8>>,
    #[cfg_attr(feature = "script", live)]
    pub websocket_transport: WebSocketTransport,
}

impl HttpRequest {
    pub fn new(url: String, method: HttpMethod) -> Self {
        Self {
            metadata_id: LiveId::empty(),
            url,
            method,
            headers: BTreeMap::new(),
            ignore_ssl_cert: false,
            is_streaming: false,
            body: None,
            websocket_transport: WebSocketTransport::Auto,
        }
    }

    pub fn split_url(&self) -> SplitUrl<'_> {
        let (proto, rest) = self.url.split_once("://").unwrap_or((&self.url, "http://"));
        let (host, port, rest) = if let Some((host, rest)) = rest.split_once(':') {
            let (port, rest) = rest.split_once('/').unwrap_or((rest, ""));
            (host, port, rest)
        } else {
            let (host, rest) = rest.split_once('/').unwrap_or((rest, ""));
            (
                host,
                match proto {
                    "http" | "ws" => "80",
                    "https" | "wss" => "443",
                    _ => "80",
                },
                rest,
            )
        };
        let (file, hash) = rest.split_once('#').unwrap_or((rest, ""));
        SplitUrl {
            proto,
            host,
            port,
            file,
            hash,
        }
    }

    pub fn set_ignore_ssl_cert(&mut self) {
        self.ignore_ssl_cert = true;
    }

    pub fn set_is_streaming(&mut self) {
        self.is_streaming = true;
    }

    pub fn set_metadata_id(&mut self, id: LiveId) {
        self.metadata_id = id;
    }

    pub fn set_header(&mut self, name: String, value: String) {
        let entry = self.headers.entry(name).or_default();
        entry.push(value);
    }

    pub fn get_headers_string(&self) -> String {
        let mut headers_string = String::new();
        for (key, value) in self.headers.iter() {
            headers_string.push_str(&format!("{}: {}\r\n", key, value.join(",")));
        }
        headers_string
    }

    pub fn set_body(&mut self, body: Vec<u8>) {
        self.body = Some(body);
    }

    pub fn set_body_string(&mut self, body: &str) {
        self.body = Some(body.as_bytes().to_vec());
    }

    pub fn set_json_body<T: SerJson>(&mut self, body: T) {
        let json_body = body.serialize_json();
        self.body = Some(json_body.into_bytes());
    }

    pub fn set_string_body(&mut self, body: String) {
        self.body = Some(body.into_bytes());
    }

    pub fn set_websocket_transport(&mut self, transport: WebSocketTransport) {
        self.websocket_transport = transport;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitUrl<'a> {
    pub proto: &'a str,
    pub host: &'a str,
    pub port: &'a str,
    pub file: &'a str,
    pub hash: &'a str,
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    #[cfg_attr(feature = "script", live)]
    pub metadata_id: LiveId,
    #[cfg_attr(feature = "script", live)]
    pub status_code: u16,
    #[cfg_attr(feature = "script", live)]
    pub headers: BTreeMap<String, Vec<String>>,
    #[cfg_attr(feature = "script", live)]
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new(
        metadata_id: LiveId,
        status_code: u16,
        headers: BTreeMap<String, Vec<String>>,
        body: Option<Vec<u8>>,
    ) -> Self {
        Self {
            metadata_id,
            status_code,
            headers,
            body,
        }
    }

    pub fn from_header_string(
        metadata_id: LiveId,
        status_code: u16,
        headers: String,
        body: Option<Vec<u8>>,
    ) -> Self {
        Self {
            metadata_id,
            status_code,
            headers: parse_headers(headers),
            body,
        }
    }

    pub fn set_header(&mut self, name: String, value: String) {
        let entry = self.headers.entry(name).or_default();
        entry.push(value);
    }

    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }

    pub fn body_string(&self) -> Option<String> {
        self.body
            .as_ref()
            .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
    }

    pub fn get_body(&self) -> Option<&Vec<u8>> {
        self.body.as_ref()
    }

    pub fn get_string_body(&self) -> Option<String> {
        self.body
            .as_ref()
            .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
    }

    pub fn get_json_body<T: DeJson>(&self) -> Result<T, DeJsonErr> {
        if let Some(body) = self.body.as_ref() {
            let json = str::from_utf8(body).map_err(|err| DeJsonErr {
                msg: err.to_string(),
                line: 0,
                col: 0,
            })?;
            DeJson::deserialize_json(json)
        } else {
            Err(DeJsonErr {
                msg: "No body present".to_string(),
                line: 0,
                col: 0,
            })
        }
    }
}

fn parse_headers(header_string: String) -> BTreeMap<String, Vec<String>> {
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in header_string.lines() {
        if let Some((key, values)) = line.split_once(':') {
            for value in values.split(',') {
                headers
                    .entry(key.to_string())
                    .or_default()
                    .push(value.trim().to_string());
            }
        }
    }
    headers
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpError {
    #[cfg_attr(feature = "script", live)]
    pub message: String,
    #[cfg_attr(feature = "script", live)]
    pub metadata_id: LiveId,
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpProgress {
    #[cfg_attr(feature = "script", live)]
    pub loaded: u64,
    #[cfg_attr(feature = "script", live)]
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketMessage {
    Error(String),
    Binary(Vec<u8>),
    String(String),
    Opened,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsSend {
    Binary(Vec<u8>),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    Binary(Vec<u8>),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkResponse {
    HttpResponse {
        request_id: LiveId,
        response: HttpResponse,
    },
    HttpStreamChunk {
        request_id: LiveId,
        response: HttpResponse,
    },
    HttpStreamComplete {
        request_id: LiveId,
        response: HttpResponse,
    },
    HttpError {
        request_id: LiveId,
        error: HttpError,
    },
    HttpProgress {
        request_id: LiveId,
        progress: HttpProgress,
    },
    WsOpened {
        socket_id: LiveId,
    },
    WsMessage {
        socket_id: LiveId,
        message: WsMessage,
    },
    WsClosed {
        socket_id: LiveId,
    },
    WsError {
        socket_id: LiveId,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkError {
    Unsupported(&'static str),
    Backend(String),
    ChannelClosed,
}

impl NetworkError {
    pub fn backend(message: impl Into<String>) -> Self {
        Self::Backend(message.into())
    }
}

impl fmt::Display for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(message) => write!(f, "unsupported network backend: {message}"),
            Self::Backend(message) => write!(f, "network backend error: {message}"),
            Self::ChannelClosed => write!(f, "network event channel closed"),
        }
    }
}

impl std::error::Error for NetworkError {}

#[cfg(test)]
mod tests {
    use super::{HttpMethod, HttpRequest};

    #[test]
    fn split_url_resolves_default_ports() {
        let request =
            HttpRequest::new("https://example.com/path#frag".to_string(), HttpMethod::GET);
        let split = request.split_url();
        assert_eq!(split.proto, "https");
        assert_eq!(split.host, "example.com");
        assert_eq!(split.port, "443");
        assert_eq!(split.file, "path");
        assert_eq!(split.hash, "frag");
    }
}
