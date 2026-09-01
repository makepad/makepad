//! The machine-local layer: one user, one box, many apps (aicore.md §3/§4).
//!
//! Three facilities, all rooted in `~/.makepad/run` (0700, [`crate::home`]):
//!
//! - **Node entries**: each in-process hub that serves anything writes
//!   `run/<node_key>.node` with its loopback port and pipes hash. This is the
//!   local rendezvous — no loopback broadcast exists or is needed. The entry
//!   dies with the process (best-effort removal on drop; a stale file is
//!   detected by its lock, not its presence).
//! - **The machine token**: `run/machine-token`, minted once at mode 0600
//!   (the asset-server admin-token pattern). Every local call between nodes
//!   must present it: a *different user* on a shared machine who guesses a
//!   port still cannot drive anyone's node.
//! - **The residency lock**: heavy model residency is arbitrated by one
//!   advisory file lock per model id. The lock IS the election (aicore §3):
//!   whoever holds it hosts the model; process death releases it at the OS
//!   level, so takeover needs no keepalive, no timeout, and no stale-owner
//!   recovery path. The holder publishes its state (loading / ready / failed)
//!   INTO the lock file so the losers wait on one load instead of stampeding
//!   into N copies — and a failed load is published too, with backoff, so
//!   four processes do not retry into the same wall in turn.
//!
//! Everything here is machine-local trust. Nothing touches the LAN.

use crate::home;
use crate::sha256::{to_hex, Sha256};
use makepad_micro_serde::*;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

// ---------------------------------------------------------------- the token

/// The machine token: minted once, mode 0600, presented on every local call.
/// Reading it succeeding IS the authorization — only this user can.
pub fn machine_token() -> io::Result<String> {
    let path = home::run_dir().join("machine-token");
    match fs::read_to_string(&path) {
        Ok(token) if token.trim().len() >= 32 => return Ok(token.trim().to_string()),
        _ => {}
    }
    let token = mint_token()?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    Ok(token)
}

/// 32 hex chars of OS randomness.
fn mint_token() -> io::Result<String> {
    Ok(to_hex(&rand16()?))
}

/// Load or mint a persisted 16-byte identity in the runtime dir (the
/// store's `server-id` pattern). Public value, stable across restarts.
pub(crate) fn load_or_create_id(name: &str) -> io::Result<[u8; 16]> {
    let path = home::run_dir().join(name);
    if let Ok(text) = fs::read_to_string(&path) {
        if let Some(id) = from_hex16(text.trim()) {
            return Ok(id);
        }
    }
    let id = rand16()?;
    fs::write(&path, to_hex(&id))?;
    Ok(id)
}

fn from_hex16(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        let hex = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(hex, 16).ok()?;
    }
    Some(out)
}

/// 16 bytes of OS randomness. `/dev/urandom` on unix; on other platforms
/// entropy is gathered from time, pid and allocation addresses and hashed —
/// weaker, and marked so a future windows FFI can replace it.
fn rand16() -> io::Result<[u8; 16]> {
    let mut bytes = [0u8; 16];
    #[cfg(unix)]
    {
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    }
    #[cfg(not(unix))]
    {
        let mut hasher = Sha256::new();
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        hasher.update(&t.to_le_bytes());
        hasher.update(&std::process::id().to_le_bytes());
        let probe = Box::new(0u64);
        hasher.update(&(&*probe as *const u64 as usize).to_le_bytes());
        bytes.copy_from_slice(&hasher.finish()[..16]);
    }
    Ok(bytes)
}

// ------------------------------------------------------------ node entries

/// What a serving node advertises to the other apps on this machine.
#[derive(Clone, Debug, SerJson, DeJson, PartialEq)]
pub struct NodeEntry {
    pub pid: u64,
    /// Loopback port the node's local surface listens on. 0 = not serving.
    pub port: u16,
    /// Hash of the published pipe set; a change tells peers to re-read.
    pub pipes_hash: u32,
}

/// Write this node's entry; returns the path for cleanup on shutdown.
pub fn write_node_entry(node_key: &str, entry: &NodeEntry) -> io::Result<PathBuf> {
    let path = home::run_dir().join(format!("{}.node", sanitize(node_key)));
    fs::write(&path, entry.serialize_json())?;
    Ok(path)
}

