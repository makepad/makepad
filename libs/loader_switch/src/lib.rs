//! An installation's loader folders (`<home>/cache/loader`) and the switch between them.
//!
//! A switch's watcher (the Runner that tries an update) holds [`LOCK_FILE`] from before the switch
//! begins until it confirms or rolls back. If the watcher dies, a presented trial can take the
//! lock and confirm. Another start that takes the lock first recovers the pending switch.
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The permanent switch lock file in the loader root. Deleting it splits the lock across two files.
pub const LOCK_FILE: &str = "switch.lock";
/// The permanent prepare lock file in the same root, shared by all loader versions.
pub const PREPARE_LOCK: &str = "prepare.lock";

/// The switch lock, held until dropped.
#[derive(Debug)]
pub struct Lock {
    root: PathBuf,
    _file: Option<File>,
}

/// Protects the loader being prepared or offered, until its trial ends or the offer is dropped.
#[derive(Debug)]
pub struct Prepare(Lock);

impl Prepare {
    pub fn try_hold(root: &Path) -> io::Result<Option<Prepare>> {
        Ok(try_hold(root, PREPARE_LOCK)?.map(Prepare))
    }
}

#[derive(Debug)]
pub enum TryLock {
    Held(Lock),
    /// Another process (or handle) holds it.
    Busy,
    /// The file system takes no locks; the caller treats it as held, as before there was a lock.
    Unsupported(io::Error),
}

fn open(root: &Path, name: &str) -> io::Result<File> {
    OpenOptions::new().create(true).write(true).truncate(false).open(root.join(name))
}

// Some filesystems return these OS codes without std mapping them to Unsupported.
fn locks_unsupported(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Unsupported
        || (cfg!(target_vendor = "apple") && error.raw_os_error() == Some(45)) // ENOTSUP
        || (cfg!(windows) && matches!(error.raw_os_error(), Some(1 | 50))) // INVALID_FUNCTION, NOT_SUPPORTED
}

