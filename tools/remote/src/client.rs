//! Tunnel client commands and key management.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use makepad_network::tls::{from_hex, to_hex};
use makepad_network::tunnel::*;

/// `host` or `host:port`, with the default port added.
pub fn with_port(addr: &str) -> String {
    if host_of(addr).len() < addr.len() {
        addr.to_string()
    } else {
        format!("{addr}:{DEFAULT_PORT}")
    }
}

/// Reads frames until the exit code; prints output as it arrives.
pub fn read_until_exit(conn: &mut TunnelConn) -> io::Result<i32> {
    loop {
        let (tag, payload) = read_msg(conn)?;
        match tag {
            TAG_OUTPUT => {
                let Some((&stream, data)) = payload.split_first() else { continue };
                if stream == STREAM_STDERR {
                    io::stderr().write_all(data)?;
                    io::stderr().flush()?;
                } else {
                    io::stdout().write_all(data)?;
                    io::stdout().flush()?;
                }
            }
            TAG_EXIT_CODE => {
                return Ok(if payload.len() >= 4 {
                    i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]])
                } else {
                    1
                })
            }
            TAG_ERROR => {
                eprintln!("server error: {}", String::from_utf8_lossy(&payload));
                return Ok(1);
            }
            other => {
                eprintln!("client: unknown tag 0x{other:02x}");
                return Ok(1);
            }
        }
    }
}

pub fn run_simple(addr: &str, tag: u8, payload: &[u8]) -> io::Result<i32> {
    let mut conn = connect(&with_port(addr))?;
    write_msg(&mut conn, tag, payload)?;
    let code = read_until_exit(&mut conn);
    conn.shutdown();
    code
}

pub fn run_client(addr: &str, cmd_args: &[String], is_shell: bool) -> io::Result<i32> {
    let files = get_changed_files()?;
    let mut conn = connect(&with_port(addr))?;
    for (rel_path, is_tracked) in &files {
        if rel_path.starts_with("local/") || rel_path.starts_with("local\\") || !rel_path.contains('/') {
            continue;
        }
        let normalized = rel_path.replace('\\', "/");
        let is_vendored = normalized.starts_with("libs/linux/");
        let is_common_src = normalized.ends_with(".rs") || normalized.ends_with(".toml");
        if !is_tracked {
            if is_vendored {
                let in_src = normalized.contains("/src/") && !normalized.contains("/src/test/");
                let keep = normalized.ends_with("/Cargo.toml")
                    || normalized.ends_with("/build.rs")
                    || in_src
                    || normalized.ends_with("/wayland.xml")
                    || (normalized.contains("/protocols/") && normalized.ends_with(".xml"));
                if !keep {
                    continue;
                }
            } else if !is_common_src {
                continue;
            }
        }
        let data = match fs::read(rel_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("client: skip {rel_path}: {e}");
                continue;
            }
        };
        eprintln!("client: sending {rel_path} ({} bytes)", data.len());
        write_msg(&mut conn, TAG_FILE_DATA, &encode_file_data(rel_path, &data))?;
    }
    let tag = if is_shell { TAG_SHELL_RUN } else { TAG_CARGO_RUN };
    write_msg(&mut conn, tag, cmd_args.join("\n").as_bytes())?;
    let code = read_until_exit(&mut conn);
    conn.shutdown();
    code
}

fn get_changed_files() -> io::Result<Vec<(String, bool)>> {
    let mut files = Vec::new();
    for (args, tracked) in [
        (&["diff", "--name-only"][..], true),
        (&["diff", "--name-only", "--cached"][..], true),
        (&["ls-files", "--others", "--exclude-standard"][..], false),
    ] {
        let output = Command::new("git").args(args).output()?;
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let line = line.trim();
                if !line.is_empty() {
                    files.push((line.to_string(), tracked));
                }
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    Ok(files)
}

pub fn pull(addr: &str, remote_path: &str, local_path: &str) -> io::Result<i32> {
    let mut conn = connect(&with_port(addr))?;
    write_msg(&mut conn, TAG_FILE_PULL, remote_path.as_bytes())?;
    let mut code = 1;
    loop {
        let (tag, payload) = read_msg(&mut conn)?;
        match tag {
            TAG_FILE_DATA => {
                let (_, data) = decode_file_data(&payload)?;
                fs::write(local_path, data)?;
                eprintln!("client: wrote {local_path} ({} bytes)", data.len());
                code = 0;
            }
            TAG_EXIT_CODE => break,
            _ => {
                eprintln!("server error: {}", String::from_utf8_lossy(&payload));
                code = 1;
                break;
            }
        }
    }
    conn.shutdown();
    Ok(code)
}

/// Writes one file into the server's working directory; the echo after it
/// confirms the write (needs a server started with --all).
pub fn push(addr: &str, local_path: &str, remote_path: &str) -> io::Result<i32> {
    let data = fs::read(local_path)?;
    let mut conn = connect(&with_port(addr))?;
    write_msg(&mut conn, TAG_FILE_DATA, &encode_file_data(remote_path, &data))?;
    write_msg(&mut conn, TAG_SHELL_RUN, b"echo push-ok")?;
    let code = read_until_exit(&mut conn);
    conn.shutdown();
    code
}

// --- key management -----------------------------------------------------------

/// A new key for `host`: stored first in this user's key file, and written
/// as a one-key server file to `<tunnel dir>/outbox/<host>.keys` for the
/// box. Never printed.
pub fn keygen(host: &str) -> io::Result<()> {
    let dir = tunnel_dir();
    let key = Psk::generate()?;
    let psk_path = dir.join("psk");
    if !get_entries(&psk_path, host).is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{host} already has a key; use `rotate {host}` to replace it over the tunnel"),
        ));
    }
    set_entries(&psk_path, host, &[key.to_hex()], true)?;
    let outbox = dir.join("outbox");
    private_dir(&dir)?;
    private_dir(&outbox)?;
    let out = outbox.join(format!("{host}.keys"));
    makepad_network::tls::write_private(&out, key_file_text(&[key.clone()]).as_bytes())?;
    println!("key {} for {host}", key.id_hex());
    println!("server key file: {} (install it as the box's tunnel keys file, then delete it here)", out.display());
    Ok(())
}

