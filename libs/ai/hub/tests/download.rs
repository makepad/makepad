//! Downloader tests against a local fixture HTTP server (plain TCP): fresh
//! download, Range resume, redirect following, servers that ignore Range,
//! and sha256 verification.

use makepad_ai_hub::backend::CancelToken;
use makepad_ai_hub::download::{part_path, Downloader};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::registry::FileSpec;
use makepad_ai_hub::sha256::sha256_hex;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Fixture server: serves `data` at /repo/resolve/main/file.bin with optional
// Range support and an optional redirect hop; records request headers.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum RangeMode {
    /// Honors Range with 206 + Content-Range.
    Honor,
    /// Ignores Range and always answers 200 with the full body.
    Ignore,
}

struct Fixture {
    addr: SocketAddr,
    /// One entry per request: the raw request head.
    requests: Arc<Mutex<Vec<String>>>,
}

fn spawn_fixture(data: Vec<u8>, range_mode: RangeMode, redirect_first: bool) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_thread = requests.clone();
    std::thread::spawn(move || {
        let mut redirect_pending = redirect_first;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let head = read_head(&mut stream);
            requests_thread.lock().unwrap().push(head.clone());
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            if redirect_pending && !path.starts_with("/cdn") {
                redirect_pending = false;
                let response =
                    "HTTP/1.1 302 Found\r\nLocation: /cdn/file.bin\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
                continue;
            }
            let range_from = parse_range(&head);
            match (range_mode, range_from) {
                (RangeMode::Honor, Some(from)) if from >= data.len() as u64 => {
                    let response = format!(
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        data.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                (RangeMode::Honor, Some(from)) => {
                    let body = &data[from as usize..];
                    let response = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        from,
                        data.len() - 1,
                        data.len(),
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.write_all(body);
                }
                _ => {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.write_all(&data);
                }
            }
        }
    });
    Fixture { addr, requests }
}