fn try_acquire(root: &Path, name: &str) -> io::Result<TryLock> {
    let file = open(root, name)?;
    match file.try_lock() {
        Ok(()) => Ok(TryLock::Held(Lock { root: root.into(), _file: Some(file) })),
        Err(TryLockError::WouldBlock) => Ok(TryLock::Busy),
        Err(TryLockError::Error(e)) if locks_unsupported(&e) => Ok(TryLock::Unsupported(e)),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

// On a filesystem without locking, retain the original single-Runner behaviour.
fn unsupported(root: &Path, name: &str, error: io::Error) -> Lock {
    static WARN: std::sync::Once = std::sync::Once::new();
    WARN.call_once(|| {
        let kind = name.trim_end_matches(".lock");
        let _ = writeln!(io::stderr(), "makepad: loader {kind} lock unsupported: {error}");
    });
    Lock { root: root.into(), _file: None }
}

fn try_hold(root: &Path, name: &str) -> io::Result<Option<Lock>> {
    match try_acquire(root, name)? {
        TryLock::Held(lock) => Ok(Some(lock)),
        TryLock::Busy => Ok(None),
        TryLock::Unsupported(e) => Ok(Some(unsupported(root, name, e))),
    }
}

impl Lock {
    pub fn root(&self) -> &Path { &self.root }

    /// Takes the switch lock if it is free; never waits.
    pub fn try_acquire(root: &Path) -> io::Result<TryLock> { try_acquire(root, LOCK_FILE) }

    /// A switch owner, or None while another watcher holds the lock.
    pub fn try_hold(root: &Path) -> io::Result<Option<Lock>> {
        try_hold(root, LOCK_FILE)
    }

    /// Takes the lock, waiting while another holds it.
    pub fn acquire(root: &Path) -> io::Result<Lock> {
        let file = open(root, LOCK_FILE)?;
        match file.lock() {
            Ok(()) => Ok(Lock { root: root.into(), _file: Some(file) }),
            Err(e) if locks_unsupported(&e) => Ok(unsupported(root, LOCK_FILE, e)),
            Err(e) => Err(e),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Active { pub name: String, pub dir: PathBuf, pub binary: PathBuf }

pub fn commit_name(name: &str) -> bool { name.len() == 40 && name.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) }
pub fn folder_name(name: &str) -> bool { name == "src" || commit_name(name) }
pub fn pointer(root: &Path, key: &str) -> Option<String> {
    fs::read_to_string(root.join(key)).ok().map(|v| v.trim().to_string()).filter(|v| folder_name(v))
}
pub const HOST_BINARY: &str = if cfg!(windows) { "host/makepad.exe" } else { "host/makepad" };
pub fn target_root(root: &Path) -> PathBuf {
    root.parent().and_then(Path::parent).expect("loader root is under home/cache").join("target/loader")
}
pub fn target_dir(root: &Path, name: &str) -> PathBuf {
    let target = target_root(root).join(name);
    if target.join(HOST_BINARY).is_file() { target } else { root.join(name).join("target") }
}
pub fn named(root: &Path, name: &str) -> Active {
    let dir = root.join(name);
    let binary = target_dir(root, name).join(HOST_BINARY);
    Active { name: name.into(), dir, binary }
}
fn select(root: &Path, key: &str) -> Active {
    if let Some(name) = pointer(root, key) {
        let selected = named(root, &name);
        if selected.binary.is_file() { return selected; }
    }
    named(root, "src")
}

pub fn active(root: &Path) -> Active { select(root, "current") }

/// The loader a starter can run straight through recovery. Both kinds of begin publish
/// previous before pending. While pending exists, it is previous that rollback restores.
pub fn confirmed(root: &Path) -> Active {
    if root.join("pending").exists() { select(root, "previous") } else { active(root) }
}

pub fn atomic_write(path: &Path, text: &str) -> io::Result<()> {
    let part = path.with_file_name(format!("{}.part", path.file_name().unwrap().to_string_lossy()));
    let mut file = fs::File::create(&part)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(part, path)?;
    sync_parent(path)
}

// Order directory entries as well as file contents across a power failure on Unix.
fn sync_parent(path: &Path) -> io::Result<()> {
    if cfg!(unix) { File::open(path.parent().unwrap())?.sync_all()?; }
    #[cfg(test)]
    switch_tests::write_completed()?;
    Ok(())
}
fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }?;
    sync_parent(path)
}
fn invalid(text: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, text) }
fn check_can_begin(root: &Path) -> io::Result<()> {
    if root.join("pending").try_exists()? {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "A loader switch is already pending"));
    }
    // Another Runner may have queued its update before recovery wrote the note.
    if root.join("rolled-back").try_exists()? {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Dismiss the previous rollback before starting an update"));
    }
    Ok(())
}

/// `record`: the Runner's pin for the new loader (runner_code), written into its folder after the
/// checks and before any pointer moves, so a refused switch never touches a folder's pin.
pub fn begin_switch(lock: &Lock, new_name: &str, record: &str) -> io::Result<()> {
    let root = lock.root();
    if !commit_name(new_name) { return Err(invalid("Invalid loader version")); }
    check_can_begin(root)?;
    let next = named(root, new_name);
    checked_dir(root, new_name)?;
    if !next.binary.is_file() || !next.dir.join(".built").is_file() { return Err(invalid("The new loader has not finished building")); }
    let old = active(root);
    if old.name == new_name { return Err(invalid("The new loader is already selected")); }
    remove_file(&next.dir.join("runner.launch.previous"))?;
    atomic_write(&next.dir.join("runner.launch"), record)?;
    atomic_write(&root.join("previous"), &old.name)?;
    atomic_write(&root.join("pending"), new_name)?;
    atomic_write(&root.join("current"), new_name)
}
pub fn begin_code_switch(lock: &Lock, name: &str, record: &str) -> io::Result<()> {
    let root = lock.root();
    check_can_begin(root)?;
    if !folder_name(name) || active(root).name != name { return Err(invalid("The running loader is no longer selected")); }
    let dir = checked_dir(root, name)?;
    let path = dir.join("runner.launch");
    let old = fs::read_to_string(&path)?;
    if old == record { return Err(invalid("The Runner already has this launch record")); }
    atomic_write(&dir.join("runner.launch.previous"), &old)?;
    atomic_write(&root.join("previous"), name)?;
    atomic_write(&root.join("pending"), name)?;
    atomic_write(&path, record)
}

