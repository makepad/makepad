//! Pure request builders and reply parsers for Fal's queue and model catalog.

use makepad_base64::base64::base64_decode;
use makepad_strict_json::{obj, Value};
use makepad_widgets::{HttpMethod, HttpRequest, HttpResponse};
use std::collections::HashSet;

pub const QUEUE_BASE: &str = "https://queue.fal.run";
pub const API_BASE: &str = "https://api.fal.ai/v1";
/// Model output and OpenAPI documents nest deeper than strict_json's default.
pub const JSON_DEPTH: u32 = 64;

fn valid_key(key: &str) -> bool {
    !key.is_empty() && !key.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub fn parse_key_text(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let key = line
        .split_once('=')
        .filter(|(name, _)| name.trim_end() == "FAL_KEY")
        .map_or(line, |(_, value)| value.trim());
    let key = ['\'', '"']
        .into_iter()
        .find_map(|quote| key.strip_prefix(quote)?.strip_suffix(quote))
        .unwrap_or(key)
        .trim();
    valid_key(key).then(|| key.to_owned())
}

pub fn normalize_endpoint_id(text: &str) -> Result<String, String> {
    let text = text.trim();
    let text = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .unwrap_or(text);
    let text = [
        "queue.fal.run/",
        "fal.run/",
        "fal.ai/models/",
        "www.fal.ai/models/",
    ]
    .into_iter()
    .find_map(|host| text.strip_prefix(host))
    .unwrap_or(text);
    let path = text
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    let path = path
        .strip_suffix("/api")
        .or_else(|| path.strip_suffix("/playground"))
        .unwrap_or(path)
        .trim_matches('/');
    if !path.contains('/') {
        return Err("An endpoint ID needs at least an owner and a model.".into());
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err("The endpoint ID contains an empty path segment.".into());
        }
        if !segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err("Endpoint segments may contain only letters, digits, dots, underscores, and hyphens.".into());
        }
    }
    Ok(path.to_owned())
}

#[derive(Clone, Debug)]
pub struct QueueUrls {
    pub status: String,
    pub response: String,
    pub cancel: String,
}

pub fn queue_urls(endpoint_id: &str, request_id: &str) -> QueueUrls {
    let mut parts = endpoint_id.split('/');
    let owner = parts.next().unwrap_or("");
    let app = parts.next().unwrap_or("");
    let response = format!(
        "{QUEUE_BASE}/{owner}/{app}/requests/{}",
        url_encode(request_id)
    );
    QueueUrls {
        status: format!("{response}/status"),
        cancel: format!("{response}/cancel"),
        response,
    }
}

pub fn url_encode(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[(b >> 4) as usize]));
            out.push(char::from(HEX[(b & 15) as usize]));
        }
    }
    out
}

fn under_base(url: &str, base: &str) -> bool {
    url.strip_prefix(base)
        .is_some_and(|path| path.starts_with('/'))
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
}

fn request(
    method: HttpMethod,
    url: String,
    key: Option<&str>,
    body: Option<Vec<u8>>,
) -> HttpRequest {
    let mut req = HttpRequest::new(url, method);
    req.set_header("Accept".into(), "application/json".into());
    if let Some(key) = key.filter(|key| valid_key(key)) {
        if under_base(&req.url, QUEUE_BASE) || under_base(&req.url, API_BASE) {
            req.set_header("Authorization".into(), format!("Key {key}"));
        }
    }
    if let Some(body) = body {
        req.set_header("Content-Type".into(), "application/json".into());
        req.set_body(body);
    }
    req
}

pub fn submit_request(key: &str, endpoint_id: &str, input: &Value) -> HttpRequest {
    request(
        HttpMethod::POST,
        format!("{QUEUE_BASE}/{endpoint_id}"),
        Some(key),
        Some(input.to_json().into_bytes()),
    )
}

pub fn status_request(key: &str, status_url: &str) -> HttpRequest {
    let (url, fragment) = status_url.split_once('#').unwrap_or((status_url, ""));
    let separator = if !url.contains('?') {
        "?"
    } else if url.ends_with(['?', '&']) {
        ""
    } else {
        "&"
    };
    let mut url = format!("{url}{separator}logs=1");
    if !fragment.is_empty() {
        url.push('#');
        url.push_str(fragment);
    }
    request(HttpMethod::GET, url, Some(key), None)
}

