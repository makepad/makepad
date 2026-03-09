use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, SerJson, SerJsonState};
use std::{
    fs,
    io::ErrorKind,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LOCK_FORMAT_VERSION: u32 = 1;
const PORT_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
const PORT_RELEASE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const PID_EXIT_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const PID_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

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
        fs::write(&self.lock_path, self.metadata.serialize_json()).map_err(|err| {
            format!(
                "failed to write wasm server lock {:?}: {}",
                self.lock_path, err
            )
        })?;
        self.active = true;
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

        let lock_state = classify_lock_state(maybe_lock.as_ref(), probes);
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
    probes: &impl ServerManagerProbes,
) -> LockState {
    match lock {
        Some(lock) if probes.is_pid_alive(lock.pid) => LockState::LiveLock,
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
    fn is_pid_alive(&self, pid: u32) -> bool;
    fn terminate_pid(&self, pid: u32) -> Result<(), String>;
    fn is_port_in_use(&self, addr: SocketAddr) -> bool;
    fn wait_for_port_release(&self, addr: SocketAddr, timeout: Duration) -> bool;
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

    fn is_pid_alive(&self, pid: u32) -> bool {
        pid_is_alive(pid)
    }

    fn terminate_pid(&self, pid: u32) -> Result<(), String> {
        terminate_pid(pid)
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
        alive_pids: HashSet<u32>,
        terminated_pids: RefCell<Vec<u32>>,
        port_in_use: bool,
        wait_port_release: bool,
    }

    impl ServerManagerProbes for MockProbes {
        fn current_pid(&self) -> u32 {
            self.current_pid
        }

        fn now_unix_secs(&self) -> u64 {
            self.now_unix_secs
        }

        fn is_pid_alive(&self, pid: u32) -> bool {
            self.alive_pids.contains(&pid)
        }

        fn terminate_pid(&self, pid: u32) -> Result<(), String> {
            self.terminated_pids.borrow_mut().push(pid);
            Ok(())
        }

        fn is_port_in_use(&self, _addr: SocketAddr) -> bool {
            self.port_in_use
        }

        fn wait_for_port_release(&self, _addr: SocketAddr, _timeout: Duration) -> bool {
            self.wait_port_release
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
            alive_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
        };

        let guard = WasmServerOwnershipGuard::prepare_with_probes(
            &workspace, "new-app", "release", 8010, false, &probes,
        )
        .unwrap();

        assert!(!lock_path.exists(), "stale lock should be removed");
        assert!(probes.terminated_pids.borrow().is_empty());
        drop(guard);
    }

    #[test]
    fn startup_scenario_decision_matrix() {
        let probes = MockProbes {
            current_pid: 100,
            now_unix_secs: 555,
            alive_pids: HashSet::new(),
            terminated_pids: RefCell::new(Vec::new()),
            port_in_use: false,
            wait_port_release: true,
        };
        assert_eq!(
            evaluate_startup_scenario(LockState::NoLock, probes.port_in_use),
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
                classify_lock_state(Some(&stale_lock), &probes),
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
            ..probes
        };
        assert_eq!(
            evaluate_startup_scenario(
                classify_lock_state(Some(&stale_lock), &live_probes),
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
}
