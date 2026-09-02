//! Completion-based owned HTTP transport primitives.
//!
//! Status, range, digest, canonical-document, and application-protocol
//! validation deliberately live above this seam. A transport moves bounded
//! owned bytes and reports exactly one completion for every accepted id.

use crate::error::ClientError;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashSet;

pub const MAX_TRANSPORT_URL_BYTES: usize = 8 * 1024;
pub const MAX_TRANSPORT_HEADERS: usize = 64;
pub const MAX_TRANSPORT_HEADER_BYTES: usize = 32 * 1024;
pub const MAX_TRANSPORT_BODY_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportMethod {
    Get,
    Head,
    Post,
    Put,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedRequest {
    pub method: TransportMethod,
    pub url_or_target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl OwnedRequest {
    pub fn new(method: TransportMethod, url_or_target: impl Into<String>) -> Self {
        Self {
            method,
            url_or_target: url_or_target.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedResponse {
    pub status: u16,
    /// Header names are always normalized to ASCII lowercase.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl OwnedResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransportId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    InvalidRequest { what: &'static str },
    OverBudget { what: &'static str, limit: u64, found: u64 },
    Network(String),
    Protocol { what: &'static str },
    Client(ClientError),
    Cancelled,
}

impl From<ClientError> for TransportError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Protocol { what } => Self::Protocol { what },
            ClientError::OverBudget { what, limit, found } => {
                Self::OverBudget { what, limit, found }
            }
            error => Self::Client(error),
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest { what } => write!(f, "invalid transport request: {what}"),
            Self::OverBudget { what, limit, found } => {
                write!(f, "transport over budget: {what} (limit {limit}, found {found})")
            }
            Self::Network(message) => write!(f, "network transport failure: {message}"),
            Self::Protocol { what } => write!(f, "transport protocol violation: {what}"),
            Self::Client(error) => write!(f, "{error}"),
            Self::Cancelled => f.write_str("transport request cancelled"),
        }
    }
}

impl std::error::Error for TransportError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportCompletion {
    pub id: TransportId,
    pub result: Result<OwnedResponse, TransportError>,
}

pub trait Transport {
    fn start(&mut self, req: OwnedRequest) -> TransportId;
    fn cancel(&mut self, id: TransportId);
    fn poll(&mut self, out: &mut Vec<TransportCompletion>);
}

fn validate_request(req: &OwnedRequest, absolute_url: bool) -> Result<(), TransportError> {
    if req.url_or_target.is_empty() || req.url_or_target.len() > MAX_TRANSPORT_URL_BYTES {
        return Err(TransportError::InvalidRequest { what: "request url" });
    }
    if absolute_url {
        if !(req.url_or_target.starts_with("http://")
            || req.url_or_target.starts_with("https://"))
        {
            return Err(TransportError::InvalidRequest { what: "absolute request url" });
        }
    } else if !req.url_or_target.starts_with('/') {
        return Err(TransportError::InvalidRequest { what: "origin-form request target" });
    }
    if req.body.len() as u64 > MAX_TRANSPORT_BODY_BYTES {
        return Err(TransportError::OverBudget {
            what: "request body",
            limit: MAX_TRANSPORT_BODY_BYTES,
            found: req.body.len() as u64,
        });
    }
    if !req.body.is_empty()
        && matches!(
            req.method,
            TransportMethod::Get | TransportMethod::Head | TransportMethod::Delete
        )
    {
        return Err(TransportError::InvalidRequest {
            what: "body on bodyless method",
        });
    }
    if req.headers.len() > MAX_TRANSPORT_HEADERS {
        return Err(TransportError::OverBudget {
            what: "request header count",
            limit: MAX_TRANSPORT_HEADERS as u64,
            found: req.headers.len() as u64,
        });
    }
    let mut total = 0usize;
    for (name, value) in &req.headers {
        total = total.saturating_add(name.len()).saturating_add(value.len());
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().all(header_name_byte_ok)
            || matches!(
                name.to_ascii_lowercase().as_str(),
                "host"
                    | "connection"
                    | "content-length"
                    | "transfer-encoding"
                    | "te"
                    | "trailer"
                    | "upgrade"
                    | "expect"
                    | "keep-alive"
                    | "proxy-connection"
            )
        {
            return Err(TransportError::InvalidRequest { what: "request header name" });
        }
        if !value.bytes().all(|b| b == b'\t' || (b >= b' ' && b != 0x7f)) {
            return Err(TransportError::InvalidRequest { what: "request header value" });
        }
    }
    if total > MAX_TRANSPORT_HEADER_BYTES {
        return Err(TransportError::OverBudget {
            what: "request headers",
            limit: MAX_TRANSPORT_HEADER_BYTES as u64,
            found: total as u64,
        });
    }
    Ok(())
}

fn normalize_response(
    status: u16,
    headers: impl IntoIterator<Item = (String, String)>,
    body: Vec<u8>,
    max_body_bytes: u64,
    is_head_request: bool,
) -> Result<OwnedResponse, TransportError> {
    if body.len() as u64 > max_body_bytes {
        return Err(TransportError::OverBudget {
            what: "response body",
            limit: max_body_bytes,
            found: body.len() as u64,
        });
    }
    if is_head_request && !body.is_empty() {
        return Err(TransportError::Protocol { what: "HEAD response body" });
    }
    if status < 200 {
        return Err(TransportError::Protocol { what: "unexpected informational status" });
    }
    if status == 204 || status == 304 {
        return Err(TransportError::Protocol { what: "unexpected bodyless status" });
    }
    if (300..400).contains(&status) {
        return Err(TransportError::Protocol { what: "redirect refused" });
    }
    if status > 599 {
        return Err(TransportError::Protocol { what: "invalid response status" });
    }
    let mut normalized = Vec::new();
    let mut content_length = None;
    let mut total = 0usize;
    for (name, value) in headers {
        if normalized.len() >= MAX_TRANSPORT_HEADERS {
            return Err(TransportError::Protocol { what: "too many response headers" });
        }
        total = total.saturating_add(name.len()).saturating_add(value.len());
        if total > MAX_TRANSPORT_HEADER_BYTES {
            return Err(TransportError::OverBudget {
                what: "response headers",
                limit: MAX_TRANSPORT_HEADER_BYTES as u64,
                found: total as u64,
            });
        }
        if name.is_empty() || !name.bytes().all(header_name_byte_ok) {
            return Err(TransportError::Protocol { what: "response header name" });
        }
        if !value.bytes().all(|b| b == b'\t' || (b >= b' ' && b != 0x7f)) {
            return Err(TransportError::Protocol { what: "response header value" });
        }
        let name = name.to_ascii_lowercase();
        if name == "transfer-encoding" {
            return Err(TransportError::Protocol { what: "transfer-encoding refused" });
        }
        if name == "content-length" {
            if content_length.is_some() {
                return Err(TransportError::Protocol { what: "duplicate content-length" });
            }
            content_length = Some(parse_content_length(&value)?);
        }
        normalized.push((name, value));
    }
    let declared = content_length
        .ok_or(TransportError::Protocol { what: "content-length required" })?;
    if !is_head_request && declared != body.len() as u64 {
        return Err(TransportError::Protocol { what: "content-length mismatch" });
    }
    Ok(OwnedResponse { status, headers: normalized, body })
}

fn parse_content_length(value: &str) -> Result<u64, TransportError> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(TransportError::Protocol { what: "malformed content-length" });
    }
    value
        .parse()
        .map_err(|_| TransportError::Protocol { what: "malformed content-length" })
}

fn header_name_byte_ok(b: u8) -> bool {
    matches!(b,
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
        | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+'
        | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~')
}

#[cfg(not(target_arch = "wasm32"))]
mod tcp {
    use super::*;
    use crate::http::{self, HttpLimits, Method, Request};
    use std::net::SocketAddr;
    use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
    use std::sync::{Arc, Mutex};

    pub struct TcpHttpTransport {
        addr: SocketAddr,
        limits: HttpLimits,
        max_response_body_bytes: u64,
        next_id: u64,
        active: HashSet<TransportId>,
        cancelled: Arc<Mutex<HashSet<TransportId>>>,
        tx: Sender<TransportCompletion>,
        rx: Receiver<TransportCompletion>,
        ready: Vec<TransportCompletion>,
    }

    impl TcpHttpTransport {
        pub fn new(addr: SocketAddr) -> Self {
            Self::with_limits(addr, HttpLimits::default_v1(), MAX_TRANSPORT_BODY_BYTES)
        }

        pub fn with_limits(
            addr: SocketAddr,
            limits: HttpLimits,
            max_response_body_bytes: u64,
        ) -> Self {
            let (tx, rx) = channel();
            Self {
                addr,
                limits,
                max_response_body_bytes,
                next_id: 1,
                active: HashSet::new(),
                cancelled: Arc::new(Mutex::new(HashSet::new())),
                tx,
                rx,
                ready: Vec::new(),
            }
        }

        fn allocate(&mut self) -> TransportId {
            let id = TransportId(self.next_id);
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            id
        }
    }

    impl Transport for TcpHttpTransport {
        fn start(&mut self, req: OwnedRequest) -> TransportId {
            let id = self.allocate();
            self.active.insert(id);
            if let Err(error) = validate_request(&req, false) {
                self.ready.push(TransportCompletion { id, result: Err(error) });
                return id;
            }
            if req.url_or_target.len() > crate::wire::MAX_TARGET_BYTES
                || !req.url_or_target.bytes().all(crate::wire::target_byte_ok)
            {
                self.ready.push(TransportCompletion {
                    id,
                    result: Err(TransportError::InvalidRequest { what: "request target" }),
                });
                return id;
            }
            let addr = self.addr;
            let limits = self.limits;
            let max = self.max_response_body_bytes;
            let tx = self.tx.clone();
            let cancelled = Arc::clone(&self.cancelled);
            std::thread::spawn(move || {
                let result = run_request(id, addr, limits, max, &req, &cancelled);
                if let Ok(mut ids) = cancelled.lock() {
                    ids.remove(&id);
                }
                let _ = tx.send(TransportCompletion { id, result });
            });
            id
        }

        fn cancel(&mut self, id: TransportId) {
            if !self.active.remove(&id) {
                return;
            }
            if let Ok(mut cancelled) = self.cancelled.lock() {
                cancelled.insert(id);
            }
            self.ready.push(TransportCompletion { id, result: Err(TransportError::Cancelled) });
        }

        fn poll(&mut self, out: &mut Vec<TransportCompletion>) {
            for completion in self.ready.drain(..) {
                if self.active.remove(&completion.id)
                    || matches!(completion.result, Err(TransportError::Cancelled))
                {
                    out.push(completion);
                }
            }
            loop {
                match self.rx.try_recv() {
                    Ok(completion) if self.active.remove(&completion.id) => out.push(completion),
                    Ok(_) => {}
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
        }
    }

    fn run_request(
        id: TransportId,
        addr: SocketAddr,
        limits: HttpLimits,
        max: u64,
        owned: &OwnedRequest,
        cancelled: &Mutex<HashSet<TransportId>>,
    ) -> Result<OwnedResponse, TransportError> {
        let method = match owned.method {
            TransportMethod::Get => Method::Get,
            TransportMethod::Head => Method::Head,
            TransportMethod::Post => Method::Post,
            TransportMethod::Put => Method::Put,
            TransportMethod::Delete => Method::Delete,
        };
        let mut req = match method {
            Method::Get => Request::get(&owned.url_or_target),
            Method::Head => Request::head(&owned.url_or_target),
            Method::Post => Request::post(&owned.url_or_target, &owned.body),
            Method::Put => Request::put(&owned.url_or_target, &owned.body),
            Method::Delete => Request::delete(&owned.url_or_target),
        };
        req.extra_headers = &owned.headers;
        let mut response = http::http_call(addr, &req, &limits)?;
        let status = response.head().status;
        let headers = response.head().headers.clone();
        let declared = response.head().content_length;
        if !matches!(owned.method, TransportMethod::Head) && declared > max {
            return Err(TransportError::OverBudget {
                what: "response body",
                limit: max,
                found: declared,
            });
        }
        let mut body = Vec::with_capacity(declared.min(64 * 1024) as usize);
        if !matches!(owned.method, TransportMethod::Head) {
            let mut chunk = [0u8; 16 * 1024];
            loop {
                if cancelled.lock().is_ok_and(|ids| ids.contains(&id)) {
                    return Err(TransportError::Cancelled);
                }
                let read = response.read_chunk(&mut chunk)?;
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read]);
            }
        }
        normalize_response(
            status,
            headers,
            body,
            max,
            matches!(owned.method, TransportMethod::Head),
        )
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use tcp::TcpHttpTransport;

#[cfg(any(target_arch = "wasm32", feature = "web"))]
mod platform {
    use super::*;
    use makepad_live_id::LiveId;
    use makepad_network::{
        HttpMethod, HttpRequest, NetworkConfig, NetworkResponse, NetworkRuntime,
        HTTP_BODY_LIMIT_ERROR,
    };

    pub struct PlatformHttpTransport {
        runtime: NetworkRuntime,
        max_response_body_bytes: u64,
        next_id: u64,
        active: std::collections::HashMap<TransportId, TransportMethod>,
        ready: Vec<TransportCompletion>,
    }

    impl Default for PlatformHttpTransport {
        fn default() -> Self {
            Self::new()
        }
    }

    impl PlatformHttpTransport {
        pub fn new() -> Self {
            Self::with_max_response_body(MAX_TRANSPORT_BODY_BYTES)
        }

        pub fn with_max_response_body(max_response_body_bytes: u64) -> Self {
            Self::with_runtime(
                NetworkRuntime::new(NetworkConfig::default()),
                max_response_body_bytes,
            )
        }

        pub fn with_runtime(runtime: NetworkRuntime, max_response_body_bytes: u64) -> Self {
            Self {
                runtime,
                max_response_body_bytes,
                next_id: 1,
                active: std::collections::HashMap::new(),
                ready: Vec::new(),
            }
        }

        fn allocate(&mut self) -> TransportId {
            let id = TransportId(self.next_id);
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            id
        }
    }

    impl Transport for PlatformHttpTransport {
        fn start(&mut self, req: OwnedRequest) -> TransportId {
            let id = self.allocate();
            self.active.insert(id, req.method);
            if let Err(error) = validate_request(&req, true) {
                self.ready.push(TransportCompletion { id, result: Err(error) });
                return id;
            }
            let method = match req.method {
                TransportMethod::Get => HttpMethod::GET,
                TransportMethod::Head => HttpMethod::HEAD,
                TransportMethod::Post => HttpMethod::POST,
                TransportMethod::Put => HttpMethod::PUT,
                TransportMethod::Delete => HttpMethod::DELETE,
            };
            let mut request = HttpRequest::new(req.url_or_target, method);
            request.set_max_response_body_bytes(self.max_response_body_bytes);
            for (name, value) in req.headers {
                request.set_header(name, value);
            }
            if !req.body.is_empty() || matches!(method, HttpMethod::POST | HttpMethod::PUT) {
                request.set_body(req.body);
            }
            if let Err(error) = self.runtime.http_start(live_id(id), request) {
                self.ready.push(TransportCompletion {
                    id,
                    result: Err(TransportError::Network(error.to_string())),
                });
            }
            id
        }

        fn cancel(&mut self, id: TransportId) {
            if self.active.remove(&id).is_none() {
                return;
            }
            let _ = self.runtime.http_cancel(live_id(id));
            self.ready.push(TransportCompletion { id, result: Err(TransportError::Cancelled) });
        }

        fn poll(&mut self, out: &mut Vec<TransportCompletion>) {
            for completion in self.ready.drain(..) {
                if self.active.remove(&completion.id).is_some()
                    || matches!(completion.result, Err(TransportError::Cancelled))
                {
                    out.push(completion);
                }
            }
            while let Some(event) = self.runtime.try_recv() {
                match event {
                    NetworkResponse::HttpResponse { request_id, response }
                    | NetworkResponse::HttpStreamComplete { request_id, response } => {
                        let id = transport_id(request_id);
                        let Some(method) = self.active.remove(&id) else { continue };
                        let headers = response.headers.into_iter().flat_map(|(name, values)| {
                            values.into_iter().map(move |value| (name.clone(), value))
                        });
                        let result = normalize_response(
                            response.status_code,
                            headers,
                            response.body.unwrap_or_default(),
                            self.max_response_body_bytes,
                            matches!(method, TransportMethod::Head),
                        );
                        out.push(TransportCompletion { id, result });
                    }
                    NetworkResponse::HttpError { request_id, error } => {
                        let id = transport_id(request_id);
                        if self.active.remove(&id).is_some() {
                            let result = if error.message == HTTP_BODY_LIMIT_ERROR {
                                Err(TransportError::OverBudget {
                                    what: "response body",
                                    limit: self.max_response_body_bytes,
                                    found: self.max_response_body_bytes.saturating_add(1),
                                })
                            } else {
                                Err(TransportError::Network(error.message))
                            };
                            out.push(TransportCompletion {
                                id,
                                result,
                            });
                        }
                    }
                    NetworkResponse::HttpStreamChunk { .. }
                    | NetworkResponse::HttpProgress { .. }
                    | NetworkResponse::WsOpened { .. }
                    | NetworkResponse::WsMessage { .. }
                    | NetworkResponse::WsClosed { .. }
                    | NetworkResponse::WsError { .. } => {}
                }
            }
        }
    }

    fn live_id(id: TransportId) -> LiveId {
        LiveId(id.0)
    }

    fn transport_id(id: LiveId) -> TransportId {
        TransportId(id.0)
    }
}

#[cfg(any(target_arch = "wasm32", feature = "web"))]
pub use platform::PlatformHttpTransport;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct MockTransport {
        next: u64,
        pending: VecDeque<TransportCompletion>,
    }

    impl Transport for MockTransport {
        fn start(&mut self, req: OwnedRequest) -> TransportId {
            let id = TransportId(self.next);
            self.next += 1;
            self.pending.push_back(TransportCompletion {
                id,
                result: Ok(OwnedResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "application/octet-stream".into())],
                    body: req.body,
                }),
            });
            id
        }

        fn cancel(&mut self, id: TransportId) {
            self.pending.retain(|completion| completion.id != id);
            self.pending.push_back(TransportCompletion {
                id,
                result: Err(TransportError::Cancelled),
            });
        }

        fn poll(&mut self, out: &mut Vec<TransportCompletion>) {
            out.extend(self.pending.drain(..));
        }
    }

    #[test]
    fn completion_protocol_preserves_ids_and_cancel_is_terminal() {
        let mut transport = MockTransport { next: 1, pending: VecDeque::new() };
        let first = transport.start(OwnedRequest::new(TransportMethod::Get, "/a"));
        let second = transport.start(
            OwnedRequest::new(TransportMethod::Put, "/b").body(b"body".to_vec()),
        );
        transport.cancel(first);
        let mut completions = Vec::new();
        transport.poll(&mut completions);
        assert_eq!(completions.len(), 2);
        assert!(completions.iter().any(|c| {
            c.id == first && matches!(c.result, Err(TransportError::Cancelled))
        }));
        assert!(completions.iter().any(|c| {
            c.id == second && c.result.as_ref().is_ok_and(|r| r.body == b"body")
        }));
    }

    #[test]
    fn request_bounds_reject_header_injection() {
        let request = OwnedRequest::new(TransportMethod::Get, "https://example.test/x")
            .header("x-test", "ok\r\ninjected: yes");
        assert!(matches!(
            validate_request(&request, true),
            Err(TransportError::InvalidRequest { what: "request header value" })
        ));
    }

    #[test]
    fn request_bounds_reject_caller_controlled_framing() {
        for name in ["content-length", "transfer-encoding", "te", "trailer", "upgrade", "expect"] {
            let request = OwnedRequest::new(TransportMethod::Post, "https://example.test/x")
                .header(name, "value")
                .body(b"body".to_vec());
            assert!(
                matches!(
                    validate_request(&request, true),
                    Err(TransportError::InvalidRequest { what: "request header name" })
                ),
                "accepted {name}"
            );
        }
    }

    #[test]
    fn shared_response_contract_rejects_bad_status_and_framing() {
        for status in [0, 199, 204, 302, 304, 600] {
            assert!(normalize_response(
                status,
                [("content-length".to_string(), "0".to_string())],
                Vec::new(),
                16,
                false,
            )
            .is_err(), "accepted status {status}");
        }
        for headers in [
            vec![],
            vec![("transfer-encoding".to_string(), "chunked".to_string())],
            vec![("content-length".to_string(), "02".to_string())],
            vec![
                ("content-length".to_string(), "2".to_string()),
                ("content-length".to_string(), "2".to_string()),
            ],
        ] {
            assert!(normalize_response(200, headers, b"ok".to_vec(), 16, false).is_err());
        }
        assert!(normalize_response(
            200,
            [("content-length".to_string(), "3".to_string())],
            b"ok".to_vec(),
            16,
            false,
        )
        .is_err());
    }
}
