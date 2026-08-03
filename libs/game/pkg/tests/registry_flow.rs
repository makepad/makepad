//! Download-verify-install and LAN fetch, end to end against a real socket.
//!
//! The server is a std TcpListener in this process: no Cx, no external service,
//! so the whole path a downloaded game takes is exercised headless.

use makepad_game_pkg::{
    library::Library, pack::read_package, registry::fetch_lan_package, sha256_hex, HttpError,
    IndexEntry, Registry,
};
use makepad_zip_file::{ZipMethod, ZipWriter};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

fn sample_package(name: &str) -> Vec<u8> {
    let mut w = ZipWriter::new();
    w.add(
        "manifest.toml",
        format!("name = \"{name}\"\ndescription = \"a test game\"\nplayers_max = 4\n").as_bytes(),
        ZipMethod::Deflate,
    )
    .unwrap();
    w.add(
        "game.splash",
        b"game.sky({})\ngame.box({pos: vec3(0,0,0)})\n",
        ZipMethod::Deflate,
    )
    .unwrap();
    w.add("assets/blob.bin", &vec![5u8; 2048], ZipMethod::Deflate)
        .unwrap();
    w.finish().unwrap()
}

/// Serves a fixed route table, one request per connection, then exits after
/// `requests` have been served so the thread never outlives the test.
fn serve(routes: Vec<(String, Vec<u8>)>, requests: usize) -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        for _ in 0..requests {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));

            // Read the head, then any declared body.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let head_end = loop {
                let Ok(n) = stream.read(&mut chunk) else { break 0 };
                if n == 0 {
                    break 0;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break p + 4;
                }
                if buf.len() > 64 * 1024 {
                    break 0;
                }
            };
            if head_end == 0 {
                continue;
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let path = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            let content_length = head
                .lines()
                .find_map(|l| {
                    let lower = l.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut body = buf[head_end..].to_vec();
            while body.len() < content_length {
                let Ok(n) = stream.read(&mut chunk) else { break };
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }
            if !body.is_empty() {
                let _ = tx.send(body);
            }

            let resp = routes.iter().find(|(p, _)| *p == path);
            match resp {
                Some((_, payload)) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(payload);
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
            let _ = stream.flush();
            drop(stream);
        }
    });

    (format!("127.0.0.1:{}", addr.port()), rx)
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "makepad-pkg-registry-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn download_verify_install_happy_path() {
    let pkg = sample_package("Speedway");
    let digest = sha256_hex(&pkg);
    let index = format!(
        r#"[{{"id":"speedway","name":"Speedway","description":"race","author":"kid","size":{},"sha256":"{}","url":"/games/speedway.arcade"}}]"#,
        pkg.len(),
        digest
    );
    let (addr, _rx) = serve(
        vec![
            ("/index.json".into(), index.into_bytes()),
            ("/games/speedway.arcade".into(), pkg.clone()),
        ],
        2,
    );

    let reg = Registry::new(&addr);
    let entries = reg.index().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "Speedway");
    assert_eq!(entries[0].size as usize, pkg.len());

    let downloaded = reg.download(&entries[0]).unwrap();
    assert_eq!(downloaded, pkg);

    let root = tmp_dir("install");
    let lib = Library::new(&root);
    let entry = lib.install("speedway", &downloaded).unwrap();
    assert_eq!(entry.manifest.name, "Speedway");
    assert!(entry.dir.join("game.splash").is_file());
    assert!(entry.dir.join("assets/blob.bin").is_file());

    let listed = lib.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].slug, "speedway");

    // And the library can pack it back up for republishing.
    let repacked = lib.pack("speedway").unwrap();
    let reread = read_package(&repacked).unwrap();
    assert_eq!(reread.manifest.name, "Speedway");
    assert_eq!(reread.assets.len(), 1);

    lib.uninstall("speedway").unwrap();
    assert!(lib.list().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_tampered_download_is_refused_before_it_can_be_installed() {
    let pkg = sample_package("Speedway");
    let honest_digest = sha256_hex(&pkg);

    // The server hands back a DIFFERENT package than the index promised.
    let evil = sample_package("NotSpeedway");
    assert_ne!(evil, pkg);

    let index = format!(
        r#"[{{"id":"speedway","name":"Speedway","size":{},"sha256":"{}","url":"/games/speedway.arcade"}}]"#,
        pkg.len(),
        honest_digest
    );
    let (addr, _rx) = serve(
        vec![
            ("/index.json".into(), index.into_bytes()),
            ("/games/speedway.arcade".into(), evil),
        ],
        2,
    );

    let reg = Registry::new(&addr);
    let entries = reg.index().unwrap();
    let err = reg.download(&entries[0]).unwrap_err();
    match err {
        HttpError::DigestMismatch { expected, got } => {
            assert_eq!(expected, honest_digest);
            assert_ne!(got, honest_digest);
        }
        other => panic!("expected a digest mismatch, got {other}"),
    }
}

#[test]
fn a_missing_game_is_a_status_error_not_a_hang() {
    let (addr, _rx) = serve(vec![], 1);
    let reg = Registry::new(&addr);
    let entry = IndexEntry {
        id: "nope".into(),
        sha256: "00".into(),
        ..Default::default()
    };
    match reg.download(&entry) {
        Err(HttpError::Status(404)) => {}
        other => panic!("expected 404, got {other:?}"),
    }
}

#[test]
fn publish_sends_the_package_body() {
    let (addr, rx) = serve(vec![("/publish".into(), b"speedway".to_vec())], 1);
    let pkg = sample_package("Speedway");
    let reg = Registry::new(&addr);
    let id = reg.publish(&pkg).unwrap();
    assert_eq!(id, "speedway");
    let received = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(received, pkg, "the server must receive the package verbatim");
}

#[test]
fn lan_fetch_then_install_lets_a_joiner_get_the_running_game() {
    // The host serves the game it is running; a joiner installs it before
    // entering the room.
    let pkg = sample_package("HostGame");
    let digest = sha256_hex(&pkg);
    let (addr, _rx) = serve(vec![("/game.arcade".into(), pkg.clone())], 2);

    let fetched = fetch_lan_package(&addr, Some(&digest)).unwrap();
    assert_eq!(fetched, pkg);

    let root = tmp_dir("lan");
    let lib = Library::new(&root);
    let entry = lib.install("hostgame", &fetched).unwrap();
    assert_eq!(entry.manifest.name, "HostGame");

    // A joiner given the wrong digest refuses rather than installing.
    let err = fetch_lan_package(&addr, Some(&sha256_hex(b"something else"))).unwrap_err();
    assert!(matches!(err, HttpError::DigestMismatch { .. }));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn library_search_finds_a_game_by_description() {
    let root = tmp_dir("search");
    let lib = Library::new(&root);
    lib.install("speedway", &sample_package("Speedway")).unwrap();

    let mut w = ZipWriter::new();
    w.add(
        "manifest.toml",
        b"name = \"Dogfight\"\ndescription = \"planes shooting planes\"\n",
        ZipMethod::Deflate,
    )
    .unwrap();
    w.add("game.splash", b"game.plane({})\n", ZipMethod::Deflate)
        .unwrap();
    lib.install("dogfight", &w.finish().unwrap()).unwrap();

    let hits = lib.search("play the one with the planes");
    assert_eq!(hits.first().map(|(e, _)| e.slug.as_str()), Some("dogfight"));

    // A phrase matching nothing returns nothing rather than a wrong guess.
    assert!(lib.search("underwater basket weaving").is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_server_that_lies_about_length_cannot_exhaust_us() {
    // Content-Length far beyond the cap must be refused on the header, before
    // the body is read.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut chunk = [0u8; 4096];
            let _ = stream.read(&mut chunk);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 999999999999\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
        }
    });
    // No warm-up connect: bind() is already listening, and an extra connection
    // would consume the single accept above.
    let reg = Registry::new(format!("127.0.0.1:{}", addr.port()));
    let err = reg.index().unwrap_err();
    assert!(
        matches!(err, HttpError::TooLarge) || matches!(err, HttpError::Io(_)),
        "expected a size refusal, got {err:?}"
    );
}
