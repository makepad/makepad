//! Opt-in per-function CPU accounting. MAKEPAD_RUN_FN_COST=<TSV prefix>; writes <prefix>.<pid>.
//! Each exec gets a separate file (build scripts inherit the switch).
//! Rows are buffered per thread, flushed every 1000 rows, once a second, and at
//! explicit shutdown. `phase` rows partition thread CPU (nested scopes are exclusive).
//! Pass columns are inclusive: some optimiser passes call other timed passes.
use std::{
    cell::RefCell,
    io::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};
use relax_session::sync::Mutex;
pub const PHASES: [&str; 11] = [
    "other", "typeck", "borrowck", "lower", "opt", "codegen", "install", "lock", "wait",
    "regalloc", "emit",
];
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_FN_COST").is_some_and(|s| !s.is_empty()))
}
/// Absolute CLOCK_MONOTONIC on Unix, process-relative Instant elsewhere (header says so).
pub fn mono() -> u64 {
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Ts {
            sec: i64,
            ns: i64,
        }
        extern "C" {
            fn clock_gettime(id: i32, ts: *mut Ts) -> i32;
        }
        #[cfg(target_os = "linux")]
        let id = 1;
        #[cfg(not(target_os = "linux"))]
        let id = 6;
        let mut ts = Ts { sec: 0, ns: 0 };
        unsafe {
            clock_gettime(id, &mut ts);
        }
        ts.sec as u64 * 1_000_000_000 + ts.ns as u64
    }
    #[cfg(not(unix))]
    {
        static T: OnceLock<std::time::Instant> = OnceLock::new();
        T.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
    }
}
type Buffer = Arc<Mutex<Vec<String>>>;
static BUFFERS: Mutex<Vec<Buffer>> = Mutex::new(Vec::new());
static FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
static IDS: AtomicU64 = AtomicU64::new(1);
type InstanceKey = (u32, Vec<u32>);
// (CPU checked, CPU already attributed to emitted code, typed expression count).
static TYPES: OnceLock<Mutex<std::collections::HashMap<InstanceKey, (u64, u64, usize)>>> =
    OnceLock::new();
fn types() -> &'static Mutex<std::collections::HashMap<InstanceKey, (u64, u64, usize)>> {
    TYPES.get_or_init(Default::default)
}
fn init() {
    FILE.get_or_init(|| {
        let mut path = std::env::var_os("MAKEPAD_RUN_FN_COST").unwrap();
        path.push(format!(".{}", std::process::id()));
        let mut f = match std::fs::File::create(&path) {
            Ok(f) => Some(f), Err(e) => { eprintln!("makepad: MAKEPAD_RUN_FN_COST {:?}: {}", path, e); None }
        };
        if let Some(f) = &mut f {
            let _ = writeln!(f, "# fncost-v1 pid={} clock={} phases={} passes={}\n# fn: id slot tier origin thread start end cpu_ns rir_in rir_out blocks code_bytes name phase_cpu[11] phase_wall[11] pass_cpu[16] instance_typeck_cpu_ns typed_nodes\n# phase: id thread phase start end cpu_ns\n# type: id def args nodes name start end cpu_ns\n# meta: slot name; call/hint: slot monotonic_ns; mark: name monotonic_ns", std::process::id(), if cfg!(unix) { "CLOCK_MONOTONIC" } else { "process-Instant" }, PHASES.join(","), crate::opt::PASS_NAMES.join(","));
        }
        Mutex::new(f)
    });
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("run-fncost-flush".into())
            .spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                flush();
            });
    });
}
fn write(rows: &mut Vec<String>) {
    if rows.is_empty() {
        return;
    }
    if let Some(file) = FILE.get() {
        if let Some(f) = file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.write_all(rows.join("").as_bytes());
        }
    }
    rows.clear();
}
pub fn flush() {
    if !enabled() {
        return;
    }
    for b in BUFFERS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        write(&mut b.lock().unwrap_or_else(|e| e.into_inner()));
    }
}
struct State {
    buf: Buffer,
    stack: Vec<Row>,
    phase: usize,
    cpu: u64,
    wall: u64,
    tid: String,
}
impl State {
    fn new() -> Self {
        init();
        let buf = Arc::new(Mutex::new(Vec::with_capacity(1000)));
        BUFFERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(buf.clone());
        Self {
            buf,
            stack: Vec::new(),
            phase: 0,
            cpu: crate::cpu_ns(),
            wall: mono(),
            tid: format!("{:?}", std::thread::current().id()),
        }
    }
    fn row(&self, s: String) {
        let mut b = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        b.push(s);
        if b.len() >= 1000 {
            write(&mut b);
        }
    }
    fn tick(&mut self) {
        let c = crate::cpu_ns();
        let w = mono();
        if let Some(r) = self.stack.last_mut() {
            let dc = c.saturating_sub(self.cpu);
            let dw = w.saturating_sub(self.wall);
            r.cpu[self.phase] += dc;
            r.wall[self.phase] += dw;
            let id = r.id;
            self.row(format!(
                "phase\t{}\t{}\t{}\t{}\t{}\t{}\n",
                id, self.tid, PHASES[self.phase], self.wall, w, dc
            ));
        }
        self.cpu = crate::cpu_ns();
        self.wall = mono();
    }
}
impl Default for State {
    fn default() -> Self { Self::new() }
}
relax_session::gen_local! { static STATE: RefCell<State>; }
// JIT TLS destructors can lazily compile after this thread's logger was destroyed.
// That late diagnostic is best-effort, as are all the existing JIT TLS statistics.
fn local<R: Default>(f: impl FnOnce(&RefCell<State>) -> R) -> R {
    STATE.try_with(f).unwrap_or_default()
}