fn private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn set_pin(host: &str, hex: &str) -> io::Result<()> {
    let pin = from_hex::<32>(hex).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "pin must be 64 hex digits"))?;
    set_entries(&tunnel_dir().join("pins"), host, &[to_hex(&pin)], false)?;
    println!("pinned {host} to {}", to_hex(&pin));
    Ok(())
}

fn set_server_keys(addr: &str, keys: &[Psk]) -> io::Result<()> {
    let mut conn = connect(addr)?;
    write_msg(&mut conn, TAG_SET_KEYS, key_file_text(keys).as_bytes())?;
    let code = read_until_exit(&mut conn)?;
    conn.shutdown();
    if code != 0 {
        return Err(io::Error::other("server refused the new key set"));
    }
    Ok(())
}

/// Replaces the key for `addr` over the tunnel without a window in which
/// either side could lock the other out: the server first accepts both
/// keys, the client switches and proves the new key, then the old key is
/// dropped on both sides.
pub fn rotate(addr: &str) -> io::Result<()> {
    let addr = with_port(addr);
    let host = host_of(&addr).to_string();
    let psk_path = tunnel_dir().join("psk");
    let old: Vec<Psk> = get_entries(&psk_path, &host).iter().filter_map(|k| Psk::from_hex(k)).collect();
    let Some(current) = old.first().cloned() else {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no key for {host} to rotate")));
    };
    let next = Psk::generate()?;
    // 1. Server accepts next + current (sent over a session using current).
    set_server_keys(&addr, &[next.clone(), current.clone()])?;
    // 2. Client tries next first, keeps current as the fallback.
    set_entries(&psk_path, &host, &[next.to_hex(), current.to_hex()], true)?;
    // 3. Prove next works, and drop current on the server (over next).
    let creds = ClientCredentials {
        keys: vec![next.clone()],
        known_hosts: Some(makepad_network::tls::KnownHosts::new(tunnel_dir().join("pins"))),
        expect: None,
    };
    {
        let mut conn = connect_with_credentials(&addr, &creds)?;
        write_msg(&mut conn, TAG_SET_KEYS, key_file_text(&[next.clone()]).as_bytes())?;
        if read_until_exit(&mut conn)? != 0 {
            return Err(io::Error::other("server refused to drop the old key; both stay valid"));
        }
        conn.shutdown();
    }
    // 4. Client forgets current.
    set_entries(&psk_path, &host, &[next.to_hex()], true)?;
    println!("rotated {host}: key {} replaced by {}", current.id_hex(), next.id_hex());
    Ok(())
}

/// Installer self-test: authenticates to a local server with a key from
/// the server's own key file and the server's own fingerprint file.
pub fn probe(addr: &str, keys_file: &Path, identity_dir: &Path) -> io::Result<()> {
    let keys = load_server_keys(keys_file)?;
    let fp_text = fs::read_to_string(identity_dir.join(makepad_network::tls::FINGERPRINT_FILE))?;
    let pin = from_hex::<32>(fp_text.trim()).ok_or_else(|| io::Error::other("bad fingerprint file"))?;
    let creds = ClientCredentials { keys, known_hosts: None, expect: Some(pin) };
    let mut conn = connect_with_credentials(&with_port(addr), &creds)?;
    write_msg(&mut conn, TAG_ADMIN, b"probe")?;
    // Any answer (an allowlist refusal) proves TLS, the pin and the key.
    let _ = read_msg(&mut conn)?;
    conn.shutdown();
    println!("probe ok: {addr} (certificate {})", fp_text.trim());
    Ok(())
}
