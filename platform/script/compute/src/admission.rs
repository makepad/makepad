//! Admission: whether a kernel job may run, decided before it is queued.
//!
//! Splash is untrusted whatever wrote it (an AI, the store, a LAN host, a
//! livecode file). Everything here is counted in ops, the units a kernel
//! counts while it runs ([`crate::work`]): the same on every machine, at
//! any load. No clock decides anything.
//!
//! - **The job's budget** ([`JobBudget::work`]) is the counted ops it may
//!   run; past them it stops ([`crate::kernel::KernelError::OverBudget`]).
//!   An untrusted job's budget is capped by
//!   [`DeviceLimits::untrusted_job_work`].
//! - **The worst case** (static: every loop at its cap, every branch at its
//!   dearer side, every host call at its largest input) only refuses the
//!   absurd before anything runs: an untrusted job whose worst case passes
//!   [`DeviceLimits::untrusted_worst_case`], or one element of which may
//!   run more than [`DeviceLimits::element_work`] (how far a job can run
//!   between two looks at its count).
//! - **The device ledger**: the ops every admitted, unfinished job may
//!   still run (each charged the least of its worst case and its budget)
//!   and the number of such jobs fit the device's limits (untrusted origins
//!   have their own, smaller share).
//!
//! A refused job is reported with the numbers that refused it; nothing is
//! silently shortened. An admitted job holds a [`Ticket`] until it ends
//! (finished, failed or stopped); dropping the ticket returns its share of
//! the ledger. The ticket's [`Ticket::work_limit`] goes to
//! `Call::set_work_limit` (or the scheduler's job).

use crate::ir::{Block, Op, Program, Stmt};
use crate::kernel::Kernel;
use crate::lower::kernel::ELEMENT_CAP;
use std::sync::{Arc, Mutex, OnceLock};

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

/// What one job may run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JobBudget {
    /// Counted ops; the job stops once it has run more.
    pub work: u64,
}

impl JobBudget {
    /// Trusted work that runs to the end whatever it costs (the call's
    /// default limit).
    pub const UNLIMITED: JobBudget = JobBudget { work: crate::kernel::DEFAULT_WORK_LIMIT };
}

/// Device-wide limits (host settings), in ops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceLimits {
    /// What all admitted, unfinished jobs may still run, summed.
    pub max_in_flight: u64,
    /// The part of `max_in_flight` untrusted jobs may hold.
    pub max_untrusted_in_flight: u64,
    pub max_jobs: u32,
    pub max_untrusted_jobs: u32,
    /// The largest budget an untrusted job may be given.
    pub untrusted_job_work: u64,
    /// The largest worst case an untrusted job may have.
    pub untrusted_worst_case: u64,
    /// Untrusted: one element's worst case, which bounds how much a job
    /// runs between two looks at its count.
    pub element_work: u64,
}

impl Default for DeviceLimits {
    fn default() -> Self {
        DeviceLimits {
            max_in_flight: 1 << 36,
            max_untrusted_in_flight: 1 << 34,
            max_jobs: 4096,
            max_untrusted_jobs: 1024,
            untrusted_job_work: 1 << 32,
            untrusted_worst_case: 1 << 40,
            element_work: 1 << 21,
        }
    }
}

/// A job's worst case and what the ledger charges it, as admission
/// computed them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// One element's worst case.
    pub element_ops: u64,
    /// All elements' worst case.
    pub worst_ops: u64,
    /// What the job may run at most: the least of its worst case and its
    /// budget (the ledger's charge).
    pub charge_ops: u64,
}

/// One element's worst case, split so host calls can be bounded per call.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementCost {
    /// Everything but host calls whose input size is only known at run
    /// time (host calls on constant-size slices included).
    pub fixed_ops: u64,
    /// Host calls per element whose slice lengths are run-time values
    /// (loop caps included): each is bounded by the per-call limit.
    pub open_calls: u64,
    /// Those calls at their declared cost for their largest input.
    pub open_max_ops: u64,
}