pub fn result_request(key: &str, response_url: &str) -> HttpRequest {
    request(HttpMethod::GET, response_url.to_owned(), Some(key), None)
}

pub fn cancel_request(key: &str, cancel_url: &str) -> HttpRequest {
    request(
        HttpMethod::PUT,
        cancel_url.to_owned(),
        Some(key),
        Some(Vec::new()),
    )
}

pub fn media_request(url: &str) -> HttpRequest {
    request(HttpMethod::GET, url.to_owned(), None, None)
}

pub fn models_request(key: Option<&str>, category: &str, cursor: Option<&str>) -> HttpRequest {
    let mut url = format!(
        "{API_BASE}/models?category={}&status=active&limit=10&expand=openapi-3.0",
        url_encode(category)
    );
    if let Some(cursor) = cursor {
        url.push_str("&cursor=");
        url.push_str(&url_encode(cursor));
    }
    request(HttpMethod::GET, url, key, None)
}

/// One model's catalog entry with its OpenAPI schema (what inputs it takes).
pub fn model_request(key: Option<&str>, endpoint_id: &str) -> HttpRequest {
    let url = format!("{API_BASE}/models?endpoint_id={}&expand=openapi-3.0", url_encode(endpoint_id));
    request(HttpMethod::GET, url, key, None)
}

pub fn pricing_request(key: &str, endpoint_ids: &[String]) -> HttpRequest {
    let mut url = format!("{API_BASE}/models/pricing");
    for (i, id) in endpoint_ids.iter().enumerate() {
        url.push_str(if i == 0 {
            "?endpoint_id="
        } else {
            "&endpoint_id="
        });
        url.push_str(&url_encode(id));
    }
    request(HttpMethod::GET, url, Some(key), None)
}

#[derive(Clone, Debug)]
pub struct Submitted {
    pub request_id: String,
    pub urls: QueueUrls,
    pub queue_position: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum Phase {
    InQueue { position: Option<u64> },
    InProgress,
    Completed,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub timestamp: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub phase: Phase,
    pub logs: Vec<LogLine>,
    pub inference_time: Option<f64>,
    pub error: Option<String>,
    pub error_type: Option<String>,
}

#[derive(Clone, Debug)]
pub enum CancelOutcome {
    Removed,
    Requested,
    AlreadyCompleted,
    NotFound,
}

#[derive(Clone, Debug)]
pub struct ValidationIssue {
    pub field: String,
    pub message: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub enum FalError {
    RejectedKey(String),
    Forbidden(String),
    NotFound(String),
    Validation(Vec<ValidationIssue>),
    RateLimited {
        detail: String,
        retry_after: Option<f64>,
    },
    StillRunning,
    Cancelled(String),
    Server {
        status: u16,
        detail: String,
        error_type: Option<String>,
    },
    Http {
        status: u16,
        detail: String,
    },
    Network(String),
    BadReply(String),
    Failed {
        error: String,
        error_type: Option<String>,
    },
    /// The run completed, but Fal fails to hand over what it made.
    ResultUnavailable(String),
}

impl FalError {
    pub fn title(&self) -> &'static str {
        match self {
            Self::RejectedKey(_) => "Key rejected",
            Self::Forbidden(_) => "Not allowed",
            Self::NotFound(_) => "Not found",
            Self::Validation(_) => "Input rejected",
            Self::RateLimited { .. } => "Rate limited",
            Self::StillRunning => "Still running",
            Self::Cancelled(_) => "Cancelled",
            Self::Server { .. } => "Fal server error",
            Self::Http { .. } => "HTTP error",
            Self::Network(_) => "Network error",
            Self::BadReply(_) => "Unexpected reply",
            Self::Failed { .. } => "Run failed",
            Self::ResultUnavailable(_) => "Result unavailable",
        }
    }

