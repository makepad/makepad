use std::env;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

mod procs;
mod protocol;
use procs::SpawnRecord;
use protocol::*;

// --- Platform-specific process tree killing ---

#[cfg(windows)]
mod process_group {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(lp_job_attributes: *mut u8, lp_name: *const u16) -> *mut u8;
        fn AssignProcessToJobObject(h_job: *mut u8, h_process: *mut u8) -> i32;
        fn TerminateJobObject(h_job: *mut u8, exit_code: u32) -> i32;
        fn CloseHandle(h_object: *mut u8) -> i32;
    }

    pub struct JobHandle(*mut u8);
    unsafe impl Send for JobHandle {}

    impl JobHandle {
        pub fn new() -> io::Result<Self> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                Ok(JobHandle(job))
            }
        }

        pub fn assign(&self, child: &Child) -> io::Result<()> {
            unsafe {
                let proc_handle = child.as_raw_handle() as *mut u8;
                if AssignProcessToJobObject(self.0, proc_handle) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn configure_command(_cmd: &mut Command) {
        // On Windows, job object handles it
    }
}

#[cfg(unix)]
mod process_group {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    pub struct JobHandle(u32);

    impl JobHandle {
        pub fn new() -> io::Result<Self> {
            // pid filled in after spawn
            Ok(JobHandle(0))
        }

        pub fn assign(&mut self, child: &Child) -> io::Result<()> {
            self.0 = child.id();
            Ok(())
        }

        pub fn terminate(&self) {
            if self.0 == 0 {
                return;
            }
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            const SIGKILL: i32 = 9;
            unsafe {
                kill(-(self.0 as i32), SIGKILL);
            }
        }
    }

    pub fn configure_command(cmd: &mut Command) {
        // Set the child as its own process group leader BEFORE exec.
        // This way all children it spawns (rustc, etc.) inherit the group.
        unsafe {
            cmd.pre_exec(|| {
                extern "C" {
                    fn setpgid(pid: i32, pgid: i32) -> i32;
                }
                setpgid(0, 0);
                Ok(())
            });
        }
    }
}

// --- Shared state for the running cargo ---

struct RunningCargo {
    child: Child,
    job: process_group::JobHandle,
    old_stream: TcpStream,
}

type CargoState = Arc<Mutex<Option<RunningCargo>>>;
type SpawnState = Arc<Mutex<Vec<SpawnRecord>>>;

fn kill_previous(state: &CargoState) {
    let mut lock = state.lock().unwrap();
    if let Some(ref mut running) = *lock {
        eprintln!(
            "server: killing previous cargo (pid {})",
            running.child.id()
        );
        // Kill entire process tree first
        running.job.terminate();
        // Wait for the direct child to be reaped
        let _ = running.child.wait();
        // Shutdown old client TCP so pipe writer threads also unblock
        let _ = running.old_stream.shutdown(Shutdown::Both);
    }
    *lock = None;
}

// --- Server ---

fn run_server(port: u16, allow_all: bool) -> io::Result<()> {
    let cwd = env::current_dir()?.canonicalize()?;
    eprintln!("server: cwd = {}", cwd.display());
    if allow_all {
        eprintln!("server: --all enabled, accepting arbitrary shell commands");
    }

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = TcpListener::bind(addr)?;
    eprintln!("server: listening on {}", addr);

    let state: CargoState = Arc::new(Mutex::new(None));
    let spawns: SpawnState = Arc::new(Mutex::new(Vec::new()));

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

        // Kill any previous cargo before handling new connection
        kill_previous(&state);

        let cwd = cwd.clone();
        let state = state.clone();
        let spawns = spawns.clone();
        thread::spawn(move || {
            if let Err(e) = handle_connection(stream, &cwd, &state, &spawns, allow_all) {
                eprintln!("server: connection error: {}", e);
            }
        });
    }
    Ok(())
}

fn validate_and_resolve_path(cwd: &Path, rel_path: &str) -> io::Result<PathBuf> {
    let rel = Path::new(rel_path);

    if rel.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "absolute paths not allowed",
        ));
    }

    for component in rel.components() {
        if let std::path::Component::ParentDir = component {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                ".. not allowed in paths",
            ));
        }
    }

    let full = cwd.join(rel);

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