impl ElementCost {
    /// The element's worst case when each open call may run `call_ops`.
    pub fn with_call_limit(&self, call_ops: u64) -> u64 {
        self.fixed_ops.saturating_add(self.open_calls.saturating_mul(call_ops))
    }

    fn add(self, o: ElementCost) -> ElementCost {
        ElementCost {
            fixed_ops: self.fixed_ops.saturating_add(o.fixed_ops),
            open_calls: self.open_calls.saturating_add(o.open_calls),
            open_max_ops: self.open_max_ops.saturating_add(o.open_max_ops),
        }
    }

    fn max(self, o: ElementCost) -> ElementCost {
        ElementCost { fixed_ops: self.fixed_ops.max(o.fixed_ops), open_calls: self.open_calls.max(o.open_calls), open_max_ops: self.open_max_ops.max(o.open_max_ops) }
    }

    fn times(self, n: u64) -> ElementCost {
        ElementCost { fixed_ops: self.fixed_ops.saturating_mul(n), open_calls: self.open_calls.saturating_mul(n), open_max_ops: self.open_max_ops.saturating_mul(n) }
    }

    fn fixed(ops: u64) -> ElementCost {
        ElementCost { fixed_ops: ops, ..Default::default() }
    }
}

/// Declared ops of one call of `h` with slice lengths `lens` (a length the
/// compiler knows, else the signature's maximum).
fn host_ops(h: &crate::host::HostFn, lens: &[Option<u32>]) -> u64 {
    let lens: Vec<u32> = lens.iter().zip(h.slices).map(|(l, s)| l.unwrap_or(s.max_words)).collect();
    h.cost_of(&lens)
}

/// The worst case of one element of `kernel`: what [`crate::work`] can
/// count for it at most (each statement its [`crate::work::op`], each pass
/// of a loop its body's ops and two, every loop at its cap, the element
/// loop once, each branch at its dearer side, a function call its body, a
/// host call its declared cost for its input).
pub fn element_cost(kernel: &Kernel) -> ElementCost {
    let program = kernel.program();
    fn consts(b: &Block, out: &mut std::collections::HashMap<u32, i32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    out.insert(v.0, *c);
                }
                Stmt::If(_, t, e) => {
                    consts(t, out);
                    consts(e, out);
                }
                Stmt::Loop { body, .. } => consts(body, out),
                _ => {}
            }
        }
    }
    fn walk(b: &Block, depth: u32, funcs: &[Program], c: &std::collections::HashMap<u32, i32>) -> ElementCost {
        let mut sum = ElementCost::default();
        for s in b {
            let x = match s {
                Stmt::If(_, t, e) => ElementCost::fixed(1).add(walk(t, depth, funcs, c).max(walk(e, depth, funcs, c))),
                Stmt::Loop { cap, body } => {
                    let reps = if depth == 0 { 1 } else { *cap as u64 };
                    walk(body, depth + 1, funcs, c).add(ElementCost::fixed(2)).times(reps)
                }
                Stmt::Call { f, .. } => match funcs.get(*f as usize) {
                    Some(g) => {
                        let mut gc = std::collections::HashMap::new();
                        consts(&g.body, &mut gc);
                        ElementCost::fixed(crate::work::op(s)).add(walk(&g.body, depth.max(1), funcs, &gc))
                    }
                    None => ElementCost::fixed(u64::MAX / 4),
                },
                Stmt::CallHost { f, slices, .. } => {
                    let call = ElementCost::fixed(crate::work::op(s));
                    match crate::host::get(*f) {
                        Some(h) => {
                            let lens: Vec<Option<u32>> = slices.iter().map(|x| c.get(&x.len.0).map(|n| *n as u32)).collect();
                            if lens.iter().all(|l| l.is_some()) {
                                call.add(ElementCost::fixed(host_ops(&h, &lens)))
                            } else {
                                call.add(ElementCost { fixed_ops: 0, open_calls: 1, open_max_ops: host_ops(&h, &vec![None; lens.len()]) })
                            }
                        }
                        // An unknown index costs everything.
                        None => ElementCost::fixed(u64::MAX / 4),
                    }
                }
                s => ElementCost::fixed(crate::work::op(s)),
            };
            sum = sum.add(x);
        }
        sum
    }
    let mut c = std::collections::HashMap::new();
    consts(&program.body, &mut c);
    walk(&program.body, 0, &program.funcs, &c)
}

