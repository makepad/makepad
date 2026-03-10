use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, SerJson, SerJsonState};
use std::{
    fs,
    io::{ErrorKind, Write},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LOCK_FORMAT_VERSION: u32 = 1;
const PORT_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
const PORT_RELEASE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const PID_EXIT_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const PID_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
#[cfg(not(test))]
const STARTUP_LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const STARTUP_LOCK_WAIT_TIMEOUT: Duration = Duration::from_millis(250);
#[cfg(not(test))]
const STARTUP_LOCK_STALE_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const STARTUP_LOCK_STALE_TIMEOUT: Duration = Duration::from_millis(50);
#[cfg(not(test))]
const STARTUP_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(test)]
const STARTUP_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct WasmServerLockMetadata {
    pub format_version: u32,
    pub pid: u32,
    pub port: u16,
    pub workspace_root: String,
    pub crate_name: String,
    pub profile: String,
    pub started_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LockState {
    NoLock,
    StaleLock,
    LiveLock,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupScenario {
    NoLock,
    StaleLock,
    LiveLock,
    UnknownOccupant,
}

pub struct WasmServerOwnershipGuard {
    lock_path: PathBuf,
    metadata: WasmServerLockMetadata,
    active: bool,
    startup_lock: Option<StartupMutexGuard>,
}

impl WasmServerOwnershipGuard {
    pub fn prepare(
        workspace_root: &Path,
        crate_name: &str,
        profile: &str,
        port: u16,
        lan: bool,
    ) -> Result<Self, String> {
        Self::prepare_with_probes(
            workspace_root,
            crate_name,
            profile,
            port,
            lan,
            &LiveServerManagerProbes,
        )
    }

    pub fn activate(&mut self) -> Result<(), String> {
        if self.active {
            return Ok(());
        }
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create wasm server lock dir {:?}: {}",
                    parent, err
                )
            })?;
        }
        write_file_atomically(&self.lock_path, &self.metadata.serialize_json())?;
        self.active = true;
        if let Some(startup_lock) = self.startup_lock.take() {
            drop(startup_lock);
        }
        Ok(())
    }

    fn prepare_with_probes(
        workspace_root: &Path,
        crate_name: &str,
        profile: &str,
        port: u16,
        lan: bool,
        probes: &impl ServerManagerProbes,
    ) -> Result<Self, String> {
        let workspace_root = normalize_path_string(workspace_root);
        let lock_path = lock_path_for_port(Path::new(&workspace_root), port);
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create wasm server lock dir {:?}: {}",
                    parent, err
                )
            })?;
        }
        let startup_lock_path = startup_lock_path_for_port(Path::new(&workspace_root), port);
        let startup_lock =
            StartupMutexGuard::acquire_with_probes(&startup_lock_path, port, probes)?;

        let listen_addr = listen_address(port, lan);
        let maybe_lock = match read_lock_file(&lock_path) {
            Ok(lock) => lock,
            Err(err) => {
                println!("server lock stale, recovering");
                let _ = remove_lock_file_if_exists(&lock_path);
                eprintln!("wasm server lock parse warning: {}", err);
                None
            }
        };

        let lock_state = classify_lock_state(maybe_lock.as_ref(), listen_addr, probes);
        if let Some(lock) = maybe_lock {
            match lock_state {
                LockState::LiveLock => {
                    probes.terminate_pid(lock.pid)?;
                    if !probes.wait_for_port_release(listen_addr, PORT_RELEASE_TIMEOUT) {
                        return Err(format!(
                            "failed to release wasm server port {} after terminating pid {}",
                            port, lock.pid
                        ));
                    }
                    println!(
                        "replaced existing wasm server pid {} on port {}",
                        lock.pid, port
                    );
                    remove_lock_file_if_exists(&lock_path)?;
                }
                LockState::StaleLock => {
                    println!("server lock stale, recovering");
                    remove_lock_file_if_exists(&lock_path)?;
                }
                LockState::NoLock => {
                    let _ = lock;
                }
            }
        }

        if probes.is_port_in_use(listen_addr) {
            if let Some(occupant) = probes.describe_port_occupant(listen_addr) {
                println!(
                    "port occupied by non-managed process on {} ({})",
                    listen_addr, occupant
                );
                return Err(format!(
                    "port occupied by non-managed process on {} ({}); stop the existing process or use --port=<port>",
                    listen_addr, occupant
                ));
            }
            println!("port occupied by non-managed process on {}", listen_addr);
            return Err(format!(
                "port occupied by non-managed process on {}; stop the existing process or use --port=<port>",
                listen_addr
            ));
        }

        let metadata = WasmServerLockMetadata {
            format_version: LOCK_FORMAT_VERSION,
            pid: probes.current_pid(),
            port,
            workspace_root,
            crate_name: crate_name.to_string(),
            profile: profile.to_string(),
            started_at: probes.now_unix_secs(),
        };

        Ok(Self {
            lock_path,
            metadata,
            active: false,
            startup_lock: Some(startup_lock),
        })
    }

    fn remove_own_lock_file(&self) {
        let Ok(Some(current_lock)) = read_lock_file(&self.lock_path) else {
            return;
        };
        if current_lock == self.metadata {
            let _ = remove_lock_file_if_exists(&self.lock_path);
        }
    }
}

