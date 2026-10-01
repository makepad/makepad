//! ai-fleet: the fleet key's tool (admin machine only).
//!
//!   ai-fleet init                                   create the fleet key
//!   ai-fleet issue <client_id> <lan|node|device> --out <file> [--days N]
//!                                                   write a client credential (0600)
//!   ai-fleet node-files <name> --out <dir> [--days N]
//!                                                   fleet.key + node.credential for a node's <cache>/fleet
//!   ai-fleet revoke <client_id>                     add an id to the revocation list
//!
//! The fleet key lives in ~/.makepad/ai-hub/admin/fleet.key (0600) and on
//! every node. Credentials are secrets: written to files, never printed.
//! See tools/aihub-fleet.md.

use makepad_ai_hub::fleet_auth::{client_dir, now_secs, FleetKey, Role, FLEET_KEY, NODE_CREDENTIAL, REVOKED};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn admin_dir() -> PathBuf {
    client_dir().join("admin")
}

fn usage() -> ExitCode {
    eprintln!("usage: ai-fleet init | issue <client_id> <lan|node|device> --out <file> [--days N] | node-files <name> --out <dir> [--days N] | revoke <client_id>");
    ExitCode::from(2)
}

fn log_issue(id: &str, role: Role, expiry: u64) {
    let line = format!("{} issued {id} {} expires {expiry}\n", now_secs(), role.as_str());
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(admin_dir().join("issued.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let days: u64 = opt("--days").and_then(|d| d.parse().ok()).unwrap_or(365);
    let expiry = now_secs() + days * 86400;
    let key_path = admin_dir().join(FLEET_KEY);
    let load = || FleetKey::load(&key_path).map_err(|e| format!("{}: {e} (run `ai-fleet init`)", key_path.display()));
    let result: Result<(), String> = (|| match args.first().map(String::as_str) {
        Some("init") => {
            if key_path.exists() {
                return Err(format!("{} already exists", key_path.display()));
            }
            FleetKey::generate().and_then(|k| k.save(&key_path)).map_err(|e| e.to_string())?;
            println!("fleet key created in {}", key_path.display());
            Ok(())
        }
        Some("issue") if args.len() >= 3 => {
            let role = Role::parse(&args[2]).ok_or("role is lan, node or device")?;
            let out = opt("--out").ok_or("--out <file> is required (credentials are never printed)")?;
            let text = load()?.issue(&args[1], role, expiry).map_err(|e| e.to_string())?;
            makepad_network::tls::write_private(Path::new(&out), format!("{text}\n").as_bytes()).map_err(|e| e.to_string())?;
            log_issue(&args[1], role, expiry);
            println!("credential for {} ({}) written to {out}", args[1], role.as_str());
            Ok(())
        }
        Some("node-files") if args.len() >= 2 => {
            let out = PathBuf::from(opt("--out").ok_or("--out <dir> is required")?);
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            let key = load()?;
            key.save(&out.join(FLEET_KEY)).map_err(|e| e.to_string())?;
            let id = format!("node-{}", args[1]);
            let text = key.issue(&id, Role::Node, expiry).map_err(|e| e.to_string())?;
            makepad_network::tls::write_private(&out.join(NODE_CREDENTIAL), format!("{text}\n").as_bytes()).map_err(|e| e.to_string())?;
            log_issue(&id, Role::Node, expiry);
            println!("{} and {} for {id} written to {}", FLEET_KEY, NODE_CREDENTIAL, out.display());
            Ok(())
        }
        Some("revoke") if args.len() == 2 => {
            let path = admin_dir().join(REVOKED);
            let mut text = fs::read_to_string(&path).unwrap_or_default();
            if !text.lines().any(|l| l.trim() == args[1]) {
                text.push_str(&format!("{}\n", args[1]));
                fs::create_dir_all(admin_dir()).map_err(|e| e.to_string())?;
                fs::write(&path, &text).map_err(|e| e.to_string())?;
            }
            println!("revoked {}; copy {} to every node's <cache>/fleet/revoked and restart it", args[1], path.display());
            Ok(())
        }
        _ => Err(String::new()),
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => usage(),
        Err(e) => {
            eprintln!("ai-fleet: {e}");
            ExitCode::from(1)
        }
    }
}
