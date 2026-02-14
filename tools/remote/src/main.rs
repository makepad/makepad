use std::env;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

// --- Protocol tags ---

const TAG_FILE_DATA: u8 = 0x01;
const TAG_CARGO_RUN: u8 = 0x02;

const TAG_OUTPUT: u8 = 0x01;
const TAG_EXIT_CODE: u8 = 0x02;
const TAG_ERROR: u8 = 0x03;

const STREAM_STDOUT: u8 = 1;
const STREAM_STDERR: u8 = 2;

// --- Protocol helpers ---

fn write_u32(w: &mut dyn Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

fn read_u32(r: &mut dyn Read) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

fn write_msg(w: &mut dyn Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    w.write_all(&[tag])?;
    write_u32(w, payload.len() as u32)?;
    w.write_all(payload)?;
    w.flush()
}

fn read_msg(r: &mut dyn Read) -> io::Result<(u8, Vec<u8>)> {
    let mut tag_buf = [0u8; 1];
    r.read_exact(&mut tag_buf)?;
    let len = read_u32(r)? as usize;
    let mut payload = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut payload)?;
    }
    Ok((tag_buf[0], payload))
}

// --- File data encoding/decoding ---

fn encode_file_data(rel_path: &str, data: &[u8]) -> Vec<u8> {
    let path_bytes = rel_path.as_bytes();
    let mut buf = Vec::with_capacity(4 + path_bytes.len() + data.len());
    buf.extend_from_slice(&(path_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(path_bytes);
    buf.extend_from_slice(data);
    buf
}

fn decode_file_data(payload: &[u8]) -> io::Result<(&str, &[u8])> {
    if payload.len() < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file data too short",
        ));
    }
    let path_len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    if payload.len() < 4 + path_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file data truncated",
        ));
    }
    let path = std::str::from_utf8(&payload[4..4 + path_len])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let data = &payload[4 + path_len..];
    Ok((path, data))
}

// --- Server ---

fn run_server(port: u16) -> io::Result<()> {
    let cwd = env::current_dir()?.canonicalize()?;
    eprintln!("server: cwd = {}", cwd.display());

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = TcpListener::bind(addr)?;
    eprintln!("server: listening on {}", addr);

    let cargo_child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("server: accept error: {}", e);
                continue;
            }
        };
        let peer = stream.peer_addr().ok();
        eprintln!("server: connection from {:?}", peer);

        let cwd = cwd.clone();
        let cargo_child = cargo_child.clone();
        thread::spawn(move || {
            if let Err(e) = handle_connection(stream, &cwd, &cargo_child) {
                eprintln!("server: connection error: {}", e);
            }
        });
    }
    Ok(())
}

fn validate_and_resolve_path(cwd: &Path, rel_path: &str) -> io::Result<PathBuf> {
    let rel = Path::new(rel_path);

    // Reject absolute paths
    if rel.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "absolute paths not allowed",
        ));
    }

    // Reject paths with .. components
    for component in rel.components() {
        if let std::path::Component::ParentDir = component {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                ".. not allowed in paths",
            ));
        }
    }

    let full = cwd.join(rel);

    // Ensure parent dir exists so we can canonicalize it
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(cwd) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("path escapes working directory: {}", rel_path),
            ));
        }
    }

    Ok(full)
}

fn kill_cargo(cargo_child: &Mutex<Option<Child>>) {
    let mut lock = cargo_child.lock().unwrap();
    if let Some(ref mut child) = *lock {
        eprintln!("server: killing previous cargo (pid {})", child.id());
        let _ = child.kill();
        let _ = child.wait();
    }
    *lock = None;
}