// Preserve pending on an I/O error. A missing or invalid folder has no backup to recover.
fn pending_dir(root: &Path) -> io::Result<Option<PathBuf>> {
    let name = match fs::read_to_string(root.join("pending")) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        result => result?,
    };
    if !folder_name(name.trim()) { return Ok(None); }
    match checked_dir(root, name.trim()) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        result => result.map(Some),
    }
}

pub fn confirm_boot(lock: &Lock) -> io::Result<()> {
    let root = lock.root();
    if let Some(dir) = pending_dir(root)? {
        remove_file(&dir.join("runner.launch.previous"))?;
    }
    remove_file(&root.join("pending"))
}
fn pending_is(root: &Path, dir: &Path) -> bool {
    pointer(root, "pending").is_some_and(|name| root.join(name) == dir)
}
fn has_backup(dir: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(dir.join("runner.launch.previous")) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub const TRIAL_ENV: &str = "MAKEPAD_RUNNER_TRIAL";
pub const TRIAL_FILE: &str = "trial";

/// Publish the token for the next trial before beginning its switch.
pub fn write_trial(lock: &Lock) -> io::Result<String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(io::Error::other)?;
    let token = format!("{}-{}", std::process::id(), now.as_nanos());
    atomic_write(&lock.root().join(TRIAL_FILE), &token)?;
    Ok(token)
}

/// A loader trial has its own folder. A code trial shares the confirmed folder and needs its token.
pub fn is_trial(root: &Path, dir: &Path, token: Option<&str>) -> bool {
    pending_is(root, dir) && (
        !pointer(root, "previous").is_some_and(|name| root.join(name) == dir)
        || token.is_some_and(|token| fs::read_to_string(root.join(TRIAL_FILE)).is_ok_and(|value| token == value.trim()))
    )
}

/// Read the confirmed record for other starts while the trial reads its own. Recheck the
/// choice after the read because begin, confirm or rollback can change it without a reader lock.
pub fn read_record_for_start(root: &Path, dir: &Path, trial: bool) -> io::Result<String> {
    let record = dir.join("runner.launch");
    let backup = dir.join("runner.launch.previous");
    let choice = || -> io::Result<&Path> {
        if !trial && pending_is(root, dir) && has_backup(dir)? { Ok(&backup) } else { Ok(&record) }
    };
    loop {
        let path = choice()?;
        let text = match fs::read_to_string(path) {
            Err(e) if path == backup && e.kind() == io::ErrorKind::NotFound => continue,
            result => result?,
        };
        if choice()? == path { return Ok(text); }
    }
}

/// Called after the trial has presented and acquired the switch lock from its watcher.
pub fn confirm_trial(lock: &Lock, dir: &Path) -> io::Result<bool> {
    if !pending_is(lock.root(), dir) { return Ok(false); }
    confirm_boot(lock)?;
    Ok(true)
}

pub fn rollback(lock: &Lock, reason: &str) -> io::Result<Active> {
    let root = lock.root();
    let target = confirmed(root);
    if !root.join("pending").try_exists()? { return Ok(target); }
    let dir = pending_dir(root)?;
    if target.name == "src" { remove_file(&root.join("current"))?; }
    else { atomic_write(&root.join("current"), &target.name)?; }

    let restore = match &dir { Some(dir) => has_backup(dir)?, None => false };
    // No backup in the same folder: confirm already removed it, or rollback already
    // restored it after writing the note. Finish without changing that record or note.
    let finished_code_switch = !restore && pointer(root, "previous").is_some_and(|name| dir.as_deref() == Some(root.join(name).as_path()));
    let note = root.join("rolled-back");
    if !finished_code_switch && !note.try_exists()? {
        atomic_write(&note, &reason.replace(['\r', '\n'], " "))?;
    }
    if let Some(dir) = dir {
        let record = dir.join("runner.launch");
        if restore { fs::rename(dir.join("runner.launch.previous"), &record)?; }
        // Also sync a previous removal/restore interrupted before its own directory sync.
        sync_parent(&record)?;
    }
    remove_file(&root.join("pending"))?;
    Ok(target)
}