    pub fn message(&self) -> String {
        fn with_type(detail: &str, kind: &Option<String>) -> String {
            match kind {
                Some(kind) if !kind.is_empty() => format!("{detail} ({kind})"),
                _ => detail.to_owned(),
            }
        }
        match self {
            Self::RejectedKey(detail) => format!("Fal rejected the API key (HTTP 401). {detail}"),
            Self::Forbidden(detail) => format!("{detail} This can mean the key may not use this model, the account is locked, or a spending limit was reached."),
            Self::Validation(issues) => issues
                .iter()
                .map(|issue| {
                    let message = if issue.message.is_empty() { &issue.kind } else { &issue.message };
                    if issue.field.is_empty() {
                        message.clone()
                    } else {
                        format!("{}: {message}", issue.field)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::StillRunning => "Request is still in progress.".into(),
            Self::Server { status, detail, error_type } => {
                let label = format!("HTTP {status}");
                let message = if detail == &label { label } else { format!("{label}: {detail}") };
                with_type(&message, error_type)
            }
            Self::Failed { error, error_type } => with_type(error, error_type),
            Self::Http { status, detail } if detail.is_empty() => format!("HTTP {status}"),
            Self::NotFound(detail) | Self::Cancelled(detail) | Self::Http { detail, .. }
            | Self::Network(detail) | Self::BadReply(detail) | Self::RateLimited { detail, .. }
            | Self::ResultUnavailable(detail) => detail.clone(),
        }
    }

    /// Worth retrying for something that already exists (a finished run's
    /// result, a stored picture): a busy or unreachable server. A plain 500
    /// for such a thing is its own answer, and asking again gets the same.
    pub fn transient(&self) -> bool {
        self.retryable() && !matches!(self, Self::Server { status: 500, .. })
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Network(_)
                | Self::Server {
                    status: 500 | 502 | 503 | 504,
                    ..
                }
        )
    }
}

pub fn parse_json(bytes: &[u8]) -> Result<Value, String> {
    makepad_strict_json::parse_depth_last_key_wins(bytes, JSON_DEPTH).map_err(str::to_owned)
}

fn string_field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name)?.as_str()
}

fn required_string<'a>(value: &'a Value, name: &str) -> Result<&'a str, FalError> {
    string_field(value, name)
        .ok_or_else(|| FalError::BadReply(format!("Expected a string in '{name}'.")))
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(i) => Some(*i as f64),
        Value::F64(f) => Some(*f),
        _ => None,
    }
}

fn unsigned(value: &Value) -> Option<u64> {
    match value {
        Value::F64(f) if *f >= 0.0 && *f < u64::MAX as f64 && f.fract() == 0.0 => Some(*f as u64),
        _ => value.as_u64(),
    }
}

fn validation_issue(value: &Value) -> Option<ValidationIssue> {
    let loc = value.get("loc").and_then(Value::as_arr).unwrap_or_default();
    let skip = usize::from(loc.first().and_then(Value::as_str) == Some("body"));
    let field = loc[skip..]
        .iter()
        .filter_map(|v| match v {
            Value::Str(s) => Some(s.clone()),
            Value::Int(i) => Some(i.to_string()),
            Value::F64(f) => Some(f.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(".");
    Some(ValidationIssue {
        field,
        message: string_field(value, "msg")?.to_owned(),
        kind: string_field(value, "type").unwrap_or_default().to_owned(),
    })
}

pub fn error_from_reply(response: &HttpResponse) -> FalError {
    let status = response.status_code;
    if (200..300).contains(&status) {
        return FalError::BadReply(format!("Unexpected HTTP {status}."));
    }
    let bytes = response.body().unwrap_or_default();
    let json = parse_json(bytes).unwrap_or(Value::Null);
    let detail = json.get("detail");
    if let Some(items) = detail.and_then(Value::as_arr) {
        let issues: Vec<_> = items.iter().filter_map(validation_issue).collect();
        if !issues.is_empty() {
            return FalError::Validation(issues);
        }
    }
    let error = json.get("error");
    let error_type = string_field(&json, "error_type")
        .or_else(|| error.and_then(|e| string_field(e, "type")))
        .or_else(|| string_field(&json, "type"))
        .map(str::to_owned);
    let detail = detail
        .and_then(Value::as_str)
        .or_else(|| error.and_then(Value::as_str))
        .or_else(|| error.and_then(|e| string_field(e, "message")))
        .or_else(|| string_field(&json, "message"))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(bytes);
            let text = text.trim();
            if text.is_empty() {
                format!("HTTP {status}")
            } else {
                text.chars().take(300).collect()
            }
        });
    match status {
        400 if detail.to_ascii_lowercase().contains("still in progress") => FalError::StillRunning,
        401 => FalError::RejectedKey(detail),
        403 => FalError::Forbidden(detail),
        404 => FalError::NotFound(detail),
        422 => FalError::Validation(vec![ValidationIssue {
            field: String::new(),
            message: detail,
            kind: error_type.unwrap_or_default(),
        }]),
        429 => {
            let retry_after = response
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("Retry-After"))
                .flat_map(|(_, values)| values)
                .filter_map(|v| v.trim().parse::<f64>().ok())
                .find(|seconds| seconds.is_finite() && *seconds >= 0.0);
            FalError::RateLimited {
                detail,
                retry_after,
            }
        }
        499 => FalError::Cancelled(detail),
        500.. => FalError::Server {
            status,
            detail,
            error_type,
        },
        _ => FalError::Http { status, detail },
    }
}

