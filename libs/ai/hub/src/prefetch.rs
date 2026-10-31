//! Keeps this node's model cache complete: every registry model this machine can
//! run (the hub's own `model_availability`) whose licence class the operator
//! allows is fetched ahead of any job, so the fleet scheduler can place a job on
//! any node without a first-use download. New registry models are picked up by
//! the next pass.
//!
//! Settings (the WM's `ai-hub.conf` `models`, `models_licences`, `models_mbps`):
//! - `MAKEPAD_AI_PREFETCH`: `all`, or a comma list of model ids; unset/`off` = no prefetch.
//! - `MAKEPAD_AI_PREFETCH_LICENCES`: licence classes fetched (`none`, `community`,
//!   `non-commercial`, `restricted`, plus `gated` for models behind a Hugging Face
//!   gate). Default `none,community`: anything else is the operator's explicit call.
//! - `MAKEPAD_AI_PREFETCH_MBPS`: receive pacing in MB/s (default 60, 0 = unpaced).
//!
//! Bytes come from the fleet first: every peer on the verified roster (signed
//! beacons, pinned TLS, fleet credentials) is asked for its inventory once per
//! pass and digest-pinned files are pulled from the peers that hold them, through
//! the same verified, resumable downloader jobs use. Only files no peer holds go
//! to Hugging Face, after every peer-held file, in an order rotated by node so
//! nodes of one fleet start on different files. Unpinned files cannot be
//! verified against a peer and are fetched from Hugging Face only.
//!
//! Each pass writes `<cache>/prefetch.status`: one line per model.

use crate::backend::{model_availability, CancelToken};
use crate::download::DownloadProgress;
use crate::http_client::{http_fetch_no_redirect, HttpClientRequest};
use crate::download::Downloader;
use crate::peer::{PeerPlan, TransferSecret};
use crate::protocol::ModelInventoryJson;
use crate::registry::{FileSpec, LicenseRestriction, ModelSpec, Registry};
use crate::server::ServiceShared;
use std::path::Path;
use makepad_micro_serde::DeJson;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FIRST_PASS_AFTER: Duration = Duration::from_secs(60);
const PASS_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DEFAULT_MBPS: f64 = 60.0;
const INVENTORY_MAX_BYTES: usize = 8 * 1024 * 1024;
/// A holder paced below this serves only files Hugging Face cannot (`local`):
/// upstream is faster than a trickle.
const SLOW_PEER_MBPS: f64 = 16.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    All,
    Only(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrefetchConfig {
    pub models: Selection,
    /// Licence classes fetched ([`LicenseRestriction::as_str`] names, plus `gated`).
    pub licences: Vec<String>,
    /// Receive pacing in bytes per second (None: unpaced).
    pub bytes_per_sec: Option<f64>,
}

impl PrefetchConfig {
    pub fn from_env() -> Option<Self> {
        let var = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Self::parse(var("MAKEPAD_AI_PREFETCH").as_deref(), var("MAKEPAD_AI_PREFETCH_LICENCES").as_deref(), var("MAKEPAD_AI_PREFETCH_MBPS").as_deref())
    }

    pub fn parse(models: Option<&str>, licences: Option<&str>, mbps: Option<&str>) -> Option<Self> {
        let list = |text: &str| text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
        let models = match models? {
            "off" | "none" | "0" => return None,
            "all" => Selection::All,
            ids => Selection::Only(list(ids)),
        };
        let licences = list(licences.unwrap_or("none,community"));
        let mbps = mbps.and_then(|m| m.parse::<f64>().ok()).unwrap_or(DEFAULT_MBPS);
        let bytes_per_sec = (mbps.is_finite() && mbps > 0.0).then_some(mbps * 1e6);
        Some(Self { models, licences, bytes_per_sec })
    }

    /// Why `spec` is not fetched by licence, or None when it is.
    pub fn licence_refusal(&self, spec: &ModelSpec) -> Option<String> {
        let class = spec.license.as_ref().map(|l| l.restriction.as_str()).unwrap_or("missing");
        if !self.licences.iter().any(|c| c == class) {
            return Some(format!("licence {class} not in models_licences"));
        }
        if spec.gated && !self.licences.iter().any(|c| c == "gated") {
            return Some("gated (add `gated` to models_licences)".to_string());
        }
        None
    }

    fn selects(&self, id: &str) -> bool {
        match &self.models {
            Selection::All => true,
            Selection::Only(ids) => ids.iter().any(|m| m == id),
        }
    }
}

/// What a pass needs of the node: the hub's own state, or (`makepad-app-ai-hub
/// --prefetch`) the same read from the cache directory without a running service.
pub struct PrefetchNode<'a> {
    pub registry: &'a Registry,
    pub cache_dir: &'a Path,
    pub downloader: Downloader,
    pub node_key: String,
    pub secret: Option<TransferSecret>,
    pub reserve_mb: u64,
}

