//! Windows management worker for the repository's persistent ConPTY host.
//! Presentation clients never own or implicitly replace the hosted process.
use super::*;
use makepad_screen::protocol::{create_private, private_directory, read_private, write_private};
use makepad_strict_json::{self as json, Value};
use std::{
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
        process::CommandExt,
    },
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::Instant,
};

const MAX_OUTPUT: usize = 64 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
type Handle = *mut c_void;
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FileTime {
    low: u32,
    high: u32,
}
impl FileTime {
    fn ticks(self) -> u64 {
        (self.high as u64) << 32 | self.low as u64
    }
}
#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    executable: [u16; 260],
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetProcessTimes(
        handle: Handle,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn QueryFullProcessImageNameW(handle: Handle, flags: u32, name: *mut u16, len: *mut u32)
        -> i32;
    fn WaitForSingleObject(handle: Handle, millis: u32) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        buffer: *mut c_void,
        len: u32,
        needed: *mut u32,
    ) -> i32;
    fn GetSecurityInfo(
        object: Handle,
        object_type: u32,
        information: u32,
        owner: *mut *mut c_void,
        group: *mut *mut c_void,
        dacl: *mut *mut c_void,
        sacl: *mut *mut c_void,
        descriptor: *mut *mut c_void,
    ) -> u32;
    fn EqualSid(a: *const c_void, b: *const c_void) -> i32;
}
struct OwnedHandle(Handle);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Process {
    pid: u32,
    parent: u32,
    program: String,
}
struct SupervisorProof {
    process: ProcessProof,
    pid: u32,
    child_pid: u32,
    instance: String,
}
struct ProcessProof {
    handle: OwnedHandle,
    start: u64,
    program: PathBuf,
}
impl ProcessProof {
    fn open(pid: u32) -> Result<Option<Self>, String> {
        Self::open_with_access(pid, 0x1000 | 0x0010_0000)
    }
    fn open_with_access(pid: u32, access: u32) -> Result<Option<Self>, String> {
        let raw = unsafe { OpenProcess(access, 0, pid) };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(87) {
                return Ok(None);
            }
            return Err(format!("Cannot verify agent process {pid}: {error}"));
        }
        let handle = OwnedHandle(raw);
        match unsafe { WaitForSingleObject(raw, 0) } {
            0 => return Ok(None),
            258 => {}
            _ => return Err("Cannot verify agent process liveness".into()),
        }
        let mut created = FileTime::default();
        let mut exited = created;
        let mut kernel = created;
        let mut user = created;
        if unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        if unsafe { QueryFullProcessImageNameW(raw, 0, name.as_mut_ptr(), &mut length) } == 0 {
            return Err("Cannot verify the live provider executable".into());
        }
        let program = PathBuf::from(
            String::from_utf16(&name[..length as usize])
                .map_err(|_| "Invalid provider executable path")?,
        );
        Ok(Some(Self {
            handle,
            start: created.ticks(),
            program,
        }))
    }
    fn still_live(&self) -> bool {
        unsafe { WaitForSingleObject(self.handle.0, 0) == 258 }
    }
}
fn same_process_owner(proof: &ProcessProof) -> Result<(), String> {
    let own = process_sid(unsafe { GetCurrentProcess() })?;
    let target = process_sid(proof.handle.0)?;
    if unsafe { EqualSid(own[0] as *const c_void, target[0] as *const c_void) } == 0 {
        return Err("The observed process belongs to another Windows user".into());
    }
    Ok(())
}
fn claude_metadata(home: &Path, pid: u32, start: u64) -> Result<Value, String> {
    let file = owned_evidence_file(&home.join("sessions").join(format!("{pid}.json")))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    // A PID-scoped file retained from a previous process is not evidence.
    if metadata.creation_time() < start || metadata.len() > 32768 {
        return Err("Claude session metadata predates this process or is too large; PID ownership cannot be proven".into());
    }
    let mut bytes = Vec::new();
    file.take(32769)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 32768 {
        return Err("Claude session metadata exceeds its bound".into());
    }
    let metadata = json::parse_depth(&bytes, 12).map_err(|_| "Invalid Claude session metadata")?;
    if metadata.get("pid").and_then(Value::as_u64) != Some(pid as u64)
        || metadata.get("kind").and_then(Value::as_str) != Some("interactive")
    {
        return Err("Claude metadata does not identify this interactive root process".into());
    }
    Ok(metadata)
}
fn revalidate_claude_identity(identity: &ResumeIdentity) -> Result<(), String> {
    let proof = ProcessProof::open(identity.observed_pid)?
        .ok_or("Claude exited before its conversation could be revalidated")?;
    let expected = identity
        .process_start
        .strip_prefix("windows-filetime:")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or("Saved conversation lacks Windows process-start evidence")?;
    if proof.start != expected
        || proof
            .program
            .canonicalize()
            .map_err(|_| "Claude executable is unavailable")?
            != Path::new(&identity.program)
                .canonicalize()
                .map_err(|_| "Saved Claude executable is unavailable")?
    {
        return Err("Claude process identity changed during verification".into());
    }
    same_process_owner(&proof)?;
    let metadata = claude_metadata(
        Path::new(&identity.provider_home),
        identity.observed_pid,
        proof.start,
    )?;
    if metadata.get("sessionId").and_then(Value::as_str) != Some(identity.conversation_id.as_str())
        || metadata
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(|value| Path::new(value).canonicalize().ok())
            != Some(
                Path::new(&identity.cwd)
                    .canonicalize()
                    .map_err(|_| "Saved Claude worktree is unavailable")?,
            )
        || !proof.still_live()
    {
        return Err(
            "Claude changed its live conversation or worktree; its terminal was left running"
                .into(),
        );
    }
    Ok(())
}
fn provider_of(program: &str) -> Option<AgentProvider> {
    match Path::new(program)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "codex.exe" => Some(AgentProvider::Codex),
        "claude.exe" => Some(AgentProvider::Fable),
        "grok.exe" => Some(AgentProvider::Grok),
        _ => None,
    }
}
fn descendants(pid: u32) -> Result<Vec<Process>, String> {
    let raw = unsafe { CreateToolhelp32Snapshot(2, 0) };
    if raw as isize == -1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let snapshot = OwnedHandle(raw);
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ProcessEntry>() as u32;
    let mut present = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    let mut all = Vec::new();
    while present {
        if all.len() >= 32768 {
            return Err("Process inventory exceeds its bounded capacity".into());
        }
        let length = entry
            .executable
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.executable.len());
        all.push(Process {
            pid: entry.pid,
            parent: entry.parent,
            program: String::from_utf16_lossy(&entry.executable[..length]),
        });
        present = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    if std::io::Error::last_os_error().raw_os_error() != Some(18) {
        return Err("Process inventory ended without a complete snapshot".into());
    }
    let by_id: BTreeMap<u32, u32> = all.iter().map(|p| (p.pid, p.parent)).collect();
    Ok(all
        .into_iter()
        .filter(|p| {
            let mut parent = p.parent;
            for _ in 0..64 {
                if parent == pid {
                    return true;
                }
                let Some(next) = by_id.get(&parent) else {
                    break;
                };
                if *next == parent {
                    break;
                }
                parent = *next;
            }
            false
        })
        .collect())
}

