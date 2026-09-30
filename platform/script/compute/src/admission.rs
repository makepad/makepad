//! Admission: whether a kernel job may run, decided before it is queued.
//!
//! Splash is untrusted whatever wrote it (an AI, the store, a LAN host, a
//! livecode file). A job is admitted only when its worst case fits:
//!
//! - **the job's budget**: the kernel's worst-case time for `count`
//!   elements (static cost per element, host-memory accesses charged as
//!   cache misses) fits the job's wall-time budget on the threads it may
//!   use;
//! - **the element bound** (untrusted origins): one element's worst case
//!   stays below [`DeviceLimits::element_latency`], so the per-element
//!   cancel poll stops a running job within that time (the cancellation
//!   latency bound);
//! - **the device ledger**: the summed worst-case worker time and the
//!   number of admitted, unfinished jobs of every document on the device
//!   fit the device's limits (untrusted origins have their own, smaller
//!   share).
//!
//! A refused job is reported with the numbers that refused it; nothing is
//! silently shortened. An admitted job holds a [`Ticket`] until it ends
//! (finished, failed or cancelled); dropping the ticket returns its share
//! of the ledger. The ticket's [`Ticket::work_limit`] goes to
//! `Call::set_work_limit`, so the call itself re-checks the same bound, and
//! [`Ticket::deadline`] is the watchdog's cancel time.

use crate::ir::{Block, Op, Region, Stmt};
use crate::kernel::{Kernel, CHUNK};
use crate::lower::kernel::ELEMENT_CAP;
use crate::Backend;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Who wrote the kernel. Everything but `Host` is untrusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Code shipped with the application (built-in generators).
    Host,
    /// Written or edited by an AI.
    Ai,
    /// An asset from the store or a LAN AI Hub.
    Store,
    /// A document or game script from a LAN host or co-editor.
    Lan,
    /// A livecode file.
    Livecode,
}

impl Origin {
    pub fn untrusted(self) -> bool {
        self != Origin::Host
    }
}

/// What one job may take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JobBudget {
    /// Wall time for the whole job (the watchdog cancels at this deadline).
    pub wall: Duration,
}

/// Device-wide limits (host settings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceLimits {
    /// Summed worst-case worker time of all admitted, unfinished jobs.
    pub max_in_flight: Duration,
    /// The part of `max_in_flight` untrusted jobs may hold.
    pub max_untrusted_in_flight: Duration,
    pub max_jobs: u32,
    pub max_untrusted_jobs: u32,
    /// The largest wall budget an untrusted job may ask for.
    pub untrusted_job_wall: Duration,
    /// Untrusted: the worst-case time of one element, which bounds how long
    /// a cancel takes to stop a running job (the requirement is 2 ms).
    pub element_latency: Duration,
}

impl Default for DeviceLimits {
    fn default() -> Self {
        DeviceLimits {
            max_in_flight: Duration::from_secs(30),
            max_untrusted_in_flight: Duration::from_secs(10),
            max_jobs: 4096,
            max_untrusted_jobs: 1024,
            untrusted_job_wall: Duration::from_secs(2),
            element_latency: Duration::from_millis(1),
        }
    }
}

/// Worst-case rates used to turn static cost into time: picoseconds per
/// AIR op, and per host-memory access (charged as a cache and TLB miss).
/// Measured with `examples/bench_admission.rs` on Apple M-series
/// (2026-09-30): at most 0.32 ns/op native and 2.46 ns/op interpreted over
/// division, sqrt, sin and fbm chains, and 114 ns per dependent load over
/// 64 MiB; the rates below keep about 1.5–2x of margin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    pub op_ps: u64,
    pub miss_ps: u64,
}

pub fn rates(backend: Backend) -> Rates {
    match backend {
        Backend::Native => Rates { op_ps: 500, miss_ps: 250_000 },
        Backend::Interp => Rates { op_ps: 3_500, miss_ps: 250_000 },
    }
}

