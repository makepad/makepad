//! MAKEPAD_RUN_PROFILE compilation census. No code/selection/publication decisions use it.
//! CPU is exclusive between nested records and phases, and includes only intervals
//! completed before the present boundary. An open interval is not extrapolated.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use relax_session::sync::Mutex;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_PROFILE").is_some())
}
/// Unit addresses may be recycled by proc-macro/CTFE hosts in the same process.
pub fn new_unit() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    if active() { NEXT.fetch_add(1, Ordering::Relaxed) } else { 0 }
}
static CLOSED: AtomicBool = AtomicBool::new(false);
pub fn active() -> bool { enabled() && !CLOSED.load(Ordering::Acquire) }

#[derive(Clone, Default)]
pub struct Metadata {
    pub name: String,
    pub krate: String,
    pub module: String,
    pub initializer: &'static str,
}
#[derive(Clone, Default)]
struct Function {
    meta: Metadata,
    // Front-only CPU is assigned to the first emitted tier at reporting, never copied.
    costs: [[u64; 4]; 5],
    builds: [u64; 4],
    first_tier: Option<usize>,
    first_reason: Option<usize>,
    reasons: [bool; 3], // call demand, speculative worker, other ahead
    called: bool,
    installed: bool,
    inlined: bool,
}
type Row = Arc<Mutex<Function>>;
#[derive(Default)]
struct Registry {
    slots: HashMap<(usize, u32), Row>,
    instances: HashMap<(usize, u32, Vec<u32>), Row>,
    rows: Vec<Row>,
}
fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Default::default)
}
fn fresh(r: &mut Registry, meta: Metadata) -> Row {
    let row = Arc::new(Mutex::new(Function { meta, ..Default::default() }));
    r.rows.push(row.clone());
    row
}
/// Called before publishing the slot to ahead helpers. Body checks without a slot
/// join the eventual slot by instance, without creating any compiler slot early.
pub fn register(unit: usize, slot: Option<u32>, instance: Option<(u32, Vec<u32>)>, meta: Metadata) -> Option<RowHandle> {
    if !active() { return None; }
    let mut r = registry().lock().unwrap();
    if !active() { return None; }
    let key = instance.map(|(d, a)| (unit, d, a));
    let existing = slot.and_then(|s| r.slots.get(&(unit, s))).or_else(|| key.as_ref().and_then(|k| r.instances.get(k))).cloned();
    let row = existing.unwrap_or_else(|| fresh(&mut r, meta.clone()));
    if let Some(key) = key { r.instances.entry(key).or_insert_with(|| row.clone()); }
    if let Some(slot) = slot {
        r.slots.insert((unit, slot), row.clone());
        let mut f = row.lock().unwrap();
        if active() { f.meta = meta; }
    }
    Some(RowHandle(row))
}
/// check_fn owns this CPU even when it checks a callee without a JIT slot yet.
pub fn instance(def: u32, args: impl FnOnce() -> Vec<u32>, meta: impl FnOnce() -> Metadata) -> Option<RowHandle> {
    if !active() { return None; }
    let unit = STATE.try_with(|s| s.borrow().unit).unwrap_or(0);
    if unit == 0 { return None; }
    register(unit, None, Some((def, args())), meta())
}
pub struct RowHandle(Row);
fn slot_row(unit: usize, slot: u32) -> Option<Row> {
    registry().lock().unwrap().slots.get(&(unit, slot)).cloned()
}
pub fn note(unit: usize, slot: u32, event: &str) {
    if !active() { return; }
    if let Some(row) = slot_row(unit, slot) {
        let mut f = row.lock().unwrap();
        if !active() { return; }
        match event {
            "call" => f.called = true,
            "install" => f.installed = true,
            "inline" => f.inlined = true,
            _ => {}
        }
    }
}
struct Frame { row: Row, tier: usize, reason: usize }
#[derive(Default)]
struct State { unit: usize, demand: bool, stack: Vec<Frame>, phase: usize, cpu: u64 }
relax_session::gen_local! { static STATE: RefCell<State>; }
impl State {
    fn tick(&mut self) {
        let now = relax_backend::cpu_ns();
        let phase = match self.phase { 1 => Some(0), 3 => Some(1), 4 => Some(2), 5 | 9 | 10 => Some(3), _ => None };
        if let (Some(p), Some(frame)) = (phase, self.stack.last()) {
            let mut f = frame.row.lock().unwrap();
            if active() { f.costs[frame.tier][p] += now.saturating_sub(self.cpu); }
        }
        self.cpu = relax_backend::cpu_ns();
    }
}
pub struct Context(Option<(usize, bool)>);
impl Context {
    pub fn new(unit: usize, demand: bool) -> Self {
        Self(if active() { STATE.try_with(|s| {
            let mut s = s.borrow_mut();
            let old = (s.unit, s.demand);
            s.unit = unit;
            s.demand = demand;
            old
        }).ok() } else { None })
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        if let Some((u, d)) = self.0 { let _ = STATE.try_with(|s| { let mut s = s.borrow_mut(); s.unit = u; s.demand = d; }); }
    }
}
pub struct Record(bool);
impl Record {
    pub fn new(slot: u32, tier: u8, origin: &str, ensure: bool) -> Self {
        if !active() { return Self(false); }
        // best-effort like fncost: JIT TLS destructors can compile after this thread's state is gone
        STATE.try_with(|s| {
            let mut s = s.borrow_mut();
            let Some(row) = slot_row(s.unit, slot) else { return Self(false); };
            if ensure && s.stack.last().is_some_and(|f| Arc::ptr_eq(&f.row, &row)) { return Self(false); }
            let reason = if origin == "spec" || origin == "hint" { 1 } else if (origin == "lazy-on-call" || origin == "O3") && s.demand { 0 } else { 2 };
            s.tick();
            // 4 = front/unclassified, 0/1/2/3 = baseline/O1/O2/O3.
            s.stack.push(Frame { row, tier: if tier == 1 { 3 } else if origin == "O2" { 2 } else { 4 }, reason });
            Self(true)
        }).unwrap_or(Self(false))
    }
    pub fn body(row: Option<RowHandle>) -> Self {
        let Some(RowHandle(row)) = row else { return Self(false); };
        let pushed = STATE.try_with(|s| {
            let mut s = s.borrow_mut();
            s.tick();
            let (tier, reason) = s.stack.last().filter(|f| Arc::ptr_eq(&f.row, &row)).map_or((4, 2), |f| (f.tier, f.reason));
            s.stack.push(Frame { row, tier, reason });
        }).is_ok();
        Self(pushed)
    }
}
impl Drop for Record {
    fn drop(&mut self) {
        if self.0 { let _ = STATE.try_with(|s| { let mut s = s.borrow_mut(); s.tick(); s.stack.pop(); }); }
    }
}
pub struct Phase(Option<usize>);
pub fn phase(p: usize) -> Phase {
    Phase(if active() { STATE.try_with(|s| {
        let mut s = s.borrow_mut(); s.tick(); let old = s.phase; s.phase = p; old
    }).ok() } else { None })
}
impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(p) = self.0 { let _ = STATE.try_with(|s| { let mut s = s.borrow_mut(); s.tick(); s.phase = p; }); }
    }
}
pub fn tier(baseline: bool, full: bool) {
    if !active() { return; }
    let _ = STATE.try_with(|s| {
        let mut s = s.borrow_mut();
        s.tick();
        if let Some(f) = s.stack.last_mut() {
            if f.tier != 3 { f.tier = if baseline { 0 } else if full { 2 } else { 1 }; }
        }
    });
}
static UNATTRIBUTED_BUILDS: AtomicU64 = AtomicU64::new(0);
pub fn code() {
    if !active() { return; }
    let _ = STATE.try_with(|s| {
        let mut s = s.borrow_mut(); s.tick();
        if let Some(frame) = s.stack.last() {
            let mut f = frame.row.lock().unwrap();
            if active() {
                let tier = if frame.tier == 4 { 1 } else { frame.tier };
                f.first_tier.get_or_insert(tier);
                f.first_reason.get_or_insert(frame.reason);
                f.builds[tier] += 1; f.reasons[frame.reason] = true;
            }
        } else if active() {
            UNATTRIBUTED_BUILDS.fetch_add(1, Ordering::Relaxed);
        }
    });
}

