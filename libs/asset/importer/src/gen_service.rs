//! The generation service: everything a host needs to turn an Asset
//! Server's job queue into answered work on the GPU fleet — published
//! catalog rows for the generating kinds, and text answers or rewritten
//! annotation records for the `vision` ones.
//!
//! Two hosts run exactly this loop — the standalone worker binary and the
//! Asset UI, which coordinates jobs in-process against its own embedded
//! server — so it lives here rather than in either of them. The shape is:
//!
//! - **one claim loop per fleet box**, kind-filtered by that box's own
//!   advertised capabilities. That is both the fan-out (N queued jobs of a
//!   kind drain across the N boxes that serve it) and the concurrency rule
//!   (a box's own queue is its limit — this side never puts two jobs on one
//!   GPU), and it is why a chat-only node never swallows an image job and a
//!   vision box drains both the questions and the annotation backlog.
//! - **one profile announcer**, so `GET /v1/job-profiles` advertises what
//!   the fleet can execute right now instead of what a config file hoped
//!   for at boot.

use crate::coordinator::{AssetAiFleet, Coordinator};
use crate::gen_kinds::kinds_for_domains;
use crate::gen_profiles::{build_profiles, covered_domains};
use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig, PublishRights};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long an announcement lives before the server drops it, and how often
/// we renew — a host that dies stops advertising within one lease without
/// anyone cleaning up after it.
const ANNOUNCE_TTL_MS: u64 = 90_000;
const ANNOUNCE_EVERY: Duration = Duration::from_secs(30);
/// Cadence for noticing a box that joined (or changed capability).
const FLEET_REFRESH: Duration = Duration::from_secs(15);
const IDLE_SLEEP: Duration = Duration::from_secs(2);
const ERROR_SLEEP: Duration = Duration::from_secs(5);

/// Where the GPU boxes come from.
#[derive(Clone, Debug)]
pub enum FleetSource {
    /// A URL list file (one base URL per line).
    File(PathBuf),
    /// The LAN discovery beacon every asset-ai node emits.
    Lan,
    /// An explicit list (tests, pinned deployments).
    Urls(Vec<String>),
}

impl FleetSource {
    pub fn open(&self, log: bool) -> Result<AssetAiFleet, String> {
        match self {
            FleetSource::File(path) => AssetAiFleet::from_fleet_file(path, log),
            FleetSource::Lan => Ok(AssetAiFleet::from_lan(log)),
            FleetSource::Urls(urls) => Ok(AssetAiFleet::from_urls(urls.clone(), log)),
        }
    }
}

/// Everything the service needs. `cache_root` gets one child per (server,
/// box) pair: an `AssetClient` cache is single-owner, so parallel claim
/// loops cannot share one.
#[derive(Clone, Debug)]
pub struct GenServiceConfig {
    pub servers: Vec<ApiEndpoints>,
    pub server_id: Option<[u8; 16]>,
    pub token: String,
    pub cache_root: PathBuf,
    /// Namespace the advertised profiles enqueue into.
    pub namespace: String,
    /// Lease-suffix/worker-id stem; each box's loop appends its own tag.
    pub suffix: String,
    pub rights: PublishRights,
    pub fleet: FleetSource,
    /// Advertise the fleet's executable profiles. Off for a host that must
    /// only drain a queue without touching what the server offers.
    pub announce: bool,
    pub log: bool,
}