impl<'a> PrefetchNode<'a> {
    pub fn of_service(shared: &'a ServiceShared) -> Self {
        Self {
            registry: &shared.registry,
            cache_dir: &shared.cache_dir,
            downloader: shared.downloader.clone(),
            node_key: shared.node_key.clone(),
            secret: shared.peer.secret.clone(),
            reserve_mb: shared.residency.reserve_mb,
        }
    }
}

pub fn describe(config: &PrefetchConfig) {
    eprintln!(
        "prefetch: {} model(s), licences {}, {}",
        match &config.models {
            Selection::All => "all".to_string(),
            Selection::Only(ids) => ids.join(","),
        },
        config.licences.join(","),
        match config.bytes_per_sec {
            Some(b) => format!("{:.0} MB/s", b / 1e6),
            None => "unpaced".to_string(),
        }
    );
}

/// The hub's prefetcher: a pass a minute after start, then every ten minutes.
pub fn start(shared: Arc<ServiceShared>, config: PrefetchConfig) {
    describe(&config);
    let spawned = std::thread::Builder::new().name("ai-hub-prefetch".into()).spawn(move || {
        // Beacons first: the roster fills within a few beacon intervals.
        let _ = crate::discovery::start_listener();
        std::thread::sleep(FIRST_PASS_AFTER);
        loop {
            run_pass(&PrefetchNode::of_service(&shared), &config, &mut |id| crate::server::note_model_files_present(&shared, id));
            std::thread::sleep(PASS_INTERVAL);
        }
    });
    if let Err(e) = spawned {
        eprintln!("prefetch: cannot start: {e}");
    }
}

/// One model's verdict for the status file.
enum Verdict {
    Skipped(String),
    Wanted,
}

/// A serving peer, its pacing (`serve_mbps`: 0 unpaced, None unknown) and the digests it holds (None: a
/// peer the operator listed in `MAKEPAD_AI_PEER_SOURCES` whose inventory is not
/// readable, asked for every file after the known holders).
pub(crate) struct Peer {
    base: String,
    mbps: Option<f64>,
    digests: Option<HashSet<String>>,
}

pub(crate) fn peers(node: &PrefetchNode) -> Vec<Peer> {
    let listed = PeerPlan::env_sources();
    let mut out = Vec::new();
    for found in crate::discovery::start_listener().nodes() {
        let base = found.base_url.trim_end_matches('/').to_string();
        if out.iter().any(|p: &Peer| p.base == base) {
            continue;
        }
        let url = format!("{base}/v1/model_inventory");
        let inventory = http_fetch_no_redirect(&HttpClientRequest::get(&url))
            .ok()
            .filter(|r| r.status == 200)
            .and_then(|r| r.read_body_to_vec(INVENTORY_MAX_BYTES).ok())
            .and_then(|body| String::from_utf8(body).ok())
            .and_then(|text| ModelInventoryJson::deserialize_json_lenient(&text).ok());
        // A peer that does not say what it holds is a source only when the operator listed it.
        match inventory {
            Some(inv) if inv.node_key != node.node_key && inv.peer_serving => {
                out.push(Peer { base, mbps: inv.serve_mbps, digests: Some(inv.artifacts.into_iter().map(|a| a.digest).collect()) })
            }
            None if !crate::peer::roster_sources(&listed, std::slice::from_ref(&base)).is_empty() => {
                out.push(Peer { base, mbps: None, digests: None })
            }
            _ => {}
        }
    }
    out
}

