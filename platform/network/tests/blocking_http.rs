//! Local-TCP tests for the no-redirect blocking HTTP client.
//! No external hosts are contacted.

use makepad_network::blocking_http::{
    post_json, request_no_redirect, CancelToken, Error, Limits, Request,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn listen() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    (listener, port)
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = &buf[..pos + 4];
            let head_s = String::from_utf8_lossy(head);
            let mut content_len = 0usize;
            for line in head_s.split("\r\n") {
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
            let body_have = buf.len() - (pos + 4);
            if body_have >= content_len {
                return buf;
            }
        }
        match stream.read(&mut tmp) {
            Ok(0) => return buf,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return buf,
        }
        if buf.len() > 64 * 1024 {
            return buf;
        }
    }
}

fn serve_one(listener: TcpListener, response: Vec<u8>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let req = read_request(&mut stream);
        let _ = stream.write_all(&response);
        let _ = stream.flush();
        req
    })
}

fn serve_hang_after_accept(listener: TcpListener) -> (thread::JoinHandle<()>, Arc<Mutex<bool>>) {
    let accepted = Arc::new(Mutex::new(false));
    let flag = accepted.clone();
    let handle = thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            *flag.lock().unwrap() = true;
            thread::sleep(Duration::from_secs(1));
            drop(stream);
        }
    });
    (handle, accepted)
}

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

fn tight_limits() -> Limits {
    Limits {
        max_head_bytes: 512,
        max_header_count: 8,
        max_header_line_bytes: 128,
        max_trailer_count: 2,
        max_trailer_bytes: 64,
        max_body_bytes: 64,
        max_chunk_line_bytes: 16,
        total_timeout: Duration::from_secs(3),
    }
}

#[test]
fn post_json_serializes_request_without_reserved_caller_headers() {
    let (listener, port) = listen();
    let handle = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    );
    let body = br#"{"model":"gpt-5.6"}"#.to_vec();
    let req = Request::post(url(port, "/v1/responses"))
        .bearer("test-token-abc")
        .unwrap()
        .json_body(body.clone())
        .unwrap();
    let resp = post_json(req).expect("post");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"ok");

    let raw = String::from_utf8(handle.join().unwrap()).unwrap();
    assert!(raw.starts_with("POST /v1/responses HTTP/1.1\r\n"), "{raw}");
    assert!(raw.contains("Host: 127.0.0.1:"), "{raw}");
    assert!(raw.contains("Authorization: Bearer test-token-abc"), "{raw}");
    assert!(raw.contains("Content-Type: application/json"), "{raw}");
    assert!(raw.contains(&format!("Content-Length: {}\r\n", body.len())), "{raw}");
    assert!(raw.contains("Connection: close"), "{raw}");
    assert_eq!(raw.matches("Content-Length:").count(), 1);
    assert!(!raw.to_ascii_lowercase().contains("transfer-encoding"));
    assert!(raw.contains("{\"model\":\"gpt-5.6\"}"), "{raw}");
}

fn expect_header_err(result: Result<Request, Error>, want: Error) {
    match result {
        Err(e) => assert_eq!(e, want),
        Ok(_) => panic!("expected {want:?}"),
    }
}

fn expect_err(result: Result<makepad_network::blocking_http::Response, Error>, want: Error) -> Error {
    match result {
        Err(e) => {
            assert_eq!(e, want);
            e
        }
        Ok(_) => panic!("expected {want:?}"),
    }
}

#[test]
fn caller_cannot_set_reserved_or_injected_headers() {
    expect_header_err(Request::post("http://127.0.0.1:1/").header("Host", "evil"), Error::ReservedHeader);
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Content-Length", "9"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Transfer-Encoding", "chunked"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Connection", "keep-alive"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("X-A", "line\r\nX-Injected: 1"),
        Error::InvalidHeader,
    );
    expect_header_err(Request::post("http://127.0.0.1:1/").header("X-A\nB", "1"), Error::InvalidHeader);
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("User-Agent", "evil"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Accept-Encoding", "gzip"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Accept", "application/json"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Expect", "100-continue"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("TE", "trailers"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Trailer", "Expires"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Upgrade", "websocket"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("Proxy-Authorization", "Basic x"),
        Error::ReservedHeader,
    );
    expect_header_err(
        Request::post("http://127.0.0.1:1/").header("X-A", "has\u{01}control"),
        Error::InvalidHeader,
    );
    let dup = Request::post("http://127.0.0.1:1/")
        .header("X-One", "a")
        .unwrap()
        .header("x-one", "b");
    expect_header_err(dup, Error::InvalidHeader);
}