impl Drop for WasmServerOwnershipGuard {
    fn drop(&mut self) {
        if self.active {
            self.remove_own_lock_file();
        }
    }
}

fn classify_lock_state(
    lock: Option<&WasmServerLockMetadata>,
    listen_addr: SocketAddr,
    probes: &impl ServerManagerProbes,
) -> LockState {
    match lock {
        Some(lock) if probes.is_pid_alive(lock.pid) && probes.pid_owns_port(lock.pid, listen_addr) => {
            LockState::LiveLock
        }
        Some(_) => LockState::StaleLock,
        None => LockState::NoLock,
    }
}

#[cfg(test)]
fn evaluate_startup_scenario(lock_state: LockState, port_in_use: bool) -> StartupScenario {
    if lock_state != LockState::LiveLock && port_in_use {
        StartupScenario::UnknownOccupant
    } else {
        match lock_state {
            LockState::NoLock => StartupScenario::NoLock,
            LockState::StaleLock => StartupScenario::StaleLock,
            LockState::LiveLock => StartupScenario::LiveLock,
        }
    }
}

fn lock_path_for_port(workspace_root: &Path, port: u16) -> PathBuf {
    workspace_root
        .join("target")
        .join("makepad-wasm-server")
        .join(format!("{port}.json"))
}

fn startup_lock_path_for_port(workspace_root: &Path, port: u16) -> PathBuf {
    workspace_root
        .join("target")
        .join("makepad-wasm-server")
        .join(format!("{port}.startup.lock"))
}

fn write_file_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let tmp_path = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp_path, contents).map_err(|err| {
        format!(
            "failed to write temporary wasm server lock {:?}: {}",
            tmp_path, err
        )
    })?;

    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::AlreadyExists | ErrorKind::PermissionDenied
            ) =>
        {
            let _ = fs::remove_file(path);
            fs::rename(&tmp_path, path).map_err(|rename_err| {
                let _ = fs::remove_file(&tmp_path);
                format!(
                    "failed to replace wasm server lock {:?}: {}",
                    path, rename_err
                )
            })
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp_path);
            Err(format!(
                "failed to move wasm server lock {:?} into place: {}",
                path, err
            ))
        }
    }
}

struct StartupMutexGuard {
    lock_path: PathBuf,
}