/// Every entry in the runtime dir. Staleness is the READER's problem: check
/// the pid or just try the port — the entry file itself proves nothing.
pub fn read_node_entries() -> Vec<(String, NodeEntry)> {
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir(home::run_dir()) else {
        return out;
    };
    for item in dir.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let Some(key) = name.strip_suffix(".node") else {
            continue;
        };
        if let Ok(text) = fs::read_to_string(item.path()) {
            if let Ok(entry) = NodeEntry::deserialize_json(&text) {
                out.push((key.to_string(), entry));
            }
        }
    }
    out
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .take(64)
        .collect()
}

// -------------------------------------------------------- residency election

/// The holder's published state, written into the lock file.
#[derive(Clone, Debug, SerJson, DeJson, PartialEq)]
pub enum ResidencyState {
    /// Weights are streaming in; the fraction is coarse and monotonic.
    Loading { fraction: f64 },
    /// The model is resident and served at this loopback port.
    Ready { port: u16 },
    /// The load failed. Nobody should retry before `retry_after_ms` (unix ms):
    /// a model that OOMs must not have four processes walk into the same wall.
    Failed { reason: String, retry_after_ms: u64 },
}

/// The full record a waiter reads.
#[derive(Clone, Debug, SerJson, DeJson, PartialEq)]
pub struct HolderRecord {
    pub pid: u64,
    pub state: ResidencyState,
}

/// The outcome of trying to become a model's host.
pub enum Claim {
    /// This process is now the host: load the model, publish state through
    /// the guard, serve everyone. Dropping the guard (or dying) releases the
    /// election at the OS level.
    Won(ResidencyGuard),
    /// Somebody else hosts it — route to them, or wait on their `Loading`.
    Held(HolderRecord),
    /// A holder exists but its record is unreadable yet (mid-write). Retry.
    Unreadable,
}

/// Held exclusive advisory lock on one model's residency.
pub struct ResidencyGuard {
    file: File,
    path: PathBuf,
}

impl ResidencyGuard {
    /// Publish the holder's state. Waiters poll [`read_holder`].
    pub fn publish(&mut self, state: ResidencyState) -> io::Result<()> {
        let record = HolderRecord {
            pid: std::process::id() as u64,
            state,
        };
        let text = record.serialize_json();
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(text.as_bytes())?;
        self.file.flush()
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for ResidencyGuard {
    fn drop(&mut self) {
        // Best-effort: empty the record so a reader never sees a dead
        // holder's last state as current. The LOCK release (file close) is
        // what actually ends the election, and the OS does that even on
        // kill -9 — this truncate is only tidiness for the graceful path.
        let _ = self.file.set_len(0);
    }
}

fn residency_path(model_id: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(model_id.as_bytes());
    let digest = hasher.finish();
    let dir = home::run_dir().join("residency");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}-{}.lock", sanitize(model_id), &to_hex(&digest)[..8]))
}

/// Try to become `model_id`'s host on this machine.
///
/// The lock file is never deleted (deleting a locked path would let a second
/// process lock a NEW file at the same name — the classic unlink race), so
/// "file exists" means nothing: only the lock state does.
pub fn claim(model_id: &str) -> io::Result<Claim> {
    let path = residency_path(model_id);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)?;
    if try_lock_exclusive(&file)? {
        return Ok(Claim::Won(ResidencyGuard { file, path }));
    }
    // Held by someone: read their record. A mid-write read can be empty or
    // torn — report Unreadable and let the caller retry in a beat.
    let mut text = String::new();
    let mut reader = &file;
    if reader.read_to_string(&mut text).is_err() || text.trim().is_empty() {
        return Ok(Claim::Unreadable);
    }
    match HolderRecord::deserialize_json(text.trim()) {
        Ok(record) => Ok(Claim::Held(record)),
        Err(_) => Ok(Claim::Unreadable),
    }
}

/// A waiter's view of the current holder. `None` = no live holder (the
/// election is open — call [`claim`]).
pub fn read_holder(model_id: &str) -> io::Result<Option<HolderRecord>> {
    match claim(model_id)? {
        Claim::Won(guard) => {
            // Winning the probe proves nobody holds it; release immediately.
            drop(guard);
            Ok(None)
        }
        Claim::Held(record) => Ok(Some(record)),
        Claim::Unreadable => Ok(None),
    }
}