static RELEASE: AtomicU8 = AtomicU8::new(0);
pub fn startup_release(present: bool) {
    if active() { RELEASE.store(if present { 1 } else { 2 }, Ordering::Relaxed); }
}
pub fn first_present() { if enabled() { CLOSED.store(true, Ordering::Release); } }
const TIERS: [&str; 5] = ["baseline", "O1", "O2", "O3", "front-only"];
#[derive(Default)]
struct Total { functions: u64, compiled_functions: u64, inlined_functions: u64, builds: u64, cpu: [u64; 4] }
impl Total {
    fn add(&mut self, builds: u64, cpu: [u64; 4], inlined: bool) {
        self.functions += 1; self.inlined_functions += u64::from(inlined); self.compiled_functions += u64::from(builds != 0); self.builds += builds;
        for (sum, n) in self.cpu.iter_mut().zip(cpu) { *sum += n; }
    }
    fn fields(&self) -> String {
        format!("functions={} compiled_functions={} inlined_functions={} builds={} typeck_cpu_ns={} lower_cpu_ns={} opt_cpu_ns={} codegen_cpu_ns={}", self.functions, self.compiled_functions, self.inlined_functions, self.builds, self.cpu[0], self.cpu[1], self.cpu[2], self.cpu[3])
    }
}
/// Snapshot after closing, without acquiring the compiler Unit lock. Frozen rows
/// are not rewritten by exit reporting. Sorting/I/O happens after all row locks.
pub fn report() {
    static ONCE: AtomicBool = AtomicBool::new(false);
    if !enabled() || ONCE.swap(true, Ordering::AcqRel) { return; }
    first_present();
    let registered = std::mem::take(&mut *registry().lock().unwrap());
    let rows = registered.rows;
    let mut groups: BTreeMap<(String, String, &'static str, &'static str, &'static str), Total> = BTreeMap::new();
    let mut called = Vec::new();
    let mut unused: BTreeMap<(String, String), Total> = BTreeMap::new();
    let mut unique = 0;
    let mut total_builds = 0;
    for row in rows {
        let f = row.lock().unwrap().clone();
        let emitted = f.builds.iter().sum::<u64>();
        if emitted == 0 && f.costs.iter().flatten().all(|n| *n == 0) { continue; }
        unique += u64::from(emitted != 0);
        total_builds += emitted;
        let use_kind = if f.called { "called" } else if f.installed { "call-unknown" } else { "never-called" };
        let why = if !f.meta.initializer.is_empty() { f.meta.initializer } else if f.first_reason == Some(1) { "speculative" } else if f.first_reason == Some(0) { "call-demand" } else if emitted == 0 && f.inlined { "inline-only" } else { "ahead" };
        let mut costs = f.costs;
        if let Some(first) = f.first_tier {
            for p in 0..4 { costs[first][p] += costs[4][p]; }
            costs[4] = [0; 4];
        }
        let mut cpu = [0; 4];
        for t in 0..5 {
            for p in 0..4 { cpu[p] += costs[t][p]; }
            let builds = f.builds.get(t).copied().unwrap_or(0);
            if builds == 0 && costs[t] == [0; 4] { continue; }
            for module in [String::new(), f.meta.module.clone()] {
                groups.entry((f.meta.krate.clone(), module, why, use_kind, TIERS[t])).or_default().add(builds, costs[t], f.inlined);
            }
        }
        if emitted > 0 && f.called { called.push((cpu.iter().sum::<u64>(), f.meta.name.clone(), f.meta.module.clone(), emitted, cpu, f.inlined)); }
        if emitted > 0 && !f.called && !f.installed && f.reasons[1] {
            unused.entry((f.meta.krate.clone(), f.meta.module.clone())).or_default().add(emitted, cpu, f.inlined);
        }
    }
    let unattributed = UNATTRIBUTED_BUILDS.load(Ordering::Relaxed);
    let release = match RELEASE.load(Ordering::Relaxed) { 2 => "timeout", 1 => "present", _ => "held-or-disabled" };
    let mut out = std::io::BufWriter::new(std::io::stderr().lock());
    let _ = writeln!(out, "exec-compile-summary: at=first-present compiled_functions={unique} emitted_builds={total_builds} unattributed_builds={unattributed} cpu=exclusive-completed-intervals calls=lazy-entry-or-hot-counter-or-explicit-initializer installed_without_call=unknown front_cpu=first-emitted-tier why=first-emission-origin startup_release={release}");
    for ((krate, module, why, usage, tier), t) in groups {
        let level = if module.is_empty() { "crate" } else { "module" };
        let _ = writeln!(out, "exec-compile-group: at=first-present level={level} crate={krate:?} module={module:?} why={why} use={usage} tier={tier} {}", t.fields());
    }
    called.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    for (i, (cpu, name, module, builds, phases, inlined)) in called.into_iter().take(50).enumerate() {
        let t = Total { functions: 1, compiled_functions: 1, inlined_functions: u64::from(inlined), builds, cpu: phases };
        let _ = writeln!(out, "exec-compile-top-called: rank={} cpu_ns={cpu} name={name:?} module={module:?} {}", i + 1, t.fields());
    }
    let mut unused: Vec<_> = unused.into_iter().collect();
    unused.sort_by(|a, b| b.1.functions.cmp(&a.1.functions).then_with(|| a.0.cmp(&b.0)));
    for (i, ((krate, module), t)) in unused.into_iter().take(50).enumerate() {
        let _ = writeln!(out, "exec-compile-top-unused: kind=speculative-never-called rank={} crate={krate:?} module={module:?} {}", i + 1, t.fields());
    }
}