struct Row {
    id: u64,
    slot: u32,
    tier: u8,
    origin: &'static str,
    start: u64,
    name: String,
    size: [usize; 4],
    cpu: [u64; 11],
    wall: [u64; 11],
    passes: [u64; 16],
    old_phase: usize,
    instance: Option<InstanceKey>,
}
pub struct Record {
    _census: crate::exec_census::Record,
    on: bool,
    _context: crate::warm::Context,
}
impl Record {
    pub fn new(slot: u32, tier: u8, origin: &'static str) -> Self {
        let context = crate::warm::Context::new(slot, tier);
        let census = crate::exec_census::Record::new(slot, tier, origin, false);
        if !enabled() {
            return Self {
                on: false,
                _census: census,
                _context: context,
            };
        }
        crate::opt::pass_timing_on();
        local(|s| {
            let mut s = s.borrow_mut();
            s.tick();
            let old_phase = s.phase;
            s.phase = 0;
            let id = IDS.fetch_add(1, Ordering::Relaxed);
            s.row(format!(
                "begin\t{}\t{}\t{}\t{}\t{}\t{}\n",
                id,
                slot,
                tier,
                origin,
                s.tid,
                mono()
            ));
            s.stack.push(Row {
                id,
                slot,
                tier,
                origin,
                start: mono(),
                name: String::new(),
                size: [0; 4],
                cpu: [0; 11],
                wall: [0; 11],
                passes: crate::opt::pass_local(),
                old_phase,
                instance: None,
            });
        });
        Self {
            on: true,
            _census: census,
            _context: context,
        }
    }
    /// Reuse a calling scope; direct compile_fn/ahead invocations still get a record.
    pub fn ensure(slot: u32, tier: u8, origin: &'static str) -> Self {
        if !enabled() {
            return Self {
                on: false,
                _census: crate::exec_census::Record::new(slot, tier, origin, true),
                _context: crate::warm::Context::new(slot, tier),
            };
        }
        if local(|s| {
            s.borrow()
                .stack
                .last()
                .is_some_and(|r| r.slot == slot && r.tier == tier)
        }) {
            Self {
                on: false,
                _census: crate::exec_census::Record::new(slot, tier, origin, true),
                _context: crate::warm::Context::new(slot, tier),
            }
        } else {
            Self::new(slot, tier, origin)
        }
    }
}
impl Drop for Record {
    fn drop(&mut self) {
        if !self.on {
            return;
        }
        local(|s| {
            let mut s = s.borrow_mut();
            s.tick();
            let r = s.stack.pop().unwrap();
            s.phase = r.old_phase;
            let now = crate::opt::pass_local();
            let passes: Vec<u64> = now
                .iter()
                .zip(r.passes)
                .map(|(a, b)| a.saturating_sub(b))
                .collect();
            if let Some(parent) = s.stack.last_mut() {
                for i in 0..16 {
                    parent.passes[i] += passes[i];
                }
            }
            let (instance_ns, nodes) = if r.size[3] > 0 {
                r.instance
                    .as_ref()
                    .and_then(|key| {
                        let mut g = types().lock().unwrap_or_else(|e| e.into_inner());
                        let t = g.get_mut(key)?;
                        let delta = t.0.saturating_sub(t.1);
                        t.1 = t.0;
                        Some((delta, t.2))
                    })
                    .unwrap_or_default()
            } else {
                (0, 0)
            };
            s.row(format!(
                "fn\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                r.id,
                r.slot,
                r.tier,
                r.origin,
                s.tid,
                r.start,
                mono(),
                r.cpu.iter().sum::<u64>(),
                r.size[0],
                r.size[1],
                r.size[2],
                r.size[3],
                clean(&r.name),
                csv(&r.cpu),
                csv(&r.wall),
                csv(&passes),
                instance_ns,
                nodes
            ));
        });
    }
}
fn csv(v: &[u64]) -> String {
    v.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}
fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}
pub struct Phase {
    _census: crate::exec_census::Phase,
    old: Option<usize>,
    _warm: Option<Box<crate::warm::Scope>>,
}
pub fn phase(p: usize) -> Phase {
    let census = crate::exec_census::phase(p);
    let warm =
        (p == 4 && crate::warm::enabled()).then(|| Box::new(crate::warm::Scope::optimization()));
    if !enabled() {
        return Phase {
            _census: census,
            old: None,
            _warm: warm,
        };
    }
    Phase {
        _census: census,
        old: Some(local(|s| {
            let mut s = s.borrow_mut();
            s.tick();
            let old = s.phase;
            s.phase = p;
            old
        })),
        _warm: warm,
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(p) = self.old {
            local(|s| {
                let mut s = s.borrow_mut();
                s.tick();
                s.phase = p;
            });
        }
    }
}
pub fn name(slot: u32, name: &str) {
    crate::warm::name(slot, name);
    if !enabled() {
        return;
    }
    local(|s| {
        let mut s = s.borrow_mut();
        if let Some(r) = s.stack.last_mut().filter(|r| r.slot == slot) {
            r.name = name.into();
        }
        s.row(format!("meta\t{}\t{}\n", slot, clean(name)));
    });
}
pub fn size(input: bool, f: &crate::rir::Func) {
    if !input { crate::exec_census::tier(f.baseline, f.full); }
    if !enabled() {
        return;
    }
    local(|s| {
        if let Some(r) = s.borrow_mut().stack.last_mut() {
            r.size[if input { 0 } else { 1 }] = crate::opt::inst_count(f);
            r.size[2] = f.blocks.len();
        }
    });
}
pub fn code(bytes: usize) {
    crate::exec_census::code();
    if enabled() {
        local(|s| {
            if let Some(r) = s.borrow_mut().stack.last_mut() {
                r.size[3] = bytes;
            }
        });
    }
}
pub fn event(kind: &str, slot: u32) {
    if enabled() {
        local(|s| {
            let s = s.borrow();
            // Explicit eager compilation installs code without calling it. It has no lazy
            // entry on its eventual first call, so do not invent an entry at install time.
            if kind == "call" && s.stack.last().is_some_and(|r| r.origin == "ahead") {
                return;
            }
            s.row(format!("{}\t{}\t{}\n", kind, slot, mono()));
        });
    }
}

pub fn mark(name: &str) {
    if enabled() {
        local(|s| {
            s.borrow()
                .row(format!("mark\t{}\t{}\n", clean(name), mono()))
        });
        flush();
    }
}
pub fn type_detail(def: u32, args: &[u32], nodes: usize, name: &str, start: u64, cpu: u64) {
    if !enabled() {
        return;
    }
    let dt = crate::cpu_ns().saturating_sub(cpu);
    local(|s| {
        let s = s.borrow();
        let row = s.stack.last();
        if !row.is_some_and(|r| r.origin == "gandalf") {
            let mut g = types().lock().unwrap_or_else(|e| e.into_inner());
            let t = g.entry((def, args.to_vec())).or_default();
            t.0 += dt;
            t.2 = t.2.max(nodes);
        }
        s.row(format!(
            "type\t{}\t{}\t{:?}\t{}\t{}\t{}\t{}\t{}\n",
            row.map_or(0, |r| r.id),
            def,
            args,
            nodes,
            clean(name),
            start,
            mono(),
            dt
        ));
    });
}

static HINTS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
pub fn hint(id: u32) {
    if enabled() {
        HINTS.lock().unwrap_or_else(|e| e.into_inner()).push(id);
        event("hint", id);
    }
}
pub fn spec_origin(id: u32) -> &'static str {
    if enabled()
        && HINTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id)
    {
        "hint"
    } else {
        "spec"
    }
}

/// Join actual type-check instances (including checks ahead) back to code slots.
pub fn instance(slot: u32, def: u32, args: &[u32]) {
    crate::warm::instance(slot, def, args);
    if enabled() {
        local(|s| {
            let mut s = s.borrow_mut();
            if let Some(r) = s.stack.last_mut().filter(|r| r.slot == slot) {
                r.instance = Some((def, args.to_vec()));
            }
            s.row(format!("instance\t{}\t{}\t{:?}\n", slot, def, args));
        });
    }
}

impl Drop for State {
    fn drop(&mut self) {
        write(&mut self.buf.lock().unwrap_or_else(|e| e.into_inner()));
    }
}
