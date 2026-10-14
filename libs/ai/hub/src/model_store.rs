//! The phone's own models, installed by the app itself: a catalogue of the
//! on-device models per capability, and per model a download that is
//! resumable, size- and sha256-verified before it counts as ready, checked
//! for free space first, and pausable / cancellable.
//!
//! Files land in `<home>/weights/<kind>/<name>` (the Makepad home layout the
//! in-process engines read). Each file has a list of sources tried in order
//! (a hub on the LAN first, then a public mirror); every byte goes through
//! [`crate::download::Downloader::ensure_file`]: `.part` + `Range` resume,
//! hash check, atomic rename, a receipt so a verified file is not hashed again.
//!
//! The store never decides on the network policy (Wi-Fi only): the app calls
//! `start` when it allows a download.

use crate::backend::CancelToken;
use crate::download::{DownloadProgress, Downloader};
use crate::registry::FileSpec;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Text,
    Vision,
    Listen,
    Speak,
}

/// One file of a model and where it can come from.
#[derive(Clone, Debug)]
pub struct StoreFile {
    /// `llm`, `stt` or `tts`: the subdirectory of `<home>/weights`.
    pub kind: &'static str,
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Absolute http(s) URLs, tried in order.
    pub urls: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ModelEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub capability: Capability,
    pub files: Vec<StoreFile>,
    /// Ids of models this one needs to be useful (the vision tower needs
    /// the translation model).
    pub needs: Vec<&'static str>,
}