#[test]
fn cleartext_non_loopback_is_refused() {
    let err = expect_err(request_no_redirect(Request::get("http://example.com/")), Error::CleartextForbidden);
    assert!(!err.to_string().contains("example.com"));
}

#[test]
fn redirect_is_refused_and_never_contacts_sink_with_bearer() {
    let sink_hits = Arc::new(Mutex::new(0u32));
    let (sink, sink_port) = listen();
    let hits = sink_hits.clone();
    let sink_thread = thread::spawn(move || {
        let _ = sink.set_nonblocking(true);
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(400) {
            if sink.accept().is_ok() {
                *hits.lock().unwrap() += 1;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });

    let (origin, origin_port) = listen();
    let location = format!("http://127.0.0.1:{sink_port}/stolen");
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
    );
    let origin_thread = serve_one(origin, response.into_bytes());

    let err = expect_err(
        post_json(
            Request::post(url(origin_port, "/v1/responses"))
                .bearer("super-secret-bearer")
                .unwrap()
                .json_body(b"{}".to_vec())
                .unwrap(),
        ),
        Error::RedirectRefused,
    );
    assert!(!err.to_string().contains("super-secret-bearer"));

    let seen = String::from_utf8(origin_thread.join().unwrap()).unwrap();
    assert!(seen.contains("Authorization: Bearer super-secret-bearer"));
    thread::sleep(Duration::from_millis(80));
    assert_eq!(*sink_hits.lock().unwrap(), 0, "sink must never be contacted");
    let _ = sink_thread.join();
}

#[test]
fn request_no_redirect_returns_3xx_without_following() {
    let (sink, sink_port) = listen();
    let sink_hits = Arc::new(Mutex::new(0u32));
    let hits = sink_hits.clone();
    thread::spawn(move || {
        if sink.accept().is_ok() {
            *hits.lock().unwrap() += 1;
        }
    });
    let (origin, origin_port) = listen();
    let location = format!("http://127.0.0.1:{sink_port}/next");
    let origin_thread = serve_one(
        origin,
        format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
            .into_bytes(),
    );
    let resp = request_no_redirect(Request::get(url(origin_port, "/go"))).expect("one hop");
    assert_eq!(resp.status, 307);
    assert_eq!(resp.header("location"), Some(location.as_str()));
    let _ = origin_thread.join();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(*sink_hits.lock().unwrap(), 0);
}

#[test]
fn small_head_with_coalesced_body_is_accepted() {
    let (listener, port) = listen();
    let mut resp = b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\n\r\n".to_vec();
    resp.extend(std::iter::repeat(b'x').take(200));
    let _ = serve_one(listener, resp);
    let mut limits = tight_limits();
    limits.max_head_bytes = 64;
    limits.max_body_bytes = 256;
    let got = request_no_redirect(Request::get(url(port, "/")).limits(limits)).expect("coalesced");
    assert_eq!(got.status, 200);
    assert_eq!(got.body.len(), 200);
}

#[test]
fn oversized_head_is_rejected() {
    let (listener, port) = listen();
    let mut resp = b"HTTP/1.1 200 OK\r\n".to_vec();
    for i in 0..40 {
        resp.extend_from_slice(format!("X-{i}: {}\r\n", "a".repeat(20)).as_bytes());
    }
    resp.extend_from_slice(b"Content-Length: 0\r\n\r\n");
    let _ = serve_one(listener, resp);
    expect_err(
        request_no_redirect(Request::get(url(port, "/")).limits(tight_limits())),
        Error::ResponseTooLarge,
    );
}

#[test]
fn oversized_body_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\n\r\n".to_vec(),
    );
    expect_err(
        request_no_redirect(Request::get(url(port, "/")).limits(tight_limits())),
        Error::ResponseTooLarge,
    );
}

#[test]
fn oversized_chunk_line_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0123456789abcdef0\r\n".to_vec(),
    );
    expect_err(
        request_no_redirect(Request::get(url(port, "/")).limits(tight_limits())),
        Error::ResponseTooLarge,
    );
}

#[test]
fn invalid_chunk_crlf_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWikiXX0\r\n\r\n".to_vec(),
    );
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);
}

