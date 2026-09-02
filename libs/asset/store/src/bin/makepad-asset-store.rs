//! The standalone Asset Server binary: parse flags into a `ServerConfig`,
//! start the runtime, and wait for SIGINT/SIGTERM to shut down cleanly.

use makepad_asset_store::{
    export_static, kind_parse, AssetServer, AssetServerCore, Budgets, DiscoveryConfig,
    ServerConfig, StaticExportOptions,
};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "\
makepad-asset-store --root <dir> [options]
makepad-asset-store export-static <store-root> <out-dir> [options]

Options:
  --root <dir>             Server root directory (catalog, CAS, tokens). Required.
  --control <addr>         Control plane bind address   (default 127.0.0.1:9701)
  --data <addr>            Data plane bind address      (default 127.0.0.1:9702)
  --bootstrap-admin        Ensure the root admin + <root>/admin-token exist
  --discovery              Enable LAN discovery beacons on the default port
  --discovery-port <port>  Enable beacons on a specific UDP port
  --discovery-ip <ip>      Beacon destination IP (default 255.255.255.255)
  --quiet                  No stderr logging
  --help                   This text

Static export options:
  --ns <namespace>                  Include one namespace
  --kind <kind>                     Include one asset kind
  --limit <n>                       Maximum root assets
  --max-bytes-per-asset <bytes>     Per-root unique blob budget
  --max-total-bytes <bytes>         Snapshot unique blob budget
  --include-video-up-to <bytes>     Include each video blob through this size
                                    (default 33554432)
";

static STOP: AtomicBool = AtomicBool::new(false);

// Minimal async-signal-safe handling: the handler only flips an atomic; the
// main thread polls it. SIGINT and SIGTERM both request a clean shutdown.
extern "C" fn on_signal(_sig: i32) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    #[cfg(unix)]
    {
        extern "C" {
            fn signal(signum: i32, handler: usize) -> usize;
        }
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        unsafe {
            signal(SIGINT, on_signal as *const () as usize);
            signal(SIGTERM, on_signal as *const () as usize);
        }
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("makepad-asset-store: {msg}");
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn parse_config() -> ServerConfig {
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    let mut control: Option<SocketAddr> = None;
    let mut data: Option<SocketAddr> = None;
    let mut bootstrap_admin = false;
    let mut discovery = false;
    let mut discovery_port: Option<u16> = None;
    let mut discovery_ip: Option<IpAddr> = None;
    let mut quiet = false;

    let value_of = |name: &str, args: &mut dyn Iterator<Item = String>| -> String {
        match args.next() {
            Some(v) => v,
            None => fail(&format!("{name} needs a value")),
        }
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => {
                root = Some(PathBuf::from(value_of("--root", &mut args)));
            }
            "--control" => {
                let v = value_of("--control", &mut args);
                control = Some(v.parse().unwrap_or_else(|_| fail("malformed --control address")));
            }
            "--data" => {
                let v = value_of("--data", &mut args);
                data = Some(v.parse().unwrap_or_else(|_| fail("malformed --data address")));
            }
            "--bootstrap-admin" => bootstrap_admin = true,
            "--discovery" => discovery = true,
            "--discovery-port" => {
                let v = value_of("--discovery-port", &mut args);
                discovery = true;
                discovery_port = Some(v.parse().unwrap_or_else(|_| fail("malformed --discovery-port")));
            }
            "--discovery-ip" => {
                let v = value_of("--discovery-ip", &mut args);
                discovery_ip = Some(v.parse().unwrap_or_else(|_| fail("malformed --discovery-ip")));
            }
            "--quiet" => quiet = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => fail(&format!("unknown flag {other}")),
        }
    }
    let Some(root) = root else { fail("--root is required") };

    let mut cfg = ServerConfig::new(root);
    if let Some(a) = control {
        cfg.control_addr = a;
    }
    if let Some(a) = data {
        cfg.data_addr = a;
    }
    cfg.bootstrap_admin = bootstrap_admin;
    cfg.log = !quiet;
    if discovery {
        let mut d = DiscoveryConfig::lan_default();
        if let Some(p) = discovery_port {
            d.port = p;
        }
        if let Some(ip) = discovery_ip {
            d.target_ip = ip;
        }
        cfg.discovery = Some(d);
    }
    cfg
}

fn parse_u64(name: &str, value: String) -> u64 {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        fail(&format!("malformed {name}"));
    }
    value.parse().unwrap_or_else(|_| fail(&format!("malformed {name}")))
}

fn run_export() {
    let mut args = std::env::args().skip(2);
    let first = args
        .next()
        .unwrap_or_else(|| fail("export-static needs <store-root>"));
    if matches!(first.as_str(), "--help" | "-h") {
        println!("{USAGE}");
        return;
    }
    let root = PathBuf::from(first);
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| fail("export-static needs <out-dir>"));
    if !root.is_dir() {
        fail("export-static store root is not a directory");
    }
    let value_of = |name: &str, args: &mut dyn Iterator<Item = String>| -> String {
        args.next().unwrap_or_else(|| fail(&format!("{name} needs a value")))
    };
    let mut options = StaticExportOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ns" => options.namespace = Some(value_of("--ns", &mut args)),
            "--kind" => {
                let value = value_of("--kind", &mut args);
                options.kind = Some(kind_parse(&value).unwrap_or_else(|| fail("malformed --kind")));
            }
            "--limit" => {
                options.limit = Some(parse_u64("--limit", value_of("--limit", &mut args)))
            }
            "--max-bytes-per-asset" => {
                options.max_bytes_per_asset = parse_u64(
                    "--max-bytes-per-asset",
                    value_of("--max-bytes-per-asset", &mut args),
                )
            }
            "--max-total-bytes" => {
                options.max_total_bytes = parse_u64(
                    "--max-total-bytes",
                    value_of("--max-total-bytes", &mut args),
                )
            }
            "--include-video-up-to" => {
                options.include_video_up_to = parse_u64(
                    "--include-video-up-to",
                    value_of("--include-video-up-to", &mut args),
                )
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => fail(&format!("unknown export-static flag {other}")),
        }
    }
    let core = AssetServerCore::open(&root, Budgets::default_v1())
        .unwrap_or_else(|error| fail(&format!("cannot open store: {error}")));
    let report = export_static(&core, &out, &options)
        .unwrap_or_else(|error| fail(&format!("static export failed: {error}")));
    println!(
        "exported {} assets, {} revisions, {} aliases, {} blobs ({} omitted) as snapshot {}",
        report.assets,
        report.revisions,
        report.aliases,
        report.blobs_present,
        report.blobs_omitted,
        report.snapshot_id,
    );
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("export-static") {
        run_export();
        return;
    }
    let cfg = parse_config();
    install_signal_handlers();
    let mut server = match AssetServer::start(cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("makepad-asset-store: failed to start: {e}");
            std::process::exit(1);
        }
    };
    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
    server.shutdown();
}
