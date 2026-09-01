//! `makepad-asset-server` — run the Asset Server as its own process.
//!
//! Parse flags into a [`HostConfig`], start the host, and wait for
//! SIGINT/SIGTERM to shut it down cleanly. With no flags at all it serves the
//! checkout's standard store root on ephemeral ports, announces itself on the
//! LAN, and publishes the ai-content library — i.e. everything the Asset
//! UI's embedded mode does, minus the window.
//!
//! See `README.md` beside this file for the deployment runbook.

use makepad_app_asset_server::{Host, HostConfig};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "\
makepad-asset-server [options]

The standalone, multiplayer-first Asset Server: catalog + CAS, events hub,
game rooms, LAN beacon, plus the ai-content library publisher. Clients
(asset-ui, vj, sandbox) attach to it and may come and go without ever
taking the store down. Generation runs in the creating apps over their own
fleet connections — the store stores.

Options:
  --root <dir>            Server root. Default: $AI_CONTENT_ASSET_ROOT, else
                          <checkout>/local/asset-ui/asset-server
  --control <addr>        Control plane bind (default 0.0.0.0:0 — ephemeral;
                          the real address is written to <root>/listen)
  --data <addr>           Data plane bind    (default 0.0.0.0:0)
  --no-beacon             Do not announce on the LAN discovery beacon
  --library <dir>         ai-content library to publish continuously.
                          Default: <checkout>/local/ai_content_library
  --no-library            Do not run the library publisher
  --namespace <ns>        Namespace for published rows (default gen)
  --work <dir>            Parent for the loops' client caches
                          (default: the server root's parent)
  --quiet                 No stderr logging
  --help                  This text
";

static STOP: AtomicBool = AtomicBool::new(false);

// Minimal async-signal-safe handling: the handler only flips an atomic and
// the main thread polls it.
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

fn fail(message: &str) -> ! {
    eprintln!("makepad-asset-server: {message}");
    eprintln!("{USAGE}");
    std::process::exit(2);
}

/// The checkout this binary was built from, so the default root and library
/// are the same paths the Asset UI uses. `MAKEPAD_ROOT` overrides, exactly
/// as it does there.
fn checkout_root() -> PathBuf {
    if let Ok(root) = std::env::var("MAKEPAD_ROOT") {
        return PathBuf::from(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn default_root() -> PathBuf {
    if let Ok(root) = std::env::var("AI_CONTENT_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    checkout_root().join("local/asset-ui/asset-server")
}

fn parse_config() -> HostConfig {
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    let mut control: Option<SocketAddr> = None;
    let mut data: Option<SocketAddr> = None;
    let mut beacon = true;
    let mut library: Option<PathBuf> = None;
    let mut no_library = false;
    let mut namespace: Option<String> = None;
    let mut work: Option<PathBuf> = None;
    let mut log = true;

    let value_of = |name: &str, args: &mut dyn Iterator<Item = String>| -> String {
        match args.next() {
            Some(value) => value,
            None => fail(&format!("{name} needs a value")),
        }
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value_of("--root", &mut args))),
            "--control" => {
                let value = value_of("--control", &mut args);
                control = Some(value.parse().unwrap_or_else(|_| fail("malformed --control address")));
            }
            "--data" => {
                let value = value_of("--data", &mut args);
                data = Some(value.parse().unwrap_or_else(|_| fail("malformed --data address")));
            }
            "--no-beacon" => beacon = false,
            "--library" => library = Some(PathBuf::from(value_of("--library", &mut args))),
            "--no-library" => no_library = true,
            "--namespace" => namespace = Some(value_of("--namespace", &mut args)),
            "--work" => work = Some(PathBuf::from(value_of("--work", &mut args))),
            "--quiet" => log = false,
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => fail(&format!("unknown flag {other}")),
        }
    }
    if no_library && library.is_some() {
        fail("--library and --no-library contradict each other");
    }

    let mut config = HostConfig::new(root.unwrap_or_else(default_root));
    if let Some(addr) = control {
        config.control_addr = addr;
    }
    if let Some(addr) = data {
        config.data_addr = addr;
    }
    config.beacon = beacon;
    config.library = if no_library {
        None
    } else {
        Some(library.unwrap_or_else(|| checkout_root().join("local/ai_content_library")))
    };
    if let Some(namespace) = namespace {
        if namespace.is_empty() {
            fail("--namespace needs a value");
        }
        config.namespace = namespace;
    }
    if let Some(work) = work {
        config.work_root = work;
    }
    config.log = log;
    config
}

fn main() {
    let config = parse_config();
    install_signal_handlers();
    if config.log {
        eprintln!(
            "[asset-host] root {} · library {} · beacon {}",
            config.root.display(),
            config
                .library
                .as_ref()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "off".to_string()),
            if config.beacon { "on" } else { "off" },
        );
    }
    let mut host = match Host::start(&config) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("makepad-asset-server: failed to start: {error}");
            std::process::exit(1);
        }
    };
    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
    host.shutdown();
}
