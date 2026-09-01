use makepad_ai_hub::backend::{
    CancelToken, ContentBackend, GenerateParams, LiveConfig, LiveFrameIn, RgbImage,
};
use makepad_ai_hub::body_backend::{
    BodyBackend, BodyWorker, BODY_TIMEOUT_ENV, BODY_WORKER_ENV,
};
use makepad_ai_hub::protocol::GenerateRequestJson;
use makepad_ai_hub::testpattern::encode_png_rgb8;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FAKE_FLAG: &str = "MAKEPAD_SAM3DBODY_FAKE_WORKER";
const FAKE_MODE: &str = "MAKEPAD_SAM3DBODY_FAKE_MODE";
const FAKE_MARKER: &str = "MAKEPAD_SAM3DBODY_FAKE_MARKER";
const PACKET: &str = r#"{"n_people":1,"people":[],"opaque":{"keep":[1, 2]}}"#;

fn main() {
    if std::env::var(FAKE_FLAG).ok().as_deref() == Some("1") {
        fake_worker_main();
        return;
    }

    worker_round_trip_keeps_child_alive();
    worker_restarts_after_child_death();
    worker_stops_after_three_restarts();
    worker_timeout_is_bounded();
    backend_live_step_echoes_frame_and_pose_aux();
    backend_generate_returns_json_artifact();
    unset_worker_command_is_clear();
    println!("body_worker: 7 passed");
}

fn fake_worker_main() {
    let mode = std::env::var(FAKE_MODE).unwrap_or_else(|_| "normal".to_string());
    if mode == "die" {
        return;
    }
    if mode == "die_once" {
        let marker = PathBuf::from(std::env::var(FAKE_MARKER).expect("fake marker"));
        if !marker.exists() {
            std::fs::write(marker, b"died").expect("write fake marker");
            return;
        }
    }

    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{{\"ready\":true}}").expect("fake worker ready");
    stdout.flush().expect("fake worker ready flush");
    loop {
        let mut length = [0u8; 4];
        match stdin.read_exact(&mut length) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return,
            Err(error) => panic!("fake worker length read: {error}"),
        }
        let length = u32::from_le_bytes(length) as usize;
        let mut png = vec![0u8; length];
        stdin.read_exact(&mut png).expect("fake worker png read");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        if mode == "timeout" {
            std::thread::sleep(Duration::from_secs(2));
        }
        writeln!(stdout, "{PACKET}").expect("fake worker response");
        stdout.flush().expect("fake worker flush");
    }
}

struct FakeEnv {
    marker: Option<PathBuf>,
}

impl FakeEnv {
    fn set(mode: &str, timeout_s: &str, marker: Option<&Path>) -> Self {
        let executable = std::env::current_exe().expect("current test executable");
        let command = executable
            .to_str()
            .expect("test executable path is utf-8")
            .to_string();
        assert!(!command.contains(char::is_whitespace));
        std::env::set_var(BODY_WORKER_ENV, command);
        std::env::set_var(BODY_TIMEOUT_ENV, timeout_s);
        std::env::set_var(FAKE_FLAG, "1");
        std::env::set_var(FAKE_MODE, mode);
        if let Some(marker) = marker {
            std::env::set_var(FAKE_MARKER, marker);
        } else {
            std::env::remove_var(FAKE_MARKER);
        }
        Self {
            marker: marker.map(Path::to_path_buf),
        }
    }
}

impl Drop for FakeEnv {
    fn drop(&mut self) {
        std::env::remove_var(BODY_WORKER_ENV);
        std::env::remove_var(BODY_TIMEOUT_ENV);
        std::env::remove_var(FAKE_FLAG);
        std::env::remove_var(FAKE_MODE);
        std::env::remove_var(FAKE_MARKER);
        if let Some(marker) = self.marker.as_ref() {
            let _ = std::fs::remove_file(marker);
        }
    }
}

fn test_png() -> Vec<u8> {
    encode_png_rgb8(&[10, 20, 30, 40, 50, 60], 2, 1).unwrap()
}

fn worker_round_trip_keeps_child_alive() {
    let _env = FakeEnv::set("normal", "1", None);
    let mut worker = BodyWorker::new().unwrap();
    let cancel = CancelToken::new();
    assert_eq!(worker.process_png(&test_png(), &cancel).unwrap(), PACKET);
    assert_eq!(worker.process_png(&test_png(), &cancel).unwrap(), PACKET);
    assert_eq!(worker.restart_count(), 0);
}

fn worker_restarts_after_child_death() {
    let marker = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("body-worker-die-once-{}", std::process::id()));
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&marker);
    let _env = FakeEnv::set("die_once", "1", Some(&marker));
    let mut worker = BodyWorker::new().unwrap();
    let pose = worker
        .process_png(&test_png(), &CancelToken::new())
        .unwrap();
    assert_eq!(pose, PACKET);
    assert_eq!(worker.restart_count(), 1);
}

fn worker_timeout_is_bounded() {
    let _env = FakeEnv::set("timeout", "0.05", None);
    let mut worker = BodyWorker::new().unwrap();
    let start = Instant::now();
    let error = worker
        .process_png(&test_png(), &CancelToken::new())
        .unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!worker.is_started());
}

fn worker_stops_after_three_restarts() {
    let _env = FakeEnv::set("die", "1", None);
    let mut worker = BodyWorker::new().unwrap();
    let error = worker
        .process_png(&test_png(), &CancelToken::new())
        .unwrap_err();
    assert!(error.to_string().contains("after 3 restarts"), "{error}");
    assert_eq!(worker.restart_count(), 3);
}

fn backend_live_step_echoes_frame_and_pose_aux() {
    let _env = FakeEnv::set("normal", "1", None);
    let worker = BodyWorker::new().unwrap();
    let mut backend = BodyBackend::with_worker("sam3dbody-ref", worker);
    let image = RgbImage {
        width: 2,
        height: 1,
        data: vec![10, 20, 30, 40, 50, 60],
    };
    let config = LiveConfig::default();
    let out = backend
        .live_step(
            LiveFrameIn {
                init: Some(&image),
                anchor: None,
                frame_index: 9,
                config: &config,
            },
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(out.image, image);
    assert_eq!(out.aux_json.as_deref(), Some(PACKET));
    assert_eq!(out.text_encode_ms, 0.0);
}

fn backend_generate_returns_json_artifact() {
    let _env = FakeEnv::set("normal", "1", None);
    let worker = BodyWorker::new().unwrap();
    let mut backend = BodyBackend::with_worker("sam3dbody-ref", worker);
    let input_b64 = String::from_utf8(makepad_ai_hub::makepad_base64::base64_encode(
        &test_png(),
        &makepad_ai_hub::makepad_base64::BASE64_STANDARD,
    ))
    .unwrap();
    let params = GenerateParams::from_request(&GenerateRequestJson {
        model: "sam3dbody-ref".to_string(),
        input_b64: Some(input_b64),
        ..Default::default()
    })
    .unwrap();
    let mut progress = |_: &str, _: f64| {};
    let artifacts = backend
        .generate(&params, &mut progress, &CancelToken::new())
        .unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].content_type, "application/json");
    assert_eq!(artifacts[0].ext, "json");
    assert_eq!(artifacts[0].bytes, PACKET.as_bytes());
}

fn unset_worker_command_is_clear() {
    std::env::remove_var(BODY_WORKER_ENV);
    std::env::remove_var(BODY_TIMEOUT_ENV);
    let error = BodyWorker::new()
        .err()
        .expect("an unset worker command must be refused");
    assert!(error.to_string().contains(BODY_WORKER_ENV), "{error}");
}