fn handle_connection(
    mut stream: TcpStream,
    cwd: &Path,
    state: &CargoState,
    spawns: &SpawnState,
    allow_all: bool,
) -> io::Result<()> {
    let run_args: Vec<String>;
    let mut is_shell = false;
    loop {
        let (tag, payload) = read_msg(&mut stream)?;
        match tag {
            TAG_FILE_DATA => {
                let (rel_path, data) = decode_file_data(&payload)?;
                let full_path = validate_and_resolve_path(cwd, rel_path)?;
                fs::write(&full_path, data)?;
                eprintln!("server: wrote {} ({} bytes)", rel_path, data.len());
            }
            TAG_SPAWN => {
                if !allow_all {
                    let msg = "spawn not allowed (server not started with --all)";
                    let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, msg));
                }
                let command = String::from_utf8(payload)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                let command = command.trim();
                if command.is_empty() {
                    let msg = "spawn requires a command";
                    let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, msg));
                }
                match spawn_detached(cwd, command) {
                    Ok((pid, log)) => {
                        if let Ok(mut list) = spawns.lock() {
                            list.retain(|j| procs::is_alive(j.pid));
                            list.push(SpawnRecord {
                                pid,
                                command: command.to_string(),
                                log: log.clone(),
                            });
                        }
                        let reply = format!("pid={pid}\nlog={}\n", log.display());
                        eprintln!("server: spawn pid={pid} log={}", log.display());
                        reply_text(&mut stream, &reply)?;
                    }
                    Err(e) => {
                        let msg = format!("spawn failed: {e}");
                        let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                    }
                }
                return Ok(());
            }
            TAG_PS => {
                if !allow_all {
                    let msg = "ps not allowed (server not started with --all)";
                    let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, msg));
                }
                let filter = String::from_utf8_lossy(&payload);
                let filter = filter.trim();
                match format_process_list(spawns, filter) {
                    Ok(text) => reply_text(&mut stream, &text)?,
                    Err(e) => {
                        let _ = write_msg(
                            &mut stream,
                            TAG_ERROR,
                            format!("ps failed: {e}").as_bytes(),
                        );
                    }
                }
                return Ok(());
            }
            TAG_KILL => {
                if !allow_all {
                    let msg = "kill not allowed (server not started with --all)";
                    let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, msg));
                }
                match parse_kill_payload(&payload) {
                    Ok((pid, tree)) => match do_kill(spawns, pid, tree) {
                        Ok(text) => reply_text(&mut stream, &text)?,
                        Err(e) => {
                            let _ = write_msg(
                                &mut stream,
                                TAG_ERROR,
                                format!("kill failed: {e}").as_bytes(),
                            );
                        }
                    },
                    Err(e) => {
                        let _ = write_msg(&mut stream, TAG_ERROR, e.as_bytes());
                    }
                }
                return Ok(());
            }
            TAG_FILE_PULL => {
                let rel_path = String::from_utf8(payload)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                let full_path = validate_and_resolve_path(cwd, &rel_path)?;
                match fs::read(&full_path) {
                    Ok(data) => {
                        eprintln!("server: pulled {} ({} bytes)", rel_path, data.len());
                        let encoded = encode_file_data(&rel_path, &data);
                        write_msg(&mut stream, TAG_FILE_DATA, &encoded)?;
                        write_msg(&mut stream, TAG_EXIT_CODE, &0i32.to_be_bytes())?;
                    }
                    Err(err) => {
                        let msg = format!("pull {}: {}", rel_path, err);
                        let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                    }
                }
                return Ok(());
            }
            TAG_CARGO_RUN => {
                let args_str = String::from_utf8(payload)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                run_args = args_str
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect();
                break;
            }
            TAG_SHELL_RUN => {
                if !allow_all {
                    let msg = "shell commands not allowed (server not started with --all)";
                    let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, msg));
                }
                let args_str = String::from_utf8(payload)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                run_args = args_str
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect();
                is_shell = true;
                break;
            }
            _ => {
                let msg = format!("unknown tag: 0x{:02x}", tag);
                let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
                return Err(io::Error::new(io::ErrorKind::InvalidData, msg));
            }
        }
    }

    // Kill any previous process (in case it wasn't killed at accept time)
    kill_previous(state);

    // Build command
    let mut cmd = if is_shell {
        eprintln!("server: shell {}", run_args.join(" "));
        let shell_line = run_args.join(" ");
        #[cfg(unix)]
        {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&shell_line);
            c
        }
        #[cfg(windows)]
        {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&shell_line);
            c
        }
    } else {
        eprintln!("server: cargo {}", run_args.join(" "));
        let mut c = Command::new("cargo");
        c.args(&run_args);
        c
    };
    cmd.current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process_group::configure_command(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| {
        let msg = format!("failed to spawn cargo: {}", e);
        let _ = write_msg(&mut stream, TAG_ERROR, msg.as_bytes());
        e
    })?;

    // Create job handle and assign the child to it
    let mut job = process_group::JobHandle::new()?;
    job.assign(&child)?;

    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    // Store everything so a future connection can kill + unblock us
    {
        let mut lock = state.lock().unwrap();
        *lock = Some(RunningCargo {
            child,
            job,
            old_stream: stream.try_clone()?,
        });
    }

    let stream_out = stream.try_clone()?;
    let stream_err = stream.try_clone()?;

    let stdout_thread = thread::spawn(move || stream_pipe(child_stdout, stream_out, STREAM_STDOUT));
    let stderr_thread = thread::spawn(move || stream_pipe(child_stderr, stream_err, STREAM_STDERR));

    stdout_thread.join().unwrap();
    stderr_thread.join().unwrap();

    let exit_code = {
        let mut lock = state.lock().unwrap();
        if let Some(ref mut running) = *lock {
            let status = running.child.wait()?;
            let code = status.code().unwrap_or(1);
            *lock = None;
            code
        } else {
            // Was killed by another connection
            137
        }
    };

    eprintln!("server: cargo exited with {}", exit_code);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(exit_code as i32).to_be_bytes());
    let _ = write_msg(&mut stream, TAG_EXIT_CODE, &payload);
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

