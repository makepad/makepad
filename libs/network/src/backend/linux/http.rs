use super::socket_stream::SocketStream;

use crate::types::{HttpError, HttpRequest, HttpResponse, NetworkResponse};
use makepad_live_id::LiveId;
use std::{
    collections::HashMap,
    io,
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

pub struct LinuxHttpSocket;

impl LinuxHttpSocket {
    pub fn open(
        request_id: LiveId,
        request: HttpRequest,
        response_sender: Sender<NetworkResponse>,
    ) {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        cancellation_map()
            .lock()
            .unwrap()
            .insert(request_id, cancel_flag.clone());

        std::thread::spawn(move || {
            let metadata_id = request.metadata_id;
            let result = run_http_request(request_id, &request, &response_sender, &cancel_flag);

            cancellation_map().lock().unwrap().remove(&request_id);

            if let Err(err) = result {
                let _ = response_sender.send(NetworkResponse::HttpError {
                    request_id,
                    error: HttpError {
                        message: err,
                        metadata_id,
                    },
                });
            }
        });
    }

    pub fn cancel(request_id: LiveId) {
        if let Some(flag) = cancellation_map().lock().unwrap().get(&request_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

fn cancellation_map() -> &'static Mutex<HashMap<LiveId, Arc<AtomicBool>>> {
    static MAP: OnceLock<Mutex<HashMap<LiveId, Arc<AtomicBool>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn run_http_request(
    request_id: LiveId,
    request: &HttpRequest,
    response_sender: &Sender<NetworkResponse>,
    cancel_flag: &AtomicBool,
) -> Result<(), String> {
    let split = request.split_url();
    let use_tls = match split.proto {
        "http" => false,
        "https" => true,
        other => {
            return Err(format!(
                "unsupported URL scheme for http_request: {other} (expected http or https)"
            ));
        }
    };

    let mut stream =
        SocketStream::connect(split.host, split.port, use_tls, request.ignore_ssl_cert)
            .map_err(|e| format!("connect failed: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    write_request(&mut stream, request, &split, use_tls)
        .map_err(|e| format!("write failed: {e}"))?;

    let (status_code, headers_string, mut body_prefix, chunked) =
        read_response_head(&mut stream, cancel_flag).map_err(|e| format!("read failed: {e}"))?;

    if request.is_streaming {
        if !body_prefix.is_empty() {
            let _ = response_sender.send(NetworkResponse::HttpStreamChunk {
                request_id,
                response: HttpResponse {
                    metadata_id: request.metadata_id,
                    status_code,
                    headers: Default::default(),
                    body: Some(std::mem::take(&mut body_prefix)),
                },
            });
        }

        let mut buf = [0u8; 16384];
        loop {
            if cancel_flag.load(Ordering::SeqCst) {
                return Err("request cancelled".to_string());
            }
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = response_sender.send(NetworkResponse::HttpStreamChunk {
                        request_id,
                        response: HttpResponse {
                            metadata_id: request.metadata_id,
                            status_code,
                            headers: Default::default(),
                            body: Some(buf[..n].to_vec()),
                        },
                    });
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(err) => return Err(format!("stream read failed: {err}")),
            }
        }

        let _ = response_sender.send(NetworkResponse::HttpStreamComplete {
            request_id,
            response: HttpResponse::from_header_string(
                request.metadata_id,
                status_code,
                headers_string,
                None,
            ),
        });
        stream.shutdown();
        return Ok(());
    }

    let mut body = std::mem::take(&mut body_prefix);
    let mut buf = [0u8; 16384];
    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            return Err("request cancelled".to_string());
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&buf[..n]),
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) => return Err(format!("read failed: {err}")),
        }
    }
    stream.shutdown();

    if chunked {
        if let Ok(decoded) = decode_chunked_body(&body) {
            body = decoded;
        }
    }

    let _ = response_sender.send(NetworkResponse::HttpResponse {
        request_id,
        response: HttpResponse::from_header_string(
            request.metadata_id,
            status_code,
            headers_string,
            Some(body),
        ),
    });
    Ok(())
}

fn write_request(
    stream: &mut SocketStream,
    request: &HttpRequest,
    split: &crate::types::SplitUrl<'_>,
    use_tls: bool,
) -> io::Result<()> {
    let path = if split.file.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", split.file)
    };
    let method = request.method.as_str();

    let default_port = if use_tls { "443" } else { "80" };
    let host_header = if split.port == default_port {
        split.host.to_string()
    } else {
        format!("{}:{}", split.host, split.port)
    };

    let mut req = String::new();
    req.push_str(&format!("{method} {path} HTTP/1.1\r\n"));
    req.push_str(&format!("Host: {host_header}\r\n"));
    req.push_str("Connection: close\r\n");

    let mut has_content_length = false;
    for (name, values) in &request.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        for value in values {
            req.push_str(name);
            req.push_str(": ");
            req.push_str(value);
            req.push_str("\r\n");
        }
    }

    if let Some(body) = &request.body {
        if !has_content_length {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
    }
    req.push_str("\r\n");

    write_all(stream, req.as_bytes())?;
    if let Some(body) = &request.body {
        write_all(stream, body)?;
    }
    stream.flush()
}

fn read_response_head(
    stream: &mut SocketStream,
    cancel_flag: &AtomicBool,
) -> io::Result<(u16, String, Vec<u8>, bool)> {
    let mut data = Vec::with_capacity(8192);
    let mut buf = [0u8; 4096];
    let mut header_end = None;

    while header_end.is_none() {
        if cancel_flag.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            ));
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed before HTTP headers",
                ));
            }
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                header_end = find_header_end(&data);
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) => return Err(err),
        }
    }

    let header_end = header_end.unwrap();
    let head = &data[..header_end];
    let body_prefix = data[header_end..].to_vec();
    let head_str = String::from_utf8_lossy(head);

    let mut lines = head_str.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or_default();

    let mut headers_string = String::new();
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            headers_string.push_str(name.trim());
            headers_string.push_str(": ");
            headers_string.push_str(value);
            headers_string.push('\n');

            if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }

    Ok((status_code, headers_string, body_prefix, chunked))
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

fn write_all(stream: &mut SocketStream, data: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        match stream.write(&data[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "socket closed while writing",
                ));
            }
            Ok(n) => offset += n,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn decode_chunked_body(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < raw.len() {
        let line_end = find_crlf(raw, i).ok_or("invalid chunked body: missing chunk size line")?;
        let size_line = std::str::from_utf8(&raw[i..line_end])
            .map_err(|_| "invalid chunked body: chunk size line is not utf-8")?;
        let size_hex = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| "invalid chunked body: bad chunk size")?;
        i = line_end + 2;

        if size == 0 {
            break;
        }
        if i + size > raw.len() {
            return Err("invalid chunked body: chunk exceeds buffer".to_string());
        }
        out.extend_from_slice(&raw[i..i + size]);
        i += size;

        if i + 2 > raw.len() || &raw[i..i + 2] != b"\r\n" {
            return Err("invalid chunked body: missing chunk terminator".to_string());
        }
        i += 2;
    }
    Ok(out)
}

fn find_crlf(data: &[u8], start: usize) -> Option<usize> {
    data[start..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|p| start + p)
}