fn reply_json(response: &HttpResponse, expected: &[u16]) -> Result<Value, FalError> {
    if !expected.contains(&response.status_code) {
        return Err(error_from_reply(response));
    }
    let bytes = response
        .body()
        .ok_or_else(|| FalError::BadReply("The reply has no body.".into()))?;
    parse_json(bytes).map_err(FalError::BadReply)
}

pub fn parse_submit(endpoint_id: &str, response: &HttpResponse) -> Result<Submitted, FalError> {
    let json = reply_json(response, &[200, 201, 202])?;
    let request_id = required_string(&json, "request_id")?;
    if request_id.is_empty() {
        return Err(FalError::BadReply(
            "The reply has an empty request_id.".into(),
        ));
    }
    let mut urls = queue_urls(endpoint_id, request_id);
    for (name, dest) in [
        ("status_url", &mut urls.status),
        ("response_url", &mut urls.response),
        ("cancel_url", &mut urls.cancel),
    ] {
        if let Some(url) = string_field(&json, name).filter(|url| under_base(url, QUEUE_BASE)) {
            *dest = url.to_owned();
        }
    }
    Ok(Submitted {
        request_id: request_id.to_owned(),
        urls,
        queue_position: json.get("queue_position").and_then(unsigned),
    })
}

pub fn parse_status(response: &HttpResponse) -> Result<Status, FalError> {
    let json = reply_json(response, &[200, 202])?;
    let phase = match required_string(&json, "status")? {
        "IN_QUEUE" => Phase::InQueue {
            position: json.get("queue_position").and_then(unsigned),
        },
        "IN_PROGRESS" => Phase::InProgress,
        "COMPLETED" => Phase::Completed,
        other => {
            return Err(FalError::BadReply(format!(
                "Unknown queue status '{other}'."
            )))
        }
    };
    let mut logs = Vec::new();
    match json.get("logs") {
        Some(Value::Arr(items)) => {
            // A line without a message is skipped, not a reason to stop polling.
            for item in items {
                if let Some(message) = string_field(item, "message") {
                    logs.push(LogLine {
                        timestamp: string_field(item, "timestamp").unwrap_or_default().to_owned(),
                        message: message.to_owned(),
                    });
                }
            }
        }
        None | Some(Value::Null) => {}
        _ => {
            return Err(FalError::BadReply(
                "Expected logs to be an array or null.".into(),
            ))
        }
    }
    Ok(Status {
        phase,
        logs,
        inference_time: json
            .get("metrics")
            .and_then(|v| v.get("inference_time"))
            .and_then(number),
        error: string_field(&json, "error").map(str::to_owned),
        error_type: string_field(&json, "error_type").map(str::to_owned),
    })
}

pub fn parse_result(response: &HttpResponse) -> Result<Value, FalError> {
    reply_json(response, &[200])
}

