//! A small client for AI hubs over the fleet TLS front: the server's certificate is checked
//! against the fleet root and this process's credential proves itself
//! (`makepad_network::fleet::id::connect`). The jail's proxy uses it with the scoped `intake`
//! credential; the prefilter evaluation uses it with the admin Mac's own.

use makepad_strict_json::{self as json, Value};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// `https://host:port` -> (host, port).
pub fn host_port(base: &str) -> Result<(String, String), String> {
    let rest = base.strip_prefix("https://").ok_or_else(|| format!("{base}: not an https hub address"))?;
    let rest = rest.trim_end_matches('/');
    let (host, port) = rest.rsplit_once(':').ok_or_else(|| format!("{base}: no port"))?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return Err(format!("{base}: not host:port"));
    }
    Ok((host.to_string(), port.to_string()))
}

/// One request; Ok with the status and the body (at most `max` bytes).
pub fn request(base: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&[u8]>, max: usize, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    let (host, port) = host_port(base)?;
    let (mut stream, _server, authorization) = makepad_network::fleet::id::connect(&host, &port).map_err(|e| format!("hub {host}:{port}: {e}"))?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nAuthorization: {authorization}\r\nAccept: application/json\r\n");
    for (name, value) in headers {
        if name.contains([':', '\r', '\n']) || value.contains(['\r', '\n']) {
            return Err("a header with a line break".into());
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let body = body.unwrap_or(&[]);
    head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()));
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body)).and_then(|_| stream.flush()).map_err(|e| format!("hub write: {e}"))?;
    let deadline = Instant::now() + timeout;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        if Instant::now() > deadline {
            return Err("hub: no complete answer in time".into());
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > max + 64 * 1024 {
                    return Err("hub: answer too large".into());
                }
                if let Some((status, start, Some(len))) = parse_head(&buf)? {
                    if buf.len() >= start + len {
                        return Ok((status, buf[start..start + len].to_vec()));
                    }
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => continue,
            Err(e) => return Err(format!("hub read: {e}")),
        }
    }
    match parse_head(&buf)? {
        Some((status, start, None)) => Ok((status, buf[start..].to_vec())),
        Some((status, start, Some(len))) if buf.len() >= start + len => Ok((status, buf[start..start + len].to_vec())),
        _ => Err("hub: the answer ended early".into()),
    }
}

/// (status, body start, content length) once the head is complete.
fn parse_head(buf: &[u8]) -> Result<Option<(u16, usize, Option<usize>)>, String> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() > 64 * 1024 { Err("hub: head too large".into()) } else { Ok(None) };
    };
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| "hub: head is not utf-8".to_string())?;
    let mut lines = head.split("\r\n");
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).ok_or("hub: no status")?;
    let mut len = None;
    for line in lines {
        if let Some((n, v)) = line.split_once(':') {
            if n.trim().eq_ignore_ascii_case("content-length") {
                len = Some(v.trim().parse().map_err(|_| "hub: bad content-length".to_string())?);
            }
            if n.trim().eq_ignore_ascii_case("transfer-encoding") {
                return Err("hub: chunked answers are not expected here".into());
            }
        }
    }
    Ok(Some((status, end + 4, len)))
}

fn json_of(status: u16, body: &[u8]) -> Result<Value, String> {
    let v = json::parse(body).map_err(|e| format!("hub answer ({status}): {e}"))?;
    if !(200..300).contains(&status) {
        let why = v.get("error").and_then(Value::as_str).unwrap_or("refused");
        return Err(format!("hub {status}: {}", why.chars().take(200).collect::<String>()));
    }
    Ok(v)
}

/// A chat job on the hub, start to end: Ok with the answer text.
pub fn generate_text(base: &str, item: &str, body: &Value, patience: Duration) -> Result<String, String> {
    let item_header = [("X-Intake-Item", item)];
    let (status, answer) = request(base, "POST", "/generate", &item_header, Some(body.to_json().as_bytes()), 1 << 20, Duration::from_secs(60))?;
    let job = json_of(status, &answer)?.get("job_id").and_then(Value::as_str).ok_or("hub: no job id")?.to_string();
    if !job.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') || job.len() > 64 {
        return Err("hub: a job id that is not a word".into());
    }
    let deadline = Instant::now() + patience;
    loop {
        let (status, answer) = request(base, "GET", &format!("/job/{job}"), &item_header, None, 4 << 20, Duration::from_secs(60))?;
        let v = json_of(status, &answer)?;
        match v.get("state").and_then(Value::as_str) {
            Some("done") => return Ok(v.get("text").and_then(Value::as_str).unwrap_or("").to_string()),
            Some("error") | Some("cancelled") => return Err(format!("hub job: {}", v.get("error").and_then(Value::as_str).unwrap_or("failed").chars().take(200).collect::<String>())),
            _ => {}
        }
        if Instant::now() > deadline {
            let _ = request(base, "POST", &format!("/job/{job}/cancel"), &item_header, None, 1 << 16, Duration::from_secs(20));
            return Err("hub job: no answer in time (cancelled)".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The prefilter's one request shape (the hubs refuse anything else from the intake credential).
pub fn prefilter_body(model: &str, system: &str, user: &str, max_tokens: u32, seed: u64) -> Value {
    json::obj(vec![
        ("model", json::s(model)),
        ("domain", json::s("chat")),
        ("chat_system", json::s(system)),
        ("chat_messages", Value::Arr(vec![json::obj(vec![("role", json::s("user")), ("text", json::s(user))])])),
        ("max_tokens", Value::Int(max_tokens as i64)),
        ("temperature", Value::Int(0)),
        ("thinking", Value::Bool(false)),
        ("seed", Value::Int((seed & 0x7fff_ffff_ffff) as i64)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_addresses_heads_and_the_prefilter_shape() {
        assert_eq!(host_port("https://192.0.2.7:8123").unwrap(), ("192.0.2.7".to_string(), "8123".to_string()));
        assert!(host_port("http://192.0.2.7:8123").is_err() && host_port("https://192.0.2.7").is_err(), "fleet TLS only, with a port");
        assert_eq!(parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap(), Some((200, 38, Some(2))));
        assert_eq!(parse_head(b"HTTP/1.1 200 OK\r\nContent-").unwrap(), None);
        assert!(parse_head(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").is_err());
        let body = prefilter_body("qwen3.8-27b", "sys", "user", 120, 7).to_json();
        // the exact keys the hub's intake scope admits, and no other
        let v = json::parse(body.as_bytes()).unwrap();
        let Value::Obj(fields) = &v else { panic!() };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["model", "domain", "chat_system", "chat_messages", "max_tokens", "temperature", "thinking", "seed"]);
        assert_eq!(v.get("thinking").and_then(Value::as_bool), Some(false));
    }
}