/// Shared tables above this many words are charged a miss per load (below
/// it they stay in cache).
const SHARED_CACHED_WORDS: usize = 32 * 1024;

/// A job's worst case, as admission computed it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// One element, picoseconds.
    pub element_ps: u64,
    /// All elements on one worker, nanoseconds (the ledger's charge).
    pub worker_ns: u64,
    /// Wall time on the threads the job may use, nanoseconds.
    pub wall_ns: u64,
    pub threads: usize,
}

/// One element's worst case, split so host calls can be bounded per call.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementCost {
    /// Everything but host calls whose input size is only known at run
    /// time, picoseconds (host calls on constant-size slices included).
    pub fixed_ps: u64,
    /// Host calls per element whose slice lengths are run-time values
    /// (loop caps included): each is bounded by the per-call limit.
    pub open_calls: u64,
    /// Those calls charged at their declared cost for their largest input.
    pub open_max_ps: u64,
}

impl ElementCost {
    /// The element's worst case when each open call may use `call_ps`.
    pub fn with_call_limit(&self, call_ps: u64) -> u64 {
        self.fixed_ps.saturating_add(self.open_calls.saturating_mul(call_ps))
    }
}

/// Declared op-equivalents of one call of `h` with slice lengths `lens`
/// (a length the compiler knows, else the signature's maximum).
fn host_ops(h: &crate::host::HostFn, lens: &[Option<u32>]) -> u64 {
    let lens: Vec<u32> = lens.iter().zip(h.slices).map(|(l, s)| l.unwrap_or(s.max_words)).collect();
    h.cost_of(&lens).saturating_add(8)
}

/// Worst-case time of one element of `kernel` on `backend`, following
/// `Program::cost` (the outermost loop is the element loop: counted once;
/// branches take their dearer side), with host-memory accesses charged as
/// misses and host calls at their declared cost for their input size. Host
/// components are Rust: their ops are charged at the native rate whatever
/// runs the kernel.
pub fn element_cost(kernel: &Kernel, backend: Backend) -> ElementCost {
    let r = rates(backend);
    let native = rates(Backend::Native);
    let shared_misses = kernel.shared_words() > SHARED_CACHED_WORDS;
    let program = kernel.program();
    // Integer constants, for slice lengths the compiler fixed.
    let mut consts = std::collections::HashMap::new();
    fn scan(b: &Block, consts: &mut std::collections::HashMap<u32, i32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    consts.insert(v.0, *c);
                }
                Stmt::If(_, t, e) => {
                    scan(t, consts);
                    scan(e, consts);
                }
                Stmt::Loop { body, .. } => scan(body, consts),
                _ => {}
            }
        }
    }
    scan(&program.body, &mut consts);
    struct W<'a> {
        r: Rates,
        native: Rates,
        shared_misses: bool,
        consts: &'a std::collections::HashMap<u32, i32>,
    }
    impl W<'_> {
        fn miss(&self, region: Region) -> bool {
            match region {
                // Buffer 0 is the one-word control buffer.
                Region::Buf(k) => k > 0,
                Region::Shared => self.shared_misses,
                _ => false,
            }
        }
        fn walk(&self, b: &Block, depth: u32) -> ElementCost {
            let mut sum = ElementCost::default();
            let add = |a: ElementCost, b: ElementCost| ElementCost {
                fixed_ps: a.fixed_ps.saturating_add(b.fixed_ps),
                open_calls: a.open_calls.saturating_add(b.open_calls),
                open_max_ps: a.open_max_ps.saturating_add(b.open_max_ps),
            };
            let fixed = |ps: u64| ElementCost { fixed_ps: ps, ..Default::default() };
            for s in b {
                let c = match s {
                    Stmt::If(_, t, e) => {
                        // Each part at its dearer side: an upper bound.
                        let (t, e) = (self.walk(t, depth), self.walk(e, depth));
                        ElementCost {
                            fixed_ps: self.r.op_ps.saturating_add(t.fixed_ps.max(e.fixed_ps)),
                            open_calls: t.open_calls.max(e.open_calls),
                            open_max_ps: t.open_max_ps.max(e.open_max_ps),
                        }
                    }
                    Stmt::Loop { cap, body } => {
                        let reps = if depth == 0 { 1 } else { *cap as u64 };
                        let c = add(self.walk(body, depth + 1), fixed(2 * self.r.op_ps));
                        ElementCost { fixed_ps: c.fixed_ps.saturating_mul(reps), open_calls: c.open_calls.saturating_mul(reps), open_max_ps: c.open_max_ps.saturating_mul(reps) }
                    }
                    Stmt::Def(_, Op::Load { region, .. }) | Stmt::Store { region, .. } if self.miss(*region) => fixed(self.r.op_ps + self.r.miss_ps),
                    Stmt::CallHost { f, slices, .. } => match crate::host::get(*f) {
                        Some(h) => {
                            let lens: Vec<Option<u32>> = slices.iter().map(|x| self.consts.get(&x.len.0).map(|c| *c as u32)).collect();
                            let misses = (h.misses as u64).saturating_mul(self.r.miss_ps);
                            let ps = |lens: &[Option<u32>]| host_ops(&h, lens).saturating_mul(self.native.op_ps).saturating_add(misses);
                            if lens.iter().all(|l| l.is_some()) {
                                fixed(ps(&lens))
                            } else {
                                ElementCost { fixed_ps: misses, open_calls: 1, open_max_ps: ps(&vec![None; lens.len()]) }
                            }
                        }
                        // An unknown index costs everything.
                        None => fixed(u64::MAX / 4),
                    },
                    _ => fixed(self.r.op_ps),
                };
                sum = add(sum, c);
            }
            sum
        }
    }
    W { r, native, shared_misses, consts: &consts }.walk(&program.body, 0)
}