pub fn dismiss_rollback(lock: &Lock) -> io::Result<()> {
    remove_file(&lock.root.join("rolled-back"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settlement { Idle, RolledBack, Live }

/// The common startup path performs only one stat when there is no switch to settle.
pub fn settle(root: &Path) -> io::Result<Settlement> {
    if !root.join("pending").try_exists()? { return Ok(Settlement::Idle); }
    match Lock::try_hold(root)? {
        Some(lock) => settle_locked(&lock),
        None => Ok(Settlement::Live),
    }
}

/// Re-read pending under the lock; the previous owner may already have confirmed.
pub fn settle_locked(lock: &Lock) -> io::Result<Settlement> {
    if !lock.root.join("pending").try_exists()? { return Ok(Settlement::Idle); }
    rollback(lock, "The update did not confirm its first frame.")?;
    Ok(Settlement::RolledBack)
}

/// The installer's `makepad recover [--wait]` command. A waiting installer takes the
/// lock first, including when the watcher has acquired it but has yet to publish pending.
pub fn recover_command(root: &Path, args: &[String]) -> i32 {
    let result = match args {
        [] => settle(root),
        [flag] if flag == "--wait" => match Lock::acquire(root) {
            Ok(lock) => settle_locked(&lock),
            // A fresh home has no switch and no directory for the lock.
            Err(e) if e.kind() == io::ErrorKind::NotFound && matches!(root.try_exists(), Ok(false)) => Ok(Settlement::Idle),
            Err(e) => Err(e),
        },
        _ => {
            let _ = writeln!(io::stderr(), "usage: makepad recover [--wait]");
            return 2;
        }
    };
    match result {
        Ok(Settlement::Idle | Settlement::RolledBack) => 0,
        Ok(Settlement::Live) => 3,
        Err(e) => { let _ = writeln!(io::stderr(), "makepad recover: {e}"); 1 }
    }
}

/// Only direct, real directories in this loader root can be recursively deleted. Symlinks
/// (including a valid-looking commit symlink) are left alone, never followed.
pub fn checked_dir(root: &Path, name: &str) -> io::Result<PathBuf> {
    if !folder_name(name) && name != ".unpack" { return Err(invalid("Invalid loader directory")); }
    let path = root.join(name);
    check_real_dir(&path)?;
    Ok(path)
}
pub fn check_real_dir(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() || fs::canonicalize(path)?.parent() != Some(fs::canonicalize(path.parent().unwrap())?.as_path()) {
        return Err(invalid("Loader directory is not a direct directory inside its root"));
    }
    Ok(())
}

fn checked_target_root(root: &Path) -> io::Result<PathBuf> {
    let targets = target_root(root);
    // A link at target or target/loader would take cleanup outside the install.
    for dir in [targets.parent().unwrap(), targets.as_path()] {
        match check_real_dir(dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            result => result?,
        }
    }
    Ok(targets)
}

/// Create a clean source directory for a build under the installation's prepare lock.
pub fn prepare_version(prepare: &Prepare, name: &str) -> io::Result<PathBuf> {
    let root = prepare.0.root();
    if !commit_name(name) { return Err(invalid("Invalid loader version")); }
    if root.join("pending").exists() { return Err(invalid("A loader switch is pending")); }
    if active(root).name == name || pointer(root, "previous").as_deref() == Some(name) { return Err(invalid("Cannot replace an active or previous loader")); }
    let source = match checked_dir(root, name) {
        Ok(dir) if dir.join(".built").exists() => return Err(invalid("This loader version is already built with a different pin")),
        Ok(dir) => Some(dir),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    match checked_dir(&checked_target_root(root)?, name) {
        Ok(dir) => fs::remove_dir_all(dir)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(dir) = source { fs::remove_dir_all(dir)?; }
    let path = root.join(name);
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Sweep only when both locks are held. A preparer may be about to try the switch lock.
/// Return `false` only when `prepare.lock` is busy and the sweep must wait for a later start.
/// Return `true` even when a pending switch or an older running loader keeps the folders in place.
pub fn prune(lock: &Lock, running: Option<&Path>) -> io::Result<bool> {
    let root = lock.root();
    let Some(_prepare) = Prepare::try_hold(root)? else { return Ok(false) };
    if root.join("pending").try_exists()? { return Ok(true); }
    let selected = active(root);
    // A directly started older loader can still be using its folder.
    if running.is_some_and(|dir| dir != selected.dir) { return Ok(true); }
    let previous = pointer(root, "previous");
    let targets = checked_target_root(root)?;
    // Sweep each root so an interruption between the two deletions leaves nothing stranded.
    for parent in [root, targets.as_path()] {
        let entries = match fs::read_dir(parent) { Err(e) if e.kind() == io::ErrorKind::NotFound => continue, result => result? };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == selected.name || previous.as_deref() == Some(name) { continue; }
            if !(folder_name(name) || (parent == root && name == ".unpack")) { continue; }
            if !entry.file_type()?.is_dir() { continue; }
            fs::remove_dir_all(checked_dir(parent, name)?)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod switch_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

    /// On Unix a child holds a copy of each file open in this process from its start until its
    /// exec, and with the file any lock taken through it: a flock lock belongs to the open file,
    /// not to the process. So while one test starts a child, a lock another test has just released
    /// can stay busy for a moment. A test that starts children runs `alone()`; a test that takes or
    /// releases locks runs `beside()` the others.
    static STARTS: RwLock<()> = RwLock::new(());
    pub(super) fn alone() -> RwLockWriteGuard<'static, ()> { STARTS.write().unwrap_or_else(|e| e.into_inner()) }
    pub(super) fn beside() -> RwLockReadGuard<'static, ()> { STARTS.read().unwrap_or_else(|e| e.into_inner()) }

    const ROLE: &str = "MAKEPAD_LOADER_SWITCH_TEST_ROLE";
    const ROOT: &str = "MAKEPAD_LOADER_SWITCH_TEST_ROOT";
    const NAME: &str = "tests::the_switch_lock_holds_across_processes";

    fn state(root: &Path) -> &'static str {
        match Lock::try_acquire(root).unwrap() {
            TryLock::Held(_) => "free",
            TryLock::Busy => "busy",
            TryLock::Unsupported(e) => panic!("the lock is unsupported here: {e}"),
        }
    }
    /// This test's own binary again, as one role; its stdout lines come back through `lines`. Its
    /// caller runs `alone()`.
    pub(super) fn spawn(root: &Path, role: &str) -> (Child, impl Iterator<Item = String>) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
            .env(ROLE, role)
            .env(ROOT, root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines().map(|l| l.unwrap());
        (child, lines)
    }
    pub(super) fn answer(lines: &mut impl Iterator<Item = String>, what: &str) -> String {
        lines.find_map(|l| l.strip_prefix("role: ").map(str::to_string)).unwrap_or_else(|| panic!("no answer from the {what} child"))
    }
    /// The roles a child plays. `hold` takes the lock and keeps it until its stdin closes; `wait`
    /// takes it blocking and says so; `idle` only waits for its stdin to close (a process started
    /// while the lock is held, as the trial is); `probe` reports the lock's state.
    fn play(role: &str, root: &Path) {
        let mut out = io::stdout();
        let wait_stdin = || { let _ = io::stdin().read_line(&mut String::new()); };
        match role {
            "hold" => { let _lock = Lock::acquire(root).unwrap(); writeln!(out, "role: held").unwrap(); out.flush().unwrap(); wait_stdin(); }
            "prepare" => {
                let version = super::switch_tests::B;
                let prepare = Prepare::try_hold(root).unwrap().unwrap();
                let source = prepare_version(&prepare, version).unwrap();
                let target = target_root(root).join(version);
                fs::create_dir_all(&target).unwrap();
                fs::write(source.join("source"), b"unpacked").unwrap();
                fs::write(target.join("partial"), b"building").unwrap();
                writeln!(out, "role: preparing").unwrap();
                out.flush().unwrap();
                wait_stdin();
            }
            "wait" => {
                writeln!(out, "role: {}", state(root)).unwrap();
                out.flush().unwrap();
                let _lock = Lock::acquire(root).unwrap();
                writeln!(out, "role: acquired").unwrap();
                out.flush().unwrap();
                wait_stdin();
            }
            "idle" => { writeln!(out, "role: idle").unwrap(); out.flush().unwrap(); wait_stdin(); }
            "probe" => writeln!(out, "role: {}", state(root)).unwrap(),
            other => panic!("unknown role {other}"),
        }
        out.flush().unwrap();
    }
    pub(super) fn free_after_exit(what: &str, free: impl Fn() -> bool) {
        // Windows releases a dead process's locks soon after it ends, not at once (LockFileEx).
        for _ in 0..1000 {
            if free() { return; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the lock stayed held after {what} ended");
    }

    fn waiter_takes_released_lock(root: &Path, release: impl FnOnce()) {
        let (mut waiter, mut lines) = spawn(root, "wait");
        assert_eq!(answer(&mut lines, "waiter"), "busy", "the waiter must encounter the holder");
        release();
        assert_eq!(answer(&mut lines, "waiter"), "acquired", "a blocking take must finish after release");
        assert_eq!(state(root), "busy", "the waiter must own the lock");
        drop(waiter.stdin.take());
        assert!(waiter.wait().unwrap().success());
    }

    /// The watcher's liveness, as the loader reads it: a lock another process holds is busy, it is
    /// free once that process ends (closed or killed), a process started while it was held does not
    /// keep it, and a blocking take waits for the holder.
    #[test]
    fn the_switch_lock_holds_across_processes() {
        if let (Ok(role), Some(root)) = (std::env::var(ROLE), std::env::var_os(ROOT)) {
            return play(&role, Path::new(&root));
        }
        let _alone = alone();
        let root: PathBuf = std::env::temp_dir().join(format!("loader-switch-lock-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        // Held here: another process sees it busy, and takes it once it is dropped here.
        let here = Lock::acquire(&root).unwrap();
        assert!(matches!(Lock::try_acquire(&root).unwrap(), TryLock::Busy), "a second handle in one process must see the lock busy");
        let (mut probe, mut lines) = spawn(&root, "probe");
        assert_eq!(answer(&mut lines, "probe"), "busy", "another process must see a held lock busy");
        assert!(probe.wait().unwrap().success());
        // A process started while it is held (the trial) does not inherit it.
        let (mut idle, mut idle_lines) = spawn(&root, "idle");
        assert_eq!(answer(&mut idle_lines, "idle"), "idle");
        drop(here);
        let (mut probe, mut lines) = spawn(&root, "probe");
        assert_eq!(answer(&mut lines, "probe"), "free", "a child started while the lock was held must not keep it");
        assert!(probe.wait().unwrap().success());
        drop(idle.stdin.take());
        assert!(idle.wait().unwrap().success());

        // Held by another process: busy here until it exits, and free after it is killed.
        for end in ["exit", "kill"] {
            let (mut holder, mut lines) = spawn(&root, "hold");
            assert_eq!(answer(&mut lines, "holder"), "held");
            assert_eq!(state(&root), "busy", "a lock another process holds must be busy here");
            if end == "kill" {
                waiter_takes_released_lock(&root, || {
                    holder.kill().unwrap();
                    assert!(!holder.wait().unwrap().success(), "the holder was killed while the waiter was starting its take");
                });
            } else {
                drop(holder.stdin.take());
                assert!(holder.wait().unwrap().success());
            }
            free_after_exit(&format!("its holder's {end}"), || state(&root) == "free");
        }

        // A blocking take waits while the lock is held here and returns once it is dropped.
        let here = Lock::acquire(&root).unwrap();
        waiter_takes_released_lock(&root, || drop(here));
        let _ = std::fs::remove_dir_all(&root);
    }
}