/// Sources for one file: its holders, unpaced first then by pace, rotated by digest
/// within a pace so receivers spread over equal holders. Slow holders, and holders
/// of unknown pace (older hubs), only for a file Hugging Face does not have.
fn sources_for(peers: &[Peer], digest: &str, local: bool) -> Vec<String> {
    let mut holders: Vec<&Peer> = peers
        .iter()
        .filter(|p| p.digests.as_ref().is_some_and(|d| d.contains(digest)) && (local || p.mbps.is_some_and(|m| m == 0.0 || m >= SLOW_PEER_MBPS)))
        .collect();
    if !holders.is_empty() {
        let spin = digest.bytes().take(8).fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
        let n = spin % holders.len();
        holders.rotate_left(n);
    }
    let rate = |p: &Peer| match p.mbps {
        Some(m) if m == 0.0 => f64::INFINITY,
        Some(m) => m,
        None => 0.0,
    };
    holders.sort_by(|a, b| rate(b).total_cmp(&rate(a)));
    let listed = peers.iter().filter(|p| p.digests.is_none());
    holders.into_iter().chain(listed).map(|p| p.base.clone()).take(crate::peer::MAX_PLAN_SOURCES).collect()
}

/// One pass: fetches what is missing and returns the status text (also written to
/// `<cache>/prefetch.status`). `complete` hears each model whose files are all present.
pub fn run_pass(node: &PrefetchNode, config: &PrefetchConfig, complete: &mut dyn FnMut(&str)) -> String {
    let started = Instant::now();
    let gpu = crate::gpu::query_gpu();
    let mut verdicts: Vec<(&ModelSpec, Verdict)> = Vec::new();
    let mut wanted: Vec<&FileSpec> = Vec::new();
    let mut seen = HashSet::new();
    for spec in &node.registry.models {
        if !config.selects(&spec.id) {
            continue;
        }
        let verdict = if let Err(reason) = model_availability(spec, &gpu, node.reserve_mb) {
            Verdict::Skipped(format!("cannot run here: {reason}"))
        } else if let Some(reason) = config.licence_refusal(spec) {
            Verdict::Skipped(reason)
        } else {
            for file in &spec.files {
                if seen.insert(file.cache_as.as_str()) && !file.is_present(node.cache_dir) {
                    wanted.push(file);
                }
            }
            Verdict::Wanted
        };
        verdicts.push((spec, verdict));
    }

    let peers = if wanted.is_empty() { Vec::new() } else { peers(node) };
    let order = fetch_order(node, &peers, &wanted);
    let cancel = CancelToken::new();
    let (fetched, failed) = fetch_files(node, &peers, &order, config.bytes_per_sec, &mut |_, _| {}, &cancel);

    let mut lines = vec![format!(
        "# prefetch pass {} s, fetched {:.1} GB, gpu {} {} MB cc {}, {} peer(s)",
        started.elapsed().as_secs(),
        fetched as f64 / 1e9,
        gpu.name.as_deref().unwrap_or("none"),
        gpu.vram_total_mb.unwrap_or(0),
        gpu.compute_cap.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
        peers.len()
    )];
    for (spec, verdict) in &verdicts {
        let gb = spec_gb(spec);
        let line = match verdict {
            Verdict::Skipped(reason) => format!("{} skipped {gb:.1}GB {reason}", spec.id),
            Verdict::Wanted => {
                let missing: Vec<&FileSpec> = spec.files.iter().filter(|f| !f.is_present(node.cache_dir)).collect();
                if missing.is_empty() {
                    complete(&spec.id);
                    let licence = spec.license.as_ref().map(|l| l.restriction).unwrap_or(LicenseRestriction::Restricted);
                    format!("{} complete {gb:.1}GB licence {}{}", spec.id, licence.as_str(), if spec.gated { " gated" } else { "" })
                } else {
                    let why = missing.iter().find_map(|f| failed.get(f.cache_as.as_str())).cloned().unwrap_or_default();
                    format!("{} missing {}/{} files {gb:.1}GB {why}", spec.id, missing.len(), spec.files.len())
                }
            }
        };
        lines.push(line);
    }
    let text = lines.join("\n") + "\n";
    let path = node.cache_dir.join("prefetch.status");
    let part = node.cache_dir.join("prefetch.status.part");
    if std::fs::write(&part, &text).and_then(|_| std::fs::rename(&part, &path)).is_err() {
        eprintln!("prefetch: cannot write {}", path.display());
    }
    eprintln!("{}", lines[0]);
    text
}