/// Worst-case time of one element with every host call at its largest
/// input (the trusted bound).
pub fn element_ps(kernel: &Kernel, backend: Backend) -> u64 {
    let c = element_cost(kernel, backend);
    c.fixed_ps.saturating_add(c.open_max_ps)
}

/// Worst-case time of `count` elements on up to `threads` threads, each
/// element at `element` picoseconds.
fn estimate_at(kernel: &Kernel, element: u64, count: usize, threads: usize) -> Estimate {
    let worker_ns = element.saturating_mul(count as u64) / 1000;
    // Element-local kernels split into fixed chunks; others run on one thread.
    let threads = if kernel.parallel_safe { threads.max(1).min(count.div_ceil(CHUNK).max(1)) } else { 1 };
    Estimate { element_ps: element, worker_ns, wall_ns: worker_ns / threads as u64, threads }
}

/// Worst-case time of `count` elements on up to `threads` threads (every
/// host call at its largest input).
pub fn estimate(kernel: &Kernel, count: usize, threads: usize) -> Estimate {
    estimate_at(kernel, element_ps(kernel, kernel.backend()), count, threads)
}

/// Why a job was not admitted.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// More elements than one call may run.
    TooMany(usize),
    /// Untrusted: one element may take longer than the cancel bound.
    ElementTooSlow { worst_ns: u64, limit_ns: u64 },
    /// The job's worst case does not fit its wall budget.
    OverBudget { worst_ns: u64, budget_ns: u64 },
    /// The device's in-flight worker time is taken (queue or retry later).
    DeviceBusy { in_flight_ns: u64, need_ns: u64, limit_ns: u64 },
    /// The device has too many admitted, unfinished jobs.
    TooManyJobs { jobs: u32, limit: u32 },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ms = |ns: u64| ns as f64 / 1e6;
        match self {
            Refused::TooMany(n) => write!(f, "{} elements is more than one job may run", n),
            Refused::ElementTooSlow { worst_ns, limit_ns } => {
                write!(f, "one element may take {:.3} ms in the worst case; untrusted kernels are limited to {:.3} ms per element (reduce loop sizes)", ms(*worst_ns), ms(*limit_ns))
            }
            Refused::OverBudget { worst_ns, budget_ns } => write!(f, "the job may take {:.1} ms in the worst case; its budget is {:.1} ms", ms(*worst_ns), ms(*budget_ns)),
            Refused::DeviceBusy { in_flight_ns, need_ns, limit_ns } => {
                write!(f, "the device has {:.1} ms of kernel work in flight; this job needs {:.1} ms more and the limit is {:.1} ms", ms(*in_flight_ns), ms(*need_ns), ms(*limit_ns))
            }
            Refused::TooManyJobs { jobs, limit } => write!(f, "{} kernel jobs are in flight; the limit is {}", jobs, limit),
        }
    }
}

