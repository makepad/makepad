#![cfg(all(feature = "web", not(target_arch = "wasm32")))]

mod common;

use common::{FixtureOptions, FixtureServer, FixtureStore};
use makepad_asset_client::{
    BaseUrl, HttpLimits, OwnedRequest, PlatformHttpTransport, TcpHttpTransport, Transport,
    TransportCompletion, TransportError, TransportId, TransportMethod,
};
use makepad_asset_data::BlobId;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Backend {
    Tcp,
    Platform,
}

const BACKENDS: [Backend; 2] = [Backend::Tcp, Backend::Platform];

fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Tcp => "tcp",
        Backend::Platform => "platform",
    }
}

fn transport_for(backend: Backend, addr: SocketAddr, max_body: u64) -> Box<dyn Transport> {
    match backend {
        Backend::Tcp => Box::new(TcpHttpTransport::with_limits(
            addr,
            HttpLimits::default_v1(),
            max_body,
        )),
        Backend::Platform => Box::new(PlatformHttpTransport::with_max_response_body(max_body)),
    }
}

fn request_for(
    backend: Backend,
    addr: SocketAddr,
    method: TransportMethod,
    body: &[u8],
) -> OwnedRequest {
    let target = match backend {
        Backend::Tcp => "/contract".to_string(),
        Backend::Platform => format!("http://{addr}/contract"),
    };
    OwnedRequest::new(method, target).body(body.to_vec())
}

fn one_shot_server(
    response: Vec<u8>,
    response_delay: Duration,
) -> (SocketAddr, mpsc::Receiver<(String, Vec<u8>)>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let join = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return,
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        let mut scratch = [0u8; 4096];
        let head_end = loop {
            let Ok(read) = stream.read(&mut scratch) else { return };
            if read == 0 {
                return;
            }
            request.extend_from_slice(&scratch[..read]);
            if let Some(at) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&request[..head_end]).to_string();
        let content_length = head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        }).unwrap_or(0);
        while request.len() - head_end < content_length {
            let Ok(read) = stream.read(&mut scratch) else { return };
            if read == 0 {
                return;
            }
            request.extend_from_slice(&scratch[..read]);
        }
        let _ = tx.send((head, request[head_end..head_end + content_length].to_vec()));
        if !response_delay.is_zero() {
            std::thread::sleep(response_delay);
        }
        let _ = stream.write_all(&response);
        let _ = stream.flush();
    });
    (addr, rx, join)
}