pub fn parse_cancel(response: &HttpResponse) -> Result<CancelOutcome, FalError> {
    let json = parse_json(response.body().unwrap_or_default()).ok();
    let status = json.as_ref().and_then(|v| string_field(v, "status"));
    match (status, response.status_code) {
        (Some("OK"), _) | (None, 200) => Ok(CancelOutcome::Removed),
        (Some("CANCELLATION_REQUESTED"), _) | (None, 202) => Ok(CancelOutcome::Requested),
        (Some("ALREADY_COMPLETED"), _) | (None, 400) => Ok(CancelOutcome::AlreadyCompleted),
        (Some("NOT_FOUND"), _) | (None, 404) => Ok(CancelOutcome::NotFound),
        (_, 200..=299) => Err(FalError::BadReply(
            "Unrecognized cancellation reply.".into(),
        )),
        _ => Err(error_from_reply(response)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Other,
}

#[derive(Clone, Debug)]
pub struct Media {
    pub url: String,
    pub content_type: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub kind: MediaKind,
}

fn mime_kind(mime: &str) -> Option<MediaKind> {
    let (major, _) = mime.split_once('/')?;
    if major.eq_ignore_ascii_case("image") {
        Some(MediaKind::Image)
    } else if major.eq_ignore_ascii_case("video") {
        Some(MediaKind::Video)
    } else if major.eq_ignore_ascii_case("audio") {
        Some(MediaKind::Audio)
    } else {
        None
    }
}

fn media_kind(url: &str, key: &str, content_type: Option<&str>) -> MediaKind {
    if let Some(kind) = content_type.and_then(mime_kind) {
        return kind;
    }
    match key {
        "image" | "images" => return MediaKind::Image,
        "video" | "videos" => return MediaKind::Video,
        "audio" | "audio_file" | "audios" => return MediaKind::Audio,
        _ => {}
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    let file = path.rsplit('/').next().unwrap_or("");
    let Some((_, ext)) = file.rsplit_once('.') else {
        return MediaKind::Other;
    };
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" => MediaKind::Image,
        "mp4" | "mov" | "webm" => MediaKind::Video,
        "mp3" | "wav" | "ogg" | "flac" | "m4a" => MediaKind::Audio,
        _ => MediaKind::Other,
    }
}

fn media_file(url: &str, key: &str, object: &Value) -> Media {
    let data_mime = url.strip_prefix("data:").and_then(|data| {
        let (header, _) = data.split_once(',')?;
        header.split(';').next().filter(|s| !s.is_empty())
    });
    let content_type = string_field(object, "content_type");
    let kind = data_mime
        .and_then(mime_kind)
        .unwrap_or_else(|| media_kind(url, key, content_type));
    Media {
        url: url.to_owned(),
        kind,
        content_type: content_type.or(data_mime).map(str::to_owned),
        width: object.get("width").and_then(unsigned),
        height: object.get("height").and_then(unsigned),
    }
}

pub fn collect_media(result: &Value) -> Vec<Media> {
    fn visit<'a>(value: &'a Value, key: &str, seen: &mut HashSet<&'a str>, files: &mut Vec<Media>) {
        match value {
            Value::Arr(items) => {
                for item in items {
                    visit(item, key, seen, files);
                }
            }
            Value::Obj(fields) => {
                for (name, child) in fields {
                    if let ("url", Value::Str(url)) = (name.as_str(), child) {
                        if seen.insert(url.as_str()) {
                            files.push(media_file(url, key, value));
                        }
                    } else {
                        visit(child, name, seen, files);
                    }
                }
            }
            _ => {}
        }
    }
    let mut files = Vec::new();
    visit(result, "", &mut HashSet::new(), &mut files);
    files
}

pub fn result_inference_time(result: &Value) -> Option<f64> {
    number(result.get("timings")?.get("inference")?)
}

pub fn decode_data_uri(uri: &str) -> Option<(String, Vec<u8>)> {
    let (header, payload) = uri.strip_prefix("data:")?.split_once(',')?;
    let mime = header.strip_suffix(";base64")?;
    let (major, subtype) = mime.split_once('/')?;
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    };
    if !token(major) || !token(subtype) || payload.len() % 4 != 0 {
        return None;
    }
    // The shared decoder also accepts URL-safe characters and interior '='.
    let unpadded = payload.trim_end_matches('=');
    if payload.len() - unpadded.len() > 2
        || !unpadded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
    {
        return None;
    }
    Some((mime.to_owned(), base64_decode(payload.as_bytes()).ok()?))
}

pub fn merge_input(base: &mut Value, overlay: &Value) -> Result<(), String> {
    let Value::Obj(base) = base else {
        return Err("The base input must be a JSON object.".into());
    };
    let Value::Obj(overlay) = overlay else {
        return Err("The input overlay must be a JSON object.".into());
    };
    for (name, value) in overlay {
        let index = base.iter().position(|(k, _)| k == name);
        match (index, value) {
            (Some(i), Value::Null) => {
                base.remove(i);
            }
            (None, Value::Null) => {}
            (Some(i), value) => base[i].1 = value.clone(),
            (None, value) => base.push((name.clone(), value.clone())),
        }
    }
    Ok(())
}