#[derive(SerRon, DeRon)]
struct Record {
    version: u32,
    session_id: String,
    cwd: String,
    created_at_ms: u64,
}
#[derive(Clone, SerRon, DeRon)]
struct LaunchRecord {
    version: u32,
    provider: AgentProvider,
    program: String,
    environment: Vec<(String, String)>,
}
#[derive(SerRon, DeRon)]
struct TransportRecord {
    version: u32,
    program: String,
    config: String,
    screen_version: String,
    truecolor: bool,
}
struct Backend {
    program: PathBuf,
    records: PathBuf,
    /// Per-lane MCP bearer tokens, minted at provider launch and revoked
    /// when the lane's terminal lifecycle ends (never on UI detach).
    mcp_tokens: std::sync::Arc<crate::mcp::TokenStore>,
}
fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn read_state(path: &Path, limit: usize) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(_) => String::from_utf8(read_private(path, limit)?)
            .map(Some)
            .map_err(|_| "Invalid UTF-8 session metadata".into()),
    }
}
fn regular_executable(path: &Path) -> bool {
    path.is_absolute()
        && path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
        && fs::metadata(path).is_ok_and(|m| m.is_file())
}
fn path_executable(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|path| path.join(name))
        .find(|path| regular_executable(path))
}
fn executable(provider: AgentProvider) -> Result<(PathBuf, Vec<String>), String> {
    let name = format!("{}.exe", provider.as_str());
    if let Some(path) = path_executable(&name) {
        return Ok((path, Vec::new()));
    }
    // Recognize the package layout, not a general batch language. The shim is
    // only a discovery hint: launch its verified package entry directly via
    // node.exe, or Codex's bundled native executable. Never evaluate the .cmd.
    let (package, entry) = match provider {
        AgentProvider::Fable => ("@anthropic-ai/claude-code", "cli.js"),
        AgentProvider::Codex => ("@openai/codex", "bin/codex.js"),
        _ => {
            return Err(format!(
                "{name} is unavailable; no replacement chat was started"
            ))
        }
    };
    for directory in std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
    {
        if !directory.is_absolute()
            || !directory
                .join(format!("{}.cmd", provider.as_str()))
                .is_file()
        {
            continue;
        }
        let package_path = directory.join("node_modules").join(package);
        let manifest = package_path.join("package.json");
        let Ok(metadata) = fs::metadata(&manifest) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > 65536 {
            continue;
        }
        let Ok(bytes) = fs::read(&manifest) else {
            continue;
        };
        if bytes.len() > 65536 {
            continue;
        }
        let Ok(value) = json::parse_depth(&bytes, 16) else {
            continue;
        };
        if value.get("name").and_then(Value::as_str) != Some(package) {
            continue;
        }
        let bin = value.get("bin").and_then(|v| {
            v.as_str()
                .or_else(|| v.get(provider.as_str()).and_then(Value::as_str))
        });
        if bin.map(|s| s.trim_start_matches("./")) != Some(entry) {
            continue;
        }
        if provider == AgentProvider::Codex {
            let target = if cfg!(target_arch = "aarch64") {
                "aarch64-pc-windows-msvc"
            } else {
                "x86_64-pc-windows-msvc"
            };
            let native = package_path
                .join("vendor")
                .join(target)
                .join("codex")
                .join("codex.exe");
            if regular_executable(&native) {
                return Ok((native, Vec::new()));
            }
        }
        let Ok(script) = package_path.join(entry).canonicalize() else {
            continue;
        };
        if !script.is_file() {
            continue;
        }
        let node = directory.join("node.exe");
        let node = if regular_executable(&node) {
            Some(node)
        } else {
            path_executable("node.exe")
        };
        if let Some(node) = node {
            return Ok((node, vec![script.to_string_lossy().into_owned()]));
        }
    }
    Err(format!("{name} or its recognized native/npm package entry is unavailable. Studio does not evaluate arbitrary batch launchers; no replacement chat was started"))
}