fn reply_text(stream: &mut TcpStream, text: &str) -> io::Result<()> {
    let mut payload = Vec::with_capacity(1 + text.len());
    payload.push(STREAM_STDOUT);
    payload.extend_from_slice(text.as_bytes());
    write_msg(stream, TAG_OUTPUT, &payload)?;
    write_msg(stream, TAG_EXIT_CODE, &0i32.to_be_bytes())
}

fn format_process_list(spawns: &SpawnState, filter: &str) -> io::Result<String> {
    let procs = procs::list_processes()?;
    let spawned = spawns.lock().map(|mut list| {
        list.retain(|j| procs::is_alive(j.pid));
        list.clone()
    }).unwrap_or_default();
    let filter = filter.to_ascii_lowercase();
    let mut lines = Vec::new();
    for proc in procs {
        let name_l = proc.name.to_ascii_lowercase();
        if !filter.is_empty() && !name_l.contains(&filter) && !filter_matches_spawn(&spawned, proc.pid, &filter)
        {
            continue;
        }
        let rec = spawned.iter().find(|j| j.pid == proc.pid);
        let mut line = format!(
            "pid={} ppid={} name={}",
            proc.pid, proc.ppid, proc.name
        );
        if let Some(rec) = rec {
            line.push_str(" spawned=yes");
            line.push_str(" log=");
            line.push_str(&rec.log.display().to_string());
            line.push_str(" cmd=");
            line.push_str(&rec.command.replace('\n', " "));
        } else {
            line.push_str(" spawned=no");
        }
        lines.push(line);
    }
    lines.sort();
    if lines.is_empty() {
        lines.push("(none)".into());
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn filter_matches_spawn(spawned: &[SpawnRecord], pid: u32, filter: &str) -> bool {
    spawned.iter().any(|j| {
        j.pid == pid && j.command.to_ascii_lowercase().contains(filter)
    })
}

fn parse_kill_payload(payload: &[u8]) -> Result<(u32, bool), String> {
    let text = std::str::from_utf8(payload).map_err(|_| "kill pid must be utf-8".to_string())?;
    let mut lines = text.lines().filter(|l| !l.is_empty());
    let pid = lines
        .next()
        .ok_or_else(|| "kill requires a pid".to_string())?
        .trim()
        .parse::<u32>()
        .map_err(|_| "kill pid must be a number".to_string())?;
    let tree = lines.any(|l| l.trim().eq_ignore_ascii_case("tree"));
    Ok((pid, tree))
}

fn do_kill(spawns: &SpawnState, pid: u32, tree: bool) -> io::Result<String> {
    let killed = if tree {
        procs::kill_tree(pid)?
    } else {
        procs::kill_pid(pid)?;
        vec![pid]
    };
    if let Ok(mut list) = spawns.lock() {
        list.retain(|j| !killed.contains(&j.pid) && procs::is_alive(j.pid));
    }
    Ok(format!(
        "killed {}\n",
        killed
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

// --- Detached spawn (survives this connection; no console window) ---

static SPAWN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn spawn_detached(cwd: &Path, command: &str) -> io::Result<(u32, PathBuf)> {
    let job_dir = cwd.join("remote-jobs");
    fs::create_dir_all(&job_dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SPAWN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let log_path = job_dir.join(format!("{stamp}-{seq}.log"));
    let log = fs::File::create(&log_path)?;
    let log_err = log.try_clone()?;
    {
        let mut header = log.try_clone()?;
        let _ = writeln!(header, "spawn {stamp}\ncmd: {command}\n---");
    }

    let mut cmd = {
        #[cfg(unix)]
        {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        }
        #[cfg(windows)]
        {
            let mut c = Command::new("powershell");
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                command,
            ]);
            c
        }
    };
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                extern "C" {
                    fn setpgid(pid: i32, pgid: i32) -> i32;
                }
                setpgid(0, 0);
                Ok(())
            });
        }
    }

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if cfg!(windows) => {
            // Some hosts reject BREAKAWAY when the parent is not in a job.
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
                let log = fs::OpenOptions::new().append(true).open(&log_path)?;
                let log_err = log.try_clone()?;
                let mut retry = Command::new("powershell");
                retry
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        command,
                    ])
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(log))
                    .stderr(Stdio::from(log_err))
                    .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
                retry.spawn().map_err(|e2| {
                    io::Error::new(
                        e2.kind(),
                        format!("spawn retry without breakaway: {e2} (first: {e})"),
                    )
                })?
            }
            #[cfg(not(windows))]
            {
                return Err(e);
            }
        }
        Err(e) => return Err(e),
    };
    // Intentionally leak the Child: this process must outlive the TCP
    // connection and must not be placed in the foreground job object.
    let pid = child.id();
    std::mem::forget(child);
    Ok((pid, log_path))
}

