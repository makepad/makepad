//! The standalone Asset Server binary: parse flags into a `ServerConfig`,
//! start the runtime, and wait for SIGINT/SIGTERM to shut down cleanly.

use makepad_asset_store::{AssetServer, DiscoveryConfig, ServerConfig};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "\
makepad-asset-store --root <dir> [options]

Options:
  --root <dir>             Server root directory (catalog, CAS, tokens). Required.
  --control <addr>         Control plane bind address   (default 127.0.0.1:9701)
  --data <addr>            Data plane bind address      (default 127.0.0.1:9702)
  --bootstrap-admin        Ensure the root admin + <root>/admin-token exist
  --discovery              Enable LAN discovery beacons on the default port
  --discovery-port <port>  Enable beacons on a specific UDP port
  --discovery-ip <ip>      Beacon destination IP (default 255.255.255.255)
  --quiet                  No stderr logging
  --chat-fleet <url>       Local Qwen fleet node (repeatable)
  --help                   This text
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
    let mut chat_fleet: Vec<String> = Vec::new();

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
            "--chat-fleet" => {
                let v = value_of("--chat-fleet", &mut args);
                if v.is_empty() {
                    fail("malformed --chat-fleet");
                }
                chat_fleet.push(v);
            }
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
    cfg.chat.fleet_bases = chat_fleet;
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

fn main() {
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
