//! makepad-asset-ai service binary. Runs on each GPU box; wraps all AI
//! content generation behind a port.
//!
//! ```text
//! makepad-asset-ai [--port N] [--host ADDR] [--cache-dir PATH] [--registry PATH]
//!
//!   --port      listen port          (env MAKEPAD_ASSET_AI_PORT, default 8765)
//!   --host      bind address         (default 0.0.0.0)
//!   --fleet     partition name       (env MAKEPAD_ASSET_AI_FLEET, default default)
//!   --cache-dir model + artifact dir (env MAKEPAD_ASSET_AI_CACHE,
//!                                     default <home>/.makepad/ai_content)
//!   --registry  registry json path   (default: <cache-dir>/registry.json if it
//!                                     exists, else the embedded registry)
//!
//!   env HF_TOKEN                     bearer token for gated HF repos (flux1-dev)
//!   env MAKEPAD_ASSET_AI_HF_BASE   alternate HF endpoint / LAN mirror
//! ```

use makepad_asset_ai::download::Downloader;
use makepad_asset_ai::registry::Registry;
use makepad_asset_ai::server::{start_service, ServiceConfig};
use makepad_asset_ai::{AssetAiError, DEFAULT_PORT, SERVICE_NAME, SERVICE_VERSION};
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

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
            "--help" | "-h" => {
                println!(
                    "{SERVICE_NAME} {SERVICE_VERSION}\nusage: {SERVICE_NAME} [--port N] [--host ADDR] [--fleet NAME] [--cache-dir PATH] [--registry PATH]"
                );
                return Ok(());
            }
            other => {
                return Err(AssetAiError::Io(format!("unknown argument {other:?}")));
            }
        }
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
    let fleet = makepad_asset_ai::discovery::normalize_fleet(
        &fleet.unwrap_or_else(makepad_asset_ai::discovery::fleet_from_env),
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
    println!(
        "  endpoints: /health /models /jobs POST:/generate /job/<id> POST:/job/<id>/cancel /artifact/<id> /v1/model_inventory /v1/model_blob/<sha256> POST:/realtime GET(ws):/realtime/<id>"
    );

    // The http listener thread runs until the process is killed.
    let _ = handle.http_thread.join();
    Ok(())
}

fn default_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MAKEPAD_ASSET_AI_CACHE") {
        return PathBuf::from(dir);
    }
    // USERPROFILE on Windows, HOME elsewhere; temp dir as a last resort.
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".makepad").join("ai_content")
}