// --- Client ---

fn run_simple(addr: &str, tag: u8, payload: &[u8]) -> io::Result<i32> {
    eprintln!("client: connecting to {addr}");
    let mut stream = TcpStream::connect(addr)?;
    eprintln!("client: connected");
    write_msg(&mut stream, tag, payload)?;
    let exit_code;
    loop {
        let (tag, payload) = read_msg(&mut stream)?;
        match tag {
            TAG_OUTPUT => {
                if payload.is_empty() {
                    continue;
                }
                let data = &payload[1..];
                match payload.first().copied() {
                    Some(STREAM_STDOUT) => {
                        io::stdout().write_all(data)?;
                        io::stdout().flush()?;
                    }
                    Some(STREAM_STDERR) => {
                        io::stderr().write_all(data)?;
                        io::stderr().flush()?;
                    }
                    _ => {}
                }
            }
            TAG_EXIT_CODE => {
                exit_code = if payload.len() >= 4 {
                    i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]])
                } else {
                    1
                };
                break;
            }
            TAG_ERROR => {
                eprintln!("server error: {}", String::from_utf8_lossy(&payload));
                exit_code = 1;
                break;
            }
            _ => {
                eprintln!("client: unknown tag 0x{tag:02x}");
                exit_code = 1;
                break;
            }
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
    Ok(exit_code)
}