impl StartupMutexGuard {
    fn acquire_with_probes(
        lock_path: &Path,
        port: u16,
        probes: &impl ServerManagerProbes,
    ) -> Result<Self, String> {
        let started = SystemTime::now();
        loop {
            match fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(lock_path)
            {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", probes.current_pid());
                    let _ = writeln!(file, "started_at={}", probes.now_unix_secs());
                    return Ok(Self {
                        lock_path: lock_path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if startup_lock_is_stale(lock_path, probes)? {
                        println!("startup lock stale, recovering");
                        remove_lock_file_if_exists(lock_path)?;
                        continue;
                    }
                    if started.elapsed().unwrap_or_default() >= STARTUP_LOCK_WAIT_TIMEOUT {
                        return Err(format!(
                            "another wasm run startup is already in progress on port {}; retry shortly or use --port=<port>",
                            port
                        ));
                    }
                    thread::sleep(STARTUP_LOCK_POLL_INTERVAL);
                }
                Err(err) => {
                    return Err(format!(
                        "failed to create wasm startup lock {:?}: {}",
                        lock_path, err
                    ));
                }
            }
        }
    }
}

impl Drop for StartupMutexGuard {
    fn drop(&mut self) {
        let _ = remove_lock_file_if_exists(&self.lock_path);
    }
}

fn startup_lock_is_stale(
    lock_path: &Path,
    probes: &impl ServerManagerProbes,
) -> Result<bool, String> {
    let content = match fs::read_to_string(lock_path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Ok(file_older_than(lock_path, STARTUP_LOCK_STALE_TIMEOUT)),
    };
    let parsed_lock = parse_startup_lock_info(&content);
    if let Some(started_at_ms) = parsed_lock.started_at_ms {
        let max_age_ms = STARTUP_LOCK_STALE_TIMEOUT.as_millis() as u64;
        if probes.now_unix_millis().saturating_sub(started_at_ms) >= max_age_ms {
            return Ok(true);
        }
    }
    if let Some(pid) = parsed_lock.pid {
        return Ok(!probes.is_pid_alive(pid));
    }
    Ok(file_older_than(lock_path, STARTUP_LOCK_STALE_TIMEOUT))
}

struct StartupLockInfo {
    pid: Option<u32>,
    started_at_ms: Option<u64>,
}

fn parse_startup_lock_info(content: &str) -> StartupLockInfo {
    let mut parsed = StartupLockInfo {
        pid: None,
        started_at_ms: None,
    };
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("pid=") {
            if let Ok(pid) = value.trim().parse::<u32>() {
                parsed.pid = Some(pid);
            }
        } else if let Some(value) = line.strip_prefix("started_at=") {
            if let Ok(started_at_ms) = value.trim().parse::<u64>() {
                parsed.started_at_ms = Some(started_at_ms);
            }
        }
    }
    parsed
}

fn file_older_than(path: &Path, min_age: Duration) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    modified.elapsed().unwrap_or_default() >= min_age
}

fn remove_lock_file_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to remove wasm server lock {:?}: {}",
            path, err
        )),
    }
}

fn read_lock_file(path: &Path) -> Result<Option<WasmServerLockMetadata>, String> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let lock = WasmServerLockMetadata::deserialize_json(&content)
                .map_err(|err| format!("invalid wasm server lock JSON: {}", err.msg))?;
            if lock.format_version != LOCK_FORMAT_VERSION {
                return Err(format!(
                    "unsupported wasm server lock format version {}",
                    lock.format_version
                ));
            }
            Ok(Some(lock))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!(
            "failed to read wasm server lock {:?}: {}",
            path, err
        )),
    }
}

fn listen_address(port: u16, lan: bool) -> SocketAddr {
    if lan {
        SocketAddr::new("0.0.0.0".parse().unwrap(), port)
    } else {
        SocketAddr::new("127.0.0.1".parse().unwrap(), port)
    }
}

fn normalize_path_string(path: &Path) -> String {
    let path = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    path.to_string_lossy().replace('\\', "/")
}

trait ServerManagerProbes {
    fn current_pid(&self) -> u32;
    fn now_unix_secs(&self) -> u64;
    fn now_unix_millis(&self) -> u64;
    fn is_pid_alive(&self, pid: u32) -> bool;
    fn terminate_pid(&self, pid: u32) -> Result<(), String>;
    fn pid_owns_port(&self, pid: u32, addr: SocketAddr) -> bool;
    fn is_port_in_use(&self, addr: SocketAddr) -> bool;
    fn wait_for_port_release(&self, addr: SocketAddr, timeout: Duration) -> bool;
    fn describe_port_occupant(&self, addr: SocketAddr) -> Option<String>;
}

struct LiveServerManagerProbes;

impl ServerManagerProbes for LiveServerManagerProbes {
    fn current_pid(&self) -> u32 {
        std::process::id()
    }

    fn now_unix_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_secs())
            .unwrap_or(0)
    }

    fn now_unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_millis() as u64)
            .unwrap_or(0)
    }

    fn is_pid_alive(&self, pid: u32) -> bool {
        pid_is_alive(pid)
    }

    fn terminate_pid(&self, pid: u32) -> Result<(), String> {
        terminate_pid(pid)
    }

    fn pid_owns_port(&self, pid: u32, addr: SocketAddr) -> bool {
        port_occupant_info(addr).is_some_and(|occupant| occupant.pid == pid)
    }

    fn is_port_in_use(&self, addr: SocketAddr) -> bool {
        TcpListener::bind(addr).is_err()
    }

    fn wait_for_port_release(&self, addr: SocketAddr, timeout: Duration) -> bool {
        let start = SystemTime::now();
        loop {
            if !self.is_port_in_use(addr) {
                return true;
            }
            if start.elapsed().unwrap_or_default() >= timeout {
                return false;
            }
            thread::sleep(PORT_RELEASE_POLL_INTERVAL);
        }
    }

    fn describe_port_occupant(&self, addr: SocketAddr) -> Option<String> {
        describe_port_occupant(addr)
    }
}