/// One element's worst case with every host call at its largest input
/// (the trusted bound).
pub fn element_ops(kernel: &Kernel) -> u64 {
    let c = element_cost(kernel);
    c.fixed_ops.saturating_add(c.open_max_ops)
}

/// The worst case of `count` elements (every host call at its largest
/// input), charged as if its budget were unlimited.
pub fn estimate(kernel: &Kernel, count: usize) -> Estimate {
    let element_ops = element_ops(kernel);
    let worst_ops = element_ops.saturating_mul(count as u64);
    Estimate { element_ops, worst_ops, charge_ops: worst_ops }
}

/// Why a job was not admitted.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// More elements than one job may run.
    TooMany(usize),
    /// Untrusted: one element may run more than the element bound.
    ElementTooHeavy { worst_ops: u64, limit_ops: u64 },
    /// Untrusted: the job's worst case passes the device's ceiling.
    OverCeiling { worst_ops: u64, ceiling_ops: u64 },
    /// The device's in-flight work is taken (queue or retry later).
    DeviceBusy { in_flight_ops: u64, need_ops: u64, limit_ops: u64 },
    /// The device has too many admitted, unfinished jobs.
    TooManyJobs { jobs: u32, limit: u32 },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::TooMany(n) => write!(f, "{} elements is more than one job may run", n),
            Refused::ElementTooHeavy { worst_ops, limit_ops } => {
                write!(f, "one element may run {} ops in the worst case; untrusted kernels are limited to {} ops per element (reduce loop sizes)", worst_ops, limit_ops)
            }
            Refused::OverCeiling { worst_ops, ceiling_ops } => write!(f, "the job may run {} ops in the worst case; the ceiling is {} ops", worst_ops, ceiling_ops),
            Refused::DeviceBusy { in_flight_ops, need_ops, limit_ops } => {
                write!(f, "the device has {} ops of kernel work in flight; this job needs {} more and the limit is {}", in_flight_ops, need_ops, limit_ops)
            }
            Refused::TooManyJobs { jobs, limit } => write!(f, "{} kernel jobs are in flight; the limit is {}", jobs, limit),
        }
    }
}

#[derive(Default, Debug)]
struct State {
    in_flight: u64,
    untrusted: u64,
    jobs: u32,
    untrusted_jobs: u32,
}

