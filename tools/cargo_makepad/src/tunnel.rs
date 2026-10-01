//! `cargo makepad tunnel`: the client of the makepad-remote tunnel server
//! (tools/remote). TLS with a pinned server certificate and a per-box key;
//! see makepad_network::tunnel and tools/remote/TUNNEL.md. The server is
//! `makepad-remote --server`.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use makepad_network::tunnel::{
    connect, decode_file_data, encode_file_data, host_of, read_msg, write_msg, TunnelConn,
    DEFAULT_PORT, STREAM_STDERR, TAG_ADMIN, TAG_CARGO_RUN, TAG_ERROR, TAG_EXIT_CODE,
    TAG_FILE_DATA, TAG_FILE_PULL, TAG_KILL, TAG_OUTPUT, TAG_PS, TAG_SHELL_RUN, TAG_SPAWN,
};

fn open(addr: &str) -> io::Result<TunnelConn> {
    if host_of(addr).len() < addr.len() {
        connect(addr)
    } else {
        connect(&format!("{addr}:{DEFAULT_PORT}"))
    }
}

/// Reads frames until the exit code, printing output as it arrives.
fn read_until_exit(conn: &mut TunnelConn) -> io::Result<i32> {
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

fn finish(mut conn: TunnelConn) -> io::Result<i32> {
    let code = read_until_exit(&mut conn);
    conn.shutdown();
    code
}

fn run_simple(addr: &str, tag: u8, payload: &[u8]) -> io::Result<i32> {
    let mut conn = open(addr)?;
    write_msg(&mut conn, tag, payload)?;
    finish(conn)
}

fn run_client(addr: &str, cmd_args: &[String], is_shell: bool, sync_files: bool) -> io::Result<i32> {
    let mut conn = open(addr)?;
    if sync_files {
        for (rel_path, is_tracked) in get_changed_files()? {
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
            let data = match fs::read(&rel_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("client: skip {rel_path}: {e}");
                    continue;
                }
            };
            eprintln!("client: sending {rel_path} ({} bytes)", data.len());
            write_msg(&mut conn, TAG_FILE_DATA, &encode_file_data(&rel_path, &data))?;
        }
    } else {
        eprintln!("client: file sync disabled (--no-sync)");
    }
    let tag = if is_shell { TAG_SHELL_RUN } else { TAG_CARGO_RUN };
    write_msg(&mut conn, tag, cmd_args.join("\n").as_bytes())?;
    finish(conn)
}

fn run_pull(addr: &str, remote_path: &str, local_path: &str) -> io::Result<i32> {
    let mut conn = open(addr)?;
    eprintln!("client: pull {remote_path} -> {local_path}");
    write_msg(&mut conn, TAG_FILE_PULL, remote_path.as_bytes())?;
    let mut exit_code = 1;
    loop {
        let (tag, payload) = read_msg(&mut conn)?;
        match tag {
            TAG_FILE_DATA => {
                let (_rel, data) = decode_file_data(&payload)?;
                if let Some(parent) = Path::new(local_path).parent() {
                    if !parent.as_os_str().is_empty() {
                        fs::create_dir_all(parent)?;
                    }
                }
                fs::write(local_path, data)?;
                eprintln!("client: wrote {local_path} ({} bytes)", data.len());
                exit_code = 0;
            }
            TAG_EXIT_CODE => break,
            TAG_ERROR => {
                eprintln!("server error: {}", String::from_utf8_lossy(&payload));
                exit_code = 1;
                break;
            }
            other => {
                eprintln!("client: unknown tag 0x{other:02x}");
                exit_code = 1;
                break;
            }
        }
    }
    conn.shutdown();
    Ok(exit_code)
}

fn run_push(addr: &str, local_path: &str, remote_path: &str) -> io::Result<i32> {
    let data = fs::read(local_path)?;
    let mut conn = open(addr)?;
    eprintln!("client: push {local_path} -> {remote_path} ({} bytes)", data.len());
    write_msg(&mut conn, TAG_FILE_DATA, &encode_file_data(remote_path, &data))?;
    // An echo after the write gives a positive confirmation and exit code.
    write_msg(&mut conn, TAG_SHELL_RUN, b"echo push-ok")?;
    finish(conn)
}

