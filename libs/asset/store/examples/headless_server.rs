//! Headless dev server for the sandbox-LLM iteration loop (sandbox.md §4):
//! serve an ISOLATED copy of a catalog root over loopback with the chat
//! broker, no UI, no LAN discovery — so the teaching-context loop can run
//! many fast chat-protocol iterations without touching the user's live app.
//!
//! ```bash
//! cp -Rc local/asset-ui/asset-server /tmp/root-snap && rm /tmp/root-snap/server.lock
//! MAKEPAD_CHAT_TAP=/tmp/chat-tap.log \
//! cargo run -p makepad-asset-store --release --example headless_server -- \
//!     --root /tmp/root-snap --control 127.0.0.1:9811 --data 127.0.0.1:9812 \
//!     --chat-fleet http://10.0.0.169:8123
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    let mut control: SocketAddr = "127.0.0.1:9811".parse().unwrap();
    let mut data: SocketAddr = "127.0.0.1:9812".parse().unwrap();
    let mut fleet_bases: Vec<String> = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().unwrap_or_else(|| panic!("{what} needs a value"));
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value("--root"))),
            "--control" => control = value("--control").parse().expect("control addr"),
            "--data" => data = value("--data").parse().expect("data addr"),
            "--chat-fleet" => fleet_bases.push(value("--chat-fleet")),
            other => panic!("unknown arg {other}"),
        }
    }
    let root = root.expect("--root is required (an ISOLATED copy, never the live root)");
    let mut cfg = makepad_asset_store::ServerConfig::new(root);
    cfg.control_addr = control;
    cfg.data_addr = data;
    cfg.discovery = None; // never beacon a dev snapshot onto the LAN
    cfg.log = true;
    cfg.chat.fleet_bases = fleet_bases;
    let server = makepad_asset_store::AssetServer::start(cfg).expect("start server");
    println!(
        "headless asset server up: control {control}, data {data} (ctrl-c to stop)"
    );
    let _ = server;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
