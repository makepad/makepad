//! Box-local dev JITs in `<work>/dev/jit`, keyed by pinned inputs. Per-id locks coalesce
//! builds; a build an earlier hub started is joined through its recorded pid. The tree lock
//! serializes Cargo's committed checkout. Lease holds survive hub restarts. LRU keeps six
//! binaries plus any older ones a live lease still uses.

use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

const KEEP: usize = 6;

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
    std::fs::create_dir_all(path.parent().ok_or("JIT lock has no parent")?).map_err(|e| e.to_string())?;
    let f = OpenOptions::new().create(true).truncate(false).write(true).open(path).map_err(|e| e.to_string())?;
    f.lock().map_err(|e| e.to_string())?;
    Ok(f)
}

/// Lease-owned waits must let go when their lease ends, including behind another build.
pub fn lock_while(path: &Path, shared: bool, active: &dyn Fn() -> Result<(), String>, waiting: &dyn Fn()) -> Result<File, String> {
    std::fs::create_dir_all(path.parent().ok_or("JIT lock has no parent")?).map_err(|e| e.to_string())?;
    let f = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).map_err(|e| e.to_string())?;
    let mut waited = false;
    loop {
        active()?;
        match if shared { f.try_lock_shared() } else { f.try_lock() } {
            Ok(()) => return Ok(f),
            Err(std::fs::TryLockError::WouldBlock) => { if !waited { waiting(); waited = true; } }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.to_string()),
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

impl Store {
    pub fn new(work: &Path) -> Self { Self { root: work.join("dev/jit") } }
    fn binary(&self, id: &str) -> PathBuf { self.root.join(format!("makepad.jit-{id}")) }
    fn stats(&self, id: &str) -> Stats {
        std::fs::read(self.root.join(format!("{id}.json"))).ok().and_then(|b| makepad_strict_json::parse(&b).ok()).map(|v| Stats::parse(&v)).unwrap_or_else(|| Stats { id: id.into(), ..Default::default() })
    }
    fn save(&self, s: &Stats) -> Result<(), String> {
        let p = self.root.join(format!("{}.json", s.id));
        let tmp = p.with_extension("part");
        std::fs::write(&tmp, s.json()).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| e.to_string())
    }
    fn valid(id: &str) -> bool { id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit()) }

    /// Hold before lookup/ensure; release only after the lease's processes have stopped.
    pub fn hold(&self, id: &str, lease: &str) -> Result<(), String> {
        if !Self::valid(id) || !super::exec::valid_name(lease) { return Err(format!("invalid JIT key or lease: {id} {lease}")); }
        let _lock = lock(&self.root.join("store.lock"))?;
        let dir = self.root.join("leases").join(lease);
        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(id), "")).map_err(|e| e.to_string())
    }

    pub fn release(&self, lease: &str) -> Result<(), String> {
        if !super::exec::valid_name(lease) { return Err("invalid JIT lease".into()); }
        let _lock = lock(&self.root.join("store.lock"))?;
        let p = self.root.join("leases").join(lease);
        if p.exists() { std::fs::remove_dir_all(p).map_err(|e| e.to_string())?; }
        self.prune();
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

    /// A waiting requester sees the first builder's result, including its error.
    pub fn ensure(&self, id: &str, lease: &str, say: &dyn Fn(&str), pending: &dyn Fn(&str), active: &dyn Fn() -> Result<(), String>, cancelled: &Arc<AtomicBool>, build: impl FnOnce(&Path, &dyn Fn() -> Result<(), String>) -> Result<(), String>) -> Result<PathBuf, String> {
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
        let _build = lock_while(&self.root.join(format!("{id}.lock")), false, active, &|| pending(&format!("waiting for the store's build of {id}")))?;
        active()?;
        if let Some(p) = self.binary(id).is_file().then(|| self.binary(id)) { return Ok(p); }
        let previous = self.stats(id);
        if previous.builds != generation && !previous.error.is_empty() { return Err(previous.error); }
        pending(&format!("building the dev JIT {id} (first overlay run on this box)"));
        let t = std::time::Instant::now();
        let shared_active = || waiters.lock().unwrap().active();
        let result = build(&self.binary(id), &shared_active).and_then(|()| {
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
            self.prune();
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