/// A short stable id for a box URL, used to keep per-box lease suffixes and
/// cache directories apart without putting a URL in either.
pub fn box_tag(url: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    let bytes = [(h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8, h as u8];
    makepad_asset_client::util::to_hex(&bytes)
}

/// Claim one job at most, using the fleet the source names. The single-shot
/// shape the worker's `--once` mode needs.
pub fn run_once(config: &GenServiceConfig, stop: &AtomicBool) -> Result<Option<String>, String> {
    let mut fleet = config.fleet.open(config.log)?;
    let kinds = kinds_for_domains(&fleet.capabilities());
    if config.log {
        eprintln!(
            "[asset-worker] claiming kinds: {}",
            if kinds.is_empty() { "nothing".to_string() } else { kinds.join(", ") }
        );
    }
    let mut client_config = ClientConfig::new(config.cache_root.join("cache"));
    client_config.token = Some(config.token.clone());
    let client = AssetClient::connect(client_config, config.servers[0], config.server_id)
        .map_err(|e| format!("connect failed: {e}"))?;
    let mut coordinator = Coordinator {
        client,
        fleet: &mut fleet,
        suffix: config.suffix.clone(),
        kinds,
        rights: config.rights.clone(),
        log: config.log,
    };
    match coordinator.run_one(stop) {
        Ok(Some(outcome)) => Ok(Some(format!("{outcome:?}"))),
        Ok(None) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Run until `stop`. Spawns and owns the per-box claim loops; returns after
/// withdrawing the advertisement and joining them.
pub fn run(config: &GenServiceConfig, stop: &'static AtomicBool) {
    let mut spawned: HashSet<(usize, String)> = HashSet::new();
    let mut threads = Vec::new();
    let mut last_announce: Option<Instant> = None;
    while !stop.load(Ordering::SeqCst) {
        // A fresh probe each pass: boxes join and leave, and their
        // capabilities change as weights land.
        let probe = match config.fleet.open(false) {
            Ok(probe) => probe,
            Err(error) => {
                if config.log {
                    eprintln!("[asset-worker] fleet source: {error}");
                }
                sleep_sliced(FLEET_REFRESH, stop);
                continue;
            }
        };
        let (snapshots, _) = probe.snapshots();
        let boxes = probe.boxes();

        for (index, endpoints) in config.servers.iter().enumerate() {
            for url in &boxes {
                if !spawned.insert((index, url.clone())) {
                    continue;
                }
                if let Some(handle) = spawn_box_loop(config, index, *endpoints, url, stop) {
                    threads.push(handle);
                }
            }
        }

        // Advertise what the fleet can execute NOW. This is what makes a
        // profile disappear when its weights leave the fleet, instead of
        // clients enqueueing jobs nothing can ever claim.
        if config.announce && last_announce.is_none_or(|at| at.elapsed() >= ANNOUNCE_EVERY) {
            last_announce = Some(Instant::now());
            let profiles = build_profiles(&snapshots, &config.namespace);
            let domains = covered_domains();
            for (index, endpoints) in config.servers.iter().enumerate() {
                let Some(client) = announce_client(config, index, *endpoints) else {
                    continue;
                };
                match client.announce_job_profiles(
                    &config.suffix,
                    &config.namespace,
                    ANNOUNCE_TTL_MS,
                    &domains,
                    &profiles,
                ) {
                    Ok(()) if config.log => eprintln!(
                        "[asset-worker] advertised {} executable profiles to {}",
                        profiles.len(),
                        endpoints.control
                    ),
                    Ok(()) => {}
                    Err(error) if config.log => {
                        eprintln!("[asset-worker] announce failed: {error}")
                    }
                    Err(_) => {}
                }
            }
        }
        sleep_sliced(FLEET_REFRESH, stop);
    }

    // Clean shutdown: withdraw the advertisement immediately rather than
    // letting clients enqueue against a worker that is already gone.
    if config.announce {
        for (index, endpoints) in config.servers.iter().enumerate() {
            if let Some(client) = announce_client(config, index, *endpoints) {
                let _ = client.retract_job_profiles(&config.suffix, &config.namespace);
            }
        }
    }
    for handle in threads {
        let _ = handle.join();
    }
    if config.log {
        eprintln!("[asset-worker] stopped");
    }
}

fn announce_client(
    config: &GenServiceConfig,
    index: usize,
    endpoints: ApiEndpoints,
) -> Option<AssetClient> {
    let mut client_config =
        ClientConfig::new(config.cache_root.join(format!("announce-{index}")));
    client_config.token = Some(config.token.clone());
    match AssetClient::connect(client_config, endpoints, config.server_id) {
        Ok(client) => Some(client),
        Err(error) => {
            if config.log {
                eprintln!("[asset-worker] announce connect failed: {error}");
            }
            None
        }
    }
}

/// One box's claim loop: pinned to that box, kind-filtered by what it
/// advertises, refreshing that filter as weights land on it.
fn spawn_box_loop(
    config: &GenServiceConfig,
    index: usize,
    endpoints: ApiEndpoints,
    url: &str,
    stop: &'static AtomicBool,
) -> Option<std::thread::JoinHandle<()>> {
    let tag = box_tag(url);
    let suffix = format!("{}-{tag}", config.suffix);
    let cache = config.cache_root.join(format!("box-{index}-{tag}"));
    let token = config.token.clone();
    let server_id = config.server_id;
    let rights = config.rights.clone();
    let log = config.log;
    let url = url.to_string();
    std::thread::Builder::new()
        .name(format!("asset-gen-{index}-{tag}"))
        .spawn(move || {
            let mut client_config = ClientConfig::new(cache);
            client_config.token = Some(token);
            let client = match AssetClient::connect(client_config, endpoints, server_id) {
                Ok(client) => client,
                Err(error) => {
                    eprintln!("[asset-worker] {url}: connect failed: {error}");
                    return;
                }
            };
            let capabilities =
                || kinds_for_domains(&AssetAiFleet::from_urls(vec![url.clone()], false).capabilities());
            let mut kinds = capabilities();
            if log {
                eprintln!(
                    "[asset-worker] {url}: claiming {}",
                    if kinds.is_empty() {
                        "nothing (no wired domain)".to_string()
                    } else {
                        kinds.join(", ")
                    }
                );
            }
            // Pinned to ONE box: its own queue is the concurrency limit, so
            // this loop never puts two jobs on one GPU.
            let mut fleet = AssetAiFleet::from_urls(vec![url.clone()], log);
            let mut last_caps = Instant::now();
            let mut coordinator = Coordinator {
                client,
                fleet: &mut fleet,
                suffix,
                kinds: kinds.clone(),
                rights,
                log,
            };
            while !stop.load(Ordering::SeqCst) {
                if last_caps.elapsed() >= FLEET_REFRESH {
                    last_caps = Instant::now();
                    let fresh = capabilities();
                    // Only act on a REAL change: a probe that failed must
                    // not look like a box that lost every capability.
                    if !fresh.is_empty() && fresh != kinds {
                        if log {
                            eprintln!("[asset-worker] {url}: now claiming {}", fresh.join(", "));
                        }
                        kinds = fresh.clone();
                        coordinator.kinds = fresh;
                    }
                }
                match coordinator.run_one(stop) {
                    Ok(Some(outcome)) => {
                        if log {
                            eprintln!("[asset-worker] {url}: {outcome:?}");
                        }
                    }
                    Ok(None) => sleep_sliced(IDLE_SLEEP, stop),
                    Err(error) => {
                        if log {
                            eprintln!("[asset-worker] {url}: server call failed: {error}");
                        }
                        sleep_sliced(ERROR_SLEEP, stop);
                    }
                }
            }
        })
        .ok()
}

fn sleep_sliced(total: Duration, stop: &AtomicBool) {
    let mut left = total;
    while !left.is_zero() && !stop.load(Ordering::SeqCst) {
        let slice = left.min(Duration::from_millis(100));
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_tags_separate_caches_and_leases_without_naming_a_box() {
        let a = box_tag("http://10.0.0.169:8123");
        let b = box_tag("http://10.0.0.123:8123");
        assert_ne!(a, b);
        assert_eq!(a, box_tag("http://10.0.0.169:8123"), "tags must be stable");
        assert_eq!(a.len(), 8);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        // A lease suffix built from it stays inside the server's charset.
        let suffix = format!("asset-ui-{a}");
        assert!(suffix
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
    }

    #[test]
    fn a_fleet_source_opens_what_it_names() {
        let urls = vec!["http://box-a".to_string(), "http://box-b".to_string()];
        let fleet = FleetSource::Urls(urls.clone()).open(false).unwrap();
        assert_eq!(fleet.boxes(), urls);
        // A missing fleet file fails closed instead of silently going LAN.
        assert!(FleetSource::File(PathBuf::from("/nonexistent/fleet.txt"))
            .open(false)
            .is_err());
    }
}
