//! The model catalogue a desktop hub shows its person: every registry model this build
//! can run on this machine (backend compiled in, GPU requirements met: the hub's own
//! [`model_availability`]), what it takes on disk, its licence, and the downloads and
//! deletes the person starts from the panel. Nothing here downloads on its own.
//!
//! Downloads go through the prefetcher's fetch path (fleet peers that hold a file
//! first, then Hugging Face; sha256-verified, resumable): one worker thread, one model
//! at a time, in the order asked. Licences: `none` and `community` start on request;
//! `non-commercial`, `restricted`, a missing licence and Hugging Face gates need the
//! person's explicit confirm, and a gated model needs `HF_TOKEN`.

use crate::backend::{model_availability, CancelToken};
use crate::download::{part_path, verification_path, Downloader};
use crate::gpu::GpuInfo;
use crate::prefetch::PrefetchNode;
use crate::registry::{FileSpec, LicenseRestriction, ModelSpec, Registry};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// What has to happen before a model may be downloaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LicenceGate {
    /// `none` / `community`: a click starts it.
    Open,
    /// The person confirms first; the text names the licence ("non-commercial",
    /// "restricted", "gated", "non-commercial, gated", "not stated").
    Confirm(String),
    /// Behind a Hugging Face gate and this process has no `HF_TOKEN`.
    NeedsToken,
}

/// The gate of `spec` for a downloader that has (`hf_token`) a Hugging Face token or not.
pub fn licence_gate(spec: &ModelSpec, hf_token: bool) -> LicenceGate {
    if spec.gated && !hf_token {
        return LicenceGate::NeedsToken;
    }
    let class = match spec.license.as_ref().map(|l| l.restriction) {
        Some(LicenseRestriction::None | LicenseRestriction::Community) => None,
        Some(other) => Some(other.as_str()),
        None => Some("not stated"),
    };
    match (class, spec.gated) {
        (None, false) => LicenceGate::Open,
        (None, true) => LicenceGate::Confirm("gated".to_string()),
        (Some(c), false) => LicenceGate::Confirm(c.to_string()),
        (Some(c), true) => LicenceGate::Confirm(format!("{c}, gated")),
    }
}

/// One model this machine can run.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    /// Registry domain ("text", "stt", …) and the panel's kind of it ([`crate::control::model_kind`]).
    pub domain: &'static str,
    pub kind: &'static str,
    /// Bytes of all its files (shared files once): the download size.
    pub download_bytes: u64,
    /// Bytes of its files (and partial downloads) on disk now.
    pub disk_bytes: u64,
    /// Every required file is present and verified: the hub can serve it.
    pub on_disk: bool,
    /// Licence class ("none", "community", "non-commercial", "restricted", "not stated").
    pub licence: String,
    pub licence_name: String,
    pub gated: bool,
    pub gate: LicenceGate,
    /// Why it cannot be downloaded at all (weights only converted by hand), if so.
    pub not_downloadable: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalogue {
    /// Grouped by kind (chat first, the panel's order), by id within a kind.
    pub entries: Vec<Entry>,
    /// Registry models this machine cannot run, with the hub's reason.
    pub elsewhere: Vec<(String, String)>,
}

impl Catalogue {
    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The ids whose files are all on disk.
    pub fn on_disk(&self) -> Vec<String> {
        self.entries.iter().filter(|e| e.on_disk).map(|e| e.id.clone()).collect()
    }
}

/// Whether `spec` is a downloadable local model at all (not the Claude CLI, not a
/// box-provisioned backend without weights).
fn is_local_model(spec: &ModelSpec) -> bool {
    spec.backend != crate::claude_cli::BACKEND && !spec.files.is_empty()
}

fn kind_order(kind: &str) -> u8 {
    match kind {
        "chat" => 0,
        "vision" => 1,
        "stt" => 2,
        "tts" => 3,
        "image" => 4,
        "music" => 5,
        "mesh" => 7,
        "video" => 8,
        _ => 9,
    }
}

fn unique_files(spec: &ModelSpec) -> Vec<&FileSpec> {
    let mut seen = HashSet::new();
    spec.files.iter().filter(|f| seen.insert(f.cache_as.as_str())).collect()
}

