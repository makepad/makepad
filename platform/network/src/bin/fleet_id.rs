//! `fleet-id`: the fleet identity tool (tools/fleet-identity.md). Nothing it
//! prints is secret: keys go to 0600 files only.
//!
//! On the admin Mac:
//!   fleet-id init                                     the root key (admin/root.key) + root.pub
//!   fleet-id issue <name> <role> --out <dir> [--from <ip>] [--days <n>]
//!                                                     a machine's files (id.key, id.cred, root.pub, revoked)
//!                                                     in a fresh 0700 <dir>, to ship over ssh
//!   fleet-id sign <name> <role> <public point hex> [--from <ip>] [--days <n>]
//!                                                     a credential for a key made on its machine (prints it)
//!   fleet-id revoke <name>                            adds the name to this machine's `revoked`
//! On any machine:
//!   fleet-id keygen                                   makes id.key if missing; prints its public point
//!   fleet-id install <credential>                     writes id.cred (must match id.key and root.pub)
//!   fleet-id show                                     this machine's credential and whether it checks out
//!
//! The fleet dir is `MAKEPAD_FLEET_DIR`, else `~/.makepad/fleet`.

use makepad_network::fleet::id::{self, Credential, Fleet, Me, Role, Root};
use makepad_network::tls::{from_hex, to_hex};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("fleet-id: {e}");
        std::process::exit(1);
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn expiry(args: &[String], default_days: u64) -> Result<u64, String> {
    let days: u64 = flag(args, "--days").map(|d| d.parse().map_err(|_| "--days: a number")).transpose()?.unwrap_or(default_days);
    Ok(id::now_secs() + days * 86_400)
}

fn from(args: &[String]) -> Result<Option<IpAddr>, String> {
    flag(args, "--from").map(|ip| ip.parse::<IpAddr>().map_err(|_| format!("--from {ip}: not an address"))).transpose()
}

fn role(text: &str) -> Result<Role, String> {
    Role::parse(text).ok_or_else(|| "role is device, node or paired".to_string())
}

fn root() -> Result<Root, String> {
    Root::load(&id::admin_dir()).map_err(|e| format!("no root key in {} ({e}): run this on the admin Mac", id::admin_dir().display()))
}

fn copy_revoked(to: &Path) -> Result<(), String> {
    let revoked = std::fs::read_to_string(id::dir().join(id::REVOKED)).unwrap_or_default();
    std::fs::write(to.join(id::REVOKED), revoked).map_err(|e| e.to_string())
}

/// A machine credential lasts ten years by default: machines are revoked by name, not left to expire.
const DEFAULT_DAYS: u64 = 3650;

fn run(args: &[String]) -> Result<(), String> {
    let dir = id::dir();
    match args.first().map(String::as_str) {
        Some("init") => {
            if id::admin_dir().join(id::ROOT_KEY).exists() {
                return Err(format!("{} exists already", id::admin_dir().join(id::ROOT_KEY).display()));
            }
            let root = Root::generate().map_err(|e| e.to_string())?;
            root.save(&id::admin_dir()).map_err(|e| e.to_string())?;
            id::write_root_pub(&dir, &root.point()).map_err(|e| e.to_string())?;
            println!("root key in {}; root.pub {}", id::admin_dir().display(), to_hex(&root.point()));
        }
        Some("issue") => {
            let name = args.get(1).ok_or("issue <name> <role> --out <dir>")?;
            let role = role(args.get(2).ok_or("issue <name> <role> --out <dir>")?)?;
            let out = PathBuf::from(flag(args, "--out").ok_or("--out <dir> (a new directory)")?);
            let root = root()?;
            std::fs::create_dir(&out).map_err(|e| format!("{}: {e} (give a new directory)", out.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
            }
            let key = id::generate_key().map_err(|e| e.to_string())?;
            let cred = root.issue(name, role, expiry(args, DEFAULT_DAYS)?, from(args)?, &id::point_of(&key)).map_err(|e| e.to_string())?;
            id::write_key(&out.join(id::ID_KEY), &key).map_err(|e| e.to_string())?;
            std::fs::write(out.join(id::ID_CRED), format!("{}\n", cred.text())).map_err(|e| e.to_string())?;
            id::write_root_pub(&out, &root.point()).map_err(|e| e.to_string())?;
            copy_revoked(&out)?;
            println!("{:?} -> {} (id.key, id.cred, root.pub, revoked)", cred, out.display());
        }
        Some("sign") => {
            let [_, name, role_text, point] = args.get(..4).ok_or("sign <name> <role> <public point hex>")? else { unreachable!() };
            let point = from_hex::<65>(point).ok_or("the public point is 65 bytes of hex (fleet-id keygen prints it)")?;
            let cred = root()?.issue(name, role(role_text)?, expiry(args, DEFAULT_DAYS)?, from(args)?, &point).map_err(|e| e.to_string())?;
            println!("{}", cred.text());
        }
        Some("revoke") => {
            let name = args.get(1).ok_or("revoke <name>")?;
            if !id::valid_name(name) {
                return Err("not a credential name".into());
            }
            let path = dir.join(id::REVOKED);
            let mut text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.lines().any(|l| l.trim() == name) {
                println!("{name} is revoked already");
                return Ok(());
            }
            text.push_str(&format!("{name}\n"));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(&path, text).map_err(|e| e.to_string())?;
            println!("{name} revoked in {}; ship that file to every machine and restart their services", path.display());
        }
        Some("keygen") => {
            let path = dir.join(id::ID_KEY);
            let key = match id::read_key(&path) {
                Ok(key) => key,
                Err(_) if !path.exists() => {
                    let key = id::generate_key().map_err(|e| e.to_string())?;
                    id::write_key(&path, &key).map_err(|e| e.to_string())?;
                    key
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            println!("{}", to_hex(&id::point_of(&key)));
        }
        Some("install") => {
            let text = args.get(1).ok_or("install <credential>")?;
            let cred = Credential::parse(text).ok_or("not a fleet credential")?;
            let key = id::read_key(&dir.join(id::ID_KEY)).map_err(|e| format!("{}: {e} (fleet-id keygen first)", dir.join(id::ID_KEY).display()))?;
            let me = Me::new(key, cred)?;
            let fleet = Fleet::load(&dir)?;
            fleet.check_credential(&me.cred.text(), id::now_secs()).map_err(|e| format!("credential: {e}"))?;
            std::fs::write(dir.join(id::ID_CRED), format!("{}\n", me.cred.text())).map_err(|e| e.to_string())?;
            println!("installed {:?}", me.cred);
        }
        Some("show") => {
            let me = Me::load(&dir)?;
            let status = match Fleet::load(&dir) {
                Ok(fleet) => match fleet.check_credential(&me.cred.text(), id::now_secs()) {
                    Ok(_) => "valid in this fleet".to_string(),
                    Err(e) => format!("INVALID: {e}"),
                },
                Err(e) => e,
            };
            println!("{:?}: {status}", me.cred);
        }
        _ => return Err("usage: fleet-id init | issue | sign | revoke | keygen | install | show (see the source header)".into()),
    }
    Ok(())
}