// ------------------------------------------------------------- lock plumbing

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    // SAFETY: flock on an owned, open fd.
    let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(err)
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x0000_0001;
    const ERROR_LOCK_VIOLATION: i32 = 33;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        h_event: *mut c_void,
    }
    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(std::mem::size_of::<Overlapped>() == 32);

    #[link(name = "kernel32")]
    extern "system" {
        fn LockFileEx(
            file: *mut c_void,
            flags: u32,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    // SAFETY: a zeroed OVERLAPPED with offset 0 locks from the file start;
    // the handle is owned and open.
    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        h_event: std::ptr::null_mut(),
    };
    let rc = unsafe {
        LockFileEx(
            file.as_raw_handle() as *mut c_void,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if rc != 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
        return Ok(false);
    }
    Err(err)
}

#[cfg(not(any(unix, windows)))]
fn try_lock_exclusive(_file: &File) -> io::Result<bool> {
    // Single-process platforms (wasm): the in-process claim always wins.
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env is process-global; every test that redirects MAKEPAD_HOME
    /// runs under this one lock (same law as home.rs's tests).
    fn with_temp_home<R>(name: &str, body: impl FnOnce() -> R) -> R {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "makepad-ai-hub-machine-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let previous = std::env::var_os("MAKEPAD_HOME");
        std::env::set_var("MAKEPAD_HOME", &dir);
        let out = body();
        match previous {
            Some(value) => std::env::set_var("MAKEPAD_HOME", value),
            None => std::env::remove_var("MAKEPAD_HOME"),
        }
        let _ = fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn the_token_is_minted_once_and_stable() {
        with_temp_home("token", || {
            let first = machine_token().unwrap();
            let second = machine_token().unwrap();
            assert_eq!(first, second);
            assert!(first.len() >= 32);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(home::run_dir().join("machine-token"))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600);
            }
        });
    }

    #[test]
    fn node_entries_roundtrip() {
        with_temp_home("nodes", || {
            let entry = NodeEntry {
                pid: 42,
                port: 12345,
                pipes_hash: 7,
            };
            write_node_entry("abcd1234", &entry).unwrap();
            let all = read_node_entries();
            assert_eq!(all, vec![("abcd1234".to_string(), entry)]);
        });
    }

    #[test]
    fn the_election_is_the_lock() {
        with_temp_home("election", || {
            // Win, publish, and let a second open observe Held.
            let Claim::Won(mut guard) = claim("llm.qwen-test").unwrap() else {
                panic!("first claim must win");
            };
            guard
                .publish(ResidencyState::Loading { fraction: 0.5 })
                .unwrap();
            match claim("llm.qwen-test").unwrap() {
                Claim::Held(record) => {
                    assert_eq!(record.pid, std::process::id() as u64);
                    assert_eq!(record.state, ResidencyState::Loading { fraction: 0.5 });
                }
                _ => panic!("second claim must observe the holder"),
            }
            // Ready state replaces loading, whole-record.
            guard.publish(ResidencyState::Ready { port: 4242 }).unwrap();
            match claim("llm.qwen-test").unwrap() {
                Claim::Held(record) => {
                    assert_eq!(record.state, ResidencyState::Ready { port: 4242 })
                }
                _ => panic!("holder must still be observed"),
            }
            // Failure is published too, with its backoff.
            guard
                .publish(ResidencyState::Failed {
                    reason: "oom".into(),
                    retry_after_ms: 123,
                })
                .unwrap();
            // Dropping the guard reopens the election immediately.
            drop(guard);
            assert!(read_holder("llm.qwen-test").unwrap().is_none());
            let Claim::Won(_next) = claim("llm.qwen-test").unwrap() else {
                panic!("election must reopen after release");
            };
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_killed_holder_releases_the_election() {
        with_temp_home("killed", || {
            // A child claims the lock and parks; kill -9 must reopen the
            // election with no cleanup path running anywhere.
            let home_dir = std::env::var_os("MAKEPAD_HOME").unwrap();
            let path = residency_path("llm.kill-test");
            // macOS ships no flock(1); perl's flock is the portable child.
            let script = r#"open(my $f, ">>", $ARGV[0]) or exit 6;
                flock($f, 6) or exit 7; $| = 1; print "locked\n"; sleep 300;"#;
            let mut child = std::process::Command::new("perl")
                .args(["-e", script, &path.display().to_string()])
                .env("MAKEPAD_HOME", &home_dir)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            // Wait for the child to actually hold the lock.
            let mut line = String::new();
            use std::io::BufRead;
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line.trim(), "locked");
            match claim("llm.kill-test").unwrap() {
                Claim::Won(_) => panic!("child must hold the election"),
                _ => {}
            }
            child.kill().unwrap();
            child.wait().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if let Claim::Won(_) = claim("llm.kill-test").unwrap() {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "election must reopen after kill -9"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
    }
}