#[derive(Default, Debug)]
struct State {
    in_flight_ns: u64,
    untrusted_ns: u64,
    jobs: u32,
    untrusted_jobs: u32,
}

/// Totals of the admitted, unfinished jobs (for stats).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LedgerStats {
    pub in_flight_ns: u64,
    pub untrusted_ns: u64,
    pub jobs: u32,
    pub untrusted_jobs: u32,
}

struct Inner {
    limits: Mutex<DeviceLimits>,
    state: Mutex<State>,
}

/// The device ledger: one per process (see [`Ledger::device`]); tests make
/// their own.
#[derive(Clone)]
pub struct Ledger(Arc<Inner>);

impl Ledger {
    pub fn new(limits: DeviceLimits) -> Ledger {
        Ledger(Arc::new(Inner { limits: Mutex::new(limits), state: Mutex::new(State::default()) }))
    }

    /// The device-wide ledger every document's jobs are admitted against.
    pub fn device() -> &'static Ledger {
        static DEVICE: OnceLock<Ledger> = OnceLock::new();
        DEVICE.get_or_init(|| Ledger::new(DeviceLimits::default()))
    }

    pub fn limits(&self) -> DeviceLimits {
        *self.0.limits.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// New limits apply to jobs admitted from now on.
    pub fn set_limits(&self, limits: DeviceLimits) {
        *self.0.limits.lock().unwrap_or_else(|e| e.into_inner()) = limits;
    }

    pub fn stats(&self) -> LedgerStats {
        let s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        LedgerStats { in_flight_ns: s.in_flight_ns, untrusted_ns: s.untrusted_ns, jobs: s.jobs, untrusted_jobs: s.untrusted_jobs }
    }

    /// Admits `count` elements of `kernel` from `origin`, to run on up to
    /// `threads` threads within `budget`, or says why not.
    pub fn admit(&self, kernel: &Kernel, count: usize, threads: usize, origin: Origin, budget: &JobBudget) -> Result<Ticket, Refused> {
        if count > ELEMENT_CAP as usize {
            return Err(Refused::TooMany(count));
        }
        let limits = self.limits();
        let untrusted = origin.untrusted();
        // Computed once per kernel: admission allocates nothing per job.
        let cost = *kernel.admission.get_or_init(|| element_cost(kernel, kernel.backend()));
        let native_op = rates(Backend::Native).op_ps.max(1);
        let mut wall = budget.wall;
        // Host calls on run-time input sizes: each is held to a per-call op
        // limit, enforced at the call (an input over it is refused there,
        // not run). Trusted code: each function's largest input.
        let (element, host_call_limit) = if untrusted {
            let limit_ps = limits.element_latency.as_nanos() as u64 * 1000;
            if cost.fixed_ps > limit_ps {
                return Err(Refused::ElementTooSlow { worst_ns: cost.fixed_ps / 1000, limit_ns: limit_ps / 1000 });
            }
            wall = wall.min(limits.untrusted_job_wall);
            if cost.open_calls == 0 {
                (cost.fixed_ps, 0)
            } else {
                // What the element bound leaves, shared by the open calls
                // (no more than their largest input needs).
                let call_ps = ((limit_ps - cost.fixed_ps) / cost.open_calls).min(cost.open_max_ps.max(1));
                if call_ps < native_op {
                    return Err(Refused::ElementTooSlow { worst_ns: cost.with_call_limit(native_op) / 1000, limit_ns: limit_ps / 1000 });
                }
                // The limit is one ctx word (K_HOST_LIMIT).
                let ops = (call_ps / native_op).clamp(1, u32::MAX as u64);
                (cost.with_call_limit(ops * native_op), ops)
            }
        } else {
            (cost.fixed_ps.saturating_add(cost.open_max_ps), 0)
        };
        let est = estimate_at(kernel, element, count, threads);
        let budget_ns = wall.as_nanos().min(u64::MAX as u128) as u64;
        if est.wall_ns > budget_ns {
            return Err(Refused::OverBudget { worst_ns: est.wall_ns, budget_ns });
        }
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.jobs >= limits.max_jobs {
            return Err(Refused::TooManyJobs { jobs: s.jobs, limit: limits.max_jobs });
        }
        if untrusted && s.untrusted_jobs >= limits.max_untrusted_jobs {
            return Err(Refused::TooManyJobs { jobs: s.untrusted_jobs, limit: limits.max_untrusted_jobs });
        }
        let limit_ns = limits.max_in_flight.as_nanos() as u64;
        if s.in_flight_ns.saturating_add(est.worker_ns) > limit_ns {
            return Err(Refused::DeviceBusy { in_flight_ns: s.in_flight_ns, need_ns: est.worker_ns, limit_ns });
        }
        if untrusted {
            let limit_ns = limits.max_untrusted_in_flight.as_nanos() as u64;
            if s.untrusted_ns.saturating_add(est.worker_ns) > limit_ns {
                return Err(Refused::DeviceBusy { in_flight_ns: s.untrusted_ns, need_ns: est.worker_ns, limit_ns });
            }
            s.untrusted_ns += est.worker_ns;
            s.untrusted_jobs += 1;
        }
        s.in_flight_ns += est.worker_ns;
        s.jobs += 1;
        Ok(Ticket {
            ledger: self.clone(),
            ns: est.worker_ns,
            untrusted,
            estimate: est,
            work_limit: kernel.cost.saturating_mul(count as u64),
            host_call_limit,
            deadline: Instant::now() + wall,
        })
    }
}