#[derive(Clone, Debug)]
struct PortOccupant {
    pid: u32,
    command: Option<String>,
}

impl PortOccupant {
    fn describe(&self) -> String {
        match &self.command {
            Some(command) => format!("pid {} ({})", self.pid, command),
            None => format!("pid {}", self.pid),
        }
    }
}

#[cfg(unix)]
fn describe_port_occupant(addr: SocketAddr) -> Option<String> {
    port_occupant_info(addr).map(|occupant| occupant.describe())
}

#[cfg(unix)]
fn port_occupant_info(addr: SocketAddr) -> Option<PortOccupant> {
    let port = addr.port();
    let output = Command::new("lsof")
        .arg("-nP")
        .arg(format!("-iTCP:{port}"))
        .arg("-sTCP:LISTEN")
        .arg("-Fp")
        .arg("-Fc")
        .output()
        .ok()?;

    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }
    parse_lsof_occupant(&output.stdout)
}

#[cfg(unix)]
fn parse_lsof_occupant(stdout: &[u8]) -> Option<PortOccupant> {
    let mut pid: Option<u32> = None;
    let mut command: Option<String> = None;
    for line in String::from_utf8_lossy(stdout).lines() {
        if let Some(rest) = line.strip_prefix('p') {
            if let Ok(parsed_pid) = rest.trim().parse::<u32>() {
                pid = Some(parsed_pid);
                if let Some(command) = command {
                    return Some(PortOccupant {
                        pid: parsed_pid,
                        command: Some(command),
                    });
                }
            }
        } else if let Some(rest) = line.strip_prefix('c') {
            let parsed_command = rest.trim().to_string();
            if !parsed_command.is_empty() {
                command = Some(parsed_command);
                if let Some(pid) = pid {
                    if let Some(command) = command.as_ref() {
                        return Some(PortOccupant {
                            pid,
                            command: Some(command.clone()),
                        });
                    }
                }
            }
        }
    }
    pid.map(|pid| PortOccupant { pid, command: None })
}

#[cfg(windows)]
fn describe_port_occupant(addr: SocketAddr) -> Option<String> {
    port_occupant_info(addr).map(|occupant| occupant.describe())
}

#[cfg(windows)]
fn port_occupant_info(addr: SocketAddr) -> Option<PortOccupant> {
    let port = addr.port();
    let output = Command::new("netstat")
        .arg("-ano")
        .arg("-p")
        .arg("tcp")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid = parse_windows_netstat_pid(&stdout, port)?;
    let task_output = Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .arg("/FO")
        .arg("CSV")
        .arg("/NH")
        .output()
        .ok();
    if let Some(task_output) = task_output {
        if task_output.status.success() {
            if let Some(name) =
                parse_windows_tasklist_name(&String::from_utf8_lossy(&task_output.stdout))
            {
                return Some(PortOccupant {
                    pid,
                    command: Some(name),
                });
            }
        }
    }
    Some(PortOccupant { pid, command: None })
}

#[cfg(windows)]
fn parse_windows_netstat_pid(stdout: &str, port: u16) -> Option<u32> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("TCP") {
            continue;
        }
        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 5 {
            continue;
        }
        let local_addr = columns[1];
        let state = columns[3];
        let pid_col = columns[4];
        if state != "LISTENING" {
            continue;
        }
        if !local_addr.ends_with(&format!(":{port}")) {
            continue;
        }
        if let Ok(pid) = pid_col.parse::<u32>() {
            return Some(pid);
        }
    }
    None
}

#[cfg(windows)]
fn parse_windows_tasklist_name(stdout: &str) -> Option<String> {
    let line = stdout.lines().find(|line| !line.trim().is_empty())?;
    let trimmed = line.trim();
    if trimmed == "INFO: No tasks are running which match the specified criteria." {
        return None;
    }
    let without_prefix = trimmed.strip_prefix('"')?;
    let name = without_prefix.split("\",\"").next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(not(any(unix, windows)))]