pub fn pretty(value: &Value) -> String {
    fn newline(out: &mut String, depth: usize) {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    }
    fn write(value: &Value, depth: usize, out: &mut String) {
        match value {
            Value::Obj(fields) => {
                out.push('{');
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    Value::Str(name.clone()).write_into(out);
                    out.push_str(": ");
                    write(value, depth + 1, out);
                }
                if !fields.is_empty() {
                    newline(out, depth);
                }
                out.push('}');
            }
            Value::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    write(item, depth + 1, out);
                }
                if !items.is_empty() {
                    newline(out, depth);
                }
                out.push(']');
            }
            _ => value.write_into(out),
        }
    }
    let mut out = String::new();
    write(value, 0, &mut out);
    out
}

#[derive(Clone, Debug)]
pub struct FieldSpec {
    pub name: String,
    pub default: Option<Value>,
    pub enum_values: Vec<Value>,
    pub accepts_size_object: bool,
}

#[derive(Clone, Debug)]
pub struct InputSchema {
    pub required: Vec<String>,
    pub fields: Vec<FieldSpec>,
}

#[derive(Clone, Debug)]
pub struct ModelEntry {
    pub endpoint_id: String,
    pub display_name: String,
    pub input: Option<InputSchema>,
}

#[derive(Clone, Debug)]
pub struct ModelPage {
    pub models: Vec<ModelEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

// A reference chain can be longer than the JSON nesting depth. Follow it on
// the same explicit stack as the branches, visiting each named component once.
fn schema_parts<'a>(
    schema: &'a Value,
    components: Option<&'a Value>,
    include_alternatives: bool,
) -> impl Iterator<Item = &'a Value> {
    let mut stack = vec![schema];
    let mut followed = HashSet::new();
    std::iter::from_fn(move || {
        let schema = stack.pop()?;
        if let Value::Obj(fields) = schema {
            for (name, value) in fields.iter().rev() {
                if name == "allOf"
                    || (include_alternatives && matches!(name.as_str(), "anyOf" | "oneOf"))
                {
                    if let Value::Arr(branches) = value {
                        stack.extend(branches.iter().rev());
                    }
                }
            }
        }
        if let Some(name) =
            string_field(schema, "$ref").and_then(|s| s.strip_prefix("#/components/schemas/"))
        {
            if followed.insert(name) {
                if let Some(target) = components.and_then(|schemas| {
                    if name.contains('~') {
                        schemas.get(&name.replace("~1", "/").replace("~0", "~"))
                    } else {
                        schemas.get(name)
                    }
                }) {
                    stack.push(target);
                }
            }
        }
        Some(schema)
    })
}

fn field_spec(name: &str, property: &Value, components: Option<&Value>) -> FieldSpec {
    let mut field = FieldSpec {
        name: name.to_owned(),
        default: None,
        enum_values: Vec::new(),
        accepts_size_object: false,
    };
    for part in schema_parts(property, components, true) {
        if field.default.is_none() {
            field.default = part.get("default").cloned();
        }
        if let Some(values) = part.get("enum").and_then(Value::as_arr) {
            field.enum_values.extend_from_slice(values);
        }
        if let Some(props) = part.get("properties") {
            field.accepts_size_object |=
                matches!(string_field(part, "type"), None | Some("object"))
                    && props.get("width").is_some()
                    && props.get("height").is_some();
        }
    }
    field
}

fn model_input(openapi: &Value) -> Option<InputSchema> {
    let Value::Obj(paths) = openapi.get("paths")? else {
        return None;
    };
    let schema = paths
        .iter()
        .filter(|(path, _)| !path.contains("/requests/"))
        .find_map(|(_, route)| {
            route
                .get("post")?
                .get("requestBody")?
                .get("content")?
                .get("application/json")?
                .get("schema")
        })?;
    let components = openapi.get("components").and_then(|v| v.get("schemas"));
    let mut fields = Vec::new();
    let mut required = Vec::new();
    let mut seen_names = HashSet::new();
    let mut found_properties = false;
    for part in schema_parts(schema, components, false) {
        if let Some(names) = part.get("required").and_then(Value::as_arr) {
            for name in names.iter().filter_map(Value::as_str) {
                if !required.iter().any(|n| n == name) {
                    required.push(name.to_owned());
                }
            }
        }
        if let Some(Value::Obj(properties)) = part.get("properties") {
            found_properties = true;
            for (name, property) in properties {
                if seen_names.insert(name.as_str()) {
                    fields.push(field_spec(name, property, components));
                }
            }
        }
    }
    found_properties.then_some(InputSchema { required, fields })
}

