//! Box-local dev JITs in `<work>/dev/jit`, keyed by pinned inputs. Per-id locks coalesce
//! builds; a build an earlier hub started is joined through its recorded pid. The tree lock
//! serializes Cargo's committed checkout. Lease holds survive hub restarts. LRU keeps six
//! binaries plus any older ones a live lease still uses.
//!
//! A build in flight leaves a note beside its lock ([`BuildNote`], `<id>.build`): which lease
//! builds it, from when, and its step (waiting for the build tree behind another id's build,
//! cargo, publishing). Its waiters' lines say that, not just "waiting". The note goes when the
//! build ends with an outcome; a waiter that takes the lock over with the note still there joins
//! the build's cargo when it runs on past its hub's restart, runs cargo again on what it built
//! when it ended after its hub (and publishes that), and else knows the build died without an
//! outcome (its thread panicked, its hub ended before cargo): it says so to its holder and to
//! the master's inbox, and builds it at once. A failed build is a warning too
//! (a lease waited 13 min behind a build that died with its hub and a prebuild of another id
//! holding the tree, saying only "waiting for the store's build", 11-03).

use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

pub(super) const KEEP: usize = 6;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub id: String,
    pub last_use: u64,
    pub hits: u64,
    pub misses: u64,
    pub builds: u64,
    pub build_seconds: f64,
    pub day: u64,
    pub builds_today: u64,
    pub error: String,
}

impl Stats {
    fn parse(v: &Value) -> Self {
        let n = |k| v.get(k).and_then(super::num).unwrap_or(0.0);
        Self {
            id: v.get("id").and_then(Value::as_str).unwrap_or("").into(), last_use: n("last_use") as u64,
            hits: n("hits") as u64, misses: n("misses") as u64, builds: n("builds") as u64,
            build_seconds: n("build_seconds"), day: n("day") as u64, builds_today: n("builds_today") as u64,
            error: v.get("error").and_then(Value::as_str).unwrap_or("").into(),
        }
    }

    fn json(&self) -> String {
        format!("{{\"id\":{},\"last_use\":{},\"hits\":{},\"misses\":{},\"builds\":{},\"build_seconds\":{:.3},\"day\":{},\"builds_today\":{},\"error\":{}}}",
            json_str(&self.id), self.last_use, self.hits, self.misses, self.builds, self.build_seconds, self.day, self.builds_today, json_str(&self.error))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub ids: Vec<String>,
    pub hits: u64,
    pub misses: u64,
    pub builds_today: u64,
    pub compiler: String,
    pub per_id: Vec<Stats>,
}

impl Snapshot {
    pub fn to_json(&self) -> String {
        format!("{{\"ids\":[{}],\"hits\":{},\"misses\":{},\"builds_today\":{},\"compiler\":{},\"per_id\":[{}]}}",
            self.ids.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","),
            self.hits, self.misses, self.builds_today,
            json_str(&self.compiler),
            self.per_id.iter().map(Stats::json).collect::<Vec<_>>().join(","))
    }

    pub fn parse(v: &Value) -> Self {
        Self {
            ids: v.get("ids").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect(),
            hits: v.get("hits").and_then(Value::as_u64).unwrap_or(0),
            misses: v.get("misses").and_then(Value::as_u64).unwrap_or(0),
            builds_today: v.get("builds_today").and_then(Value::as_u64).unwrap_or(0),
            compiler: v.get("compiler").and_then(Value::as_str).unwrap_or("").to_string(),
            per_id: v.get("per_id").and_then(Value::as_arr).unwrap_or(&[]).iter().map(Stats::parse).collect(),
        }
    }
}

pub struct Store { root: PathBuf }

/// A store build in flight (`<id>.build` beside its lock).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildNote {
    pub lease: String,
    /// the hub process that runs it
    pub hub: u32,
    pub since_ms: u64,
    pub step: String,
}

impl BuildNote {
    fn json(&self) -> String {
        format!("{{\"lease\":{},\"hub\":{},\"since_ms\":{},\"step\":{}}}", json_str(&self.lease), self.hub, self.since_ms, json_str(&self.step))
    }

    fn parse(b: &[u8]) -> Option<Self> {
        let v = makepad_strict_json::parse(b).ok()?;
        Some(Self {
            lease: v.get("lease")?.as_str()?.to_string(),
            hub: v.get("hub").and_then(Value::as_u64)? as u32,
            since_ms: v.get("since_ms").and_then(Value::as_u64)?,
            step: v.get("step").and_then(Value::as_str).unwrap_or("").to_string(),
        })
    }
}