/// The paths a file may occupy in the cache: the download, its receipt, its partial
/// download, and its converted form with its receipt.
fn file_paths(file: &FileSpec, cache_dir: &Path) -> Vec<PathBuf> {
    let dest = file.dest_path(cache_dir);
    let mut out = vec![verification_path(&dest), part_path(&dest), dest];
    if let Some(converted) = file.converted_path(cache_dir) {
        out.push(verification_path(&converted));
        out.push(converted);
    }
    out
}

fn bytes_on_disk(file: &FileSpec, cache_dir: &Path) -> u64 {
    file_paths(file, cache_dir).iter().filter_map(|p| std::fs::metadata(p).ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
}

/// The catalogue of `registry` for this machine (`gpu`), reading `cache_dir`.
pub fn scan(registry: &Registry, cache_dir: &Path, gpu: &GpuInfo, hf_token: bool) -> Catalogue {
    let mut out = Catalogue::default();
    for spec in registry.models.iter().filter(|s| is_local_model(s)) {
        if let Err(reason) = model_availability(spec, gpu, 0) {
            out.elsewhere.push((spec.id.clone(), reason));
            continue;
        }
        let files = unique_files(spec);
        let not_downloadable = files
            .iter()
            .find(|f| f.local && (f.sha256.is_none() || f.size.is_none()) && !f.is_present(cache_dir))
            .map(|f| format!("{} is converted locally and cannot be downloaded", f.cache_as));
        let domain = spec.domain.as_str();
        out.entries.push(Entry {
            id: spec.id.clone(),
            domain,
            kind: crate::control::model_kind(domain),
            download_bytes: files.iter().filter_map(|f| f.size).sum(),
            disk_bytes: files.iter().map(|f| bytes_on_disk(f, cache_dir)).sum(),
            on_disk: spec.files_present(cache_dir),
            licence: spec.license.as_ref().map_or("not stated", |l| l.restriction.as_str()).to_string(),
            licence_name: spec.license.as_ref().map(|l| l.name.clone()).unwrap_or_default(),
            gated: spec.gated,
            gate: licence_gate(spec, hf_token),
            not_downloadable,
        });
    }
    out.entries.sort_by(|a, b| kind_order(a.kind).cmp(&kind_order(b.kind)).then_with(|| a.id.cmp(&b.id)));
    out
}

/// Deletes `id`'s files from `cache_dir` to free space; files another model that is on
/// disk also uses stay. Refused while the model is loaded, working or downloading
/// (`in_use`). Returns the bytes freed.
pub fn delete_model(registry: &Registry, cache_dir: &Path, id: &str, in_use: bool) -> Result<u64, String> {
    let spec = registry.find(id).ok_or_else(|| format!("{id} is not in the registry"))?;
    if !is_local_model(spec) {
        return Err(format!("{id} has no files to delete"));
    }
    if in_use {
        return Err(format!("{id} is loaded or in use: unload it first"));
    }
    let kept: HashSet<&str> = registry
        .models
        .iter()
        .filter(|other| other.id != spec.id && is_local_model(other) && other.files_present(cache_dir))
        .flat_map(|other| other.files.iter().map(|f| f.cache_as.as_str()))
        .collect();
    let mut freed = 0u64;
    let mut failed = Vec::new();
    for file in unique_files(spec).into_iter().filter(|f| !kept.contains(f.cache_as.as_str())) {
        for path in file_paths(file, cache_dir) {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if !meta.is_file() {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => freed += meta.len(),
                Err(e) => failed.push(format!("{}: {e}", path.display())),
            }
        }
        // Empty folders the model's files lived in go too (never the cache itself).
        let mut dir = file.dest_path(cache_dir).parent().map(Path::to_path_buf);
        while let Some(d) = dir.filter(|d| d.starts_with(cache_dir) && d != cache_dir) {
            if std::fs::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    if !failed.is_empty() {
        return Err(format!("freed {freed} bytes, could not delete {}", failed.join(", ")));
    }
    Ok(freed)
}

/// A download the person started.
#[derive(Clone, Debug, PartialEq)]
pub enum DownloadState {
    /// Waiting for the one before it.
    Queued,
    /// Bytes of the model on disk so far (resumed bytes included) of `total`.
    Fetching { done: u64, total: u64, file: String },
    /// It stopped; the reason is the downloader's. Asking again resumes it.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DownloadRow {
    pub id: String,
    pub state: DownloadState,
}

/// What a download request was refused for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotInCatalogue(String),
    /// The licence needs the person's confirm (its class).
    NeedsConfirm(String),
    /// A Hugging Face gate and no `HF_TOKEN`.
    NeedsToken,
    NotDownloadable(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotInCatalogue(why) => write!(f, "{why}"),
            Refusal::NeedsConfirm(class) => write!(f, "the licence is {class}: it needs a confirm"),
            Refusal::NeedsToken => write!(f, "gated on Hugging Face: set HF_TOKEN (an account with access granted) and start AI Hub again"),
            Refusal::NotDownloadable(why) => write!(f, "{why}"),
        }
    }
}

/// Whether `id` may be downloaded now: in the catalogue, downloadable, its licence gate
/// passed (`confirmed`: the person said yes to a licence that needs it).
pub fn check_download(catalogue: &Catalogue, id: &str, confirmed: bool) -> Result<(), Refusal> {
    let entry = catalogue.find(id).ok_or_else(|| Refusal::NotInCatalogue(format!("{id} cannot run on this machine")))?;
    if let Some(why) = &entry.not_downloadable {
        return Err(Refusal::NotDownloadable(why.clone()));
    }
    match &entry.gate {
        LicenceGate::Open => Ok(()),
        LicenceGate::Confirm(_) if confirmed => Ok(()),
        LicenceGate::Confirm(class) => Err(Refusal::NeedsConfirm(class.clone())),
        LicenceGate::NeedsToken => Err(Refusal::NeedsToken),
    }
}

struct Job {
    id: String,
    state: DownloadState,
    cancel: CancelToken,
}

#[derive(Default)]
struct DlShared {
    jobs: Vec<Job>,
    /// Models whose download ended (done, failed or cancelled) since the last take.
    ended: Vec<String>,
}

/// The person's downloads: one worker thread fetches one model at a time, in the order
/// asked. It lives as long as the app (closing a panel does not stop it).
pub struct Downloads {
    shared: Arc<Mutex<DlShared>>,
    tx: Sender<String>,
}

impl Downloads {
    /// Downloads into `cache_dir` for the models of `registry`, with `downloader`.
    pub fn start(registry: Registry, cache_dir: PathBuf, downloader: Downloader) -> Downloads {
        let shared = Arc::new(Mutex::new(DlShared::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = shared.clone();
        let spawned = std::thread::Builder::new().name("ai-hub-downloads".into()).spawn(move || worker(rx, s, registry, cache_dir, downloader));
        if let Err(e) = spawned {
            eprintln!("downloads: cannot start: {e}");
        }
        Downloads { shared, tx }
    }

    /// Queues `id` (already checked with [`check_download`]); a failed one starts again.
    pub fn request(&self, id: &str) {
        let Ok(mut s) = self.shared.lock() else { return };
        match s.jobs.iter_mut().find(|j| j.id == id) {
            Some(j) if matches!(j.state, DownloadState::Failed(_)) => {
                j.state = DownloadState::Queued;
                j.cancel = CancelToken::new();
            }
            Some(_) => return,
            None => s.jobs.push(Job { id: id.to_string(), state: DownloadState::Queued, cancel: CancelToken::new() }),
        }
        drop(s);
        let _ = self.tx.send(id.to_string());
    }

    /// Stops `id` at its next chunk (the partial file stays, a new request resumes it),
    /// or takes it off the queue; a failed one's row goes.
    pub fn cancel(&self, id: &str) {
        let Ok(mut s) = self.shared.lock() else { return };
        if let Some(j) = s.jobs.iter().find(|j| j.id == id) {
            j.cancel.cancel();
        }
        s.jobs.retain(|j| j.id != id || matches!(j.state, DownloadState::Fetching { .. }));
    }

    pub fn rows(&self) -> Vec<DownloadRow> {
        self.shared.lock().map(|s| s.jobs.iter().map(|j| DownloadRow { id: j.id.clone(), state: j.state.clone() }).collect()).unwrap_or_default()
    }

    /// Whether `id` is queued or fetching.
    pub fn busy(&self, id: &str) -> bool {
        self.shared.lock().is_ok_and(|s| s.jobs.iter().any(|j| j.id == id && !matches!(j.state, DownloadState::Failed(_))))
    }

    /// The models whose download ended since the last call (the caller rescans the disk).
    pub fn take_ended(&self) -> Vec<String> {
        self.shared.lock().map(|mut s| std::mem::take(&mut s.ended)).unwrap_or_default()
    }
}

fn worker(rx: Receiver<String>, shared: Arc<Mutex<DlShared>>, registry: Registry, cache_dir: PathBuf, downloader: Downloader) {
    let node_key = crate::server::load_or_create_node_key(&cache_dir);
    let secret = crate::peer::TransferSecret::resolve(None, &cache_dir);
    // Fleet peers are found by their beacons.
    let _ = crate::discovery::start_listener();
    while let Ok(id) = rx.recv() {
        let cancel = {
            let Ok(s) = shared.lock() else { return };
            match s.jobs.iter().find(|j| j.id == id && j.state == DownloadState::Queued) {
                Some(j) => j.cancel.clone(),
                // Cancelled while queued, or a repeated request.
                None => continue,
            }
        };
        let set = |state: DownloadState| {
            if let Ok(mut s) = shared.lock() {
                if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
                    j.state = state;
                }
            }
            crate::control::changed();
        };
        let Some(spec) = registry.find(&id) else {
            set(DownloadState::Failed("not in the registry".to_string()));
            continue;
        };
        let node = PrefetchNode { registry: &registry, cache_dir: &cache_dir, downloader: downloader.clone(), node_key: node_key.clone(), secret: secret.clone(), reserve_mb: 0 };
        let files = unique_files(spec);
        let total: u64 = files.iter().filter_map(|f| f.size).sum();
        let wanted: Vec<&FileSpec> = files.iter().copied().filter(|f| !f.is_present(&cache_dir)).collect();
        // Bytes of the files already complete count as done.
        let base: u64 = files.iter().filter(|f| f.is_present(&cache_dir)).filter_map(|f| f.size).sum();
        set(DownloadState::Fetching { done: base, total, file: String::new() });
        let peers = if wanted.is_empty() { Vec::new() } else { crate::prefetch::peers(&node) };
        let order = crate::prefetch::fetch_order(&node, &peers, &wanted);
        let mut finished_before = vec![0u64; order.len() + 1];
        for (i, f) in order.iter().enumerate() {
            finished_before[i + 1] = finished_before[i] + f.size.unwrap_or(0);
        }
        let mut last_publish = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let mut progress = |index: usize, p: &crate::download::DownloadProgress| {
            if last_publish.elapsed() < std::time::Duration::from_millis(250) {
                return;
            }
            last_publish = std::time::Instant::now();
            let done = base + finished_before[index] + p.done;
            set(DownloadState::Fetching { done: done.min(total.max(done)), total: total.max(done), file: order[index].cache_as.clone() });
        };
        let (_, failed) = crate::prefetch::fetch_files(&node, &peers, &order, None, &mut progress, &cancel);
        let cancelled = cancel.is_cancelled();
        if let Ok(mut s) = shared.lock() {
            if failed.is_empty() || cancelled {
                s.jobs.retain(|j| j.id != id);
            } else if let Some(j) = s.jobs.iter_mut().find(|j| j.id == id) {
                let mut why: Vec<String> = failed.into_iter().map(|(f, e)| format!("{f}: {e}")).collect();
                why.sort();
                j.state = DownloadState::Failed(why.join("; "));
            }
            s.ended.push(id.clone());
        }
        crate::control::changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY: &str = r#"{"models":[
      {"id":"chat-a","domain":"text","backend":"llm","available":true,"gated":false,
       "license":{"name":"Apache 2.0","url":"https://a","summary":"s","restriction":"none"},
       "files":[{"role":"model","repo":"r/a","path":"a.gguf","cache_as":"t/a.gguf"},{"role":"shared","repo":"r/s","path":"s.bin","cache_as":"t/shared.bin"}]},
      {"id":"chat-b","domain":"text","backend":"llm","available":true,"gated":false,
       "license":{"name":"Community","url":"https://b","summary":"s","restriction":"community"},
       "files":[{"role":"model","repo":"r/b","path":"b.gguf","cache_as":"t/b.gguf"},{"role":"shared","repo":"r/s","path":"s.bin","cache_as":"t/shared.bin"}]},
      {"id":"song-nc","domain":"music","backend":"llm","available":true,"gated":false,
       "license":{"name":"NC","url":"https://c","summary":"s","restriction":"non-commercial"},
       "files":[{"role":"model","repo":"r/c","path":"c.bin","cache_as":"m/c.bin"}]},
      {"id":"img-gated","domain":"image","backend":"llm","available":true,"gated":true,
       "license":{"name":"NC","url":"https://d","summary":"s","restriction":"non-commercial"},
       "files":[{"role":"model","repo":"r/d","path":"d.bin","cache_as":"i/d.bin"}]},
      {"id":"big","domain":"text","backend":"llm","available":true,"gated":false,"min_vram_gb":80,
       "license":{"name":"Apache 2.0","url":"https://e","summary":"s","restriction":"none"},
       "files":[{"role":"model","repo":"r/e","path":"e.bin","cache_as":"t/e.bin"}]},
      {"id":"not-built","domain":"video","backend":"no-such-backend","available":true,"gated":false,
       "files":[{"role":"model","repo":"r/f","path":"f.bin","cache_as":"v/f.bin"}]},
      {"id":"oracle","domain":"rig","backend":"llm","available":true,"gated":false,"files":[]}
    ]}"#;

    fn registry() -> Registry {
        Registry::parse(REGISTRY).unwrap()
    }

    fn temp_cache(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hub-catalogue-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(cache: &Path, rel: &str, bytes: usize) {
        let p = cache.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![7u8; bytes]).unwrap();
    }

    fn gpu(vram_mb: Option<u64>) -> GpuInfo {
        GpuInfo { name: Some("test".into()), vram_free_mb: vram_mb, vram_total_mb: vram_mb, compute_cap: Some(8.9) }
    }

    #[test]
    fn the_catalogue_lists_what_this_machine_can_run_grouped_by_kind() {
        let cache = temp_cache("scan");
        let c = scan(&registry(), &cache, &gpu(Some(24 * 1024)), false);
        if !crate::backend::backend_compiled("llm") {
            assert!(c.entries.is_empty(), "nothing runs without the llm backend");
            return;
        }
        let ids: Vec<&str> = c.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["chat-a", "chat-b", "img-gated", "song-nc"], "chat first, then image, then music; no oracle without files");
        let elsewhere: Vec<&str> = c.elsewhere.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(elsewhere, ["big", "not-built"]);
        assert!(c.elsewhere[0].1.contains("VRAM"), "{}", c.elsewhere[0].1);
        assert!(c.elsewhere[1].1.contains("not compiled"), "{}", c.elsewhere[1].1);
        // A 96 GB card runs the big one too.
        assert!(scan(&registry(), &cache, &gpu(Some(96 * 1024)), false).find("big").is_some());
        // Nothing on disk yet.
        assert!(c.entries.iter().all(|e| !e.on_disk && e.disk_bytes == 0));
        put(&cache, "t/a.gguf", 1000);
        put(&cache, "t/shared.bin", 500);
        let c = scan(&registry(), &cache, &gpu(Some(24 * 1024)), false);
        let a = c.find("chat-a").unwrap();
        assert!(a.on_disk && a.disk_bytes == 1500);
        assert_eq!(c.on_disk(), ["chat-a"]);
        let b = c.find("chat-b").unwrap();
        assert!(!b.on_disk && b.disk_bytes == 500, "the shared file counts for both");
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn licences_beyond_none_and_community_need_a_confirm_and_gates_a_token() {
        let r = registry();
        assert_eq!(licence_gate(r.find("chat-a").unwrap(), false), LicenceGate::Open);
        assert_eq!(licence_gate(r.find("chat-b").unwrap(), false), LicenceGate::Open, "community starts on click");
        assert_eq!(licence_gate(r.find("song-nc").unwrap(), false), LicenceGate::Confirm("non-commercial".into()));
        assert_eq!(licence_gate(r.find("img-gated").unwrap(), false), LicenceGate::NeedsToken);
        assert_eq!(licence_gate(r.find("img-gated").unwrap(), true), LicenceGate::Confirm("non-commercial, gated".into()));
        assert_eq!(licence_gate(r.find("not-built").unwrap(), false), LicenceGate::Confirm("not stated".into()));
        let cache = temp_cache("gate");
        let c = scan(&r, &cache, &gpu(Some(24 * 1024)), false);
        if c.entries.is_empty() {
            return;
        }
        assert_eq!(check_download(&c, "chat-a", false), Ok(()));
        assert_eq!(check_download(&c, "song-nc", false), Err(Refusal::NeedsConfirm("non-commercial".into())));
        assert_eq!(check_download(&c, "song-nc", true), Ok(()));
        assert_eq!(check_download(&c, "img-gated", true), Err(Refusal::NeedsToken), "a confirm does not replace the token");
        assert!(matches!(check_download(&c, "big", false), Err(Refusal::NotInCatalogue(_))));
        let with_token = scan(&r, &cache, &gpu(Some(24 * 1024)), true);
        assert_eq!(check_download(&with_token, "img-gated", false), Err(Refusal::NeedsConfirm("non-commercial, gated".into())));
        assert_eq!(check_download(&with_token, "img-gated", true), Ok(()));
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn delete_frees_the_models_own_files_never_while_in_use() {
        let r = registry();
        let cache = temp_cache("delete");
        put(&cache, "t/a.gguf", 1000);
        put(&cache, "t/b.gguf", 300);
        put(&cache, "t/shared.bin", 500);
        put(&cache, "t/b.gguf.part", 40);
        assert!(delete_model(&r, &cache, "chat-b", true).unwrap_err().contains("unload"), "loaded or working: refused");
        assert!(cache.join("t/b.gguf").is_file(), "a refused delete touches nothing");
        // chat-a is on disk and shares t/shared.bin: it stays.
        assert_eq!(delete_model(&r, &cache, "chat-b", false), Ok(340));
        assert!(!cache.join("t/b.gguf").exists() && !cache.join("t/b.gguf.part").exists());
        assert!(cache.join("t/shared.bin").is_file() && cache.join("t/a.gguf").is_file());
        // The last user of the shared file takes it along, and the emptied folder goes.
        assert_eq!(delete_model(&r, &cache, "chat-a", false), Ok(1500));
        assert!(!cache.join("t").exists() && cache.is_dir(), "the cache itself stays");
        assert!(delete_model(&r, &cache, "oracle", false).is_err(), "nothing to delete");
        assert!(delete_model(&r, &cache, "nope", false).is_err());
        let _ = std::fs::remove_dir_all(&cache);
    }

    /// The real thing, over the network (`--ignored`): the smallest registry model
    /// (basic-pitch, 230 KB) through the download worker (fleet peers, then upstream),
    /// verified on disk, then deleted.
    #[test]
    #[ignore]
    fn downloads_and_deletes_the_smallest_model_for_real() {
        let registry = Registry::embedded().unwrap();
        let cache = temp_cache("real");
        let d = Downloads::start(registry.clone(), cache.clone(), Downloader::from_env().unwrap());
        d.request("basic-pitch");
        let started = std::time::Instant::now();
        let mut seen_progress = false;
        while d.take_ended().is_empty() {
            assert!(started.elapsed().as_secs() < 300, "basic-pitch did not finish in 5 min: {:?}", d.rows());
            seen_progress |= d.rows().iter().any(|r| matches!(r.state, DownloadState::Fetching { .. }));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(d.rows().is_empty(), "it ended without a failure: {:?}", d.rows());
        let spec = registry.find("basic-pitch").unwrap();
        assert!(spec.files_present(&cache), "verified on disk");
        eprintln!("downloaded basic-pitch in {:?} (progress seen: {seen_progress})", started.elapsed());
        let freed = delete_model(&registry, &cache, "basic-pitch", false).unwrap();
        assert!(freed >= 230_444, "freed {freed}");
        assert!(!spec.files_present(&cache));
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn a_download_queue_takes_requests_once_and_cancels_queued_ones() {
        let cache = temp_cache("queue");
        // A registry with nothing the worker could fetch: the rows are what is checked.
        let d = Downloads { shared: Arc::new(Mutex::new(DlShared::default())), tx: std::sync::mpsc::channel().0 };
        d.request("chat-a");
        d.request("chat-a");
        d.request("chat-b");
        assert_eq!(d.rows().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["chat-a", "chat-b"]);
        assert!(d.busy("chat-a"));
        d.cancel("chat-b");
        assert_eq!(d.rows().len(), 1, "a queued download is taken off the queue");
        if let Ok(mut s) = d.shared.lock() {
            s.jobs[0].state = DownloadState::Failed("x".into());
        }
        assert!(!d.busy("chat-a"), "a failed download is not busy (delete may run)");
        d.request("chat-a");
        assert_eq!(d.rows()[0].state, DownloadState::Queued, "asking again retries");
        let _ = std::fs::remove_dir_all(&cache);
    }
}
