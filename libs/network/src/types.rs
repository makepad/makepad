use std::collections::BTreeMap;
use std::fmt;

pub type RequestId = u64;
pub type SocketId = u64;
pub type MetadataId = u64;
pub type Headers = BTreeMap<String, Vec<String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HttpMethod {
    #[default]
    Get,
    Head,
    Post,
    Put,
    Delete,
    Connect,
    Options,
    Trace,
    Patch,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Connect => "CONNECT",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
            Self::Patch => "PATCH",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpRequest {
    pub metadata_id: MetadataId,
    pub url: String,
    pub method: HttpMethod,
    pub headers: Headers,
    pub ignore_ssl_cert: bool,
    pub is_streaming: bool,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn new(url: String, method: HttpMethod) -> Self {
        Self {
            metadata_id: 0,
            url,
            method,
            headers: Headers::new(),
            ignore_ssl_cert: false,
            is_streaming: false,
            body: None,
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

    pub fn set_metadata_id(&mut self, id: MetadataId) {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitUrl<'a> {
    pub proto: &'a str,
    pub host: &'a str,
    pub port: &'a str,
    pub file: &'a str,
    pub hash: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub metadata_id: MetadataId,
    pub status_code: u16,
    pub headers: Headers,
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new(
        metadata_id: MetadataId,
        status_code: u16,
        headers: Headers,
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
        metadata_id: MetadataId,
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
}

fn parse_headers(header_string: String) -> Headers {
    let mut headers = Headers::new();
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpError {
    pub message: String,
    pub metadata_id: MetadataId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpProgress {
    pub loaded: u64,
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkResponseItem {
    pub request_id: RequestId,
    pub response: NetworkResponse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkResponse {
    HttpRequestError(HttpError),
    HttpResponse(HttpResponse),
    HttpStreamResponse(HttpResponse),
    HttpStreamComplete(HttpResponse),
    HttpProgress(HttpProgress),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebSocketMessage {
    Error(String),
    Binary(Vec<u8>),
    String(String),
    Opened,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WsOpenRequest {
    pub url: String,
    pub headers: Headers,
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
pub enum NetworkEvent {
    HttpResponse {
        request_id: RequestId,
        response: HttpResponse,
    },
    HttpStreamChunk {
        request_id: RequestId,
        response: HttpResponse,
    },
    HttpStreamComplete {
        request_id: RequestId,
        response: HttpResponse,
    },
    HttpError {
        request_id: RequestId,
        error: HttpError,
    },
    HttpProgress {
        request_id: RequestId,
        progress: HttpProgress,
    },
    WsOpened {
        socket_id: SocketId,
    },
    WsMessage {
        socket_id: SocketId,
        message: WsMessage,
    },
    WsClosed {
        socket_id: SocketId,
    },
    WsError {
        socket_id: SocketId,
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
            HttpRequest::new("https://example.com/path#frag".to_string(), HttpMethod::Get);
        let split = request.split_url();
        assert_eq!(split.proto, "https");
        assert_eq!(split.host, "example.com");
        assert_eq!(split.port, "443");
        assert_eq!(split.file, "path");
        assert_eq!(split.hash, "frag");
    }
}