fn shell() -> Result<PathBuf, String> {
    let path = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("Windows system directory is unavailable")?
        .join("System32")
        .join("cmd.exe");
    if !regular_executable(&path) {
        return Err("Windows command interpreter is unavailable".into());
    }
    Ok(path)
}
// MpTerm wraps commands in cmd.exe /c. Reject metacharacter expansion rather
// than allowing a state path to become a command or inherited-variable lookup.
fn cmd_quote(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or("Terminal paths must be valid Unicode")?;
    if text
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '%' | '!' | '^' | '&' | '|' | '<' | '>'))
    {
        return Err("Terminal attachment path contains unsupported cmd.exe metacharacters".into());
    }
    Ok(format!("\"{text}\""))
}
fn owned_evidence_file(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(0x0020_0000)
        .open(path)
        .map_err(|_| "Conversation evidence is unavailable")?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
        return Err("Conversation evidence must be a regular file, not a reparse point".into());
    }
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), 8, &mut token) } == 0 {
        return Err("Current process ownership is unavailable".into());
    }
    let token = OwnedHandle(token);
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.0, 1, std::ptr::null_mut(), 0, &mut needed);
    }
    if !(8..=16384).contains(&needed) {
        return Err("Invalid current-user identity size".into());
    }
    let mut identity = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            1,
            identity.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err("Current user identity cannot be read".into());
    }
    let mut owner = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            1,
            1,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err("Conversation file ownership cannot be verified".into());
    }
    let matches = !owner.is_null() && unsafe { EqualSid(owner, identity[0] as *const c_void) } != 0;
    unsafe {
        LocalFree(descriptor);
    }
    if !matches {
        return Err("Conversation evidence belongs to another Windows user".into());
    }
    Ok(file)
}
fn evidence_rows(path: &Path) -> Result<Vec<Value>, String> {
    let mut reader = BufReader::new(owned_evidence_file(path)?.take(256 * 1024));
    let mut rows = Vec::new();
    for _ in 0..64 {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .map_err(|_| "Invalid conversation metadata")?
            == 0
        {
            break;
        }
        if let Ok(value) = json::parse_depth(line.as_bytes(), 32) {
            rows.push(value);
        }
    }
    Ok(rows)
}
fn evidence_matches(identity: &ResumeIdentity) -> Result<(), String> {
    if !uuid(&identity.conversation_id)
        || !Path::new(&identity.cwd).is_absolute()
        || !regular_executable(Path::new(&identity.program))
    {
        return Err("Saved conversation identity or executable is invalid".into());
    }
    let path = Path::new(&identity.evidence_path)
        .canonicalize()
        .map_err(|_| "Saved conversation evidence is missing; refusing a fresh chat")?;
    let home = Path::new(&identity.provider_home)
        .canonicalize()
        .map_err(|_| "Provider home is unavailable")?;
    let cwd = Path::new(&identity.cwd)
        .canonicalize()
        .map_err(|_| "Saved worktree is unavailable")?;
    if !path.starts_with(&home) {
        return Err("Conversation evidence escaped its provider directory".into());
    }
    if !evidence_rows(&path)?.iter().any(|row| {
        let (value, field) = match identity.provider {
            AgentProvider::Fable => (row, "sessionId"),
            AgentProvider::Codex
                if row.get("type").and_then(Value::as_str) == Some("session_meta") =>
            {
                (row.get("payload").unwrap_or(&Value::Null), "id")
            }
            AgentProvider::Grok => (row.get("info").unwrap_or(&Value::Null), "id"),
            _ => return false,
        };
        value.get(field).and_then(Value::as_str) == Some(identity.conversation_id.as_str())
            && value
                .get("cwd")
                .and_then(Value::as_str)
                .is_some_and(|s| Path::new(s).canonicalize().ok().as_ref() == Some(&cwd))
    }) {
        return Err("Provider history does not prove this exact conversation and worktree".into());
    }
    Ok(())
}
fn provider_home(provider: AgentProvider) -> Result<PathBuf, String> {
    let (env, name) = match provider {
        AgentProvider::Fable => ("CLAUDE_CONFIG_DIR", ".claude"),
        AgentProvider::Codex => ("CODEX_HOME", ".codex"),
        AgentProvider::Grok => ("GROK_HOME", ".grok"),
        _ => return Err("This process has no supported conversation store".into()),
    };
    std::env::var_os(env)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(name)))
        .ok_or("Provider home is unavailable")?
        .canonicalize()
        .map_err(|_| "Provider history directory is unavailable".into())
}