#[test]
fn transfer_encoding_with_content_length_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\n4\r\nWiki\r\n0\r\n\r\n"
            .to_vec(),
    );
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);
}

#[test]
fn duplicate_content_length_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nContent-Length: 4\r\n\r\nxxxx".to_vec(),
    );
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);
}

#[test]
fn malformed_content_length_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nContent-Length: 12abc\r\n\r\n".to_vec(),
    );
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);
}

#[test]
fn unsupported_transfer_encoding_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n".to_vec(),
    );
    expect_err(
        request_no_redirect(Request::get(url(port, "/"))),
        Error::UnsupportedTransferEncoding,
    );
}

#[test]
fn chunked_body_roundtrip() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n"
            .to_vec(),
    );
    let resp = request_no_redirect(Request::get(url(port, "/"))).expect("chunked");
    assert_eq!(resp.body, b"Wikipedia");
}

#[test]
fn cancel_returns_without_waiting_for_body() {
    let (listener, port) = listen();
    let (hang, accepted) = serve_hang_after_accept(listener);
    let token = CancelToken::new();
    let token2 = token.clone();
    let url = url(port, "/slow");
    let worker = thread::spawn(move || {
        request_no_redirect(Request::get(url).cancel_token(token2).limits(Limits {
            total_timeout: Duration::from_secs(5),
            ..Limits::default()
        }))
    });
    let start = std::time::Instant::now();
    while !*accepted.lock().unwrap() && start.elapsed() < Duration::from_secs(2) {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(*accepted.lock().unwrap(), "server should have accepted");
    token.cancel();
    expect_err(worker.join().unwrap(), Error::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(3));
    let _ = hang.join();
}

#[test]
fn framing_trailers_and_trailing_bytes_are_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n0\r\nContent-Length: 4\r\n\r\n"
            .to_vec(),
    );
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);

    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n0\r\n\r\nGARBAGE".to_vec(),
    );
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);
}

#[test]
fn close_delimited_clean_eof_succeeds() {
    let (listener, port) = listen();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_request(&mut stream);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello");
        let _ = stream.flush();
        drop(stream);
    });
    let resp = request_no_redirect(Request::get(url(port, "/"))).expect("clean eof");
    assert_eq!(resp.body, b"hello");
    let _ = handle.join();
}

#[cfg(unix)]
#[test]
fn close_delimited_reset_is_not_success() {
    let (listener, port) = listen();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut stream = stream;
        let _ = read_request(&mut stream);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhe");
        let _ = stream.flush();
        rst_stream(stream);
    });
    let err = match request_no_redirect(Request::get(url(port, "/"))) {
        Err(e) => e,
        Ok(_) => panic!("reset must not parse as a clean body"),
    };
    assert!(err == Error::Reset || err == Error::Io, "{err:?}");
    let _ = handle.join();
}

#[cfg(unix)]
fn rst_stream(stream: TcpStream) {
    #[repr(C)]
    struct Linger {
        l_onoff: i32,
        l_linger: i32,
    }
    #[cfg(target_os = "macos")]
    const SOL_SOCKET: i32 = 0xffff;
    #[cfg(target_os = "macos")]
    const SO_LINGER: i32 = 0x80;
    #[cfg(target_os = "linux")]
    const SOL_SOCKET: i32 = 1;
    #[cfg(target_os = "linux")]
    const SO_LINGER: i32 = 13;
    unsafe extern "C" {
        fn setsockopt(
            socket: i32,
            level: i32,
            name: i32,
            value: *const Linger,
            len: u32,
        ) -> i32;
    }
    use std::os::fd::AsRawFd;
    let linger = Linger { l_onoff: 1, l_linger: 0 };
    unsafe {
        let _ = setsockopt(
            stream.as_raw_fd(),
            SOL_SOCKET,
            SO_LINGER,
            &linger,
            std::mem::size_of::<Linger>() as u32,
        );
    }
    drop(stream);
}