fn read_head(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => buf.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn parse_range(head: &str) -> Option<u64> {
    for line in head.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("range: bytes=") {
            return rest.trim_end_matches('-').trim().parse().ok();
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "makepad-asset-ai-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_spec(sha256: Option<String>) -> FileSpec {
    FileSpec {
        role: None,
        repo: "test-org/test-repo".to_string(),
        path: "file.bin".to_string(),
        revision: None,
        cache_as: "unet/file.bin".to_string(),
        size: None,
        sha256,
        local: false,
        optional: false,
        converts_to: None,
        conversion: None,
    }
}

fn downloader_for(fixture: &Fixture) -> Downloader {
    Downloader::new(&format!("http://{}", fixture.addr), None).unwrap()
}

fn test_data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fresh_download() {
    let data = test_data(10_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("fresh");
    let spec = file_spec(None);

    let mut progress_reports = Vec::new();
    let dest = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |p| {
            progress_reports.push((p.done, p.total))
        }, &CancelToken::new())
        .unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert!(!part_path(&dest).exists(), ".part must be renamed away");
    // First request had no Range header.
    let requests = fixture.requests.lock().unwrap();
    assert!(!requests[0].to_ascii_lowercase().contains("range:"));
    // Progress reached the total.
    let last = progress_reports.last().unwrap();
    assert_eq!(last.0, data.len() as u64);
    assert_eq!(last.1, Some(data.len() as u64));
    // The downloader identifies itself.
    assert!(requests[0].contains("makepad-asset-ai"));
}

#[test]
fn resume_uses_range_and_appends() {
    let data = test_data(50_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("resume");
    let spec = file_spec(None);

    // Pre-seed a partial download: first 12_345 bytes.
    let dest = spec.dest_path(&cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(part_path(&dest), &data[..12_345]).unwrap();

    let mut first_done = None;
    let out = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |p| {
            if first_done.is_none() {
                first_done = Some(p.done);
            }
        }, &CancelToken::new())
        .unwrap();

    assert_eq!(std::fs::read(&out).unwrap(), data);
    // The request asked to resume exactly where the .part ended.
    let requests = fixture.requests.lock().unwrap();
    assert!(
        requests[0].to_ascii_lowercase().contains("range: bytes=12345-"),
        "expected Range header in: {}",
        requests[0]
    );
    // Progress started from the resumed offset, not zero.
    assert_eq!(first_done, Some(12_345));
}

#[test]
fn resume_restarts_when_server_ignores_range() {
    let data = test_data(20_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Ignore, false);
    let cache = test_dir("norange");
    let spec = file_spec(None);

    // Pre-seed garbage that would corrupt the file if it were kept.
    let dest = spec.dest_path(&cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(part_path(&dest), vec![0xffu8; 5000]).unwrap();

    let out = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    // Server answered 200: the stale prefix must have been discarded.
    assert_eq!(std::fs::read(&out).unwrap(), data);
}

#[test]
fn follows_redirect() {
    let data = test_data(4_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, true);
    let cache = test_dir("redirect");
    let spec = file_spec(None);

    let out = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "redirect + follow");
    assert!(requests[1].starts_with("GET /cdn/file.bin"));
}

#[test]
fn sha256_verify_pass_and_fail() {
    let data = test_data(8_000);
    let good = sha256_hex(&data);

    // Passing case.
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("shapass");
    let out = downloader_for(&fixture)
        .ensure_file(&file_spec(Some(good)), &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);

    // Failing case: wrong hash -> error, partial data discarded, no dest.
    let fixture = spawn_fixture(data, RangeMode::Honor, false);
    let cache = test_dir("shafail");
    let bad_spec = file_spec(Some("00".repeat(32)));
    let err = downloader_for(&fixture)
        .ensure_file(&bad_spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap_err();
    match err {
        AssetAiError::Download(message) => assert!(message.contains("sha256 mismatch")),
        other => panic!("expected Download error, got {other:?}"),
    }
    let dest = bad_spec.dest_path(&cache);
    assert!(!dest.exists());
    assert!(!part_path(&dest).exists());
}

#[test]
fn existing_file_is_not_refetched() {
    let data = test_data(1_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("cached");
    let spec = file_spec(None);

    let dest = spec.dest_path(&cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, &data).unwrap();

    let out = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(out, dest);
    assert!(
        fixture.requests.lock().unwrap().is_empty(),
        "no network traffic for a cached file"
    );
}

#[test]
fn fully_downloaded_part_finishes_via_416() {
    let data = test_data(6_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("part416");
    let spec = file_spec(None);

    // The whole file is already in .part (e.g. the process died between the
    // last write and the rename).
    let dest = spec.dest_path(&cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(part_path(&dest), &data).unwrap();

    let mut spec = spec;
    spec.size = Some(data.len() as u64);
    let out = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert!(!part_path(&dest).exists());
}

#[test]
fn local_files_never_download() {
    // No fixture server involved: local files must not touch the network.
    let cache = test_dir("localfile");
    let mut spec = file_spec(None);
    spec.local = true;
    let downloader = Downloader::new("http://127.0.0.1:9", None).unwrap();

    // Missing -> helpful error naming the expected cache path.
    let err = downloader
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("locally-converted"), "{message}");
    assert!(message.contains("unet"), "{message}");

    // Present -> returned as-is.
    let dest = spec.dest_path(&cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, b"weights").unwrap();
    let out = downloader
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(out, dest);
}

#[test]
fn pinned_revision_is_used_in_resolve_url() {
    let downloader = Downloader::new("https://huggingface.co", None).unwrap();
    let mut spec = file_spec(Some("11".repeat(32)));
    spec.revision = Some("a".repeat(40));
    spec.size = Some(1);
    assert_eq!(
        downloader.file_url(&spec),
        format!(
            "https://huggingface.co/test-org/test-repo/resolve/{}/file.bin",
            "a".repeat(40)
        )
    );
}

#[test]
fn corrupt_existing_file_is_verified_and_replaced() {
    let data = test_data(16_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("corrupt-existing");
    let mut spec = file_spec(Some(sha256_hex(&data)));
    spec.size = Some(data.len() as u64);
    let dest = spec.dest_path(&cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, vec![0xff; data.len()]).unwrap();

    let out = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(out).unwrap(), data);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);

    // The identity receipt makes the second verification network- and
    // multi-megabyte-hash-free while still binding current len+mtime.
    downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[test]
fn wrong_size_partial_416_is_rejected() {
    let data = test_data(6_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("wrong-416");
    let mut spec = file_spec(None);
    // Server truth is 6000, manifest claims 7000. A complete server-sized
    // partial provokes 416 but must not be promoted to final.
    spec.size = Some(7_000);
    let dest = spec.dest_path(&cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(part_path(&dest), &data).unwrap();
    let err = downloader_for(&fixture)
        .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
        .unwrap_err();
    assert!(err.to_string().contains("expected exact manifest size"));
    assert!(!dest.exists());
    assert!(part_path(&dest).exists(), "resume bytes must be retained");
}

#[test]
fn cancelled_download_keeps_resumable_part() {
    let data = test_data(200_000);
    let fixture = spawn_fixture(data, RangeMode::Honor, false);
    let cache = test_dir("cancel-resume");
    let spec = file_spec(None);
    let cancel = CancelToken::new();
    let cancel_progress = cancel.clone();
    let err = downloader_for(&fixture)
        .ensure_file(
            &spec,
            &cache,
            &mut |progress| {
                if progress.done >= 65_536 {
                    cancel_progress.cancel();
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert_eq!(err, AssetAiError::Cancelled);
    let dest = spec.dest_path(&cache);
    assert!(!dest.exists());
    let partial = part_path(&dest);
    assert!(partial.exists());
    let partial_len = std::fs::metadata(partial).unwrap().len();
    assert!(partial_len >= 65_536 && partial_len < 200_000, "{partial_len}");
}

#[test]
fn concurrent_downloaders_share_one_artifact_transaction() {
    let data = test_data(512_000);
    let fixture = spawn_fixture(data.clone(), RangeMode::Honor, false);
    let cache = test_dir("concurrent-lock");
    let mut spec = file_spec(Some(sha256_hex(&data)));
    spec.size = Some(data.len() as u64);
    let downloader = downloader_for(&fixture);
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let spec = spec.clone();
        let cache = cache.clone();
        let downloader = downloader.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            downloader
                .ensure_file(&spec, &cache, &mut |_| {}, &CancelToken::new())
                .unwrap()
        }));
    }
    barrier.wait();
    for thread in threads {
        assert_eq!(std::fs::read(thread.join().unwrap()).unwrap(), data);
    }
    assert_eq!(
        fixture.requests.lock().unwrap().len(),
        1,
        "only the lock winner may hit the network"
    );
    let dest = spec.dest_path(&cache);
    assert!(!part_path(&dest).exists());
    let mut lock = dest.as_os_str().to_os_string();
    lock.push(".lock");
    assert!(!PathBuf::from(lock).exists());
}