/// The order to fetch `wanted` in: files a peer holds first; the rest (Hugging Face)
/// after, rotated by this node's key so the nodes of a fleet start on different files.
pub(crate) fn fetch_order<'f>(node: &PrefetchNode, peers: &[Peer], wanted: &[&'f FileSpec]) -> Vec<&'f FileSpec> {
    let held = |file: &FileSpec| file.sha256.as_deref().is_some_and(|d| peers.iter().any(|p| p.digests.as_ref().is_some_and(|s| s.contains(d))));
    let (mut order, mut rest): (Vec<&FileSpec>, Vec<&FileSpec>) = wanted.iter().copied().partition(|f| held(f));
    if !rest.is_empty() {
        let spin = node.node_key.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize)) % rest.len();
        rest.rotate_left(spin);
    }
    order.extend(rest);
    order
}

/// Fetches `files` in order through the verified, resumable downloader: each from the
/// peers that hold it (fast holders first), else Hugging Face; paced to `bytes_per_sec`.
/// `progress` hears each file's index in `files` and its progress. Returns the bytes
/// received and the failures by cache path; a cancel ends it at the next chunk.
pub(crate) fn fetch_files<'f>(
    node: &PrefetchNode,
    peers: &[Peer],
    files: &[&'f FileSpec],
    bytes_per_sec: Option<f64>,
    progress: &mut dyn FnMut(usize, &DownloadProgress),
    cancel: &CancelToken,
) -> (u64, HashMap<&'f str, String>) {
    let mut failed: HashMap<&str, String> = HashMap::new();
    let mut fetched = 0u64;
    for (index, file) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            failed.insert(file.cache_as.as_str(), "cancelled".to_string());
            continue;
        }
        let sources = file.sha256.as_deref().map(|d| sources_for(peers, d, file.local)).unwrap_or_default();
        let plan = PeerPlan::for_job(&[], &[], &sources, &node.node_key, node.secret.clone());
        let downloader = node.downloader.clone().with_peer_plan(plan);
        let pace = Pace::new(bytes_per_sec);
        let mut first: Option<u64> = None;
        let mut last = 0u64;
        let mut on_progress = |p: DownloadProgress| {
            let start = *first.get_or_insert(p.done);
            last = p.done;
            progress(index, &p);
            pace.wait(p.done.saturating_sub(start));
        };
        match downloader.ensure_file(file, node.cache_dir, &mut on_progress, cancel) {
            Ok(_) => {
                if let Some(start) = first {
                    fetched += last.saturating_sub(start);
                    eprintln!("prefetch: {} done ({:.2} GB)", file.cache_as, file.size.unwrap_or(last) as f64 / 1e9);
                }
            }
            Err(e) => {
                eprintln!("prefetch: {}: {e}", file.cache_as);
                failed.insert(file.cache_as.as_str(), e.to_string());
            }
        }
    }
    (fetched, failed)
}