pub fn parse_models_page(response: &HttpResponse) -> Result<ModelPage, FalError> {
    let json = reply_json(response, &[200])?;
    let items = json
        .get("models")
        .and_then(Value::as_arr)
        .ok_or_else(|| FalError::BadReply("Expected a models array.".into()))?;
    let has_more = json
        .get("has_more")
        .and_then(Value::as_bool)
        .ok_or_else(|| FalError::BadReply("Expected a boolean in 'has_more'.".into()))?;
    let next_cursor = string_field(&json, "next_cursor").map(str::to_owned);
    let mut models = Vec::with_capacity(items.len());
    for model in items {
        let endpoint_id = required_string(model, "endpoint_id")?;
        let display_name = model
            .get("metadata")
            .and_then(|v| string_field(v, "display_name"))
            .filter(|name| !name.is_empty())
            .unwrap_or(endpoint_id);
        models.push(ModelEntry {
            endpoint_id: endpoint_id.to_owned(),
            display_name: display_name.to_owned(),
            input: model.get("openapi").and_then(model_input),
        });
    }
    Ok(ModelPage {
        models,
        next_cursor,
        has_more,
    })
}

/// Fal's image size presets: the pixels each stands for, and its aspect
/// ratio as the models that take `aspect_ratio` write it.
pub const SIZE_PRESETS: &[(&str, u64, u64, &str)] = &[
    ("square_hd", 1024, 1024, "1:1"),
    ("square", 512, 512, "1:1"),
    ("portrait_4_3", 768, 1024, "3:4"),
    ("portrait_16_9", 576, 1024, "9:16"),
    ("landscape_4_3", 1024, 768, "4:3"),
    ("landscape_16_9", 1024, 576, "16:9"),
];