/// Admits a job against the device ledger ([`Ledger::device`]).
pub fn admit(kernel: &Kernel, count: usize, threads: usize, origin: Origin, budget: &JobBudget) -> Result<Ticket, Refused> {
    Ledger::device().admit(kernel, count, threads, origin, budget)
}

/// An admitted job's share of the device ledger; returned on drop.
#[derive(Debug)]
pub struct Ticket {
    ledger: Ledger,
    ns: u64,
    untrusted: bool,
    estimate: Estimate,
    work_limit: u64,
    host_call_limit: u64,
    deadline: Instant,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ledger({:?})", self.stats())
    }
}

impl Ticket {
    /// For `Call::set_work_limit`: the admitted worst-case ops.
    pub fn work_limit(&self) -> u64 {
        self.work_limit
    }

    /// The op-equivalents one host call on a run-time input size may use
    /// (0: no limit, trusted code); an input over it is refused at the call.
    pub fn host_call_limit(&self) -> u64 {
        self.host_call_limit
    }

    /// When the watchdog cancels the job.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn estimate(&self) -> Estimate {
        self.estimate
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut s = self.ledger.0.state.lock().unwrap_or_else(|e| e.into_inner());
        s.in_flight_ns = s.in_flight_ns.saturating_sub(self.ns);
        s.jobs = s.jobs.saturating_sub(1);
        if self.untrusted {
            s.untrusted_ns = s.untrusted_ns.saturating_sub(self.ns);
            s.untrusted_jobs = s.untrusted_jobs.saturating_sub(1);
        }
    }
}