/// Unique bytes of a model's files.
fn spec_gb(spec: &ModelSpec) -> f64 {
    let mut seen = HashSet::new();
    spec.files.iter().filter(|f| seen.insert(f.cache_as.as_str())).filter_map(|f| f.size).sum::<u64>() as f64 / 1e9
}

/// Paces one file's receive to a byte rate by sleeping in the progress callback
/// (the peer path reports per chunk, the Hugging Face path per read).
struct Pace {
    bytes_per_sec: Option<f64>,
    started: Instant,
}

impl Pace {
    fn new(bytes_per_sec: Option<f64>) -> Self {
        Self { bytes_per_sec, started: Instant::now() }
    }

    fn wait(&self, bytes: u64) {
        let Some(rate) = self.bytes_per_sec else { return };
        let due = Duration::from_secs_f64(bytes as f64 / rate);
        let elapsed = self.started.elapsed();
        if due > elapsed {
            std::thread::sleep(due - elapsed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse() {
        assert_eq!(PrefetchConfig::parse(None, None, None), None);
        assert_eq!(PrefetchConfig::parse(Some("off"), None, None), None);
        let c = PrefetchConfig::parse(Some("all"), None, None).unwrap();
        assert_eq!(c.models, Selection::All);
        assert_eq!(c.licences, ["none", "community"]);
        assert_eq!(c.bytes_per_sec, Some(60e6));
        let c = PrefetchConfig::parse(Some("kokoro, yue2-3b"), Some("none,non-commercial,gated"), Some("0")).unwrap();
        assert_eq!(c.models, Selection::Only(vec!["kokoro".into(), "yue2-3b".into()]));
        assert!(c.selects("yue2-3b") && !c.selects("flux1-dev"));
        assert_eq!(c.bytes_per_sec, None);
    }

    #[test]
    fn licence_classes_and_gates_are_opt_in() {
        let registry = crate::registry::Registry::load_file(&std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/registry.json"))).unwrap();
        let c = PrefetchConfig::parse(Some("all"), None, None).unwrap();
        let open = c.licence_refusal(registry.find("qwen3.5-4b").unwrap());
        assert_eq!(open, None);
        assert!(c.licence_refusal(registry.find("yue2-3b").unwrap()).is_some(), "non-commercial needs the operator");
        let nc = PrefetchConfig::parse(Some("all"), Some("none,non-commercial"), None).unwrap();
        assert_eq!(nc.licence_refusal(registry.find("yue2-3b").unwrap()), None);
        assert!(nc.licence_refusal(registry.find("flux1-dev").unwrap()).unwrap().contains("gated"));
    }

    #[test]
    fn sources_are_holders_fast_first_and_slow_only_for_local_files() {
        let peer = |base: &str, mbps: Option<f64>, d: &[&str]| Peer { base: base.into(), mbps, digests: Some(d.iter().map(|s| s.to_string()).collect()) };
        let peers = [peer("https://a", Some(0.0), &["x"]), peer("https://slow", Some(4.0), &["x", "y"]), peer("https://c", Some(100.0), &["x"]), peer("https://d", Some(0.0), &["x", "y"]), peer("https://old", None, &["x", "y"])];
        let s = sources_for(&peers, "x", false);
        assert_eq!(s.len(), 3, "the 4 MB/s and the unknown-pace holders are left out: upstream is faster");
        assert_eq!(s[2], "https://c", "unpaced first, then by pace");
        assert!(s[..2].contains(&"https://a".to_string()) && s[..2].contains(&"https://d".to_string()));
        assert_eq!(sources_for(&peers, "y", true), ["https://d", "https://slow", "https://old"]);
        assert!(sources_for(&peers, "z", false).is_empty());
        let mut peers: Vec<Peer> = peers.into_iter().collect();
        peers.push(Peer { base: "https://listed".into(), mbps: None, digests: None });
        assert_eq!(sources_for(&peers, "z", false), ["https://listed"]);
        assert_eq!(sources_for(&peers, "y", true).last().map(String::as_str), Some("https://listed"));
    }
}