fn wait_for_dyn(transport: &mut dyn Transport, id: TransportId) -> TransportCompletion {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut completions = Vec::new();
        transport.poll(&mut completions);
        if let Some(completion) = completions.into_iter().find(|item| item.id == id) {
            return completion;
        }
        assert!(Instant::now() < deadline, "transport request did not complete");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(
    transport: &mut PlatformHttpTransport,
    id: TransportId,
) -> TransportCompletion {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut completions = Vec::new();
        transport.poll(&mut completions);
        if let Some(completion) = completions.into_iter().find(|item| item.id == id) {
            return completion;
        }
        assert!(Instant::now() < deadline, "platform request did not complete");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn platform_transport_get_range_not_found_and_cancel() {
    let mut store = FixtureStore::default();
    let revision = store.add_prop(7, "portable", None, "portable", b"0123456789".to_vec(), vec![]);
    let blob = store
        .assets
        .iter()
        .find(|asset| asset.revision == revision.revision)
        .unwrap()
        .manifest
        .files[0]
        .blob;
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let base = BaseUrl::parse(format!("http://127.0.0.1:{}", fixture.data.addr.port())).unwrap();
    let blob_target = makepad_asset_client::wire::path_blob(&blob);
    let blob_url = base.join(&blob_target).unwrap();
    let mut transport = PlatformHttpTransport::new();

    let get = transport.start(OwnedRequest::new(TransportMethod::Get, blob_url.clone()));
    let response = wait_for(&mut transport, get).result.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"0123456789");
    assert!(response.headers.iter().all(|(name, _)| *name == name.to_ascii_lowercase()));

    let range = transport.start(
        OwnedRequest::new(TransportMethod::Get, blob_url).header("Range", "bytes=4-"),
    );
    let response = wait_for(&mut transport, range).result.unwrap();
    assert_eq!(response.status, 206);
    assert_eq!(response.body, b"456789");
    assert_eq!(response.header("content-range"), Some("bytes 4-9/10"));

    let missing = transport.start(OwnedRequest::new(
        TransportMethod::Get,
        base.join(&makepad_asset_client::wire::path_blob(&BlobId::from_bytes([0; 32])))
            .unwrap(),
    ));
    assert_eq!(wait_for(&mut transport, missing).result.unwrap().status, 404);

    let cancelled = transport.start(OwnedRequest::new(
        TransportMethod::Get,
        base.join("/v1/missing").unwrap(),
    ));
    transport.cancel(cancelled);
    assert!(matches!(
        wait_for(&mut transport, cancelled).result,
        Err(TransportError::Cancelled)
    ));
}

#[test]
fn adapters_refuse_redirects_bodyless_statuses_and_bad_framing() {
    let cases = [
        b"HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nok".to_vec(),
    ];
    for backend in BACKENDS {
        for response in &cases {
            let (addr, _captured, server) =
                one_shot_server(response.clone(), Duration::ZERO);
            let mut transport = transport_for(backend, addr, 1024);
            let request = request_for(backend, addr, TransportMethod::Get, &[]);
            let id = transport.start(request);
            assert!(
                wait_for_dyn(&mut *transport, id).result.is_err(),
                "{} accepted a refused response",
                backend_name(backend)
            );
            server.join().unwrap();
        }
    }
}

#[test]
fn adapters_enforce_body_limit_and_support_head_post_put() {
    for backend in BACKENDS {
        let (addr, _captured, server) = one_shot_server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n0123456789abcdef".to_vec(),
            Duration::ZERO,
        );
        let mut transport = transport_for(backend, addr, 8);
        let request = request_for(backend, addr, TransportMethod::Get, &[]);
        let id = transport.start(request);
        assert!(
            matches!(
                wait_for_dyn(&mut *transport, id).result,
                Err(TransportError::OverBudget { what: "response body", .. })
            ),
            "{} did not report the body cap",
            backend_name(backend)
        );
        server.join().unwrap();

        for (method, body) in [
            (TransportMethod::Head, &b""[..]),
            (TransportMethod::Post, &b"post-body"[..]),
            (TransportMethod::Put, &b"put-body"[..]),
        ] {
            let response = if matches!(method, TransportMethod::Head) {
                b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n".to_vec()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
            };
            let (addr, captured, server) = one_shot_server(response, Duration::ZERO);
            let mut transport = transport_for(backend, addr, 1024);
            let request = request_for(backend, addr, method, body);
            let id = transport.start(request);
            let response = wait_for_dyn(&mut *transport, id).result.unwrap_or_else(|error| {
                panic!("{} {method:?} failed: {error}", backend_name(backend))
            });
            if matches!(method, TransportMethod::Head) {
                assert!(response.body.is_empty());
            } else {
                assert_eq!(response.body, b"ok");
            }
            let (head, captured_body) = captured.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(head.starts_with(method_name(method)));
            assert_eq!(captured_body, body);
            server.join().unwrap();
        }
    }
}

fn method_name(method: TransportMethod) -> &'static str {
    match method {
        TransportMethod::Get => "GET ",
        TransportMethod::Head => "HEAD ",
        TransportMethod::Post => "POST ",
        TransportMethod::Put => "PUT ",
        TransportMethod::Delete => "DELETE ",
    }
}

#[test]
fn cancellation_race_has_exactly_one_terminal_completion() {
    for backend in BACKENDS {
        let (addr, _captured, server) = one_shot_server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
            Duration::from_millis(100),
        );
        let mut transport = transport_for(backend, addr, 1024);
        let request = request_for(backend, addr, TransportMethod::Get, &[]);
        let id = transport.start(request);
        transport.cancel(id);
        assert!(matches!(
            wait_for_dyn(&mut *transport, id).result,
            Err(TransportError::Cancelled)
        ));
        server.join().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut extra = Vec::new();
        transport.poll(&mut extra);
        assert!(
            extra.iter().all(|completion| completion.id != id),
            "{} emitted a second terminal completion",
            backend_name(backend)
        );
    }
}
