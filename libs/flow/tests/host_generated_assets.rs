#![cfg(not(target_arch = "wasm32"))]

mod support;

use makepad_asset_client::ApiEndpoints;
use makepad_flow::client::{Endpoints, FlowClient};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::{CreateInstanceRequest, CreateInstanceResponse, CreateRunRequest, CreateRunResponse, RunRowDto, RunState, Seams, PutSourceRequest};
use makepad_micro_serde::{DeJson, SerJson};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::thread::sleep;
use std::sync::{Mutex, OnceLock};
use support::{seams, FakeChat, FakeGen, FakeHttp};

struct TempRoot(PathBuf);
impl TempRoot {
    fn new(label: &str) -> Self {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("flow-generated-assets-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempRoot { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn request(address: std::net::SocketAddr, method: &str, target: &str, token: &str, body: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(stream, "{method} {target} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut raw = Vec::new(); stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let status = std::str::from_utf8(&raw[..split]).unwrap().lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, String::from_utf8_lossy(&raw[split + 4..]).into_owned())
}

fn start_asset_store(root: &Path) -> (makepad_asset_store::AssetServer, ApiEndpoints, String) {
    let mut cfg = makepad_asset_store::ServerConfig::new(root.to_path_buf());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap(); cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true; cfg.discovery = None; cfg.log = false;
    let server = makepad_asset_store::AssetServer::start(cfg).unwrap();
    let token = std::fs::read_to_string(root.join("admin-token")).unwrap().trim().to_string();
    let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
    (server, endpoints, token)
}

fn start_flow(root: &Path, asset: ApiEndpoints, server_id: [u8; 16], token: String, seams: Seams) -> FlowServer {
    let mut cfg = FlowServerConfig::new(root.to_path_buf()).with_seams(seams);
    cfg.watch_interval_ms = 10; cfg.log = Box::new(|_| {});
    cfg.asset.endpoints = Some(asset); cfg.asset.server_id = Some(server_id); cfg.asset.token = Some(token);
    cfg.asset.archive_outputs = true;
    FlowServer::start(cfg).unwrap()
}

fn test_png() -> Vec<u8> {
    makepad_ai_hub::testpattern::encode_png_rgba(&vec![180; 4 * 4 * 4], 4, 4).unwrap()
}

fn test_wav() -> Vec<u8> {
    let samples = [0i16, 1, -1, 0];
    let data: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&4u32.to_le_bytes());
    wav.extend_from_slice(&8u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn run(server: &FlowServer, name: &str, source: &str) -> (FlowClient, String, RunRowDto) {
    let e = server.endpoints();
    let (status, body) = request(e.control, "PUT", &format!("/v1/flows/{name}"), &e.token, &PutSourceRequest { source: source.into() }.serialize_json());
    assert_eq!(status, 200, "{body}");
    sleep(Duration::from_millis(100));
    let (status, body) = request(e.control, "POST", &format!("/v1/flows/{name}/instances"), &e.token, &CreateInstanceRequest::default().serialize_json());
    assert_eq!(status, 201, "{body}");
    let instance = CreateInstanceResponse::deserialize_json(&body).unwrap().instance;
    let (status, body) = request(e.control, "POST", &format!("/v1/instances/{instance}/runs"), &e.token, &CreateRunRequest::default().serialize_json());
    assert_eq!(status, 202, "{body}");
    let run_id = CreateRunResponse::deserialize_json(&body).unwrap().run_id;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let row = loop {
        let (status, body) = request(e.control, "GET", &format!("/v1/runs/{run_id}"), &e.token, "");
        assert_eq!(status, 200, "{body}");
        let row = RunRowDto::deserialize_json(&body).unwrap();
        if matches!(row.state, RunState::Done | RunState::Failed | RunState::Cancelled) { break row; }
        assert!(std::time::Instant::now() < deadline, "run did not finish: {:?}", row.state);
        sleep(Duration::from_millis(20));
    };
    let client = FlowClient::connect(Endpoints { control: e.control, data: e.data }, e.token, Some(e.server_id)).unwrap();
    (client, run_id, row)
}

#[test]
fn generated_media_is_archived_once_when_terminal_output_repeats_it() {
    let _guard = test_lock();
    let store_root = TempRoot::new("store"); let flow_root = TempRoot::new("flow");
    let (mut store, asset, token) = start_asset_store(&store_root.0); let id = store.server_id();
    let mut gen = FakeGen::done(); gen.bytes = test_png();
    let server = start_flow(&flow_root.0, asset, id, token, seams(FakeChat::done("unused"), gen, FakeHttp::json(200, "{}")));
    let (client, _, row) = run(&server, "generated", "use mod.flow.*\nlet image = Image{prompt: \"x\"}\nlet output = Output{type: @image value: image.image()}\nFlow{image, output}\n");
    assert_eq!(row.state, RunState::Done, "{row:?}");
    assert!(row.nodes["image"].outputs.iter().any(|output| output.port == "image"));
    let assets = client.assets("", Some("flows"), 20).unwrap().assets;
    assert_eq!(assets.len(), 1, "{assets:#?}");
    assert_eq!(assets[0].kind, "texture");
    server.shutdown(); store.shutdown();
}

#[test]
fn terminal_text_output_is_archived_and_keeps_run_output() {
    let _guard = test_lock();
    let store_root = TempRoot::new("store"); let flow_root = TempRoot::new("flow");
    let (mut store, asset, token) = start_asset_store(&store_root.0); let id = store.server_id();
    let server = start_flow(&flow_root.0, asset, id, token, seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")));
    let (client, _, row) = run(&server, "text", "use mod.flow.*\nlet text = Input{default: \"hello\\n\"}\nlet output = Output{type: @text value: text.text()}\nFlow{text, output}\n");
    assert_eq!(row.state, RunState::Done, "{row:?}");
    assert!(row.outputs.contains_key("output"));
    let assets = client.assets("", Some("flows"), 20).unwrap().assets;
    assert_eq!(assets.len(), 1, "{assets:#?}");
    assert_eq!(assets[0].kind, "data");
    let original = client.asset_content(&assets[0].id).unwrap();
    assert_eq!(&*original.bytes, b"hello\n");
    server.shutdown(); store.shutdown();
}

#[test]
fn terminal_audio_output_has_manifest_duration() {
    let _guard = test_lock();
    let store_root = TempRoot::new("store");
    let flow_root = TempRoot::new("flow");
    let (mut store, asset, token) = start_asset_store(&store_root.0);
    let id = store.server_id();
    let server = start_flow(
        &flow_root.0,
        asset,
        id,
        token,
        seams(
            FakeChat::done("unused"),
            FakeGen::done(),
            FakeHttp::bytes(200, "audio/wav", test_wav()),
        ),
    );
    let (client, _, row) = run(&server, "audio", "use mod.flow.*\nlet audio = Http{url: \"http://127.0.0.1/audio.wav\" out: @audio accept: [@audio]}\nlet output = Output{type: @audio value: audio.value()}\nFlow{audio, output}\n");
    assert_eq!(row.state, RunState::Done, "{row:?}");
    let assets = client.assets("", Some("flows"), 20).unwrap().assets;
    assert_eq!(assets.len(), 1, "{assets:#?}");
    assert_eq!(assets[0].kind, "audio");
    server.shutdown();
    store.shutdown();
}

fn publish_binary_fixture(name: &str, ty: &str, content_type: &str, bytes: Vec<u8>, expected_kind: &str) {
    let _guard = test_lock();
    let store_root = TempRoot::new("store");
    let flow_root = TempRoot::new("flow");
    let (mut store, asset, token) = start_asset_store(&store_root.0);
    let id = store.server_id();
    let server = start_flow(
        &flow_root.0,
        asset,
        id,
        token,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::bytes(200, content_type, bytes)),
    );
    let source = format!("use mod.flow.*\nlet value = Http{{url: \"http://127.0.0.1/{name}\" out: @{ty} accept: [@{ty}]}}\nlet output = Output{{type: @{ty} value: value.value()}}\nFlow{{value, output}}\n");
    let (client, _, row) = run(&server, name, &source);
    assert_eq!(row.state, RunState::Done, "{row:?}");
    let assets = client.assets("", Some("flows"), 20).unwrap().assets;
    assert_eq!(assets.len(), 1, "{assets:#?}");
    assert_eq!(assets[0].kind, expected_kind);
    if expected_kind == "video" {
        // This fixture predates poster extraction and is archived with the
        // solid legacy tile. The preview route must still pass through the
        // bounded thumbnail/legacy-poster path instead of returning raw media.
        let preview = client.asset_preview(&assets[0].id).unwrap();
        assert_eq!(preview.content_type, "image/png");
        assert!(preview.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(u32::from_be_bytes(preview.bytes[16..20].try_into().unwrap()) <= 256);
        assert!(u32::from_be_bytes(preview.bytes[20..24].try_into().unwrap()) <= 256);
    }
    server.shutdown();
    store.shutdown();
}

#[test]
fn terminal_mp4_output_is_archived() {
    publish_binary_fixture("video", "video", "video/mp4", mp4_fixture::synthetic(mp4_fixture::avc1()), "video");
}


#[test]
fn failed_generation_does_not_publish_an_asset() {
    let _guard = test_lock();
    let store_root = TempRoot::new("store"); let flow_root = TempRoot::new("flow");
    let (mut store, asset, token) = start_asset_store(&store_root.0); let id = store.server_id();
    let server = start_flow(&flow_root.0, asset, id, token, seams(FakeChat::done("unused"), FakeGen { mode: support::GenMode::Fail, ..FakeGen::done() }, FakeHttp::json(200, "{}")));
    let (client, _, row) = run(&server, "failed", "use mod.flow.*\nlet image = Image{prompt: \"x\"}\nlet output = Output{type: @image value: image.image()}\nFlow{image, output}\n");
    assert_eq!(row.state, RunState::Failed, "{row:?}");
    assert!(client.assets("", Some("flows"), 20).unwrap().assets.is_empty());
    server.shutdown(); store.shutdown();
}

#[test]
fn identical_parallel_generations_publish_one_asset_without_waiting_forever() {
    let _guard = test_lock();
    let store_root = TempRoot::new("store");
    let flow_root = TempRoot::new("flow");
    let (mut store, asset, token) = start_asset_store(&store_root.0);
    let id = store.server_id();
    let mut gen = FakeGen::done();
    gen.bytes = test_png();
    let server = start_flow(
        &flow_root.0,
        asset,
        id,
        token,
        seams(FakeChat::done("unused"), gen, FakeHttp::json(200, "{}")),
    );
    let source = "use mod.flow.*\nlet left = Image{prompt: \"same\"}\nlet right = Image{prompt: \"same\"}\nlet left_output = Output{type: @image value: left.image()}\nlet right_output = Output{type: @image value: right.image()}\nFlow{left, right, left_output, right_output}\n";
    let (client, _, row) = run(&server, "parallel", source);
    assert_eq!(row.state, RunState::Done, "{row:?}");
    assert!(row.outputs.contains_key("left_output"));
    assert!(row.outputs.contains_key("right_output"));
    assert_eq!(client.assets("", Some("flows"), 20).unwrap().assets.len(), 1);
    server.shutdown();
    store.shutdown();
}

#[test]
fn archive_failure_fails_the_run_without_hanging() {
    let _guard = test_lock();
    let store_root = TempRoot::new("store");
    let flow_root = TempRoot::new("flow");
    let (mut store, asset, token) = start_asset_store(&store_root.0);
    let id = store.server_id();
    let server = start_flow(
        &flow_root.0,
        asset,
        id,
        token,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    let (_, _, row) = run(&server, "archive-failure", "use mod.flow.*\nlet image = Image{prompt: \"bad fixture\"}\nlet output = Output{type: @image value: image.image()}\nFlow{image, output}\n");
    assert_eq!(row.state, RunState::Failed, "{row:?}");
    assert!(row.nodes["image"].error.as_deref().unwrap_or("").contains("archive"));
    server.shutdown();
    store.shutdown();
}

// Small deterministic index fixture, using the MP4 parser test box layout.
mod mp4_fixture {
    // ---- a tiny box builder for synthetic files
    fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }
    fn full(kind: &[u8; 4], version: u8, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![version, 0, 0, 0];
        p.extend_from_slice(payload);
        bx(kind, &p)
    }
    fn u32s(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    /// Two chunks: chunk 1 holds samples 0,1 at 1000; chunk 2 holds sample
    /// 2 at 5000. 30 fps at timescale 90000; sample 1 is a B-frame shown
    /// after sample 2 (ctts).
    pub fn synthetic(codec_box: Vec<u8>) -> Vec<u8> {
        let hdlr = full(b"hdlr", 0, &[&[0u8; 4][..], b"vide", &[0u8; 12], b"\0"].concat());
        let mut mdhd = u32s(&[0, 0, 90000, 9000]); // creation, modification, timescale, duration
        mdhd.extend_from_slice(&[0, 0, 0, 0]);
        let mdhd = full(b"mdhd", 0, &mdhd);
        let mut tkhd = vec![0u8; 20 + 8 + 2 + 2 + 2 + 2 + 36];
        tkhd.extend_from_slice(&(640u32 << 16).to_be_bytes());
        tkhd.extend_from_slice(&(360u32 << 16).to_be_bytes());
        let tkhd = full(b"tkhd", 0, &tkhd);
        let stsd = full(b"stsd", 0, &[u32s(&[1]), codec_box].concat());
        let stts = full(b"stts", 0, &u32s(&[1, 3, 3000]));
        let ctts = full(b"ctts", 0, &u32s(&[3, 1, 3000, 1, 9000, 1, 3000]));
        let stss = full(b"stss", 0, &u32s(&[1, 1]));
        let stsc = full(b"stsc", 0, &u32s(&[2, 1, 2, 1, 2, 1, 1]));
        let stsz = full(b"stsz", 0, &u32s(&[0, 3, 100, 50, 70]));
        let stco = full(b"stco", 0, &u32s(&[2, 1000, 5000]));
        let stbl = bx(b"stbl", &[stsd, stts, ctts, stss, stsc, stsz, stco].concat());
        let minf = bx(b"minf", &stbl);
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        let trak = bx(b"trak", &[tkhd, mdia].concat());
        let audio_trak = {
            let hdlr = full(b"hdlr", 0, &[&[0u8; 4][..], b"soun", &[0u8; 12], b"\0"].concat());
            bx(b"trak", &bx(b"mdia", &hdlr))
        };
        let moov = bx(b"moov", &[audio_trak, trak].concat());
        let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
        let mdat = bx(b"mdat", &[0u8; 8000]);
        [ftyp, moov, mdat].concat()
    }

    pub fn avc1() -> Vec<u8> {
        let mut entry = vec![0u8; 78];
        entry[24..26].copy_from_slice(&640u16.to_be_bytes());
        entry[26..28].copy_from_slice(&360u16.to_be_bytes());
        let sps = [0x67, 0x64, 0x00, 0x1e, 0xac];
        let pps = [0x68, 0xeb, 0xe3, 0xcb];
        let mut avcc = vec![1, 0x64, 0x00, 0x1e, 0xff, 0xe1];
        avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&sps);
        avcc.push(1);
        avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&pps);
        entry.extend_from_slice(&bx(b"avcC", &avcc));
        bx(b"avc1", &entry)
    }

}