impl ModelEntry {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Absent,
    Downloading { done: u64, total: u64, bytes_per_s: u64 },
    Paused { done: u64, total: u64 },
    Verifying,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelState {
    pub id: &'static str,
    pub label: &'static str,
    pub capability: Capability,
    pub bytes: u64,
    pub state: Phase,
    pub needs: Vec<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoreEvent {
    Changed(ModelState),
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;

struct Job {
    cancel: CancelToken,
    /// Set by `cancel` (also delete the partial file), not by `pause`.
    discard: Arc<std::sync::atomic::AtomicBool>,
}

struct Shared {
    states: HashMap<&'static str, Phase>,
    jobs: HashMap<&'static str, Job>,
}

/// One download at a time: the sources share one network and a hub caps
/// its concurrent blob serves (excess answers 503).
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

pub struct ModelStore {
    home: PathBuf,
    entries: Vec<ModelEntry>,
    shared: Arc<Mutex<Shared>>,
    events_tx: Sender<StoreEvent>,
    events: Receiver<StoreEvent>,
    wake: Option<Wake>,
}

/// The download identity of one file from one source.
fn spec(file: &StoreFile, url: &str) -> FileSpec {
    FileSpec {
        role: None,
        repo: "makepad-on-device".to_string(),
        path: url.to_string(),
        revision: None,
        cache_as: format!("weights/{}/{}", file.kind, file.name),
        size: Some(file.size),
        sha256: Some(file.sha256.to_string()),
        local: false,
        optional: false,
        converts_to: None,
        conversion: None,
    }
}

fn file_ready(home: &Path, file: &StoreFile) -> bool {
    // The content counts, not the source it came from (the paired hub may
    // be unknown at startup, or another one now).
    crate::download::content_is_verified(&home.join("weights").join(file.kind).join(file.name), file.sha256, file.size)
}

impl ModelStore {
    /// `home` is the Makepad home (`<home>/weights/...`).
    pub fn new(home: PathBuf, entries: Vec<ModelEntry>, wake: Option<Wake>) -> ModelStore {
        let states = entries
            .iter()
            .map(|e| {
                let ready = e.files.iter().all(|f| file_ready(&home, f));
                (e.id, if ready { Phase::Ready } else { Phase::Absent })
            })
            .collect();
        let (events_tx, events) = channel();
        ModelStore {
            home,
            entries,
            shared: Arc::new(Mutex::new(Shared { states, jobs: HashMap::new() })),
            events_tx,
            events,
            wake,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    fn state_of(&self, entry: &ModelEntry, phase: Phase) -> ModelState {
        ModelState {
            id: entry.id,
            label: entry.label,
            capability: entry.capability,
            bytes: entry.bytes(),
            state: phase,
            needs: entry.needs.clone(),
        }
    }

    pub fn models(&self) -> Vec<ModelState> {
        let shared = self.shared.lock().unwrap();
        self.entries.iter().map(|e| self.state_of(e, shared.states[e.id].clone())).collect()
    }

    /// True when the model's files are all on the device and verified.
    pub fn is_ready(&self, id: &str) -> bool {
        self.shared.lock().unwrap().states.get(id) == Some(&Phase::Ready)
    }

    /// Download (or resume, or just verify files already present) the model.
    pub fn start(&self, id: &str) {
        let Some(entry) = self.entries.iter().find(|e| e.id == id).cloned() else { return };
        let mut shared = self.shared.lock().unwrap();
        if shared.jobs.contains_key(entry.id) || shared.states[entry.id] == Phase::Ready {
            return;
        }
        let job = Job { cancel: CancelToken::new(), discard: Arc::new(Default::default()) };
        let (cancel, discard) = (job.cancel.clone(), job.discard.clone());
        shared.jobs.insert(entry.id, job);
        drop(shared);
        let state = self.state_of(&entry, Phase::Downloading { done: 0, total: entry.bytes(), bytes_per_s: 0 });
        self.set(&state);
        let (home, shared, tx, wake) = (self.home.clone(), self.shared.clone(), self.events_tx.clone(), self.wake.clone());
        std::thread::Builder::new()
            .name(format!("model-store-{}", entry.id))
            .spawn(move || {
                let publish = |phase: Phase| {
                    shared.lock().unwrap().states.insert(entry.id, phase.clone());
                    let _ = tx.send(StoreEvent::Changed(ModelState {
                        id: entry.id,
                        label: entry.label,
                        capability: entry.capability,
                        bytes: entry.bytes(),
                        state: phase,
                        needs: entry.needs.clone(),
                    }));
                    if let Some(wake) = &wake {
                        wake();
                    }
                };
                let result = {
                    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
                    if cancel.is_cancelled() { Err("cancelled".to_string()) } else { install(&home, &entry, &cancel, &publish) }
                };
                shared.lock().unwrap().jobs.remove(entry.id);
                match result {
                    Ok(()) => publish(Phase::Ready),
                    Err(_) if discard.load(std::sync::atomic::Ordering::SeqCst) => {
                        for file in &entry.files {
                            let dest = home.join("weights").join(file.kind).join(file.name);
                            let _ = std::fs::remove_file(crate::download::part_path(&dest));
                        }
                        publish(Phase::Absent)
                    }
                    Err(_) if cancel.is_cancelled() => {
                        let done = partial_bytes(&home, &entry);
                        publish(Phase::Paused { done, total: entry.bytes() })
                    }
                    Err(reason) => publish(Phase::Failed(reason)),
                }
            })
            .expect("spawn model-store download");
    }

    /// Stop the download, keeping what arrived (`start` resumes it).
    pub fn pause(&self, id: &str) {
        if let Some(job) = self.shared.lock().unwrap().jobs.get(id) {
            job.cancel.cancel();
        }
    }

    /// Stop the download and delete what arrived.
    pub fn cancel(&self, id: &str) {
        let shared = self.shared.lock().unwrap();
        if let Some(job) = shared.jobs.get(id) {
            job.discard.store(true, std::sync::atomic::Ordering::SeqCst);
            job.cancel.cancel();
            return;
        }
        drop(shared);
        if let Some(entry) = self.entries.iter().find(|e| e.id == id).cloned() {
            if matches!(self.shared.lock().unwrap().states[entry.id], Phase::Paused { .. } | Phase::Failed(_)) {
                for file in &entry.files {
                    let dest = self.home.join("weights").join(file.kind).join(file.name);
                    let _ = std::fs::remove_file(crate::download::part_path(&dest));
                }
                self.set(&self.state_of(&entry, Phase::Absent));
            }
        }
    }

    /// Delete an installed (or partial) model to free the space. Files
    /// shared with a model that stays ready are kept.
    pub fn remove(&self, id: &str) {
        self.pause(id);
        let Some(entry) = self.entries.iter().find(|e| e.id == id).cloned() else { return };
        let keep: Vec<&StoreFile> = self
            .entries
            .iter()
            .filter(|e| e.id != id && self.is_ready(e.id))
            .flat_map(|e| e.files.iter())
            .collect();
        for file in &entry.files {
            if keep.iter().any(|k| k.kind == file.kind && k.name == file.name) {
                continue;
            }
            let dest = self.home.join("weights").join(file.kind).join(file.name);
            let _ = std::fs::remove_file(&dest);
            let _ = std::fs::remove_file(crate::download::part_path(&dest));
            let _ = std::fs::remove_file(crate::download::verification_path(&dest));
        }
        self.set(&self.state_of(&entry, Phase::Absent));
    }

    pub fn poll(&self) -> Vec<StoreEvent> {
        self.events.try_iter().collect()
    }

    fn set(&self, state: &ModelState) {
        self.shared.lock().unwrap().states.insert(state.id, state.state.clone());
        let _ = self.events_tx.send(StoreEvent::Changed(state.clone()));
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

fn partial_bytes(home: &Path, entry: &ModelEntry) -> u64 {
    entry
        .files
        .iter()
        .map(|file| {
            let dest = home.join("weights").join(file.kind).join(file.name);
            if dest.is_file() {
                file.size
            } else {
                std::fs::metadata(crate::download::part_path(&dest)).map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Every file of `entry`, from the first source that delivers it intact.
fn install(home: &Path, entry: &ModelEntry, cancel: &CancelToken, publish: &dyn Fn(Phase)) -> Result<(), String> {
    let downloader = Downloader::new(crate::download::DEFAULT_HF_BASE, None).map_err(|e| e.to_string())?;
    let total = entry.bytes();
    let mut before = 0u64;
    let started = Instant::now();
    let resumed = partial_bytes(home, entry);
    let mut last = Instant::now();
    for file in &entry.files {
        let mut errors = Vec::new();
        let mut installed = false;
        for url in &file.urls {
            let mut progress = |p: DownloadProgress| {
                if last.elapsed().as_millis() < 250 && p.done != file.size {
                    return;
                }
                last = Instant::now();
                let done = before + p.done;
                let secs = started.elapsed().as_secs_f64().max(0.001);
                let fresh = done.saturating_sub(resumed);
                let phase = if p.done >= file.size {
                    Phase::Verifying
                } else {
                    Phase::Downloading { done, total, bytes_per_s: (fresh as f64 / secs) as u64 }
                };
                publish(phase);
            };
            // A source may answer in bounded chunks (the hub's blob endpoint
            // sends at most 32 MiB per response): each call resumes the
            // .part, so keep going while it grows.
            let dest = home.join("weights").join(file.kind).join(file.name);
            let part = crate::download::part_path(&dest);
            let part_len = || std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            // A busy source (a hub at its serve cap answers 503) gets a few
            // retries with backoff before the next source is tried.
            let mut stalls = 0u32;
            loop {
                let before = part_len();
                match downloader.ensure_file(&spec(file, url), home, &mut progress, cancel) {
                    Ok(_) => {
                        installed = true;
                        break;
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            return Err("cancelled".to_string());
                        }
                        if part_len() > before {
                            stalls = 0;
                            continue;
                        }
                        stalls += 1;
                        if stalls <= 3 && e.to_string().contains("503") {
                            std::thread::sleep(std::time::Duration::from_secs(1 << stalls));
                            continue;
                        }
                        errors.push(format!("{url}: {e}"));
                        break;
                    }
                }
            }
            if installed {
                break;
            }
        }
        if !installed {
            return Err(format!("{}: {}", file.name, errors.join("; ")));
        }
        before += file.size;
    }
    Ok(())
}

/// Where the Kokoro G2P lexicon tables are hosted.
const G2P_BASE_URL: &str = "https://makepad.nl/models/speech/kokoro-g2p";

/// The phone translator's models, all converted by our own tools: translation
/// (Qwen3.5-4B, mix B incl. its MTP head, 2.84 GB), its vision tower (Q8_0)
/// with the text-line detector (PP-OCRv5 mobile, 4.7 MB),
/// listening (Whisper large-v3 turbo, Q5_K) and speaking (Kokoro v1.0 and
/// v1.1-zh with a few voices). `lan_hub` (a paired hub's base URL) is tried
/// before `mirror` (a base URL holding the same file names).
pub fn phone_translator_catalogue(lan_hub: Option<&str>, mirror: &str) -> Vec<ModelEntry> {
    let urls = |sha: &str, name: &str| {
        let mut urls = Vec::new();
        if let Some(hub) = lan_hub {
            urls.push(format!("{}{}{sha}", hub.trim_end_matches('/'), crate::peer_serve::BLOB_PATH_PREFIX));
        }
        urls.push(format!("{}/{name}", mirror.trim_end_matches('/')));
        urls
    };
    let file = |kind: &'static str, name: &'static str, size: u64, sha256: &'static str| StoreFile {
        kind,
        name,
        size,
        sha256,
        urls: urls(sha256, name),
    };
    let voice = |name: &'static str, sha256: &'static str| file("tts", name, 522304, sha256);
    // The G2P lexicon tables (makepad_ai_speech::g2p::data), next to the
    // models; also on makepad.nl, which hosts them whatever the mirror holds.
    let g2p = |name: &'static str, size: u64, sha256: &'static str| {
        let mut f = file("tts", name, size, sha256);
        f.urls.push(format!("{G2P_BASE_URL}/{name}"));
        f
    };
    vec![
        ModelEntry {
            id: "translate",
            label: "Translation (Qwen3.5-4B)",
            capability: Capability::Text,
            files: vec![file("llm", "Qwen3.5-4B-MTP-MIX-b.gguf", 2838537952, "8fe179be0ffba55be29ec80b51696ce759ec6cedd50b8b58951fe766cbf2646f")],
            needs: vec![],
        },
        ModelEntry {
            id: "vision",
            label: "Camera (Qwen3.5-4B vision + text finder)",
            capability: Capability::Vision,
            files: vec![
                file("llm", "mmproj-Makepad-q8_0.gguf", 363748896, "ef483750b8ed23d4ba4772d73f5954b625964c30970ac6e77a8a83ed1925edf9"),
                // Where the lines of text are (boxes for placing each translation):
                // PP-OCRv5 mobile detection converted by `textdet convert`.
                file("ocr", "ppocrv5_mobile_det.gguf", 4697376, "3cc9b62fd71a69740ef635ba2b360e5c28bf51db4fff310cbe1de777bf1153ad"),
            ],
            needs: vec!["translate"],
        },
        ModelEntry {
            id: "listen",
            label: "Listening (Whisper large-v3 turbo)",
            capability: Capability::Listen,
            files: vec![file("stt", "ggml-large-v3-turbo-q5_k.bin", 661176075, "243a6cac5d9fe3896045e4fdb78127e9c96049f54ce52438421329a0b7c41872")],
            needs: vec![],
        },
        ModelEntry {
            id: "speak",
            label: "Speaking (Kokoro)",
            capability: Capability::Speak,
            files: vec![
                file("tts", "kokoro-v1_0.mktts", 327096160, "c79e3a0f064736a8680394602f0618efea94e6213e2039ac9fbc588a70b888c1"),
                file("tts", "kokoro-v1_1-zh.mktts", 327096160, "50ee2c6e4d87e81de75e992d4117267ffab0ab09aac26494a2039ccf27592d6b"),
                voice("af_heart.mkvoice", "48d8d8c3bd81597a9e2860b32177ab8f0159482a71024f3410ef2c050ab587e1"),
                voice("bm_daniel.mkvoice", "547a9247a43cb9675701512f88a09065478bed7f3ca43f7b4dc6fd241103ed70"),
                voice("zf_xiaobei.mkvoice", "f69fd0acfcc0fef387e626642a6c069618997552dfb8c70159a0e27d4777514a"),
                voice("zf_xiaoxiao.mkvoice", "f1b6f2ea462f1d1e30feb738cd24fc359258d2b1b94988bec005ce650ec8d486"),
                voice("zf_xiaoyi.mkvoice", "48dcc0ae428a37bd1281ea123da938c816347d8fe1ed426e2fb6007b3de26167"),
                voice("zf_001.mkvoice", "442ff04974ea92b772ae90f5a05d07ab2d1da0df7113c6832df581e36b389af5"),
                voice("zf_002.mkvoice", "4eac26a680b278e198414998e34dc826d75ded2298b0177a360f91861809e0a7"),
                voice("zf_003.mkvoice", "dee3446dfa14706f0ea2310ab08381349b80b626756ac2dee11f9c5070041f02"),
                g2p("us_lexicon.bin", 2917518, "294682efc873a8201106f4dddfcb8e1b16c5d47c0fb8ddaf00c351d62212245c"),
                g2p("zh_pinyin.bin", 1393676, "4d266969277671e6f3238cde2211ab5c6956fc86e08e71e3d9a5f5fdfa3e0471"),
                g2p("zh_words.bin", 1838159, "ab066b416e8952f97791e09c3770e417ba111df80c13336eb76d4fac0c008169"),
            ],
            needs: vec![],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::sha256_hex;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    fn sha(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }

    /// A one-file-per-path HTTP server honouring `Range: bytes=N-`.
    fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut from = 0usize;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap() <= 2 {
                        break;
                    }
                    if let Some(r) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        from = r.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                let body = files.iter().find(|(p, _)| path.ends_with(p)).map(|(_, b)| b.clone());
                let response = match body {
                    Some(b) if from > 0 || path.contains("chunked") => {
                        // `chunked` paths answer at most 64 kB per request.
                        let end = if path.contains("chunked") { (from + 65536).min(b.len()) } else { b.len() };
                        let part = &b[from.min(b.len())..end];
                        let mut r = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            part.len(), from, from + part.len() - 1, b.len()
                        )
                        .into_bytes();
                        r.extend_from_slice(part);
                        r
                    }
                    Some(b) => {
                        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len()).into_bytes();
                        r.extend_from_slice(&b);
                        r
                    }
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&response);
                let _ = reader.read(&mut [0u8; 1]);
            }
        });
        base
    }

    fn wait_for(store: &ModelStore, id: &str, done: impl Fn(&Phase) -> bool) -> Phase {
        let started = Instant::now();
        loop {
            let phase = store.models().into_iter().find(|m| m.id == id).unwrap().state;
            if done(&phase) || started.elapsed() > Duration::from_secs(20) {
                return phase;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn entry(base: &str, bad_first: bool, name: &'static str, body: &[u8]) -> ModelEntry {
        let sha256: &'static str = Box::leak(sha(body).into_boxed_str());
        let mut urls = vec![format!("{base}/{name}")];
        if bad_first {
            urls.insert(0, format!("{base}/missing-{name}"));
        }
        ModelEntry {
            id: "m",
            label: "M",
            capability: Capability::Text,
            files: vec![StoreFile { kind: "llm", name, size: body.len() as u64, sha256, urls }],
            needs: vec![],
        }
    }

    #[test]
    fn installs_from_the_first_source_that_has_the_file_and_resumes_a_part() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let base = serve(vec![("w.bin", body.clone())]);
        let home = std::env::temp_dir().join(format!("model-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        // A partial download from an earlier run.
        let dest = home.join("weights/llm/w.bin");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(crate::download::part_path(&dest), &body[..100_000]).unwrap();
        let store = ModelStore::new(home.clone(), vec![entry(&base, true, "w.bin", &body)], None);
        assert_eq!(store.models()[0].state, Phase::Absent);
        store.start("m");
        assert_eq!(wait_for(&store, "m", |p| matches!(p, Phase::Ready | Phase::Failed(_))), Phase::Ready);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // A new store trusts the receipt without hashing again, whichever
        // sources it lists now.
        let again = ModelStore::new(home.clone(), vec![entry(&base, true, "w.bin", &body)], None);
        assert!(again.is_ready("m"));
        let mut elsewhere = entry(&base, false, "w.bin", &body);
        elsewhere.files[0].urls = vec!["http://127.0.0.1:9/other-mirror/w.bin".to_string()];
        assert!(ModelStore::new(home.clone(), vec![elsewhere], None).is_ready("m"));
        again.remove("m");
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_source_answering_in_chunks_is_followed_to_the_end() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i * 13 % 251) as u8).collect();
        let base = serve(vec![("chunked/c.bin", body.clone())]);
        let home = std::env::temp_dir().join(format!("model-store-test-chunked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut e = entry(&base, false, "c.bin", &body);
        e.files[0].urls = vec![format!("{base}/chunked/c.bin")];
        let store = ModelStore::new(home.clone(), vec![e], None);
        store.start("m");
        assert_eq!(wait_for(&store, "m", |p| matches!(p, Phase::Ready | Phase::Failed(_))), Phase::Ready);
        assert_eq!(std::fs::read(home.join("weights/llm/c.bin")).unwrap(), body);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(feature = "tts")]
    #[test]
    fn the_speak_entry_carries_the_g2p_tables() {
        let catalogue = phone_translator_catalogue(None, "https://mirror.invalid");
        let speak = catalogue.iter().find(|m| m.id == "speak").unwrap();
        for table in makepad_ai_speech::g2p::data::FILES {
            let file = speak.files.iter().find(|f| f.name == table.name).unwrap();
            assert_eq!((file.kind, file.size, file.sha256), ("tts", table.size, table.sha256));
            assert_eq!(file.urls.last(), Some(&table.url()));
        }
    }

    #[test]
    fn a_wrong_hash_fails_and_leaves_nothing_ready() {
        let body = vec![7u8; 50_000];
        let base = serve(vec![("x.bin", body.clone())]);
        let home = std::env::temp_dir().join(format!("model-store-test-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let mut e = entry(&base, false, "x.bin", &body);
        e.files[0].sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let store = ModelStore::new(home.clone(), vec![e], None);
        store.start("m");
        let phase = wait_for(&store, "m", |p| matches!(p, Phase::Ready | Phase::Failed(_)));
        assert!(matches!(phase, Phase::Failed(_)), "{phase:?}");
        assert!(!home.join("weights/llm/x.bin").exists());
        let _ = std::fs::remove_dir_all(&home);
    }
}