fn handle_connection(
    mut stream: TcpStream,
    cwd: &Path,
    cargo_child: &Arc<Mutex<Option<Child>>>,
) -> io::Result<()> {
    // Read messages until we get CargoRun
    let cargo_args;
    loop {
        let (tag, payload) = read_msg(&mut stream)?;
        match tag {
            TAG_FILE_DATA => {
                let (rel_path, data) = decode_file_data(&payload)?;
                let full_path = validate_and_resolve_path(cwd, rel_path)?;
                fs::write(&full_path, data)?;
                eprintln!("server: wrote {} ({} bytes)", rel_path, data.len());
            }
            TAG_CARGO_RUN => {
                let args_str = String::from_utf8(payload)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                cargo_args = args_str
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect::<Vec<_>>();
                break;
            }
            _ => {
                let msg = format!("unknown tag: 0x{:02x}", tag);
                let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                return Err(io::Error::new(io::ErrorKind::InvalidData, msg));
            }
        }
    }

    eprintln!("server: cargo {}", cargo_args.join(" "));

    // Kill any previous cargo
    kill_cargo(cargo_child);

    // Spawn cargo
    let mut child = Command::new("cargo")
        .args(&cargo_args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let msg = format!("failed to spawn cargo: {}", e);
            let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
            e
        })?;

    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    // Store child so it can be killed by future connections
    {
        let mut lock = cargo_child.lock().unwrap();
        *lock = Some(child);
    }

    // Stream stdout and stderr back to client via two threads
    let stream_out = stream.try_clone()?;
    let stream_err = stream.try_clone()?;

    let stdout_thread = thread::spawn(move || stream_pipe(child_stdout, stream_out, STREAM_STDOUT));
    let stderr_thread = thread::spawn(move || stream_pipe(child_stderr, stream_err, STREAM_STDERR));

    stdout_thread.join().unwrap();
    stderr_thread.join().unwrap();

    // Wait for cargo to finish and get exit code
    let exit_code = {
        let mut lock = cargo_child.lock().unwrap();
        if let Some(ref mut child) = *lock {
            let status = child.wait()?;
            let code = status.code().unwrap_or(1);
            *lock = None;
            code
        } else {
            // Was killed by another connection
            137 // SIGKILL
        }
    };

    eprintln!("server: cargo exited with {}", exit_code);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(exit_code as i32).to_be_bytes());
    write_msg(&mut stream, TAG_EXIT_CODE, &payload)?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn stream_pipe(reader: impl Read, mut writer: TcpStream, stream_id: u8) {
    let mut reader = BufReader::new(reader);
    let mut buf = vec![0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                // Also print on server's own stdout/stderr
                match stream_id {
                    STREAM_STDOUT => {
                        let _ = io::stdout().write_all(chunk);
                    }
                    STREAM_STDERR => {
                        let _ = io::stderr().write_all(chunk);
                    }
                    _ => {}
                }
                let mut payload = Vec::with_capacity(1 + n);
                payload.push(stream_id);
                payload.extend_from_slice(chunk);
                if write_msg(&mut writer, TAG_OUTPUT, &payload).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

// --- Client ---

fn run_client(addr: &str, cargo_args: &[String]) -> io::Result<i32> {
    // Get changed files from git
    let files = get_changed_files()?;

    eprintln!("client: connecting to {}", addr);
    let mut stream = TcpStream::connect(addr)?;
    eprintln!("client: connected");

    // Send changed files
    for (rel_path, is_tracked) in &files {
        // Filter: skip local/ directory
        if rel_path.starts_with("local/") || rel_path.starts_with("local\\") {
            continue;
        }
        // Filter: skip files in repo root (no path separator)
        if !rel_path.contains('/') {
            continue;
        }
        // Filter: untracked files — only *.rs
        if !is_tracked && !rel_path.ends_with(".rs") {
            continue;
        }

        let data = match fs::read(rel_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("client: skip {}: {}", rel_path, e);
                continue;
            }
        };
        eprintln!("client: sending {} ({} bytes)", rel_path, data.len());
        let payload = encode_file_data(rel_path, &data);
        write_msg(&mut stream, TAG_FILE_DATA, &payload)?;
    }

    // Send cargo run command (also signals end of files)
    let args_str = cargo_args.join("\n");
    write_msg(&mut stream, TAG_CARGO_RUN, args_str.as_bytes())?;
    eprintln!("client: cargo {}", cargo_args.join(" "));

    // Read output from server
    let exit_code;
    loop {
        let (tag, payload) = read_msg(&mut stream)?;
        match tag {
            TAG_OUTPUT => {
                if payload.is_empty() {
                    continue;
                }
                let stream_id = payload[0];
                let data = &payload[1..];
                match stream_id {
                    STREAM_STDOUT => {
                        io::stdout().write_all(data)?;
                        io::stdout().flush()?;
                    }
                    STREAM_STDERR => {
                        io::stderr().write_all(data)?;
                        io::stderr().flush()?;
                    }
                    _ => {}
                }
            }
            TAG_EXIT_CODE => {
                if payload.len() >= 4 {
                    exit_code =
                        i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                } else {
                    exit_code = 1;
                }
                break;
            }
            TAG_ERROR => {
                let msg = String::from_utf8_lossy(&payload);
                eprintln!("server error: {}", msg);
                exit_code = 1;
                break;
            }
            _ => {
                eprintln!("client: unknown tag 0x{:02x}", tag);
                exit_code = 1;
                break;
            }
        }
    }

    let _ = stream.shutdown(Shutdown::Both);
    Ok(exit_code)
}

/// Returns list of (relative_path, is_tracked) for changed/untracked files.
fn get_changed_files() -> io::Result<Vec<(String, bool)>> {
    let mut files = Vec::new();

    // Tracked files that have been modified (unstaged + staged)
    let output = Command::new("git").args(["diff", "--name-only"]).output()?;
    if output.status.success() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                files.push((line.to_string(), true));
            }
        }
    }

    // Staged changes (in case of files that are staged but also show in diff above — dedup later)
    let output = Command::new("git")
        .args(["diff", "--name-only", "--cached"])
        .output()?;
    if output.status.success() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                files.push((line.to_string(), true));
            }
        }
    }

    // Untracked files
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()?;
    if output.status.success() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                files.push((line.to_string(), false));
            }
        }
    }

    // Deduplicate by path, preferring tracked
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);

    Ok(files)
}

// --- Main ---

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  Server: makepad-remote --server [--port PORT]");
    eprintln!("  Client: makepad-remote <ip:port> cargo [args...]");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        return ExitCode::from(1);
    }

    if args[0] == "--server" {
        let mut port: u16 = 8384;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--port" && i + 1 < args.len() {
                port = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("invalid port: {}", args[i + 1]);
                    std::process::exit(1);
                });
                i += 2;
            } else {
                eprintln!("unknown server option: {}", args[i]);
                print_usage();
                std::process::exit(1);
            }
        }
        if let Err(e) = run_server(port) {
            eprintln!("server error: {}", e);
            return ExitCode::from(1);
        }
        ExitCode::SUCCESS
    } else {
        // Client mode: <addr> cargo [args...]
        let addr = &args[0];

        if args.len() < 2 || args[1] != "cargo" {
            eprintln!("client mode requires: <ip:port> cargo [args...]");
            print_usage();
            return ExitCode::from(1);
        }

        let cargo_args = &args[2..];

        match run_client(addr, cargo_args) {
            Ok(code) => {
                if code == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(code as u8)
                }
            }
            Err(e) => {
                eprintln!("client error: {}", e);
                ExitCode::from(1)
            }
        }
    }
}
