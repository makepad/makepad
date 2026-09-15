//! makepad-ai-hub service binary. Runs on each GPU box; wraps all AI
//! content generation behind a port.
//!
//! ```text
//! makepad-ai-hub [--port N] [--host ADDR] [--cache-dir PATH] [--registry PATH]
//!
//!   --port      listen port          (env MAKEPAD_ASSET_AI_PORT, default 8765)
//!   --host      bind address         (default 0.0.0.0)
//!   --fleet     partition name       (env MAKEPAD_ASSET_AI_FLEET, default default)
//!   --cache-dir model + artifact dir (env MAKEPAD_ASSET_AI_CACHE,
//!                                     default <home>/.makepad/weights)
//!   --registry  registry json path   (default: <cache-dir>/registry.json if it
//!                                     exists, else the embedded registry)
//!
//!   env HF_TOKEN                     bearer token for gated HF repos (flux1-dev)
//!   env MAKEPAD_ASSET_AI_HF_BASE   alternate HF endpoint / LAN mirror
//! ```

use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::registry::Registry;
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_ai_hub::{AssetAiError, DEFAULT_PORT, SERVICE_NAME, SERVICE_VERSION};
use std::path::PathBuf;

fn main() {
    if let Err(err) = run() {
        eprintln!("{SERVICE_NAME}: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), AssetAiError> {
    let mut port: Option<u16> = None;
    let mut host = "0.0.0.0".to_string();
    let mut fleet: Option<String> = None;
    let mut cache_dir: Option<PathBuf> = None;
    let mut registry_path: Option<PathBuf> = None;
    let mut machine = false;
    let mut activity_probe = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--activity-probe" => {
                activity_probe = Some(args.next().ok_or_else(|| AssetAiError::Io("--activity-probe needs seconds (1..3600)".into()))?.parse::<u64>().map_err(|_| AssetAiError::Io("invalid activity probe seconds".into()))?);
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| AssetAiError::Io("--port needs a value".into()))?;
                port = Some(
                    value
                        .parse()
                        .map_err(|_| AssetAiError::Io(format!("bad --port {value:?}")))?,
                );
            }
            "--host" => {
                host = args
                    .next()
                    .ok_or_else(|| AssetAiError::Io("--host needs a value".into()))?;
            }
            "--fleet" => {
                fleet = Some(args.next().ok_or_else(|| {
                    AssetAiError::Io("--fleet needs a value".into())
                })?);
            }
            "--cache-dir" => {
                cache_dir = Some(PathBuf::from(args.next().ok_or_else(|| {
                    AssetAiError::Io("--cache-dir needs a value".into())
                })?));
            }
            "--registry" => {
                registry_path = Some(PathBuf::from(args.next().ok_or_else(|| {
                    AssetAiError::Io("--registry needs a value".into())
                })?));
            }
            // The machine node (aicore §3): loopback-only, registered in
            // ~/.makepad/run for the apps on this machine, and gone on its
            // own once nothing needs it — a cache, not a daemon.
            "--machine" => {
                machine = true;
            }
            "--help" | "-h" => {
                println!(
                    "{SERVICE_NAME} {SERVICE_VERSION}\nusage: {SERVICE_NAME} [--port N] [--host ADDR] [--fleet NAME] [--cache-dir PATH] [--registry PATH] [--machine] [--activity-probe SECONDS]"
                );
                return Ok(());
            }
            other => {
                return Err(AssetAiError::Io(format!("unknown argument {other:?}")));
            }
        }
    }

    if let Some(seconds) = activity_probe {
        return makepad_ai_hub::activity::run_probe(seconds);
    }

    let port = match port {
        Some(port) => port,
        None => match std::env::var("MAKEPAD_ASSET_AI_PORT") {
            Ok(value) => value
                .parse()
                .map_err(|_| AssetAiError::Io(format!("bad MAKEPAD_ASSET_AI_PORT {value:?}")))?,
            Err(_) => DEFAULT_PORT,
        },
    };
    let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
    let fleet = makepad_ai_hub::discovery::normalize_fleet(
        &fleet.unwrap_or_else(makepad_ai_hub::discovery::fleet_from_env),
    );

    // Registry: explicit path > registry.json dropped into the cache dir
    // (per-box extension without a rebuild) > embedded seed registry.
    let registry = if let Some(path) = registry_path {
        Registry::load_file(&path)?
    } else {
        let cache_registry = cache_dir.join("registry.json");
        if cache_registry.is_file() {
            Registry::load_file(&cache_registry)?
        } else {
            Registry::embedded()?
        }
    };

    // The machine node is machine-local by definition: loopback bind, no
    // matter what --host said.
    if machine {
        host = "127.0.0.1".to_string();
    }
    let downloader = Downloader::from_env()?;
    let handle = start_service(ServiceConfig {
        host,
        port,
        cache_dir: cache_dir.clone(),
        registry,
        downloader,
        // Peer-cache lane: everything resolves from env / cache-dir files
        // (MAKEPAD_AI_PEER_SECRET or <cache>/peer-secret enables serving;
        // MAKEPAD_AI_PEER_SOURCES injects download sources).
        peer: Default::default(),
        fleet: fleet.clone(),
    })?;

    println!("{SERVICE_NAME} {SERVICE_VERSION}");
    println!("  listening on http://{}", handle.addr);
    println!("  fleet {fleet}");
    println!("  cache dir {}", cache_dir.display());
    if std::env::var("HF_TOKEN").is_ok() {
        println!("  HF_TOKEN present (gated repos enabled)");
    }
    println!("  lora dir  {}", cache_dir.join("loras").display());
    println!(
        "  endpoints: /health /models /jobs /loras POST:/generate /job/<id> POST:/job/<id>/cancel /artifact/<id> /v1/model_inventory /v1/model_blob/<sha256> POST:/realtime GET(ws):/realtime/<id>"
    );

    if machine {
        return run_machine_node(handle);
    }

    // The http listener thread runs until the process is killed.
    let _ = handle.http_thread.join();
    Ok(())
}

