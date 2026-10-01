//! ai-fleet: the fleet authority's tool (admin machine only).
//!
//!   ai-fleet init                                  create the authority key; trust it here
//!   ai-fleet pub                                   print the authority public key (authority.pub)
//!   ai-fleet endorse <fleet> <node_key> <tls_sha256> [--days N]
//!                                                  print a node endorsement (node.endorsement)
//!   ai-fleet issue <client_id> <lan|node|device> --out <file> [--days N]
//!                                                  write a client credential (0600)
//!   ai-fleet revoke <id>                           add a client id or node key to the revocation list
//!
//! The key lives in ~/.makepad/ai-hub/authority (0600). Credentials are
//! bearer secrets: written to files, never printed. See tools/aihub-fleet.md.

use makepad_ai_hub::fleet_auth::{client_dir, now_secs, Authority, Role, AUTHORITY_PUB, REVOKED};
use std::fs;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: ai-fleet init | pub | endorse <fleet> <node_key> <tls_sha256> [--days N] | issue <client_id> <lan|node|device> --out <file> [--days N] | revoke <id>");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let days: u64 = opt("--days").and_then(|d| d.parse().ok()).unwrap_or(365);
    let expiry = now_secs() + days * 86400;
    let dir = Authority::dir();
    let result: Result<(), String> = (|| match args.first().map(String::as_str) {
        Some("init") => {
            let a = Authority::create(&dir).map_err(|e| e.to_string())?;
            fs::create_dir_all(client_dir()).map_err(|e| e.to_string())?;
            fs::write(client_dir().join(AUTHORITY_PUB), format!("{}\n", a.public_hex())).map_err(|e| e.to_string())?;
            println!("authority created in {}; public key in {}", dir.display(), client_dir().join(AUTHORITY_PUB).display());
            Ok(())
        }
        Some("pub") => {
            println!("{}", Authority::load(&dir).map_err(|e| e.to_string())?.public_hex());
            Ok(())
        }
        Some("endorse") if args.len() >= 4 => {
            let fp = makepad_network::tls::from_hex::<32>(&args[3]).ok_or("tls_sha256 must be 64 hex digits")?;
            let a = Authority::load(&dir).map_err(|e| e.to_string())?;
            println!("{}", a.endorse(&args[1], &args[2], &fp, expiry).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some("issue") if args.len() >= 3 => {
            let role = Role::parse(&args[2]).ok_or("role is lan, node or device")?;
            let out = opt("--out").ok_or("--out <file> is required (credentials are never printed)")?;
            let a = Authority::load(&dir).map_err(|e| e.to_string())?;
            let token = a.issue(&args[1], role, expiry).map_err(|e| e.to_string())?;
            makepad_network::tls::write_private(std::path::Path::new(&out), format!("{token}\n").as_bytes()).map_err(|e| e.to_string())?;
            let log = dir.join("issued.log");
            let line = format!("{} issued {} {} expires {}\n", now_secs(), args[1], role.as_str(), expiry);
            let _ = fs::OpenOptions::new().create(true).append(true).open(log).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
            println!("credential for {} ({}) written to {out}", args[1], role.as_str());
            Ok(())
        }
        Some("revoke") if args.len() == 2 => {
            let path = dir.join(REVOKED);
            let mut text = fs::read_to_string(&path).unwrap_or_default();
            if !text.lines().any(|l| l.trim() == args[1]) {
                text.push_str(&format!("{}\n", args[1]));
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