#[test]
fn errors_do_not_embed_bearer_or_body() {
    let err = Request::post("http://127.0.0.1:1/")
        .bearer("sk-live-secret-value")
        .unwrap()
        .json_body(br#"{"prompt":"do not leak"}"#.to_vec())
        .unwrap()
        .header("X-Custom", "nope")
        .unwrap();
    let fail = match post_json(err) {
        Err(e) => e,
        Ok(_) => panic!("expected connect failure"),
    };
    let text = fail.to_string();
    assert!(!text.contains("sk-live-secret-value"), "{text}");
    assert!(!text.contains("do not leak"), "{text}");
    assert!(!text.contains("Authorization"), "{text}");
    assert!(!text.contains("Bearer"), "{text}");
}

#[test]
fn url_rejects_userinfo_fragment_space_and_controls() {
    expect_err(
        request_no_redirect(Request::get("http://user:pass@127.0.0.1/")),
        Error::InvalidUrl,
    );
    expect_err(
        request_no_redirect(Request::get("http://127.0.0.1/path#frag")),
        Error::InvalidUrl,
    );
    expect_err(
        request_no_redirect(Request::get("http://127.0.0.1/pa th")),
        Error::InvalidUrl,
    );
    expect_err(
        request_no_redirect(Request::get("http://127.0.0.1/pa\nth")),
        Error::InvalidUrl,
    );
}

#[test]
fn ipv6_loopback_host_is_bracketed() {
    let listener = TcpListener::bind("[::1]:0").expect("bind v6");
    let port = listener.local_addr().expect("addr").port();
    let handle = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    );
    let resp = request_no_redirect(Request::get(format!("http://[::1]:{port}/"))).expect("v6");
    assert_eq!(resp.body, b"ok");
    let raw = String::from_utf8(handle.join().unwrap()).unwrap();
    assert!(raw.contains("Host: [::1]"), "{raw}");
}

#[test]
fn informational_1xx_is_consumed_not_final() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    );
    let resp = request_no_redirect(Request::get(url(port, "/"))).expect("1xx then 200");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"ok");
}

#[test]
fn space_before_header_colon_is_rejected() {
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nX-Foo : bar\r\nContent-Length: 0\r\n\r\n".to_vec(),
    );
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);
}

#[test]
fn header_then_stall_cancel_returns_promptly() {
    let (listener, port) = listen();
    let accepted = Arc::new(Mutex::new(false));
    let flag = accepted.clone();
    let hang = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            *flag.lock().unwrap() = true;
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
            let _ = stream.flush();
            thread::sleep(Duration::from_secs(4));
        }
    });
    let token = CancelToken::new();
    let token2 = token.clone();
    let u = url(port, "/stall");
    let worker = thread::spawn(move || {
        request_no_redirect(Request::get(u).cancel_token(token2).limits(Limits {
            total_timeout: Duration::from_secs(5),
            ..Limits::default()
        }))
    });
    let start = std::time::Instant::now();
    while !*accepted.lock().unwrap() && start.elapsed() < Duration::from_secs(2) {
        thread::sleep(Duration::from_millis(5));
    }
    token.cancel();
    expect_err(worker.join().unwrap(), Error::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(3));
    let _ = hang.join();
}

#[test]
fn delayed_trailing_bytes_after_chunked_are_rejected() {
    let (listener, port) = listen();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_request(&mut stream);
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n0\r\n\r\n",
        );
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(30));
        let _ = stream.write_all(b"GARBAGE");
        let _ = stream.flush();
    });
    expect_err(request_no_redirect(Request::get(url(port, "/"))), Error::InvalidResponse);
    let _ = handle.join();
}

#[test]
fn trailer_count_cap_is_enforced() {
    let (listener, port) = listen();
    let mut body = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n".to_vec();
    for i in 0..8 {
        body.extend_from_slice(format!("X-T{i}: v\r\n").as_bytes());
    }
    body.extend_from_slice(b"\r\n");
    let _ = serve_one(listener, body);
    expect_err(
        request_no_redirect(Request::get(url(port, "/")).limits(tight_limits())),
        Error::ResponseTooLarge,
    );
}

#[test]
fn timed_out_connect_does_not_wedge_later_loopback() {
    let mut limits = Limits::default();
    limits.total_timeout = Duration::from_millis(150);
    let first = request_no_redirect(Request::get("https://172.16.0.4:1/").limits(limits));
    assert!(
        matches!(first, Err(Error::Timeout) | Err(Error::Connect) | Err(Error::Cancelled)),
        "{first:?}"
    );
    let (listener, port) = listen();
    let _ = serve_one(
        listener,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    );
    let resp = request_no_redirect(Request::get(url(port, "/"))).expect("loopback after timeout");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"ok");
}
