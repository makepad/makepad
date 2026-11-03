//! Compute programs on a node: a loopback node, its kernel stream and the remote session. What a
//! node sends back must be what the same program gives in-process, bit for bit.

use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::kernel_client::{KernelNode, KernelStream};
use makepad_ai_hub::program_client::{RemoteConfig, RemoteSession};
use makepad_ai_hub::registry::Registry;
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_compute_program::codec::Codec;
use makepad_compute_program::{programs, Class, Limits, OutputData, Program, ProgramError, Session, SessionConfig, Updates, Value};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// One node for the test binary (the service holds a process lock); one test at a time.
fn node() -> (MutexGuard<'static, ()>, Arc<KernelStream>) {
    static TEST: Mutex<()> = Mutex::new(());
    let guard = TEST.lock().unwrap_or_else(|e| e.into_inner());
    static URL: OnceLock<String> = OnceLock::new();
    let url = URL.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("makepad-ai-hub-program-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let handle = start_service(ServiceConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            cache_dir: dir,
            registry: Registry::embedded().unwrap(),
            downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
            peer: makepad_ai_hub::peer_serve::PeerOptions { serve: Some(false), sources: Some(Vec::new()), ..Default::default() },
            fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
        })
        .unwrap();
        let url = format!("http://{}", handle.addr);
        std::mem::forget(handle);
        url
    });
    let stream = KernelStream::open(&KernelNode::new(url)).unwrap();
    (guard, Arc::new(stream))
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

fn nums(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|x| n(*x)).collect())
}

/// A fractal tile message: corner and texel step `at`, `res` texels a side.
fn tile(slot: u32, at: [f64; 3], res: f64) -> Value {
    Value::object(&[
        ("slot", n(slot as f64)),
        ("x", n(at[0])),
        ("y", n(at[1])),
        ("step", n(at[2])),
        ("res", n(res)),
        ("kind", n(0.0)),
        ("power", n(2.0)),
        ("max_iter", n(200.0)),
        ("de", n(1.0)),
        ("trap", n(0.0)),
        ("julia", nums(&[0.0, 0.0])),
    ])
}

/// The next `name` output past `version`.
fn next_output(mut take: impl FnMut() -> Updates, name: &str, version: u64) -> makepad_compute_program::Output {
    loop {
        let u = take();
        if let Some(Err(e)) = &u.end {
            panic!("the program ended: {e}");
        }
        if let Some(o) = u.outputs.into_iter().find(|o| o.name == name && o.version > version) {
            return o;
        }
    }
}

fn words(o: &makepad_compute_program::Output) -> Vec<u32> {
    match &o.data {
        OutputData::Words(w) => w.to_vec(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_remote_fractal_tile_is_the_in_process_tile() {
    let (_guard, stream) = node();
    let program = Program::new("fractal.splash", programs::FRACTAL);
    let views = [[-2.0, -1.25, 2.5 / 32.0], [-0.75, 0.05, 0.1 / 32.0]];
    for codec in [Codec::DeltaLz4, Codec::DeltaDeflate, Codec::Raw] {
        let remote = RemoteSession::open(&stream, &program, RemoteConfig { codec, class: Class::Interactive, ..RemoteConfig::default() }).unwrap();
        let device = remote.device();
        for (key, want) in [("kind", "cpu"), ("backend", "native"), ("class", "interactive")] {
            assert_eq!(device.get(key), Some(&Value::String(want.into())), "{device:?}");
        }
        // an interactive session takes the box where the node runs in a scope it can weigh
        assert!(matches!(device.get("weight"), Some(Value::Nil) | Some(Value::Number(10000.0))), "{device:?}");
        let local = Session::open(&program, SessionConfig::default());
        for (k, at) in views.iter().enumerate() {
            let name = ["tile0", "tile1"][k];
            let rev = remote.send("tile", tile(k as u32, *at, 32.0));
            local.send("tile", tile(k as u32, *at, 32.0));
            let theirs = next_output(|| remote.wait(), name, 0);
            let mine = next_output(|| local.wait(), name, 0);
            assert_eq!(theirs.rev, rev);
            assert_eq!(words(&theirs), words(&mine), "{codec:?}: tile {k}");
        }
        remote.send("stop", Value::Bool(true));
        assert_eq!(remote.wait_end(), Ok(n(2.0)));
        // The end stays: a later wait returns at once.
        assert!(remote.wait().is_empty());
    }
}

#[test]
fn a_remote_program_ends_with_its_typed_error() {
    let (_guard, stream) = node();
    let open = |source: &str, limits: Limits| RemoteSession::open(&stream, &Program::new("t.splash", source), RemoteConfig { limits, ..RemoteConfig::default() }).unwrap();
    let small = Limits { fuel: 1_000_000, ..Limits::default() };
    assert_eq!(open("{run: fn(s, p) { var k = 0 loop { k = k + 1 } }}", small).wait_end(), Err(ProgramError::OverBudget("instructions")));
    match open("{run: fn(s, p) { return s.nothing() }}", Limits::default()).wait_end() {
        Err(ProgramError::Program(m)) => assert!(m.contains("nothing"), "{m}"),
        other => panic!("{other:?}"),
    }
    let waiting = open("{run: fn(s, p) { s.wait_inbox() return 1 }}", Limits::default());
    waiting.cancel();
    assert_eq!(waiting.wait_end(), Err(ProgramError::Cancelled));
    // The node's ceiling, not the client's ask: a refusal value the program sees.
    let big = open("use mod.pod.*\n{run: fn(s, p) { let b = s.buffer(f32, [32768, 16384, 3]) return b.refused }}", Limits { buffer_bytes: u64::MAX, ..Limits::default() });
    assert_eq!(big.wait_end(), Ok(Value::Bool(true)));
}

#[test]
fn a_slow_app_gets_the_newest_versions_not_a_backlog() {
    let (_guard, stream) = node();
    // 200 versions as fast as the program can publish, then it waits to be told to stop.
    let source = r#"
use mod.pod.*
{run: fn(s, p) {
    let b = s.buffer(f32, 1024)
    for k in 0..200 {
        s.fill(b, k)
        s.publish("level", b)
    }
    s.value("done", true)
    s.wait_inbox()
    return 1
}}
"#;
    let remote = RemoteSession::open(&stream, &Program::new("t.splash", source), RemoteConfig::default()).unwrap();
    let mut seen = Vec::new();
    let mut stopped = false;
    loop {
        let u = remote.wait();
        seen.extend(u.outputs.iter().map(|o| o.version));
        if !stopped && u.value("done").is_some() {
            remote.send("stop", Value::Bool(true));
            stopped = true;
        }
        if let Some(end) = u.end {
            assert_eq!(end, Ok(n(1.0)));
            break;
        }
    }
    // The newest version always arrives; the link carried far fewer than 200.
    assert_eq!(seen.iter().max(), Some(&200), "{seen:?}");
    assert!(seen.len() < 200, "every version came: {}", seen.len());
}