fn run_client(addr: &str, cmd_args: &[String], is_shell: bool) -> io::Result<i32> {
    let files = get_changed_files()?;

    eprintln!("client: connecting to {}", addr);
    let mut stream = TcpStream::connect(addr)?;
    eprintln!("client: connected");

    for (rel_path, is_tracked) in &files {
        if rel_path.starts_with("local/") || rel_path.starts_with("local\\") {
            continue;
        }
        if !rel_path.contains('/') {
            continue;
        }
        let normalized = rel_path.replace('\\', "/");
        let is_vendored = normalized.starts_with("libs/linux/");
        let is_common_src = normalized.ends_with(".rs") || normalized.ends_with(".toml");
        if !is_tracked {
            if is_vendored {
                let in_src = normalized.contains("/src/") && !normalized.contains("/src/test/");
                let keep_vendored = normalized.ends_with("/Cargo.toml")
                    || normalized.ends_with("/build.rs")
                    || in_src
                    || normalized.ends_with("/wayland.xml")
                    || (normalized.contains("/protocols/") && normalized.ends_with(".xml"));
                if !keep_vendored {
                    continue;
                }
            } else if !is_common_src {
                continue;
            }
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

    let args_str = cmd_args.join("\n");
    let tag = if is_shell {
        TAG_SHELL_RUN
    } else {
        TAG_CARGO_RUN
    };
    write_msg(&mut stream, tag, args_str.as_bytes())?;
    if is_shell {
        eprintln!("client: shell {}", cmd_args.join(" "));
    } else {
        eprintln!("client: cargo {}", cmd_args.join(" "));
    }

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

fn get_changed_files() -> io::Result<Vec<(String, bool)>> {
    let mut files = Vec::new();

    let output = Command::new("git").args(["diff", "--name-only"]).output()?;
    if output.status.success() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                files.push((line.to_string(), true));
            }
        }
    }

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

    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);

    Ok(files)
}

// --- Main ---

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  Server: makepad-remote --server [--port PORT] [--all]");
    eprintln!("  Client: makepad-remote <ip:port> cargo [args...]");
    eprintln!("          makepad-remote <ip:port> shell <command...>  (requires --all)");
    eprintln!("          makepad-remote <ip:port> spawn <command...>  (hidden, survives disconnect)");
    eprintln!("          makepad-remote <ip:port> ps [filter]");
    eprintln!("          makepad-remote <ip:port> kill [--tree] <pid>");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        return ExitCode::from(1);
    }

    if args[0] == "--server" {
        let mut port: u16 = 8384;
        let mut allow_all = false;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--port" && i + 1 < args.len() {
                port = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("invalid port: {}", args[i + 1]);
                    std::process::exit(1);
                });
                i += 2;
            } else if args[i] == "--all" {
                allow_all = true;
                i += 1;
            } else {
                eprintln!("unknown server option: {}", args[i]);
                print_usage();
                std::process::exit(1);
            }
        }
        if let Err(e) = run_server(port, allow_all) {
            eprintln!("server error: {}", e);
            return ExitCode::from(1);
        }
        ExitCode::SUCCESS
    } else {
        let addr = &args[0];

        if args.len() < 2 {
            print_usage();
            return ExitCode::from(1);
        }
        let result = match args[1].as_str() {
            "cargo" => run_client(addr, &args[2..], false),
            "shell" => run_client(addr, &args[2..], true),
            "spawn" => {
                if args.len() < 3 {
                    eprintln!("spawn requires a command");
                    print_usage();
                    return ExitCode::from(1);
                }
                run_simple(addr, TAG_SPAWN, args[2..].join(" ").as_bytes())
            }
            "ps" => {
                let filter = args.get(2).cloned().unwrap_or_default();
                run_simple(addr, TAG_PS, filter.as_bytes())
            }
            "kill" => {
                let mut tree = false;
                let mut pid = None;
                for a in &args[2..] {
                    if a == "--tree" {
                        tree = true;
                    } else {
                        pid = Some(a.clone());
                    }
                }
                let Some(pid) = pid else {
                    eprintln!("kill requires a pid");
                    print_usage();
                    return ExitCode::from(1);
                };
                let payload = if tree {
                    format!("{pid}\ntree")
                } else {
                    pid
                };
                run_simple(addr, TAG_KILL, payload.as_bytes())
            }
            _ => {
                eprintln!("client mode requires: cargo|shell|spawn|ps|kill");
                print_usage();
                return ExitCode::from(1);
            }
        };

        match result {
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
