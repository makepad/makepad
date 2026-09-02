use makepad_network::http_server::{HttpServerResponse, HttpServerResponseSender};

pub const APP_ISOLATION_HEADERS: &str = "Cross-Origin-Opener-Policy: same-origin\r\n\
Cross-Origin-Embedder-Policy: require-corp\r\n";
pub const PUBLIC_ASSET_HEADERS: &str = "Access-Control-Allow-Origin: *\r\n\
Cross-Origin-Resource-Policy: cross-origin\r\n\
Access-Control-Expose-Headers: Accept-Ranges, Content-Range, Content-Length, ETag\r\n";

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

pub fn response(
    status: u16,
    content_type: Option<&str>,
    cache_control: &str,
    extra_headers: &str,
    body: Vec<u8>,
) -> HttpServerResponse {
    let content_type = content_type
        .map(|value| format!("Content-Type: {value}\r\n"))
        .unwrap_or_default();
    let header = format!(
        "HTTP/1.1 {status} {}\r\n{content_type}{APP_ISOLATION_HEADERS}\
         Cache-Control: {cache_control}\r\n{extra_headers}Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    HttpServerResponse::new(header, body)
}

pub fn send_response(sender: &HttpServerResponseSender, response: HttpServerResponse) {
    let _ = sender.send(response);
}

pub fn json_response(status: u16, cache_control: &str, json: String) -> HttpServerResponse {
    response(
        status,
        Some("application/json; charset=utf-8"),
        cache_control,
        "",
        json.into_bytes(),
    )
}

pub fn api_error(status: u16, code: &str, message: &str) -> HttpServerResponse {
    json_response(
        status,
        "private, no-store",
        format!(
            "{{\"error\":{{\"code\":{},\"message\":{}}}}}",
            json_string(code),
            json_string(message)
        ),
    )
}

pub fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn percent_decode(input: &str, max_len: usize) -> Result<String, ()> {
    percent_decode_inner(input, max_len, true)
}

/// Percent-decodes a URL path without applying HTML form semantics. A literal
/// `+` is a legal path byte and must not silently name a space-containing file.
pub fn percent_decode_path(input: &str, max_len: usize) -> Result<String, ()> {
    percent_decode_inner(input, max_len, false)
}

fn percent_decode_inner(input: &str, max_len: usize, plus_as_space: bool) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len().min(max_len));
    let mut i = 0;
    while i < bytes.len() {
        let byte = match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex(bytes[i + 1]).ok_or(())?;
                let lo = hex(bytes[i + 2]).ok_or(())?;
                i += 3;
                (hi << 4) | lo
            }
            b'%' => return Err(()),
            b'+' if plus_as_space => {
                i += 1;
                b' '
            }
            byte => {
                i += 1;
                byte
            }
        };
        if out.len() >= max_len {
            break;
        }
        out.push(byte);
    }
    String::from_utf8(out).map_err(|_| ())
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn query_pairs(query: Option<&str>) -> Result<Vec<(String, String)>, ()> {
    query
        .unwrap_or("")
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Ok((percent_decode(key, 128)?, percent_decode(value, 1 << 20)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_has_stable_shape_and_escaping() {
        let response = api_error(400, "bad_request", "bad \"value\"");
        assert_eq!(
            String::from_utf8(response.body).unwrap(),
            r#"{"error":{"code":"bad_request","message":"bad \"value\""}}"#
        );
    }

    #[test]
    fn percent_decodes_form_components() {
        assert_eq!(percent_decode("Oude%20Gracht+1", 100), Ok("Oude Gracht 1".into()));
        assert_eq!(percent_decode_path("a+b%20c", 100), Ok("a+b c".into()));
        assert!(percent_decode("%zz", 100).is_err());
    }
}