/// `HH:MM:SSZ` of a wall time.
fn clock(ms: u64) -> String {
    let s = ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

struct Waiter { cancelled: Arc<AtomicBool> }

#[derive(Default)]
struct Waiters {
    leases: Vec<Weak<Waiter>>,
    stopping: bool,
}

// Registration precedes the per-id lock: a waiter behind the builder owns it too.
static BUILD_WAITERS: Mutex<std::collections::BTreeMap<PathBuf, Weak<Mutex<Waiters>>>> = Mutex::new(std::collections::BTreeMap::new());

impl Waiters {
    fn active(&mut self) -> Result<(), String> {
        self.leases.retain(|w| w.upgrade().is_some_and(|w| !w.cancelled.load(Ordering::Acquire)));
        self.stopping |= self.leases.is_empty();
        if self.stopping { Err("dev JIT build cancelled: no lease waits for it".into()) } else { Ok(()) }
    }
}

pub fn lock(path: &Path) -> Result<File, String> {
    std::fs::create_dir_all(path.parent().ok_or("JIT lock has no parent")?).map_err(|e| format!("{}: {e}", path.display()))?;
    let f = OpenOptions::new().create(true).truncate(false).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.lock().map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(f)
}

/// Lease-owned waits must let go when their lease ends, including behind another build.
/// `waiting` says what it waits on: when it first waits, then every [`WAIT_LOOK`] (what it
/// waits behind moves on: another build's step, a build that died).
pub fn lock_while(path: &Path, shared: bool, active: &dyn Fn() -> Result<(), String>, waiting: &dyn Fn()) -> Result<File, String> {
    std::fs::create_dir_all(path.parent().ok_or("JIT lock has no parent")?).map_err(|e| format!("{}: {e}", path.display()))?;
    let f = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut looked: Option<std::time::Instant> = None;
    loop {
        active()?;
        match if shared { f.try_lock_shared() } else { f.try_lock() } {
            Ok(()) => return Ok(f),
            Err(std::fs::TryLockError::WouldBlock) => {
                if looked.is_none_or(|t| t.elapsed() >= WAIT_LOOK) {
                    waiting();
                    looked = Some(std::time::Instant::now());
                }
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("{}: {e}", path.display())),
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// How often a lock waiter looks again at what it waits behind.
pub const WAIT_LOOK: std::time::Duration = std::time::Duration::from_secs(2);

/// Takes the build tree's lock for the build of `id`, saying (`step`) whose build it waits
/// behind meanwhile; the file beside the lock then names `id` for the next one. Beside it: a
/// held lock keeps every other handle from reading the file itself on Windows.
pub fn lock_tree(store: &Store, path: &Path, id: &str, active: &dyn Fn() -> Result<(), String>, step: &dyn Fn(&str)) -> Result<File, String> {
    let holder = path.with_extension("holder");
    let f = lock_while(path, false, active, &|| {
        let other = std::fs::read_to_string(&holder).unwrap_or_default();
        let other = other.trim();
        step(&match store.note(other) {
            Some(n) => format!("waiting for the build tree, behind the build of {other} for {} ({} since {})", n.lease, n.step, clock(n.since_ms)),
            None if !other.is_empty() => format!("waiting for the build tree, behind the build of {other}"),
            None => "waiting for the build tree".to_string(),
        });
    })?;
    if let Err(e) = std::fs::write(&holder, id) { eprintln!("devhub: JIT store holder {}: {e}", holder.display()); }
    Ok(f)
}

/// A store's binary is `<work>/dev/jit/<BINARY_PREFIX><id>`.
pub const BINARY_PREFIX: &str = "makepad.jit-";

/// The bytes of a JIT's copy one `jit-read` / `jit-put` moves ([`Store::read`], [`Store::put`]).
pub const COPY_CHUNK: usize = 256 << 10;

/// What a chunk of a JIT's copy did on the box it goes to ([`Store::put`]).
#[derive(Debug, PartialEq)]
pub enum Put {
    /// the box holds it already: nothing more goes
    Held,
    /// the next chunk starts here
    More(u64),
    /// its last byte: the store keeps it
    Kept,
}

impl Store {
    pub fn new(work: &Path) -> Self { Self { root: work.join("dev/jit") } }
    fn binary(&self, id: &str) -> PathBuf { self.root.join(format!("{BINARY_PREFIX}{id}")) }
    fn stats(&self, id: &str) -> Stats {
        std::fs::read(self.root.join(format!("{id}.json"))).ok().and_then(|b| makepad_strict_json::parse(&b).ok()).map(|v| Stats::parse(&v)).unwrap_or_else(|| Stats { id: id.into(), ..Default::default() })
    }
    fn note_path(&self, id: &str) -> PathBuf { self.root.join(format!("{id}.build")) }

    /// The note of `id`'s build in flight (or of one that died without an outcome).
    pub fn note(&self, id: &str) -> Option<BuildNote> {
        if !Self::valid(id) { return None; }
        std::fs::read(self.note_path(id)).ok().and_then(|b| BuildNote::parse(&b))
    }

    fn write_note(&self, id: &str, n: &BuildNote) {
        let p = self.note_path(id);
        let tmp = p.with_extension("build-part");
        if let Err(e) = std::fs::write(&tmp, n.json()).and_then(|_| std::fs::rename(&tmp, &p)) { eprintln!("devhub: JIT store note {}: {e}", p.display()); }
    }

    /// What a waiter on `id`'s lock says.
    fn waiting_line(&self, id: &str) -> String {
        match self.note(id) {
            Some(n) => format!("waiting for the store's build of {id} for {} ({} since {})", n.lease, n.step, clock(n.since_ms)),
            None => format!("waiting for the store's build of {id}"),
        }
    }

    /// The cargo an earlier hub's build of `id` started: the pid and start time its hub recorded
    /// (`build/<id>/cargo.pid`), and whether that process runs still. The record goes only when
    /// a hub reaped its cargo.
    fn cargo_record(&self, id: &str) -> Option<(u32, bool)> {
        let text = std::fs::read_to_string(self.root.join("build").join(id).join("cargo.pid")).ok()?;
        let mut fields = text.split_whitespace();
        let (pid, start) = (fields.next()?.parse::<u32>().ok()?, fields.next()?.parse::<u64>().ok()?);
        Some((pid, crate::os::same(pid, start)))
    }

    /// Why a build whose note outlived it ended without an outcome.
    fn died_why(n: &BuildNote) -> String {
        if n.hub != std::process::id() {
            if crate::procinfo::alive(n.hub) { format!("its hub (pid {}) let go of it", n.hub) } else { format!("its hub (pid {}) ended, a restart", n.hub) }
        } else {
            "its builder thread ended without an outcome".to_string()
        }
    }

    fn save(&self, s: &Stats) -> Result<(), String> {
        let p = self.root.join(format!("{}.json", s.id));
        let tmp = p.with_extension("part");
        std::fs::write(&tmp, s.json()).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("{}: {e}", p.display()))
    }
    fn valid(id: &str) -> bool { id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit()) }

    /// Hold before lookup/ensure; release only after the lease's processes have stopped.
    pub fn hold(&self, id: &str, lease: &str) -> Result<(), String> {
        if !Self::valid(id) || !super::exec::valid_name(lease) { return Err(format!("invalid JIT key or lease: {id} {lease}")); }
        let _lock = lock(&self.root.join("store.lock"))?;
        let dir = self.root.join("leases").join(lease);
        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(id), "")).map_err(|e| format!("{}: {e}", dir.join(id).display()))
    }

    pub fn release(&self, lease: &str) -> Result<(), String> {
        if !super::exec::valid_name(lease) { return Err("invalid JIT lease".into()); }
        let _lock = lock(&self.root.join("store.lock"))?;
        let p = self.root.join("leases").join(lease);
        // (a lease that held nothing frees nothing to prune: every tree lease's end asks)
        if p.exists() {
            std::fs::remove_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            self.prune();
        }
        Ok(())
    }

    pub fn lookup(&self, id: &str) -> Option<PathBuf> {
        if !Self::valid(id) { return None; }
        let result = (|| -> Result<_, String> {
            let _lock = lock(&self.root.join("store.lock"))?;
            let path = self.binary(id);
            let mut s = self.stats(id);
            let hit = path.is_file();
            if hit { s.hits += 1; s.last_use = crate::proto::wall_ms(); } else { s.misses += 1; }
            self.save(&s)?;
            Ok(hit.then_some(path))
        })();
        result.unwrap_or_else(|e| { eprintln!("devhub: JIT store: {e}"); None })
    }

    /// The binary of `id` held for `lease` until it ends: a lease's explicit `--jit`, which
    /// placement gives only a box that holds it; missing, the error names what the box holds.
    pub fn held(&self, id: &str, lease: &str) -> Result<PathBuf, String> {
        self.hold(id, lease)?;
        self.lookup(id).ok_or_else(|| {
            let ids = self.snapshot().ids;
            format!("JIT {id} is not in this box's store ({})", if ids.is_empty() { "it holds none".to_string() } else { format!("it holds {}", ids.join(" ")) })
        })
    }

    /// A binary cargo built for `id` outside the store (a native tree's run).
    pub fn keep(&self, id: &str, binary: &Path) -> Result<(), String> {
        if !Self::valid(id) { return Err(format!("invalid JIT key {id}")); }
        let _lock = lock(&self.root.join("store.lock"))?;
        let dst = self.binary(id);
        if !dst.is_file() {
            // a binary a run still uses is never replaced: the key says its content is the same
            super::loaders::publish_binary(binary, &dst, &|staged, dst| std::fs::rename(staged, dst).map_err(|e| format!("{}: {e}", dst.display())))?;
        }
        let mut s = self.stats(id);
        s.last_use = crate::proto::wall_ms();
        self.save(&s)?;
        self.prune();
        Ok(())
    }

    /// A chunk of `id`'s binary from `off` (at most [`COPY_CHUNK`] bytes) and the binary's size:
    /// for a box of the same platform that lacks it ([`Self::put`] there).
    pub fn read(&self, id: &str, off: u64) -> Result<(Vec<u8>, u64), String> {
        use std::io::{Read, Seek, SeekFrom};
        if !Self::valid(id) { return Err(format!("invalid JIT key {id}")); }
        let p = self.binary(id);
        let mut f = File::open(&p).map_err(|e| format!("JIT {id} is not in this box's store ({e})"))?;
        let size = f.metadata().map_err(|e| format!("{}: {e}", p.display()))?.len();
        let mut buf = Vec::new();
        f.seek(SeekFrom::Start(off)).and_then(|_| f.take(COPY_CHUNK as u64).read_to_end(&mut buf)).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok((buf, size))
    }

    /// A chunk of `id`'s binary copied in from a box that holds it, `size` bytes in all, the
    /// chunks in order: after its last byte the store keeps it as if built here. At its first
    /// chunk a store that holds `id` takes nothing (the key names the same binary).
    pub fn put(&self, id: &str, off: u64, size: u64, data: &[u8]) -> Result<Put, String> {
        use std::io::{Seek, SeekFrom, Write};
        if !Self::valid(id) { return Err(format!("invalid JIT key {id}")); }
        if off == 0 && self.binary(id).is_file() { return Ok(Put::Held); }
        let end = off + data.len() as u64;
        if size == 0 || end > size { return Err(format!("JIT {id}: a chunk to {end} of a {size} byte binary")); }
        let part = self.root.join("copies").join(id);
        // a chunk past what the part holds: a copy whose start this box did not see (its hub
        // restarted under it) is copied again from its start, never kept with a gap
        let held = std::fs::metadata(&part).map_or(0, |m| m.len());
        if off > 0 && held != off { return Err(format!("JIT {id}: its copy here holds {held} bytes, the chunk starts at {off}: copy it again from its start")); }
        let written = std::fs::create_dir_all(self.root.join("copies"))
            .and_then(|_| OpenOptions::new().write(true).create(true).truncate(off == 0).open(&part))
            .and_then(|mut f| f.seek(SeekFrom::Start(off)).and_then(|_| f.write_all(data)));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{}: {e}", part.display()));
        }
        if end < size { return Ok(Put::More(end)); }
        #[cfg(unix)]
        let kept = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {e}", part.display())).and_then(|_| self.keep(id, &part))
        };
        #[cfg(not(unix))]
        let kept = self.keep(id, &part);
        let _ = std::fs::remove_file(&part);
        kept.map(|_| Put::Kept)
    }

    /// Copies a hub left unfinished when it ended: the master's copy ended with its link.
    pub fn clear_copies(&self) {
        let dir = self.root.join("copies");
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound { eprintln!("devhub: JIT store {}: {e}", dir.display()); }
        }
    }

    /// A waiting requester sees the first builder's result, including its error. `warn` reaches
    /// the master's inbox: a build that died without an outcome, a build that failed. `build`
    /// gets the shared liveness and a way to name its step.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure(&self, id: &str, lease: &str, say: &dyn Fn(&str), pending: &dyn Fn(&str), warn: &dyn Fn(&str), active: &dyn Fn() -> Result<(), String>, cancelled: &Arc<AtomicBool>, build: impl FnOnce(&Path, &dyn Fn() -> Result<(), String>, &dyn Fn(&str)) -> Result<(), String>) -> Result<PathBuf, String> {
        active()?;
        if !Self::valid(id) { return Err(format!("invalid JIT key {id}")); }
        let waiter = Arc::new(Waiter { cancelled: cancelled.clone() });
        let waiters = {
            let mut builds = BUILD_WAITERS.lock().unwrap();
            builds.retain(|_, w| w.strong_count() > 0);
            let path = self.root.join(format!("{id}.lock"));
            let current = builds.get(&path).and_then(Weak::upgrade).unwrap_or_default();
            let mut group = current.lock().unwrap();
            let waiters = if group.stopping {
                Arc::new(Mutex::new(Waiters { leases: vec![Arc::downgrade(&waiter)], ..Default::default() }))
            } else {
                group.leases.push(Arc::downgrade(&waiter));
                current.clone()
            };
            drop(group);
            builds.insert(path, Arc::downgrade(&waiters));
            waiters
        };
        self.hold(id, lease)?;
        let generation = self.stats(id).builds;
        if let Some(p) = self.lookup(id) { return Ok(p); }
        let _build = lock_while(&self.root.join(format!("{id}.lock")), false, active, &|| pending(&self.waiting_line(id)))?;
        active()?;
        // the lock is ours: a note still there is a build whose hub let go of it. Its cargo runs
        // in a session (a scope) of its own and outlives a hub's restart: then the build goes on,
        // and this one joins it ([`Self::cargo_record`]; a node's restart "killed" a build that went
        // on for 5 min more, 11-03). A cargo that ended after its hub did (a restart came as it
        // finished, before its hub published: two boxes' builds "died" whose cargo had run its 54 s
        // to the end, 11-04) left its work in the target: this build's cargo finds it done and
        // publishes it. Without a cargo, the build died without an outcome.
        let note = self.note(id);
        let (died, joining) = match (&note, self.cargo_record(id)) {
            (Some(n), Some((pid, true))) => (None, Some(format!("the store's build of {id} for {} (since {}) goes on after its hub's restart: waiting for its cargo (pid {pid})", n.lease, clock(n.since_ms)))),
            (Some(n), Some((pid, false))) => (None, Some(format!("the store's build of {id} for {} (since {}) went on after its hub's restart and its cargo (pid {pid}) ended: cargo runs again on what it built and publishes it", n.lease, clock(n.since_ms)))),
            (Some(n), None) => (Some(format!("the store's build of {id} for {} (since {}, {}) died: {}", n.lease, clock(n.since_ms), n.step, Self::died_why(n))), None),
            (None, _) => (None, None),
        };
        if note.is_some() { let _ = std::fs::remove_file(self.note_path(id)); }
        if let Some(line) = &died {
            warn(&format!("dev JIT {line}; {lease} builds it now"));
        }
        if let Some(p) = self.binary(id).is_file().then(|| self.binary(id)) { return Ok(p); }
        let previous = self.stats(id);
        if previous.builds != generation && !previous.error.is_empty() { return Err(previous.error); }
        pending(&match (&died, joining) {
            (Some(line), _) => format!("{line}; building it now"),
            (None, Some(line)) => line,
            (None, None) => format!("building the dev JIT {id} (first overlay run on this box)"),
        });
        let t = std::time::Instant::now();
        let note = std::cell::RefCell::new(BuildNote { lease: lease.to_string(), hub: std::process::id(), since_ms: crate::proto::wall_ms(), step: "starting".into() });
        self.write_note(id, &note.borrow());
        let step = |s: &str| {
            let mut n = note.borrow_mut();
            if n.step != s {
                n.step = s.to_string();
                self.write_note(id, &n);
            }
        };
        let shared_active = || waiters.lock().unwrap().active();
        let result = build(&self.binary(id), &shared_active, &step).and_then(|()| {
            let p = self.binary(id);
            if p.is_file() { Ok(p) } else { Err(format!("JIT build {id} did not publish {}", p.display())) }
        });
        let seconds = t.elapsed().as_secs_f64();
        let now = crate::proto::wall_ms();
        {
            let _lock = lock(&self.root.join("store.lock"))?;
            let mut s = self.stats(id);
            s.builds += 1;
            s.build_seconds += seconds;
            if s.day != now / 86_400_000 { s.day = now / 86_400_000; s.builds_today = 0; }
            s.builds_today += 1;
            s.last_use = now;
            // A new waiter after cancellation starts a new build; it must not inherit the
            // cancelled generation's error. Real compiler failures still reach all waiters.
            s.error = if waiters.lock().unwrap().stopping { String::new() } else { result.as_ref().err().cloned().unwrap_or_default() };
            self.save(&s)?;
            // an outcome: the next one to take the lock finds no note
            let _ = std::fs::remove_file(self.note_path(id));
            self.prune();
            if !s.error.is_empty() {
                warn(&format!("dev JIT {id}: the store's build for {lease} failed after {seconds:.0} s: {}", s.error.lines().find(|l| l.trim_start().starts_with("error")).unwrap_or_else(|| s.error.lines().next().unwrap_or(""))));
            }
        }
        if result.is_ok() { say(&format!("devhub: built the dev JIT {id} in {seconds:.3} s")); }
        active()?;
        result
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut per_id: Vec<_> = std::fs::read_dir(&self.root).into_iter().flatten().flatten().filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?;
            Self::valid(id).then(|| self.stats(id))
        }).collect();
        per_id.sort_by(|a, b| b.last_use.cmp(&a.last_use).then_with(|| a.id.cmp(&b.id)));
        let ids = per_id.iter().filter(|s| self.binary(&s.id).is_file()).map(|s| s.id.clone()).collect();
        let today = crate::proto::wall_ms() / 86_400_000;
        Snapshot {
            ids,
            hits: per_id.iter().map(|s| s.hits).sum(),
            misses: per_id.iter().map(|s| s.misses).sum(),
            builds_today: per_id.iter().filter(|s| s.day == today).map(|s| s.builds_today).sum(),
            compiler: super::loaders::compiler().to_string(),
            per_id,
        }
    }

    fn prune(&self) {
        let held: std::collections::HashSet<_> = std::fs::read_dir(self.root.join("leases")).into_iter().flatten().flatten()
            .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        for id in self.snapshot().ids.into_iter().skip(KEEP).filter(|id| !held.contains(id)) {
            if let Err(e) = std::fs::remove_file(self.binary(&id)) { eprintln!("devhub: prune JIT {id}: {e}"); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const ID: &str = "0123456789abcdef01234567";
    const OTHER: &str = "89abcdef0123456789abcdef";

    fn work(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("devhub-jitstore-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The id of a process that has ended.
    fn ended_pid() -> u32 {
        let mut c = std::process::Command::new(if cfg!(windows) { "hostname" } else { "true" }).stdout(std::process::Stdio::null()).spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    }

    struct Heard { pending: RefCell<Vec<String>>, warned: RefCell<Vec<String>> }

    fn ensure(store: &Store, heard: &Heard, build: impl FnOnce(&Path, &dyn Fn() -> Result<(), String>, &dyn Fn(&str)) -> Result<(), String>) -> Result<PathBuf, String> {
        let cancelled = Arc::new(AtomicBool::new(false));
        store.ensure(ID, "L1", &|_| {}, &|l| heard.pending.borrow_mut().push(l.to_string()), &|l| heard.warned.borrow_mut().push(l.to_string()), &|| Ok(()), &cancelled, build)
    }

    /// A lease's explicit `--jit`: the binary, held while the lease lasts; one the box lacks is
    /// an error naming what it holds.
    #[test]
    fn an_asked_jit_is_held_for_its_lease_or_named_missing() {
        let w = work("held");
        let store = Store::new(&w);
        assert_eq!(store.held(ID, "L1"), Err(format!("JIT {ID} is not in this box's store (it holds none)")));
        let built = w.join("built");
        std::fs::write(&built, b"jit").unwrap();
        store.keep(OTHER, &built).unwrap();
        assert_eq!(store.held(ID, "L1"), Err(format!("JIT {ID} is not in this box's store (it holds {OTHER})")));
        let p = store.held(OTHER, "L2").unwrap();
        assert!(p.is_file());
        assert!(store.root.join("leases/L2").join(OTHER).is_file(), "held for its lease");
        store.release("L2").unwrap();
        assert!(!store.root.join("leases/L2").exists());
        let _ = std::fs::remove_dir_all(&w);
    }

    /// A JIT copied from a box that holds it ([`Store::read`] there, [`Store::put`] here), chunk
    /// by chunk: kept by its key once whole, runnable; a store that holds it takes nothing; a
    /// chunk past what the copy here holds (its start went with a hub restart) is refused, never
    /// kept with a gap.
    #[test]
    fn a_jit_copied_chunk_by_chunk_is_kept_by_its_key() {
        let (wa, wb) = (work("copy-from"), work("copy-to"));
        let (from, to) = (Store::new(&wa), Store::new(&wb));
        let bytes: Vec<u8> = (0..COPY_CHUNK * 2 + 17).map(|i| (i * 7) as u8).collect();
        std::fs::write(wa.join("built"), &bytes).unwrap();
        from.keep(ID, &wa.join("built")).unwrap();
        assert!(to.read(ID, 0).unwrap_err().starts_with(&format!("JIT {ID} is not in this box's store")));
        let mut off = 0;
        let last = loop {
            let (data, size) = from.read(ID, off).unwrap();
            assert_eq!(size, bytes.len() as u64);
            match to.put(ID, off, size, &data).unwrap() {
                Put::More(end) => off = end,
                done => break done,
            }
        };
        assert_eq!(last, Put::Kept);
        let kept = to.lookup(ID).expect("kept by its key");
        assert_eq!(std::fs::read(&kept).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&kept).unwrap().permissions().mode() & 0o111, 0o111, "runnable");
        }
        assert!(!wb.join("dev/jit/copies").join(ID).exists(), "the part goes");
        assert_eq!(to.put(ID, 0, bytes.len() as u64, &bytes[..COPY_CHUNK]), Ok(Put::Held));
        assert!(to.put(OTHER, COPY_CHUNK as u64, bytes.len() as u64, &bytes[..COPY_CHUNK]).unwrap_err().contains("its copy here holds 0 bytes, the chunk starts at 262144"));
        assert_eq!(to.put(OTHER, 0, bytes.len() as u64, &bytes[..COPY_CHUNK]), Ok(Put::More(COPY_CHUNK as u64)));
        to.clear_copies();
        assert!(to.put(OTHER, COPY_CHUNK as u64, bytes.len() as u64, &bytes[COPY_CHUNK..COPY_CHUNK * 2]).is_err(), "a hub restart cleared it");
        assert_eq!(to.lookup(OTHER), None);
        let _ = std::fs::remove_dir_all(&wa);
        let _ = std::fs::remove_dir_all(&wb);
    }

    /// The lock taken over with a build's note still there: that build died without an outcome
    /// (here its hub is gone). The taker says so to its holder and the inbox, and builds it.
    #[test]
    fn a_build_that_died_is_said_and_taken_over() {
        let w = work("died");
        let store = Store::new(&w);
        let dead = ended_pid();
        std::fs::create_dir_all(&store.root).unwrap();
        store.write_note(ID, &BuildNote { lease: "L273406".into(), hub: dead, since_ms: 3_723_000, step: "cargo".into() });
        let heard = Heard { pending: RefCell::new(Vec::new()), warned: RefCell::new(Vec::new()) };
        let built = ensure(&store, &heard, |dst, _, step| {
            assert_eq!(store.note(ID).map(|n| n.lease), Some("L1".into()), "the new build's note");
            step("cargo");
            assert_eq!(store.note(ID).map(|n| n.step), Some("cargo".into()));
            std::fs::write(dst, b"jit").map_err(|e| e.to_string())
        });
        assert!(built.is_ok_and(|p| p.is_file()));
        let died = format!("the store's build of {ID} for L273406 (since 01:02:03Z, cargo) died: its hub (pid {dead}) ended, a restart");
        assert_eq!(*heard.warned.borrow(), vec![format!("dev JIT {died}; L1 builds it now")]);
        assert!(heard.pending.borrow().contains(&format!("{died}; building it now")), "{:?}", heard.pending.borrow());
        assert_eq!(store.note(ID), None, "an outcome clears the note");
        let _ = std::fs::remove_dir_all(&w);
    }

    /// A hub restart lets go of the lock, not of the build: its cargo runs on in a scope of its
    /// own. The taker says it waits for that cargo (its build joins it) and warns of no death.
    #[test]
    fn a_build_whose_cargo_outlived_its_hub_is_waited_for_not_called_dead() {
        let w = work("outlived");
        let store = Store::new(&w);
        let dead = ended_pid();
        std::fs::create_dir_all(store.root.join("build").join(ID)).unwrap();
        store.write_note(ID, &BuildNote { lease: "L280491".into(), hub: dead, since_ms: 3_723_000, step: "cargo".into() });
        // the recorded cargo: a process that still runs (this one)
        let me = std::process::id();
        std::fs::write(store.root.join("build").join(ID).join("cargo.pid"), format!("{me} {}\n", crate::os::proc_start(me).unwrap())).unwrap();
        let heard = Heard { pending: RefCell::new(Vec::new()), warned: RefCell::new(Vec::new()) };
        let built = ensure(&store, &heard, |dst, _, _| std::fs::write(dst, b"jit").map_err(|e| e.to_string()));
        assert!(built.is_ok_and(|p| p.is_file()));
        assert_eq!(*heard.warned.borrow(), Vec::<String>::new());
        let joined = format!("the store's build of {ID} for L280491 (since 01:02:03Z) goes on after its hub's restart: waiting for its cargo (pid {me})");
        assert!(heard.pending.borrow().contains(&joined), "{:?}", heard.pending.borrow());
        let _ = std::fs::remove_dir_all(&w);
    }

    /// A hub restart that came as its build's cargo finished, before it published: the cargo
    /// ended after its hub. Nothing died; the taker's cargo runs on what it built (done at once)
    /// and publishes it, and says so, with no warning.
    #[test]
    fn a_build_whose_cargo_ended_after_its_hub_is_finished_not_called_dead() {
        let w = work("ended");
        let store = Store::new(&w);
        let (hub, cargo) = (ended_pid(), ended_pid());
        std::fs::create_dir_all(store.root.join("build").join(ID)).unwrap();
        store.write_note(ID, &BuildNote { lease: "L301415".into(), hub, since_ms: 3_723_000, step: "cargo".into() });
        std::fs::write(store.root.join("build").join(ID).join("cargo.pid"), format!("{cargo} 1\n")).unwrap();
        let heard = Heard { pending: RefCell::new(Vec::new()), warned: RefCell::new(Vec::new()) };
        let built = ensure(&store, &heard, |dst, _, _| std::fs::write(dst, b"jit").map_err(|e| e.to_string()));
        assert!(built.is_ok_and(|p| p.is_file()));
        assert_eq!(*heard.warned.borrow(), Vec::<String>::new(), "nothing died");
        let finished = format!("the store's build of {ID} for L301415 (since 01:02:03Z) went on after its hub's restart and its cargo (pid {cargo}) ended: cargo runs again on what it built and publishes it");
        assert!(heard.pending.borrow().contains(&finished), "{:?}", heard.pending.borrow());
        let _ = std::fs::remove_dir_all(&w);
    }

    /// A failed build is a warning naming its first error, besides its waiters' error.
    #[test]
    fn a_failed_build_is_a_warning() {
        let w = work("failed");
        let store = Store::new(&w);
        let heard = Heard { pending: RefCell::new(Vec::new()), warned: RefCell::new(Vec::new()) };
        let r = ensure(&store, &heard, |_, _, _| Err("dev JIT cargo failed (exit status: 101):\n   Compiling relax-base\nerror[E0425]: cannot find value `x`\n".into()));
        assert!(r.is_err());
        let warned = heard.warned.borrow();
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].starts_with(&format!("dev JIT {ID}: the store's build for L1 failed after ")) && warned[0].ends_with(": error[E0425]: cannot find value `x`"), "{warned:?}");
        assert_eq!(store.note(ID), None);
        let _ = std::fs::remove_dir_all(&w);
    }

    /// A waiter names the build it waits on and its step; one waiting for the build tree names
    /// the build ahead of it there, and then holds the tree in its own name.
    #[test]
    fn a_waiter_names_what_it_waits_behind() {
        let w = work("waits");
        let store = Store::new(&w);
        std::fs::create_dir_all(&store.root).unwrap();
        store.write_note(OTHER, &BuildNote { lease: "L273462".into(), hub: std::process::id(), since_ms: 53_339_000, step: "cargo".into() });
        assert_eq!(store.waiting_line(OTHER), format!("waiting for the store's build of {OTHER} for L273462 (cargo since 14:48:59Z)"));
        assert_eq!(store.waiting_line(ID), format!("waiting for the store's build of {ID}"));
        let tree = w.join("tree.lock");
        let held = lock(&tree).unwrap();
        std::fs::write(w.join("tree.holder"), OTHER).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(held);
        });
        let steps = RefCell::new(Vec::new());
        let ours = lock_tree(&store, &tree, ID, &|| Ok(()), &|s| steps.borrow_mut().push(s.to_string())).unwrap();
        release.join().unwrap();
        assert_eq!(*steps.borrow(), vec![format!("waiting for the build tree, behind the build of {OTHER} for L273462 (cargo since 14:48:59Z)")]);
        assert_eq!(std::fs::read_to_string(w.join("tree.holder")).unwrap(), ID, "the tree names its holder");
        drop(ours);
        let _ = std::fs::remove_dir_all(&w);
    }
}