fn describe_port_occupant(_addr: SocketAddr) -> Option<String> {
    None
}

#[cfg(not(any(unix, windows)))]
fn port_occupant_info(_addr: SocketAddr) -> Option<PortOccupant> {
    None
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    unsafe {
        if kill(pid as i32, 0) == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
    }
}

#[cfg(unix)]
fn terminate_pid(pid: u32) -> Result<(), String> {
    unsafe {
        if kill(pid as i32, SIGTERM) != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(ESRCH) {
                return Err(format!("failed to terminate pid {}: {}", pid, err));
            }
        }
    }

    let start = SystemTime::now();
    while pid_is_alive(pid) && start.elapsed().unwrap_or_default() < PID_EXIT_WAIT_TIMEOUT {
        thread::sleep(PID_EXIT_POLL_INTERVAL);
    }
    if !pid_is_alive(pid) {
        return Ok(());
    }

    unsafe {
        if kill(pid as i32, SIGKILL) != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(ESRCH) {
                return Err(format!("failed to force kill pid {}: {}", pid, err));
            }
        }
    }

    let kill_start = SystemTime::now();
    while pid_is_alive(pid) && kill_start.elapsed().unwrap_or_default() < PID_EXIT_WAIT_TIMEOUT {
        thread::sleep(PID_EXIT_POLL_INTERVAL);
    }
    if pid_is_alive(pid) {
        Err(format!("pid {} did not exit after SIGKILL", pid))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
const EPERM: i32 = 1;
#[cfg(unix)]
const ESRCH: i32 = 3;
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut exit_code = 0u32;
        let ok = GetExitCodeProcess(process, &mut exit_code);
        CloseHandle(process);
        ok != 0 && exit_code == STILL_ACTIVE
    }
}