/// The machine node's life: register in ~/.makepad/run so the apps on this
/// machine find it, then idle down and exit once nothing has needed it for
/// the TTL — it reads as a cache, not a daemon (aicore §3). Visible in any
/// process list as makepad-ai-hub.
fn run_machine_node(handle: makepad_ai_hub::server::ServiceHandle) -> Result<(), AssetAiError> {
    use makepad_ai_hub::machine::{write_node_entry, NodeEntry};
    use std::time::{Duration, Instant};

    let ttl_min: u64 = std::env::var("MAKEPAD_AI_HUB_MACHINE_TTL_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    let entry = NodeEntry {
        pid: std::process::id() as u64,
        port: handle.addr.port(),
        pipes_hash: 0,
    };
    let entry_path = write_node_entry(&handle.shared.node_key, &entry)
        .map_err(|e| AssetAiError::Io(format!("write node entry: {e}")))?;
    println!("  machine node: registered {} (ttl {ttl_min}m idle)", entry_path.display());

    let mut idle_since = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(30));
        // Busy = queued/running work, or a model somebody paid to load.
        let pending = handle.shared.jobs.with(|store| store.pending_count()) > 0;
        let resident = handle
            .shared
            .models
            .lock()
            .unwrap()
            .values()
            .any(|track| matches!(track, makepad_ai_hub::server::ModelTrack::Loaded));
        if pending || resident {
            idle_since = Instant::now();
        } else if idle_since.elapsed() > Duration::from_secs(ttl_min * 60) {
            println!("{SERVICE_NAME}: machine node idle for {ttl_min}m — exiting");
            let _ = std::fs::remove_file(&entry_path);
            return Ok(());
        }
    }
}

fn default_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MAKEPAD_ASSET_AI_CACHE") {
        return PathBuf::from(dir);
    }
    makepad_ai_hub::home::default_weights_dir_with_migration(&mut |message| {
        eprintln!("{SERVICE_NAME}: {message}");
    })
}