/// Pushes a local script and runs it in one connection: TAG_FILE_DATA
/// stages it in the server's working directory, TAG_SHELL_RUN starts the
/// interpreter. Each run stages under a fresh name.
fn run_script(addr: &str, local_script: &str, script_args: &[String]) -> io::Result<i32> {
    let data = fs::read(local_script)?;
    let ext = Path::new(local_script)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let stamp = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let (staged, interpreter) = match ext.as_str() {
        "ps1" => {
            let s = format!("_tunnel_staged_{stamp}.ps1");
            let i = format!("powershell -NoProfile -ExecutionPolicy Bypass -File {s}");
            (s, i)
        }
        "py" => {
            let s = format!("_tunnel_staged_{stamp}.py");
            let i = format!("python {s}");
            (s, i)
        }
        "bat" | "cmd" => {
            let s = format!("_tunnel_staged_{stamp}.bat");
            (s.clone(), s)
        }
        "sh" => {
            let s = format!("_tunnel_staged_{stamp}.sh");
            let i = format!("sh {s}");
            (s, i)
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("run: unsupported script extension .{other} (ps1/py/bat/sh)"),
            ))
        }
    };
    let mut shell_line = interpreter;
    for a in script_args {
        shell_line.push(' ');
        shell_line.push_str(a);
    }
    let mut conn = open(addr)?;
    eprintln!("client: run {local_script} ({} bytes) as `{shell_line}`", data.len());
    write_msg(&mut conn, TAG_FILE_DATA, &encode_file_data(&staged, &data))?;
    write_msg(&mut conn, TAG_SHELL_RUN, shell_line.as_bytes())?;
    finish(conn)
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

fn print_usage() {
    eprintln!("Usage (server: makepad-remote --server; keys and pins: makepad-remote keygen|pin|rotate):");
    eprintln!("  cargo makepad tunnel <host[:port]> [--no-sync] cargo [args...]");
    eprintln!("  cargo makepad tunnel <host[:port]> [--no-sync] shell <command...>   (server --all)");
    eprintln!("  cargo makepad tunnel <host[:port]> pull <remote-rel-path> <local-path>");
    eprintln!("  cargo makepad tunnel <host[:port]> push <local-path> <remote-rel-path>");
    eprintln!("  cargo makepad tunnel <host[:port]> [--no-sync] run <script.(ps1|py|bat|sh)> [args...]");
    eprintln!("  cargo makepad tunnel <host[:port]> spawn <command...>");
    eprintln!("  cargo makepad tunnel <host[:port]> ps [filter]");
    eprintln!("  cargo makepad tunnel <host[:port]> kill [--tree] <pid>");
    eprintln!("  cargo makepad tunnel <host[:port]> admin <node-status|node-stop|node-start|node-restart|tunnel-restart>");
}

fn exit_on(result: io::Result<i32>) -> Result<(), String> {
    match result {
        Ok(0) => Ok(()),
        Ok(code) => std::process::exit(u8::try_from(code).unwrap_or(1).max(1) as i32),
        Err(e) => Err(format!("client error: {e}")),
    }
}

pub fn handle_tunnel(args: &[String]) -> Result<(), String> {
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_usage();
        return if args.is_empty() { Err("tunnel mode requires arguments".into()) } else { Ok(()) };
    }
    if args[0] == "--server" {
        return Err("the tunnel server is `makepad-remote --server` (tools/remote)".into());
    }
    let addr = &args[0];
    let mut sync_files = true;
    let mut i = 1;
    while i < args.len() && args[i] == "--no-sync" {
        sync_files = false;
        i += 1;
    }
    let Some(mode) = args.get(i) else {
        print_usage();
        return Err("missing tunnel command".into());
    };
    let rest = &args[i + 1..];
    match mode.as_str() {
        "pull" if rest.len() == 2 => exit_on(run_pull(addr, &rest[0], &rest[1])),
        "push" if rest.len() == 2 => exit_on(run_push(addr, &rest[0], &rest[1])),
        "run" if !rest.is_empty() => exit_on(run_script(addr, &rest[0], &rest[1..])),
        "spawn" if !rest.is_empty() => exit_on(run_simple(addr, TAG_SPAWN, rest.join(" ").as_bytes())),
        "ps" => exit_on(run_simple(addr, TAG_PS, rest.first().map(String::as_str).unwrap_or("").as_bytes())),
        "kill" => {
            let tree = rest.iter().any(|a| a == "--tree");
            let Some(pid) = rest.iter().find(|a| *a != "--tree") else {
                print_usage();
                return Err("kill requires a pid".into());
            };
            let payload = if tree { format!("{pid}\ntree") } else { pid.clone() };
            exit_on(run_simple(addr, TAG_KILL, payload.as_bytes()))
        }
        "admin" if rest.len() == 1 => exit_on(run_simple(addr, TAG_ADMIN, rest[0].as_bytes())),
        "cargo" => exit_on(run_client(addr, rest, false, sync_files)),
        "shell" => exit_on(run_client(addr, rest, true, sync_files)),
        _ => {
            print_usage();
            Err("client mode requires: <host[:port]> [--no-sync] cargo|shell|pull|push|run|spawn|ps|kill|admin".into())
        }
    }
}
