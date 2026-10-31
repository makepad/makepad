//! Loopback tests (plain http to 127.0.0.1); no external hosts.
use crate::*;
use std::io::{Read, Write};
use std::net::TcpListener;

fn serve(reply: &'static [u8]) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let t = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut got = Vec::new();
        let mut b = [0u8; 4096];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut b).unwrap();
            got.extend_from_slice(&b[..n]);
        }
        s.write_all(reply).unwrap();
        got
    });
    (port, t)
}

#[test]
fn content_length_body() {
    let (port, t) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nETag: x\r\n\r\nhello");
    let url = format!("http://127.0.0.1:{port}/a?b=c");
    let (r, body) = fetch(&Request { headers: &[("Git-Protocol", "version=2")], ..Request::get(&url) }).unwrap();
    assert_eq!((r.status, body.as_slice(), r.header("etag")), (200, &b"hello"[..], Some("x")));
    let sent = String::from_utf8(t.join().unwrap()).unwrap();
    assert!(sent.starts_with("GET /a?b=c HTTP/1.1\r\n") && sent.contains(&format!("Host: 127.0.0.1:{port}\r\n")), "{sent}");
    assert!(sent.contains("Git-Protocol: version=2\r\n") && sent.contains("User-Agent: makepad-run/0.1\r\n"));
}

#[test]
fn chunked_body_and_progress() {
    let (port, _t) = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=y\r\nde\r\n0\r\n\r\n");
    let url = format!("http://127.0.0.1:{port}/");
    let mut body = Vec::new();
    let mut seen = 0;
    send(&Request::get(&url), &mut body, &mut |n, _| seen = n).unwrap();
    assert_eq!((body.as_slice(), seen), (&b"abcde"[..], 5));
}

#[test]
fn close_delimited_body_redirect_and_cap() {
    let (port, _t) = serve(b"HTTP/1.1 302 Found\r\nLocation: /x\r\n\r\nmoved");
    let url = format!("http://127.0.0.1:{port}/");
    let (r, body) = fetch(&Request::get(&url)).unwrap();
    assert_eq!((r.status, body.as_slice()), (302, &b"moved"[..]));
    let (port, _t) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789");
    let url = format!("http://127.0.0.1:{port}/");
    assert!(fetch(&Request { max_body: 4, ..Request::get(&url) }).is_err());
}

#[test]
fn refuses_plain_http_off_loopback() {
    assert!(fetch(&Request::get("http://example.com/")).is_err());
    assert!(fetch(&Request::get("https://user@example.com/")).is_err());
}

/// The system TLS stack against a real host (run by hand: `cargo test -- --ignored`).
#[test]
#[ignore]
fn https_github_info_refs() {
    let mut seen = 0;
    let mut body = Vec::new();
    let r = send(&Request::get("https://github.com/git/git.git/info/refs?service=git-upload-pack"), &mut body, &mut |n, _| seen = n).unwrap();
    assert_eq!(r.status, 200);
    assert!(body.starts_with(b"001e# service=git-upload-pack"), "{:?}", &body[..40.min(body.len())]);
    assert_eq!(seen, body.len() as u64);
    // a redirect comes back as it is
    let (r, _) = fetch(&Request::get("https://github.com/git/git.git")).unwrap();
    assert!((300..400).contains(&r.status), "{}", r.status);
    assert!(r.header("location").is_some());
    assert!(fetch(&Request::get("http://127.0.0.1:1/")).is_err());
}