/// The width and height a size value stands for: a preset, "WxH", a
/// {width, height} object, or a ratio "W:H" (its sides, not pixels).
fn size_shape(value: &Value) -> Option<(f64, f64)> {
    if let Some(text) = value.as_str() {
        if let Some((_, w, h, _)) = SIZE_PRESETS.iter().find(|(name, ..)| *name == text) {
            return Some((*w as f64, *h as f64));
        }
        let (w, h) = text.split_once(['x', 'X', ':'])?;
        let (w, h): (f64, f64) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
        return (w > 0.0 && h > 0.0).then_some((w, h));
    }
    let w = value.get("width").and_then(number)?;
    let h = value.get("height").and_then(number)?;
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// The pixels a size value asks for (presets, "WxH" and objects; not ratios).
pub fn size_pixels(value: &Value) -> Option<u64> {
    if value.as_str().is_some_and(|text| text.contains(':')) {
        return None;
    }
    size_shape(value).map(|(w, h)| (w * h) as u64)
}

/// Of a field's listed values, the one nearest a shape: the closest aspect
/// ratio, then the closest area ("auto" and other words don't count).
fn nearest_choice(choices: &[Value], w: f64, h: f64) -> Option<Value> {
    let ratio = (w / h).ln();
    let area = w * h;
    let mut best: Option<(f64, f64, &Value)> = None;
    for choice in choices {
        let Some((cw, ch)) = size_shape(choice) else { continue };
        let ratio_gap = ((cw / ch).ln() - ratio).abs();
        // A ratio ("16:9") has no area; it only competes on ratio.
        let area_gap = if choice.as_str().is_some_and(|t| t.contains(':')) { 0.0 } else { ((cw * ch) / area).ln().abs() };
        if best.is_none_or(|(r, a, _)| ratio_gap < r - 1e-9 || (ratio_gap <= r + 1e-9 && area_gap < a)) {
            best = Some((ratio_gap, area_gap, choice));
        }
    }
    best.map(|(.., choice)| choice.clone())
}

/// How a model wants one of Fal's size presets, from its schema: as
/// `image_size` (the preset, else its width and height, else the listed size
/// nearest it), or else as `aspect_ratio` (the preset's ratio, else the
/// listed ratio nearest it). None when the model takes neither.
pub fn size_input(schema: &InputSchema, preset: &str) -> Option<(String, Value)> {
    let &(_, w, h, ratio) = SIZE_PRESETS.iter().find(|(name, ..)| *name == preset)?;
    let (w, h) = (w as f64, h as f64);
    let field = |name: &str| schema.fields.iter().find(|field| field.name == name);
    let offers = |field: &FieldSpec, choice: &str| field.enum_values.iter().any(|v| v.as_str() == Some(choice));
    if let Some(field) = field("image_size") {
        let value = if field.enum_values.is_empty() && !field.accepts_size_object || offers(field, preset) {
            Value::Str(preset.into())
        } else if field.accepts_size_object {
            obj(vec![("width", Value::Int(w as i64)), ("height", Value::Int(h as i64))])
        } else {
            nearest_choice(&field.enum_values, w, h)?
        };
        return Some(("image_size".into(), value));
    }
    let field = field("aspect_ratio")?;
    let value = if field.enum_values.is_empty() || offers(field, ratio) {
        Value::Str(ratio.into())
    } else {
        nearest_choice(&field.enum_values, w, h)?
    };
    Some(("aspect_ratio".into(), value))
}

/// The input a short sentence names: "image_size square", "aspect_ratio 3:4".
pub fn describe_size(name: &str, value: &Value) -> String {
    match value {
        Value::Str(text) => format!("{name} {text}"),
        _ => match size_shape(value) {
            Some((w, h)) => format!("{name} {w}x{h}"),
            None => format!("{name} {}", value.to_json()),
        },
    }
}

pub fn sweep_input(schema: &InputSchema, prompt: &str, seed: i64) -> Result<Value, String> {
    let has_field = |name: &str| schema.fields.iter().any(|field| field.name == name);
    if !has_field("prompt") {
        return Err("no prompt input".into());
    }
    // Every required input the request can't fill, named together.
    let mut missing: Vec<String> = schema.required.iter().filter(|name| !has_field(name)).cloned().collect();
    // The smallest common size, in the form the model takes.
    let size = size_input(schema, "square");
    let mut input = Vec::new();
    for field in &schema.fields {
        let value = match field.name.as_str() {
            "prompt" => Value::Str(prompt.to_owned()),
            "seed" => Value::Int(seed),
            "num_images" => Value::Int(1),
            name if size.as_ref().is_some_and(|(size_name, _)| size_name == name) => size.as_ref().map(|(_, value)| value.clone()).unwrap_or(Value::Null),
            _ if schema.required.contains(&field.name) => match field.default.as_ref().or_else(|| field.enum_values.first()) {
                Some(value) => value.clone(),
                None => {
                    missing.push(field.name.clone());
                    continue;
                }
            },
            _ => continue,
        };
        input.push((field.name.clone(), value));
    }
    if !missing.is_empty() {
        return Err(format!("needs {}", missing.join(", ")));
    }
    Ok(Value::Obj(input))
}

#[derive(Clone, Debug)]
pub struct Price {
    pub endpoint_id: String,
    pub unit_price: f64,
    pub unit: String,
    pub currency: String,
}

pub fn parse_pricing(response: &HttpResponse) -> Result<Vec<Price>, FalError> {
    let json = reply_json(response, &[200])?;
    let items = json
        .get("prices")
        .and_then(Value::as_arr)
        .ok_or_else(|| FalError::BadReply("Expected a prices array.".into()))?;
    items
        .iter()
        .map(|item| {
            Ok(Price {
                endpoint_id: required_string(item, "endpoint_id")?.to_owned(),
                unit_price: item.get("unit_price").and_then(number).ok_or_else(|| {
                    FalError::BadReply("Expected a number in 'unit_price'.".into())
                })?,
                unit: required_string(item, "unit")?.to_owned(),
                currency: required_string(item, "currency")?.to_owned(),
            })
        })
        .collect()
}

pub fn civil_utc(unix_secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    // Howard Hinnant's civil_from_days, with March as the first month of
    // each civil year so the leap day falls at the end of a 400-year cycle.
    let days = (unix_secs / 86_400) as i64 + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = march_month + if march_month < 10 { 3 } else { -9 };
    let year = era * 400 + year_of_era + i64::from(month <= 2);
    let time = (unix_secs % 86_400) as u32;
    (
        year,
        month as u32,
        day as u32,
        time / 3_600,
        time / 60 % 60,
        time % 60,
    )
}