/// Totals of the admitted, unfinished jobs (for stats).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LedgerStats {
    pub in_flight_ops: u64,
    pub untrusted_ops: u64,
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
        LedgerStats { in_flight_ops: s.in_flight, untrusted_ops: s.untrusted, jobs: s.jobs, untrusted_jobs: s.untrusted_jobs }
    }

    /// Admits `count` elements of `kernel` from `origin` to run within
    /// `budget`, or says why not.
    pub fn admit(&self, kernel: &Kernel, count: usize, origin: Origin, budget: &JobBudget) -> Result<Ticket, Refused> {
        if count > ELEMENT_CAP as usize {
            return Err(Refused::TooMany(count));
        }
        let limits = self.limits();
        let untrusted = origin.untrusted();
        // Computed once per kernel: admission allocates nothing per job.
        let cost = *kernel.admission.get_or_init(|| element_cost(kernel));
        let mut work = budget.work;
        // Host calls on run-time input sizes: each is held to a per-call op
        // limit, enforced at the call (an input over it is refused there,
        // not run). Trusted code: each function's largest input.
        let (element, host_call_limit) = if untrusted {
            let limit = limits.element_work;
            if cost.fixed_ops > limit {
                return Err(Refused::ElementTooHeavy { worst_ops: cost.fixed_ops, limit_ops: limit });
            }
            work = work.min(limits.untrusted_job_work);
            if cost.open_calls == 0 {
                (cost.fixed_ops, 0)
            } else {
                // What the element bound leaves, shared by the open calls
                // (no more than their largest input needs).
                let call = ((limit - cost.fixed_ops) / cost.open_calls).min(cost.open_max_ops.max(1));
                if call == 0 {
                    return Err(Refused::ElementTooHeavy { worst_ops: cost.with_call_limit(1), limit_ops: limit });
                }
                // The limit is one ctx word (K_HOST_LIMIT).
                let call = call.min(u32::MAX as u64);
                (cost.with_call_limit(call), call)
            }
        } else {
            (cost.fixed_ops.saturating_add(cost.open_max_ops), 0)
        };
        let worst_ops = element.saturating_mul(count as u64);
        if untrusted && worst_ops > limits.untrusted_worst_case {
            return Err(Refused::OverCeiling { worst_ops, ceiling_ops: limits.untrusted_worst_case });
        }
        let est = Estimate { element_ops: element, worst_ops, charge_ops: worst_ops.min(work) };
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.jobs >= limits.max_jobs {
            return Err(Refused::TooManyJobs { jobs: s.jobs, limit: limits.max_jobs });
        }
        if untrusted && s.untrusted_jobs >= limits.max_untrusted_jobs {
            return Err(Refused::TooManyJobs { jobs: s.untrusted_jobs, limit: limits.max_untrusted_jobs });
        }
        if s.in_flight.saturating_add(est.charge_ops) > limits.max_in_flight {
            return Err(Refused::DeviceBusy { in_flight_ops: s.in_flight, need_ops: est.charge_ops, limit_ops: limits.max_in_flight });
        }
        if untrusted {
            if s.untrusted.saturating_add(est.charge_ops) > limits.max_untrusted_in_flight {
                return Err(Refused::DeviceBusy { in_flight_ops: s.untrusted, need_ops: est.charge_ops, limit_ops: limits.max_untrusted_in_flight });
            }
            s.untrusted += est.charge_ops;
            s.untrusted_jobs += 1;
        }
        s.in_flight += est.charge_ops;
        s.jobs += 1;
        Ok(Ticket { ledger: self.clone(), untrusted, estimate: est, work_limit: work, host_call_limit })
    }
}

/// Admits a job against the device ledger ([`Ledger::device`]).
pub fn admit(kernel: &Kernel, count: usize, origin: Origin, budget: &JobBudget) -> Result<Ticket, Refused> {
    Ledger::device().admit(kernel, count, origin, budget)
}

/// An admitted job's share of the device ledger; returned on drop.
#[derive(Debug)]
pub struct Ticket {
    ledger: Ledger,
    untrusted: bool,
    estimate: Estimate,
    work_limit: u64,
    host_call_limit: u64,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ledger({:?})", self.stats())
    }
}

impl Ticket {
    /// For `Call::set_work_limit`: the counted ops the job may run.
    pub fn work_limit(&self) -> u64 {
        self.work_limit
    }

    /// The ops one host call on a run-time input size may run (0: no
    /// limit, trusted code); an input over it is refused at the call.
    pub fn host_call_limit(&self) -> u64 {
        self.host_call_limit
    }

    pub fn estimate(&self) -> Estimate {
        self.estimate
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut s = self.ledger.0.state.lock().unwrap_or_else(|e| e.into_inner());
        s.in_flight = s.in_flight.saturating_sub(self.estimate.charge_ops);
        s.jobs = s.jobs.saturating_sub(1);
        if self.untrusted {
            s.untrusted = s.untrusted.saturating_sub(self.estimate.charge_ops);
            s.untrusted_jobs = s.untrusted_jobs.saturating_sub(1);
        }
    }
}