impl Backend {
    fn open(state: PathBuf) -> Result<Self, String> {
        let mcp_tokens = std::sync::Arc::new(crate::mcp::TokenStore::open(&state)?);
        let records = state.join("agent_sessions");
        private_directory(&records, true)?;
        let records = records.canonicalize().map_err(|e| e.to_string())?;
        let program = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("Studio executable has no directory")?
            .join("makepad-screen.exe");
        if !regular_executable(&program) {
            return Err("Build makepad-screen.exe alongside Studio: cargo build --release -p makepad-screen -p makepad-director".into());
        }
        // Validate presentation quoting before any hosted process is created.
        cmd_quote(&program)?;
        cmd_quote(&records)?;
        Ok(Self {
            program,
            records,
            mcp_tokens,
        })
    }
    fn file(&self, id: &str, suffix: &str) -> PathBuf {
        self.records.join(format!("{id}{suffix}"))
    }
    fn record(&self, id: &str) -> Result<Option<Record>, String> {
        validate_id(id)?;
        let Some(text) = read_state(&self.file(id, ".ron"), 16384)? else {
            return Ok(None);
        };
        let record =
            Record::deserialize_ron(&text).map_err(|_| "Invalid durable terminal record")?;
        if record.version != 1 || record.session_id != id || !Path::new(&record.cwd).is_absolute() {
            return Err("Mismatched durable terminal record".into());
        }
        Ok(Some(record))
    }
    fn launch_record(&self, id: &str) -> Result<Option<LaunchRecord>, String> {
        let Some(text) = read_state(&self.file(id, ".provider.ron"), 16384)? else {
            return Ok(None);
        };
        let launch = LaunchRecord::deserialize_ron(&text)
            .map_err(|_| "Invalid saved launch configuration")?;
        if launch.version != 1
            || launch.environment.len() > 3
            || !regular_executable(Path::new(&launch.program))
        {
            return Err("Saved executable or launch configuration is unavailable".into());
        }
        let mut names = std::collections::HashSet::new();
        for (key, value) in &launch.environment {
            if !matches!(
                key.as_str(),
                "MAKEPAD_STUDIO_FLOW_ID"
                    | "MAKEPAD_STUDIO_CONTROL_DIR"
                    | "MAKEPAD_STUDIO_CLI"
                    | "MAKEPAD_STUDIO_MCP_TOKEN"
            ) || !names.insert(key)
                || value.len() > 4096
                || value.chars().any(char::is_control)
            {
                return Err("Invalid saved Studio environment".into());
            }
        }
        Ok(Some(launch))
    }
    fn arguments(&self, id: &str) -> Result<Vec<String>, String> {
        let Some(text) = read_state(&self.file(id, ".windows-argv.ron"), 16384)? else {
            return Ok(Vec::new());
        };
        let arguments = Vec::<String>::deserialize_ron(&text)
            .map_err(|_| "Invalid Windows provider entry arguments")?;
        if arguments.len() > 1
            || arguments.iter().any(|s| {
                s.len() > 4096
                    || s.chars().any(char::is_control)
                    || !Path::new(s).is_absolute()
                    || !Path::new(s).is_file()
            })
        {
            return Err("Saved Windows provider entry is unavailable or invalid".into());
        }
        Ok(arguments)
    }
    fn saved_resume(&self, id: &str) -> Result<Option<ResumeIdentity>, String> {
        read_state(&self.file(id, ".resume.ron"), 16384)?
            .map(|text| {
                ResumeIdentity::deserialize_ron(&text)
                    .map_err(|_| "Invalid saved conversation identity".into())
            })
            .transpose()
    }
    fn control(&self, verb: &str, id: Option<&str>) -> Command {
        let mut command = Command::new(&self.program);
        command.arg(verb).arg("--state-dir").arg(&self.records);
        if let Some(id) = id {
            command.arg("--session").arg(id);
        }
        command
    }
    fn run_command(
        &self,
        command: &mut Command,
        stop: &AtomicBool,
    ) -> Result<(ExitStatus, String), String> {
        if stop.load(Ordering::Relaxed) {
            return Err("Agent session request canceled".into());
        }
        let path = self
            .records
            .join(format!(".control-{}.tmp", new_session_id(0)));
        create_private(&path, b"")?;
        let result = (|| {
            let output = OpenOptions::new()
                .write(true)
                .custom_flags(0x0020_0000)
                .open(&path)
                .map_err(|e| e.to_string())?;
            command
                .stdin(Stdio::null())
                .stdout(output.try_clone().map_err(|e| e.to_string())?)
                .stderr(output)
                .creation_flags(CREATE_NO_WINDOW);
            let mut child = command
                .spawn()
                .map_err(|e| format!("PTY control could not start: {e}"))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => {}
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(e.to_string());
                    }
                }
                if stop.load(Ordering::Relaxed)
                    || Instant::now() >= deadline
                    || fs::metadata(&path).is_ok_and(|m| m.len() > MAX_OUTPUT as u64)
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("PTY control was canceled, timed out or exceeded its output bound; hosted outcome must be inspected".into());
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            let text = String::from_utf8(read_private(&path, MAX_OUTPUT)?)
                .map_err(|_| "Invalid PTY control output")?;
            Ok((status, text))
        })();
        let _ = fs::remove_file(path);
        result
    }
    fn pin(&self, id: &str, creating: bool, stop: &AtomicBool) -> Result<Self, String> {
        validate_id(id)?;
        if let Some(text) = read_state(&self.file(id, ".transport.ron"), 8192)? {
            let record = TransportRecord::deserialize_ron(&text)
                .map_err(|_| "Invalid terminal transport identity")?;
            if record.version != 2
                || record.config != "makepad-screen-v1"
                || !regular_executable(Path::new(&record.program))
            {
                return Err("This terminal was created by another or unavailable transport; it was not replaced".into());
            }
            return Ok(Self {
                records: self.records.clone(),
                program: PathBuf::from(record.program),
                mcp_tokens: self.mcp_tokens.clone(),
            });
        }
        if !creating {
            return Err(
                "The terminal has no saved transport identity; attachment cannot create one".into(),
            );
        }
        let (status, version) =
            self.run_command(Command::new(&self.program).arg("--version"), stop)?;
        if !status.success() || !version.trim().starts_with("makepad-screen ") {
            return Err("The sibling executable is not the repository PTY host".into());
        }
        let record = TransportRecord {
            version: 2,
            program: self.program.to_string_lossy().into_owned(),
            config: "makepad-screen-v1".into(),
            screen_version: version.trim().to_owned(),
            truecolor: true,
        };
        create_private(
            &self.file(id, ".transport.ron"),
            record.serialize_ron().as_bytes(),
        )?;
        Ok(Self {
            records: self.records.clone(),
            program: self.program.clone(),
            mcp_tokens: self.mcp_tokens.clone(),
        })
    }
    fn info(&self, value: &Value) -> Result<Option<SessionInfo>, String> {
        if value.get("running").and_then(Value::as_bool) == Some(false) {
            return Ok(None);
        }
        if value.get("running").and_then(Value::as_bool) != Some(true)
            || value.get("version").and_then(Value::as_u64) != Some(1)
        {
            return Err("Invalid PTY status response".into());
        }
        let id = value
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or("PTY status has no session identity")?;
        validate_id(id)?;
        let pid = value
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or("Invalid PTY supervisor PID")?;
        let cwd = PathBuf::from(
            value
                .get("cwd")
                .and_then(Value::as_str)
                .ok_or("PTY status has no working directory")?,
        );
        if !cwd.is_absolute() {
            return Err("Invalid PTY working directory".into());
        }
        let state_dir = value
            .get("state_dir")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.records.clone());
        let provider = self
            .launch_record(id)?
            .map(|l| l.provider)
            .unwrap_or(AgentProvider::Unknown);
        Ok(Some(SessionInfo {
            session_id: id.into(),
            state_dir: state_dir.clone(),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(id)
                .into(),
            activity: value
                .get("activity")
                .and_then(Value::as_str)
                .unwrap_or("running")
                .into(),
            started_at_ms: value
                .get("started_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            supervisor_pid: pid,
            attach_command: format!(
                "\"{} attach --state-dir {} --session {}\"",
                cmd_quote(&self.program)?,
                cmd_quote(&state_dir)?,
                id
            ),
            backend: "makepad-screen",
            transport_program: self.program.to_string_lossy().into_owned(),
            transport_version: "makepad-screen protocol 1 · ConPTY".into(),
            transport_warning: None,
            scrollback_lines: SCROLLBACK_LINES,
            cwd,
            clients: value
                .get("clients")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(256) as usize,
            provider,
            resume: None,
            resume_error: None,
            recovery: None,
        }))
    }
    fn live_value(&self, id: &str, stop: &AtomicBool) -> Result<Value, String> {
        let (status, output) = self.run_command(&mut self.control("status", Some(id)), stop)?;
        if !status.success() {
            return Err(format!(
                "PTY status is unavailable: {}",
                output.trim().chars().take(512).collect::<String>()
            ));
        }
        let value =
            json::parse_depth(output.as_bytes(), 16).map_err(|_| "Invalid PTY status JSON")?;
        let info = self.info(&value)?;
        if info.as_ref().is_some_and(|i| i.session_id != id) {
            return Err("PTY status returned another session".into());
        }
        Ok(value)
    }
    fn live(&self, id: &str, stop: &AtomicBool) -> Result<Option<SessionInfo>, String> {
        self.info(&self.live_value(id, stop)?)
    }
    fn pin_supervisor(
        &self,
        id: &str,
        info: &SessionInfo,
        stop: &AtomicBool,
    ) -> Result<SupervisorProof, String> {
        let value = self.live_value(id, stop)?;
        let current = self
            .info(&value)?
            .ok_or("The PTY exited during ownership inspection")?;
        if current.supervisor_pid != info.supervisor_pid || current.cwd != info.cwd {
            return Err("The owned PTY changed during inspection".into());
        }
        let child_pid = value
            .get("child_pid")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or("PTY status lacks its owned child PID")?;
        let instance = value
            .get("instance")
            .and_then(Value::as_str)
            .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("PTY status lacks its startup identity")?
            .to_owned();
        let process = ProcessProof::open(info.supervisor_pid)?
            .ok_or("The PTY supervisor exited during inspection")?;
        same_process_owner(&process)?;
        if process
            .program
            .canonicalize()
            .map_err(|_| "PTY executable is unavailable")?
            != self
                .program
                .canonicalize()
                .map_err(|_| "Pinned PTY executable is unavailable")?
        {
            return Err("The PTY supervisor executable changed during inspection".into());
        }
        Ok(SupervisorProof {
            process,
            pid: info.supervisor_pid,
            child_pid,
            instance,
        })
    }
    fn verify_root_scope(
        &self,
        id: &str,
        info: &SessionInfo,
        supervisor: &SupervisorProof,
        root_pid: u32,
        root: &ProcessProof,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        if !supervisor.process.still_live()
            || !root.still_live()
            || root.start < supervisor.process.start
        {
            return Err("The owned PTY or provider changed during inspection".into());
        }
        same_process_owner(root)?;
        let value = self.live_value(id, stop)?;
        let current = self
            .info(&value)?
            .ok_or("The PTY exited during ownership inspection")?;
        if current.supervisor_pid != supervisor.pid
            || current.cwd != info.cwd
            || value.get("child_pid").and_then(Value::as_u64) != Some(supervisor.child_pid as u64)
            || value.get("instance").and_then(Value::as_str) != Some(supervisor.instance.as_str())
        {
            return Err("The PTY startup or child identity changed during inspection".into());
        }
        // Toolhelp parent IDs alone are vulnerable to PID reuse. Pin every
        // live ancestor and require creation times to follow the parent chain.
        let processes = descendants(supervisor.pid)?;
        let mut pid = root_pid;
        let mut child_start = root.start;
        let mut includes_child = pid == supervisor.child_pid;
        let mut ancestors = Vec::new();
        for _ in 0..64 {
            let process = processes
                .iter()
                .find(|p| p.pid == pid)
                .ok_or("The provider no longer belongs to the owned PTY")?;
            if process.parent == supervisor.pid {
                if !includes_child
                    || !supervisor.process.still_live()
                    || !root.still_live()
                    || ancestors.iter().any(|p: &ProcessProof| !p.still_live())
                {
                    return Err(
                        "The provider's owned parent chain changed during inspection".into(),
                    );
                }
                return Ok(());
            }
            let parent = ProcessProof::open(process.parent)?
                .ok_or("A provider ancestor exited during ownership inspection")?;
            if parent.start > child_start || parent.start < supervisor.process.start {
                return Err(
                    "A provider ancestor PID was reused; ownership cannot be proven".into(),
                );
            }
            same_process_owner(&parent)?;
            child_start = parent.start;
            pid = process.parent;
            includes_child |= pid == supervisor.child_pid;
            ancestors.push(parent);
        }
        Err("The provider parent chain exceeds its bounded capacity".into())
    }
    fn discover(
        &self,
        id: &str,
        info: &SessionInfo,
        stop: &AtomicBool,
    ) -> Result<Option<ResumeIdentity>, String> {
        let record = self
            .record(id)?
            .ok_or("Durable terminal identity is missing")?;
        let supervisor = self.pin_supervisor(id, info, stop)?;
        let processes = descendants(info.supervisor_pid)?;
        let launch = self.launch_record(id)?;
        let configured_root =
            if info.provider == AgentProvider::Fable && self.arguments(id)?.len() == 1 {
                launch.as_ref().and_then(|launch| {
                    let name = Path::new(&launch.program).file_name()?.to_str()?;
                    let mut roots = processes.iter().filter(|p| {
                        p.parent == info.supervisor_pid && p.program.eq_ignore_ascii_case(name)
                    });
                    let pid = roots.next()?.pid;
                    if roots.next().is_some() {
                        None
                    } else {
                        Some(pid)
                    }
                })
            } else {
                None
            };
        let providers: Vec<_> = processes
            .iter()
            .filter(|p| provider_of(&p.program).is_some() || configured_root == Some(p.pid))
            .collect();
        let roots: Vec<_> = providers
            .iter()
            .filter(|p| {
                let mut parent = p.parent;
                for _ in 0..64 {
                    if providers.iter().any(|q| q.pid == parent) {
                        return false;
                    }
                    let Some(q) = processes.iter().find(|q| q.pid == parent) else {
                        break;
                    };
                    parent = q.parent;
                }
                true
            })
            .copied()
            .collect();
        let Some(root) = roots.first() else {
            if info.provider != AgentProvider::Shell {
                return Err("The configured root AI is not observable; its persistent session remains running".into());
            }
            // Shells can start providers through scripts. Never kill an unknown
            // interpreter subtree merely because its executable is called node.
            if processes.iter().any(|p| {
                !matches!(
                    p.program.to_ascii_lowercase().as_str(),
                    "cmd.exe" | "conhost.exe" | "openconsole.exe"
                )
            }) {
                return Err("This shell has running child work without a proven conversation identity; finish it in the terminal before Stop".into());
            }
            return Ok(None);
        };
        if roots.len() != 1 {
            return Err(
                "Several independent AIs share this terminal; conversation ownership is ambiguous"
                    .into(),
            );
        }
        let provider = provider_of(&root.program)
            .or_else(|| {
                configured_root
                    .filter(|pid| *pid == root.pid)
                    .map(|_| info.provider)
            })
            .ok_or("Unrecognized provider root")?;
        let proof = ProcessProof::open(root.pid)?.ok_or("The root AI exited during inspection")?;
        let configured_match = configured_root == Some(root.pid)
            && launch.as_ref().is_some_and(|launch| {
                Path::new(&launch.program).canonicalize().ok() == proof.program.canonicalize().ok()
            });
        if provider_of(&proof.program.to_string_lossy()) != Some(provider) && !configured_match {
            return Err("The provider PID changed during inspection".into());
        }
        self.verify_root_scope(id, info, &supervisor, root.pid, &proof, stop)?;
        if provider != AgentProvider::Fable {
            return self
                .discover_opened(id, info, &supervisor, root, &proof, provider, stop)
                .map(Some);
        }
        let home = provider_home(provider)?;
        let metadata = claude_metadata(&home, root.pid, proof.start)?;
        let conversation = metadata
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|s| uuid(s))
            .ok_or("Claude has not published a conversation UUID")?;
        let cwd = Path::new(&record.cwd)
            .canonicalize()
            .map_err(|_| "Agent worktree is unavailable")?;
        if metadata
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(|s| Path::new(s).canonicalize().ok())
            .as_ref()
            != Some(&cwd)
        {
            return Err("Claude conversation belongs to another worktree".into());
        }
        let mut matches = Vec::new();
        for (index, entry) in fs::read_dir(home.join("projects"))
            .map_err(|_| "Claude project history is unavailable")?
            .take(2049)
            .enumerate()
        {
            if index == 2048 {
                return Err("Claude project history exceeds its bounded capacity".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().is_ok_and(|t| t.is_dir())
                || fs::symlink_metadata(entry.path())
                    .is_ok_and(|m| m.file_attributes() & 0x400 != 0)
            {
                continue;
            }
            let path = entry.path().join(format!("{conversation}.jsonl"));
            if !path.exists() {
                continue;
            }
            let identity = ResumeIdentity {
                provider,
                conversation_id: conversation.into(),
                cwd: cwd.to_string_lossy().into_owned(),
                evidence_path: path.to_string_lossy().into_owned(),
                program: proof.program.to_string_lossy().into_owned(),
                provider_home: home.to_string_lossy().into_owned(),
                observed_pid: root.pid,
                process_start: format!("windows-filetime:{}", proof.start),
                verified_at_ms: timestamp_ms(),
            };
            if evidence_matches(&identity).is_ok() {
                matches.push(identity);
            }
        }
        if matches.len() != 1 || !proof.still_live() {
            return Err(
                "Claude's live root and unique saved conversation could not be verified".into(),
            );
        }
        let identity = matches.remove(0);
        self.verify_root_scope(id, info, &supervisor, root.pid, &proof, stop)?;
        revalidate_claude_identity(&identity)?;
        self.persist_identity(id, &identity)?;
        Ok(Some(identity))
    }
    fn observed(&self, id: &str, mut info: SessionInfo, stop: &AtomicBool) -> SessionInfo {
        match self.discover(id, &info, stop) {
            Ok(Some(identity)) => {
                info.provider = identity.provider;
                info.resume = Some(identity);
            }
            Ok(None) => {}
            Err(error) => {
                info.resume = self.saved_resume(id).ok().flatten();
                info.resume_error = Some(error);
            }
        }
        info
    }
    fn existing(&self, id: &str, stop: &AtomicBool) -> Result<Option<SessionInfo>, String> {
        let record = self
            .record(id)?
            .ok_or("Unknown terminal; reattachment cannot create a replacement")?;
        let info = self.live(id, stop)?;
        if info.as_ref().is_some_and(|i| {
            i.cwd.canonicalize().ok() != Path::new(&record.cwd).canonicalize().ok()
        }) {
            return Err("PTY working directory differs from its recorded ownership".into());
        }
        Ok(info.map(|i| self.observed(id, i, stop)))
    }
    fn attach(&self, id: &str, stop: &AtomicBool) -> Result<SessionOutcome, String> {
        self.existing(id, stop)?
            .map(SessionOutcome::Ready)
            .ok_or_else(|| {
                "Agent session ended or is unavailable; no replacement process was started".into()
            })
    }
    fn launch(
        &self,
        record: &Record,
        launch: &LaunchRecord,
        identity: Option<&ResumeIdentity>,
        restart: bool,
        stop: &AtomicBool,
    ) -> Result<SessionOutcome, String> {
        if !regular_executable(Path::new(&launch.program)) {
            return Err("Saved native executable is unavailable".into());
        }
        let cwd = Path::new(&record.cwd)
            .canonicalize()
            .map_err(|_| "The agent worktree is unavailable")?;
        let mut command = self.control("start", Some(&record.session_id));
        command.arg("--cwd").arg(cwd);
        if restart {
            command.arg("--restart");
        }
        command.arg("--").arg(&launch.program);
        // An npm wrapper can launch a native provider child. A proven resume
        // targets that native executable directly and must not inherit the old
        // node entry script as an accidental prompt/argument.
        if Path::new(&launch.program)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("node.exe"))
        {
            command.args(self.arguments(&record.session_id)?);
        }
        for (key, value) in &launch.environment {
            command.env(key, value);
        }
        command
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "makepad")
            .env_remove("NO_COLOR");
        if launch.provider == AgentProvider::Shell {
            command.arg("/D");
        } else {
            if let Some(identity) = identity {
                evidence_matches(identity)?;
                if identity.provider != launch.provider {
                    return Err("Saved provider and resume identity disagree".into());
                }
                match launch.provider {
                    AgentProvider::Codex => {
                        command
                            .arg("resume")
                            .arg(&identity.conversation_id)
                            .env("CODEX_HOME", &identity.provider_home);
                    }
                    AgentProvider::Fable => {
                        command
                            .arg("--resume")
                            .arg(&identity.conversation_id)
                            .env("CLAUDE_CONFIG_DIR", &identity.provider_home);
                    }
                    AgentProvider::Grok => {
                        command
                            .arg("--resume")
                            .arg(&identity.conversation_id)
                            .env("GROK_HOME", &identity.provider_home);
                    }
                    _ => return Err("This provider has no verified resume adapter".into()),
                }
            }
            command.arg(format!("{PROVIDER_BOOTSTRAP} This is Windows: adapt the shell examples to PowerShell or cmd.exe; invoke the executable in MAKEPAD_STUDIO_CLI with --url to resolve this same lane. Never execute the POSIX environment wrapper."));
        }
        let (status, _) = self.run_command(&mut command, stop)?;
        if !status.success() {
            return Err("PTY launch failed; its durable identity was retained for inspection, not retried as a new chat".into());
        }
        self.attach(&record.session_id, stop)
    }
    fn prepare(
        &self,
        spec: SessionSpec,
        provider: AgentProvider,
        stop: &AtomicBool,
    ) -> Result<SessionOutcome, String> {
        let cwd = spec
            .cwd
            .canonicalize()
            .map_err(|_| "Agent working directory is unavailable")?;
        if let Some(record) = self.record(&spec.session_id)? {
            if Path::new(&record.cwd).canonicalize().ok().as_ref() != Some(&cwd) {
                return Err("Saved terminal belongs to another worktree".into());
            }
            return self.attach(&spec.session_id, stop);
        }
        if self.live(&spec.session_id, stop)?.is_some() {
            return Err("An unrecorded live session already owns this identity".into());
        }
        // Studio flow shells use the same parsed environment as native provider
        // launches. Arbitrary POSIX command strings must not run in cmd.exe.
        let mut environment = studio_environment(spec.command.as_deref())?;
        // The lane's MCP credential, owned by the durable terminal origin and
        // exported through the same allowlisted launch environment so a
        // re-attach after a Studio restart keeps it.
        environment.retain(|(key, _)| key != "MAKEPAD_STUDIO_MCP_TOKEN");
        let token = self.mcp_tokens.mint(&spec.session_id, &spec.session_id)?;
        environment.push(("MAKEPAD_STUDIO_MCP_TOKEN".into(), token.as_str().into()));
        let (program, arguments) = if provider == AgentProvider::Shell {
            (shell()?, Vec::new())
        } else {
            executable(provider)?
        };
        let launch = LaunchRecord {
            version: 1,
            provider,
            program: program.to_string_lossy().into_owned(),
            environment,
        };
        let record = Record {
            version: 1,
            session_id: spec.session_id,
            cwd: cwd.to_string_lossy().into_owned(),
            created_at_ms: timestamp_ms(),
        };
        create_private(
            &self.file(&record.session_id, ".ron"),
            record.serialize_ron().as_bytes(),
        )?;
        write_private(
            &self.file(&record.session_id, ".provider.ron"),
            launch.serialize_ron().as_bytes(),
        )?;
        write_private(
            &self.file(&record.session_id, ".windows-argv.ron"),
            arguments.serialize_ron().as_bytes(),
        )?;
        self.launch(&record, &launch, None, false, stop)
    }
    fn verify_exited(identity: &ResumeIdentity) -> Result<(), String> {
        let expected = identity
            .process_start
            .strip_prefix("windows-filetime:")
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or("Saved conversation lacks Windows process-start evidence")?;
        if ProcessProof::open(identity.observed_pid)?.is_some_and(|p| p.start == expected) {
            return Err(
                "The saved root AI is still running; no competing conversation was started".into(),
            );
        }
        Ok(())
    }
    fn capture(&self, id: &str, stop: &AtomicBool) -> Result<ResumeIdentity, String> {
        if let Some(info) = self.existing(id, stop)? {
            if let Some(error) = info.resume_error {
                return Err(error);
            }
            return info
                .resume
                .ok_or_else(|| "This shell has no proven AI conversation".into());
        }
        let identity = self
            .saved_resume(id)?
            .ok_or("No conversation identity was saved before this session ended")?;
        evidence_matches(&identity)?;
        Ok(identity)
    }
    fn stop_session(&self, id: &str, stop: &AtomicBool) -> Result<SessionOutcome, String> {
        let Some(info) = self.existing(id, stop)? else {
            return Ok(match self.saved_resume(id)? {
                Some(identity) => SessionOutcome::StoppedWithResume(identity),
                None => SessionOutcome::Stopped,
            });
        };
        if let Some(error) = info.resume_error {
            return Err(error);
        }
        let supervisor = self.pin_supervisor(id, &info, stop)?;
        let root = if let Some(identity) = info.resume.as_ref() {
            let proof = ProcessProof::open(identity.observed_pid)?
                .ok_or("The provider exited before Stop")?;
            if identity.process_start != format!("windows-filetime:{}", proof.start) {
                return Err(
                    "The provider changed before Stop; its terminal was left running".into(),
                );
            }
            self.verify_root_scope(id, &info, &supervisor, identity.observed_pid, &proof, stop)?;
            if identity.provider == AgentProvider::Fable {
                revalidate_claude_identity(identity)?;
            }
            Some(proof)
        } else {
            None
        };
        let identity = info.resume;
        if !supervisor.process.still_live() || root.as_ref().is_some_and(|p| !p.still_live()) {
            return Err(
                "The owned processes changed before Stop; no replacement was stopped".into(),
            );
        }
        let (status, _) = self.run_command(&mut self.control("stop", Some(id)), stop)?;
        if !status.success() || self.live(id, stop)?.is_some() {
            return Err(
                "The owned PTY has not confirmed shutdown; the saved identity remains intact"
                    .into(),
            );
        }
        // The lane's terminal lifecycle has ended: its MCP credential stops
        // working now. A failed revoke is reported, never hidden.
        self.mcp_tokens.revoke(id)?;
        if let Some(identity) = identity {
            Self::verify_exited(&identity)?;
            Ok(SessionOutcome::StoppedWithResume(identity))
        } else {
            Ok(SessionOutcome::Stopped)
        }
    }
    fn restore(
        &self,
        id: &str,
        environment: Option<(&str, &Path)>,
        stop: &AtomicBool,
    ) -> Result<SessionOutcome, String> {
        let record = self
            .record(id)?
            .ok_or("Unknown durable terminal; restore cannot create it")?;
        let mut launch = self
            .launch_record(id)?
            .ok_or("Saved launch configuration is missing; refusing a replacement")?;
        if let Some((command, cwd)) = environment {
            if cwd.canonicalize().ok() != Path::new(&record.cwd).canonicalize().ok() {
                return Err("Restore will not move a conversation to another worktree".into());
            }
            launch.environment = studio_environment(Some(command))?;
            write_private(
                &self.file(id, ".provider.ron"),
                launch.serialize_ron().as_bytes(),
            )?;
        }
        if let Some(info) = self.existing(id, stop)? {
            return Ok(SessionOutcome::Ready(info));
        }
        let identity = self.saved_resume(id)?;
        if let Some(identity) = &identity {
            evidence_matches(identity)?;
            Self::verify_exited(identity)?;
            launch.provider = identity.provider;
            launch.program = identity.program.clone();
        } else if launch.provider != AgentProvider::Shell {
            return Err(
                "No proven conversation identity was saved; refusing a fresh AI chat".into(),
            );
        }
        self.launch(&record, &launch, identity.as_ref(), true, stop)
    }
    fn recover(
        &self,
        id: &str,
        command: &str,
        cwd: &Path,
        stop: &AtomicBool,
    ) -> Result<SessionOutcome, String> {
        studio_environment(Some(command))?;
        let record = self.record(id)?.ok_or("Unknown durable terminal")?;
        if cwd.canonicalize().ok() != Path::new(&record.cwd).canonicalize().ok() {
            return Err("Recovery cannot move an agent to another worktree".into());
        }
        let mut info = self
            .existing(id, stop)?
            .ok_or("The agent has ended; use explicit saved-conversation resume")?;
        if let Some(error) = &info.resume_error {
            return Err(error.clone());
        }
        let identity = info
            .resume
            .as_ref()
            .ok_or("Recovery requires a proven live conversation")?;
        if identity.provider != AgentProvider::Fable {
            return Err("Windows browser account recovery is unavailable until the provider's live conversation can be preserved and verified; the running agent was left untouched".into());
        }
        revalidate_claude_identity(identity)?;
        info.recovery = Some(RecoveryInfo { provider: identity.provider, phase: RecoveryPhase::LoginRequested, conversation_id: identity.conversation_id.clone(), message: "Claude conversation preserved; submit /login only through its existing empty input prompt".into(), started_at_ms: timestamp_ms() });
        Ok(SessionOutcome::FableLoginReady(info))
    }
    fn views(&self) -> Result<Vec<(u64, String)>, String> {
        let Some(text) = read_state(&self.records.join("terminal-views.ron"), 16384)? else {
            return Ok(Vec::new());
        };
        let views = Vec::<(u64, String)>::deserialize_ron(&text)
            .map_err(|_| "Invalid terminal view selection")?;
        let mut tabs = std::collections::HashSet::new();
        if views.len() > 256 {
            return Err("Terminal view selection exceeds its limit".into());
        }
        for (tab, id) in &views {
            view_target(id, &self.records)?;
            if !tabs.insert(*tab) {
                return Err("Duplicate terminal view selection".into());
            }
        }
        Ok(views)
    }
    fn inventory(&self, repo: &Path, stop: &AtomicBool) -> Result<SessionOutcome, String> {
        let mut sessions = Vec::new();
        for studio in [true, false] {
            let mut command = Command::new(&self.program);
            command.arg("list");
            if studio {
                command.arg("--state-dir").arg(&self.records);
            } else {
                // Use the launcher's workspace resolution without passing
                // start/TUI-only options to list.
                command.current_dir(repo);
            }
            let (status, output) = self.run_command(&mut command, stop)?;
            if !status.success() {
                return Err(format!("Cannot list running agents: {}", output.trim()));
            }
            let value = json::parse(output.as_bytes()).map_err(|e| e.to_string())?;
            let rows = value
                .as_arr()
                .or_else(|| value.get("sessions").and_then(Value::as_arr))
                .ok_or("Invalid agent inventory")?;
            if rows.len() > 1024 {
                return Err("Agent inventory exceeds its bound".into());
            }
            for row in rows {
                if let Some(info) = self.info(row)? {
                    if !sessions
                        .iter()
                        .any(|old: &SessionInfo| old.key() == info.key())
                    {
                        sessions.push(info);
                    }
                }
            }
        }
        Ok(SessionOutcome::Inventory {
            state_dir: self.records.clone(),
            sessions,
            views: self.views()?,
        })
    }
    fn select_view(
        &self,
        tab: u64,
        selected: Option<String>,
        stop: &AtomicBool,
    ) -> Result<SessionOutcome, String> {
        let session = if let Some(id) = &selected {
            let (records, id) = view_target(id, &self.records)?;
            let backend = Self {
                records,
                program: self.program.clone(),
                mcp_tokens: self.mcp_tokens.clone(),
            };
            Some(
                backend
                    .live(&id, stop)?
                    .ok_or("Selected agent is no longer running")?,
            )
        } else {
            None
        };
        let mut views = self.views()?;
        views.retain(|(id, _)| *id != tab);
        if let Some(id) = selected {
            if views.len() >= 256 {
                return Err("Terminal view selection exceeds its limit".into());
            }
            views.push((tab, id));
        }
        write_private(
            &self.records.join("terminal-views.ron"),
            views.serialize_ron().as_bytes(),
        )?;
        Ok(SessionOutcome::ViewReady { tab, session })
    }
}