#[cfg(windows)]
fn terminate_pid(pid: u32) -> Result<(), String> {
    unsafe {
        let process = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if process.is_null() {
            return Err(format!("failed to open pid {} for termination", pid));
        }
        if TerminateProcess(process, 1) == 0 {
            let _ = CloseHandle(process);
            return Err(format!("failed to terminate pid {}", pid));
        }
        let _ = CloseHandle(process);
    }

    let start = SystemTime::now();
    while pid_is_alive(pid) && start.elapsed().unwrap_or_default() < PID_EXIT_WAIT_TIMEOUT {
        thread::sleep(PID_EXIT_POLL_INTERVAL);
    }
    if pid_is_alive(pid) {
        Err(format!(
            "pid {} did not exit after termination request",
            pid
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
const PROCESS_TERMINATE: u32 = 0x0001;
#[cfg(windows)]
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
#[cfg(windows)]
const STILL_ACTIVE: u32 = 259;

#[cfg(windows)]
type HANDLE = *mut std::ffi::c_void;

#[cfg(windows)]
unsafe extern "system" {
    fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> HANDLE;
    fn TerminateProcess(process: HANDLE, exit_code: u32) -> i32;
    fn GetExitCodeProcess(process: HANDLE, exit_code: *mut u32) -> i32;
    fn CloseHandle(object: HANDLE) -> i32;
}

#[cfg(not(any(unix, windows)))]
fn pid_is_alive(_pid: u32) -> bool {
    false
}

#[cfg(not(any(unix, windows)))]
fn terminate_pid(pid: u32) -> Result<(), String> {
    Err(format!(
        "process replacement is not supported on this platform for pid {}",
        pid
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        collections::HashSet,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct MockProbes {
        current_pid: u32,
        now_unix_secs: u64,
        now_unix_millis: u64,
        alive_pids: HashSet<u32>,
        owned_port_pids: HashSet<u32>,
        terminated_pids: RefCell<Vec<u32>>,
        port_in_use: bool,
        wait_port_release: bool,
        occupant_description: Option<String>,
    }

    impl ServerManagerProbes for MockProbes {
        fn current_pid(&self) -> u32 {
            self.current_pid
        }

        fn now_unix_secs(&self) -> u64 {
            self.now_unix_secs
        }

        fn now_unix_millis(&self) -> u64 {
            self.now_unix_millis
        }

        fn is_pid_alive(&self, pid: u32) -> bool {
            self.alive_pids.contains(&pid)
        }

        fn terminate_pid(&self, pid: u32) -> Result<(), String> {
            self.terminated_pids.borrow_mut().push(pid);
            Ok(())
        }

        fn pid_owns_port(&self, pid: u32, _addr: SocketAddr) -> bool {
            self.owned_port_pids.contains(&pid)
        }

        fn is_port_in_use(&self, _addr: SocketAddr) -> bool {
            self.port_in_use
        }

        fn wait_for_port_release(&self, _addr: SocketAddr, _timeout: Duration) -> bool {
            self.wait_port_release
        }

        fn describe_port_occupant(&self, _addr: SocketAddr) -> Option<String> {
            self.occupant_description.clone()
        }
    }

    fn new_temp_workspace(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("makepad-wasm-server-tests-{name}-{id}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_test_lock(path: &Path, lock: &WasmServerLockMetadata) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, lock.serialize_json()).unwrap();
    }

    fn write_startup_lock(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn lock_file_roundtrip() {
        let workspace = new_temp_workspace("roundtrip");
        let lock_path = lock_path_for_port(&workspace, 8010);
        let lock = WasmServerLockMetadata {
            format_version: LOCK_FORMAT_VERSION,
            pid: 4242,
            port: 8010,
            workspace_root: normalize_path_string(&workspace),
            crate_name: "makepad-example-counter".to_string(),
            profile: "release".to_string(),
            started_at: 123456,
        };
        write_test_lock(&lock_path, &lock);

        let loaded = read_lock_file(&lock_path).unwrap().unwrap();
        assert_eq!(loaded, lock);
    }

    #[test]
    fn stale_lock_is_removed_during_prepare() {
        let workspace = new_temp_workspace("stale");
        let lock_path = lock_path_for_port(&workspace, 8010);
        let startup_lock_path = startup_lock_path_for_port(&workspace, 8010);
        write_test_lock(
            &lock_path,
            &WasmServerLockMetadata {
                format_version: LOCK_FORMAT_VERSION,
                pid: 900001,
                port: 8010,
                workspace_root: normalize_path_string(&workspace),
                crate_name: "old-app".to_string(),
                profile: "release".to_string(),
                started_at: 1,
            },
        );

        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            now_unix_millis: 555_000,
            alive_pids: HashSet::new(),
            owned_port_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
            occupant_description: None,
        };

        let guard = WasmServerOwnershipGuard::prepare_with_probes(
            &workspace, "new-app", "release", 8010, false, &probes,
        )
        .unwrap();

        assert!(!lock_path.exists(), "stale lock should be removed");
        assert!(
            startup_lock_path.exists(),
            "startup lock should be held while guard is alive"
        );
        assert!(probes.terminated_pids.borrow().is_empty());
        drop(guard);
        assert!(
            !startup_lock_path.exists(),
            "startup lock should be cleaned when guard drops"
        );
    }

    #[test]
    fn stale_startup_lock_is_recovered() {
        let workspace = new_temp_workspace("startup-stale");
        let startup_lock_path = startup_lock_path_for_port(&workspace, 8010);
        write_startup_lock(&startup_lock_path, "pid=999001\nstarted_at=1\n");

        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            now_unix_millis: 555_000,
            alive_pids: HashSet::new(),
            owned_port_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
            occupant_description: None,
        };

        let guard = WasmServerOwnershipGuard::prepare_with_probes(
            &workspace, "new-app", "release", 8010, false, &probes,
        )
        .unwrap();
        assert!(
            startup_lock_path.exists(),
            "startup lock should be re-acquired after stale recovery"
        );
        drop(guard);
        assert!(
            !startup_lock_path.exists(),
            "startup lock should be removed on drop"
        );
    }

    #[test]
    fn live_startup_lock_blocks_prepare() {
        let workspace = new_temp_workspace("startup-live");
        let startup_lock_path = startup_lock_path_for_port(&workspace, 8010);
        write_startup_lock(&startup_lock_path, "pid=42\nstarted_at=555000\n");

        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            now_unix_millis: 555_000,
            alive_pids: HashSet::from([42]),
            owned_port_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
            occupant_description: None,
        };

        let err = match WasmServerOwnershipGuard::prepare_with_probes(
            &workspace, "new-app", "release", 8010, false, &probes,
        ) {
            Ok(_) => panic!("prepare should fail while live startup lock is held"),
            Err(err) => err,
        };
        assert!(
            err.contains("startup is already in progress"),
            "unexpected error: {err}"
        );
        assert!(
            startup_lock_path.exists(),
            "existing startup lock must remain"
        );
    }

    #[test]
    fn startup_scenario_decision_matrix() {
        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            now_unix_millis: 555_000,
            alive_pids: HashSet::new(),
            owned_port_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
            occupant_description: None,
        };
        let listen_addr = listen_address(8010, false);
        assert_eq!(
            evaluate_startup_scenario(classify_lock_state(None, listen_addr, &probes), probes.port_in_use),
            StartupScenario::NoLock
        );

        let stale_lock = WasmServerLockMetadata {
            format_version: LOCK_FORMAT_VERSION,
            pid: 42,
            port: 8010,
            workspace_root: "/tmp/work".to_string(),
            crate_name: "app".to_string(),
            profile: "release".to_string(),
            started_at: 0,
        };
        assert_eq!(
            evaluate_startup_scenario(
                classify_lock_state(Some(&stale_lock), listen_addr, &probes),
                probes.port_in_use
            ),
            StartupScenario::StaleLock
        );
        assert_eq!(
            evaluate_startup_scenario(LockState::StaleLock, true),
            StartupScenario::UnknownOccupant
        );

        let live_probes = MockProbes {
            alive_pids: HashSet::from([42]),
            owned_port_pids: HashSet::from([42]),
            ..probes
        };
        assert_eq!(
            evaluate_startup_scenario(
                classify_lock_state(Some(&stale_lock), listen_addr, &live_probes),
                live_probes.port_in_use
            ),
            StartupScenario::LiveLock
        );

        let occupied_probes = MockProbes {
            port_in_use: true,
            ..live_probes
        };
        assert_eq!(
            evaluate_startup_scenario(LockState::NoLock, occupied_probes.port_in_use),
            StartupScenario::UnknownOccupant
        );
    }

    #[test]
    fn unknown_occupant_error_includes_pid_hint() {
        let workspace = new_temp_workspace("unknown-occupant");
        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            now_unix_millis: 555_000,
            alive_pids: HashSet::new(),
            owned_port_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: true,
            wait_port_release: true,
            occupant_description: Some("pid 4321 (python3)".to_string()),
        };

        let err = match WasmServerOwnershipGuard::prepare_with_probes(
            &workspace, "new-app", "release", 8010, false, &probes,
        ) {
            Ok(_) => panic!("prepare should fail when port is occupied"),
            Err(err) => err,
        };
        assert!(
            err.contains("pid 4321 (python3)"),
            "unexpected error text: {err}"
        );
    }

    #[test]
    fn lock_pid_reuse_is_treated_as_stale_without_termination() {
        let workspace = new_temp_workspace("pid-reuse");
        let lock_path = lock_path_for_port(&workspace, 8010);
        write_test_lock(
            &lock_path,
            &WasmServerLockMetadata {
                format_version: LOCK_FORMAT_VERSION,
                pid: 4242,
                port: 8010,
                workspace_root: normalize_path_string(&workspace),
                crate_name: "old-app".to_string(),
                profile: "release".to_string(),
                started_at: 1,
            },
        );

        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            now_unix_millis: 555_000,
            alive_pids: HashSet::from([4242]),
            owned_port_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
            occupant_description: None,
        };

        let guard = WasmServerOwnershipGuard::prepare_with_probes(
            &workspace, "new-app", "release", 8010, false, &probes,
        )
        .unwrap();
        assert!(!lock_path.exists(), "reused-pid lock should be removed");
        assert!(
            probes.terminated_pids.borrow().is_empty(),
            "reused pid must not be terminated when it does not own the port"
        );
        drop(guard);
    }

    #[test]
    fn startup_lock_ages_out_even_if_pid_is_alive() {
        let workspace = new_temp_workspace("startup-lock-aged");
        let startup_lock_path = startup_lock_path_for_port(&workspace, 8010);
        write_startup_lock(&startup_lock_path, "pid=42\nstarted_at=1000\n");

        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            now_unix_millis: 1000 + STARTUP_LOCK_STALE_TIMEOUT.as_millis() as u64 + 1,
            alive_pids: HashSet::from([42]),
            owned_port_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
            occupant_description: None,
        };

        assert!(
            startup_lock_is_stale(&startup_lock_path, &probes).unwrap(),
            "startup lock should be stale once it exceeds max age"
        );
    }
}