pub(super) fn delete_lane_session(state: PathBuf, tab: u64, id: &str) -> Result<(), String> {
    let backend = Backend::open(state)?;
    let stop = AtomicBool::new(false);
    backend.stop_session(id, &stop)?;
    backend.select_view(tab, None, &stop)?;
    backend.mcp_tokens.revoke(id)?;
    Ok(())
}

pub(super) fn run(
    state: PathBuf,
    commands: Receiver<Request>,
    replies: SyncSender<SessionReply>,
    stop: Arc<AtomicBool>,
) {
    let backend = Backend::open(state);
    while !stop.load(Ordering::Relaxed) {
        let request = match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let result = match &backend {
            Err(error) => Err(error.clone()),
            Ok(backend) => match request.operation {
                Operation::List(repo) => backend.inventory(&repo, &stop),
                Operation::NewView(tab, spec) => backend
                    .pin(&spec.session_id, true, &stop)
                    .and_then(|session| session.prepare(spec, AgentProvider::Shell, &stop))
                    .and_then(|outcome| {
                        if let SessionOutcome::Ready(info) = outcome {
                            backend.select_view(tab, Some(info.key()), &stop)
                        } else {
                            Err("New terminal did not become ready".into())
                        }
                    }),
                Operation::SelectView(tab, selected) => backend.select_view(tab, selected, &stop),
                operation => backend
                    .pin(
                        &request.session_id,
                        matches!(
                            &operation,
                            Operation::Prepare(_) | Operation::PrepareProvider(_, _)
                        ),
                        &stop,
                    )
                    .and_then(|backend| match operation {
                        Operation::Prepare(spec) => {
                            backend.prepare(spec, AgentProvider::Shell, &stop)
                        }
                        Operation::PrepareProvider(spec, provider) => {
                            backend.prepare(spec, provider, &stop)
                        }
                        Operation::Attach => backend.attach(&request.session_id, &stop),
                        Operation::Inspect => backend
                            .existing(&request.session_id, &stop)
                            .map(SessionOutcome::Status),
                        Operation::Stop => backend.stop_session(&request.session_id, &stop),
                        Operation::CaptureResume => backend
                            .capture(&request.session_id, &stop)
                            .map(SessionOutcome::ResumeCaptured),
                        Operation::Restore => backend.restore(&request.session_id, None, &stop),
                        Operation::RestoreEnvironment(command, cwd) => {
                            backend.restore(&request.session_id, Some((&command, &cwd)), &stop)
                        }
                        Operation::Recover(command, cwd) => {
                            backend.recover(&request.session_id, &command, &cwd, &stop)
                        }
                        Operation::List(_)
                        | Operation::NewView(_, _)
                        | Operation::SelectView(_, _) => unreachable!(),
                    }),
            },
        };
        if replies
            .send(SessionReply {
                request_id: request.request_id,
                session_id: request.session_id,
                result,
            })
            .is_err()
        {
            break;
        }
        SignalToUI::set_ui_signal();
    }
    // Ending Studio's management worker only detaches. The ConPTY daemon owns
    // its process group and outlives all Studio presentation clients.
}

include!("agent_session_windows_handles.rs");
