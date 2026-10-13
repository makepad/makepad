//! The owner thread (local/agent_state/fast-rust/JIT_THREADS.md): the one thread that runs every
//! step reading or changing a unit's mutable state -- lazy compiles' fronts and installs, constant
//! and static evaluation (and the JIT code they run), the disk cache, patches, tier installs, the
//! exit summary. There is no unit lock. Other threads hand it closures and wait for them
//! (`call`); it never blocks on another thread: every wait on it is a pump (`pump_wait`) that
//! serves the requests which may be what it waits for.
//!
//! Every request runs in a fresh frame (jit::frame_enter) and in its requester's query context
//! (relax_base::query): queries it runs record their reads where the requester's would, and a
//! query the requester is running is the parent of the ones the request starts, so a real cycle
//! through a request is the query engine's cycle error. A request served nested inside an
//! earlier request's wait that reaches something only that earlier request can finish (a query
//! or a build it has open, a load) is cancelled (`OwnerBusy`) and its requester posts it again.

use std::any::Any;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use relax_session::sync::{Condvar, Mutex};

/// Request classes, in service order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Class {
    /// asked by a pure job that may hold what the owner waits for (a constant's value for a type
    /// check, a report): served also inside every pump
    Job = 0,
    /// a back end's result (store, install): never opens a build; served also inside every pump
    Finish = 1,
    /// the program's main thread
    Main = 2,
    /// other program threads, patches, the run's own foreground needs
    App = 3,
    /// speculation, tiers, seeds, teardown, completion, monitors: only when nothing above waits
    Background = 4,
}
const CLASSES: usize = 5;
pub const CLASS_NAMES: [&str; CLASSES] = ["job", "finish", "main", "app", "background"];
/// makepad inspect's timeline span names of the owner's service, by class
const SERVE_NAMES: [&str; CLASSES] = ["serve job", "serve finish", "serve main", "serve app", "serve background"];

const QUEUED: u8 = 0;
const RUNNING: u8 = 1;
const DONE: u8 = 2;
/// the owner touches the request no more: the requester may return (and free it)
const RELEASED: u8 = 3;
/// How long a requester spins for its answer, and the idle owner for the next request, before
/// sleeping (a sleep/wake pair costs tens of microseconds; warm launches make thousands of
/// short requests in a row)
const SPIN: std::time::Duration = std::time::Duration::from_micros(60);

struct Req {
    run: *mut dyn FnMut(),
    class: Class,
    /// requester (os thread id)
    from: u64,
    /// the requester's query context
    ctx: *const (),
    /// the requester's compile record (jit::main_rec)
    rec: *const (),
    /// reads no slot identity (the startup image's identities stay encoded: jit::ids_on_serve)
    quick: bool,
    state: AtomicU8,
    panic: Cell<Option<Box<dyn Any + Send>>>,
    /// false: posted (heap, nobody waits); the owner frees it
    waited: bool,
    /// the requester sleeps on these until Done (no thread handle: a thread's TLS destructors
    /// compile too, after std::thread::current() is gone)
    done: Mutex<bool>,
    wake: Condvar,
}

struct Queue {
    lanes: [VecDeque<usize>; CLASSES],
}

static Q: Mutex<Queue> = Mutex::new(Queue { lanes: [const { VecDeque::new() }; CLASSES] });

pub(crate) struct ZoneTaken(Queue);

pub(crate) fn zone_take() -> ZoneTaken {
    let mut queue = Q.lock().unwrap_or_else(|e| e.into_inner());
    // Quiescence includes draining every request before taking the lanes' buffers.
    assert!(queue.lanes.iter().all(VecDeque::is_empty), "memlane swap with queued owner requests");
    let taken = ZoneTaken(std::mem::replace(&mut *queue, Queue { lanes: [const { VecDeque::new() }; CLASSES] }));
    #[cfg(test)]
    assert!(queue.lanes.iter().all(|lane| lane.is_empty() && lane.capacity() == 0));
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut queue = Q.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *queue, taken.0));
}

static WAKE: Condvar = Condvar::new();
static STARTED: std::sync::Once = std::sync::Once::new();
/// Main or App requests queued (background work gives way: `urgent`)
static FOREGROUND: AtomicU32 = AtomicU32::new(0);
/// Main requests queued
static MAIN_QUEUED: AtomicU32 = AtomicU32::new(0);
/// requests queued (any class): the idle owner spins on it
static QUEUED_N: AtomicU32 = AtomicU32::new(0);
/// the owner sleeps on WAKE (a push then notifies it; otherwise it is spinning or busy)
static OWNER_ASLEEP: AtomicBool = AtomicBool::new(false);

/// os thread id of the owner thread
static OWNER_TID: AtomicU64 = AtomicU64::new(0);
static OWNER_THREAD: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
/// the threads ending the process (0: none): from then on only their requests (and Job / Finish
/// ones) are served -- other threads' requests wait until the process is gone
static EXIT_TID: AtomicU64 = AtomicU64::new(0);
static EXIT_HELPER: AtomicU64 = AtomicU64::new(0);
/// a forked child: one thread, which runs every request itself
static CHILD: AtomicBool = AtomicBool::new(false);
/// wall ns the owner spent serving, by class (the run summary)
pub static BUSY_NS: [AtomicU64; CLASSES] = [const { AtomicU64::new(0) }; CLASSES];
/// the main thread's longest wait for a request after the first present (ns), and the class and
/// requester the owner was serving when it began (a frame hitch: run summary)
pub static MAIN_WAIT_MAX: AtomicU64 = AtomicU64::new(0);
static MAIN_WAIT_BEHIND: AtomicU64 = AtomicU64::new(u64::MAX);
/// what the owner serves now: class (u8::MAX: idle) and requester
static NOW_CLASS: AtomicU8 = AtomicU8::new(u8::MAX);
static NOW_FROM: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static IS_OWNER: Cell<bool> = const { Cell::new(false) };
    /// requests this thread waits for (0: none): a watchdog kick then is ignored
    static IN_CALL: Cell<u32> = const { Cell::new(0) };
    /// owner: the request being served (class, requester) and how many are open on its stack
    static SERVING: Cell<(Class, u64)> = const { Cell::new((Class::Background, 0)) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

fn me() -> u64 {
    crate::os::thread::current()
}

/// Is this the owner thread (or the only thread of a forked child)?
#[inline]
pub fn is_owner() -> bool {
    IS_OWNER.try_with(|o| o.get()).unwrap_or(false)
}

/// Is this thread waiting for a request?
pub fn in_call() -> bool {
    IN_CALL.try_with(|c| c.get() > 0).unwrap_or(false)
}

fn in_call_add(d: i32) {
    let _ = IN_CALL.try_with(|c| c.set((c.get() as i32 + d) as u32));
}

/// The request being served on the owner: (class, requester).
pub fn serving() -> (Class, u64) {
    SERVING.with(|s| s.get())
}

/// A request served inside an earlier request's wait (owner only).
pub fn nested() -> bool {
    DEPTH.with(|d| d.get()) > 1
}

/// A program thread's class: Main on the program's main thread, App elsewhere.
pub fn program_class() -> Class {
    if crate::jit::MAIN_TID.load(Ordering::Relaxed) == me() { Class::Main } else { Class::App }
}

/// Is the main thread the one this work is for (it runs on main, or the owner serves main)?
pub fn for_main() -> bool {
    let main = crate::jit::MAIN_TID.load(Ordering::Relaxed);
    main != 0 && if is_owner() { serving().1 == main } else { me() == main }
}

/// Background work on the owner gives way when this is true (a program thread waits).
#[inline]
pub fn urgent() -> bool {
    FOREGROUND.load(Ordering::Relaxed) > 0
}

/// The main thread waits for a request now.
pub fn main_waiting() -> bool {
    MAIN_QUEUED.load(Ordering::Relaxed) > 0
}

/// What the owner serves now (diagnostics): "idle", or the class and its requester's name.
pub fn serving_note() -> String {
    match NOW_CLASS.load(Ordering::Relaxed) {
        u8::MAX => "idle".to_string(),
        c => format!("{} for thread `{}`", CLASS_NAMES[c as usize], crate::os::thread::name(NOW_FROM.load(Ordering::Relaxed))),
    }
}

/// The request queue as a JSON object (makepad inspect; atomics only, never the queue's lock):
/// queued requests, of which foreground (main + app) and main.
pub fn queued_note() -> String {
    format!(
        "{{\"total\":{},\"foreground\":{},\"main\":{}}}",
        QUEUED_N.load(Ordering::Relaxed),
        FOREGROUND.load(Ordering::Relaxed),
        MAIN_QUEUED.load(Ordering::Relaxed)
    )
}

/// What the main thread's longest post-present wait was behind (diagnostics).
pub fn main_wait_behind() -> String {
    match MAIN_WAIT_BEHIND.load(Ordering::Relaxed) {
        u64::MAX => "-".to_string(),
        c => CLASS_NAMES[c as usize & 7].to_string(),
    }
}

/// The owner thread's id (0: not started).
pub fn owner_tid() -> u64 {
    OWNER_TID.load(Ordering::Relaxed)
}

/// Starts the owner thread (once; every unit shares it).
pub fn start() {
    STARTED.call_once(|| {
        // The blocking context retains the owner's thread handle, including its name.
        let _k = crate::zone_alloc::kept();
        crate::query::set_wait_hook(crate::query::WaitHook { on_owner: is_owner, pump: pump_latch, wake, me });
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("run-owner".into())
            .stack_size(256 << 20)
            .spawn(move || {
                {
                    let _k = crate::zone_alloc::kept();
                    // std's blocking context survives every compiler generation on this thread.
                    let (_tx, rx) = std::sync::mpsc::sync_channel::<()>(0);
                    let _ = rx.recv_timeout(std::time::Duration::ZERO);
                }
                let _z = crate::zone_alloc::compiler_thread();
                IS_OWNER.with(|o| o.set(true));
                OWNER_TID.store(me(), Ordering::SeqCst);
                crate::waits::register();
                let _ = OWNER_THREAD.set(std::thread::current());
                crate::jit::ensure_altstack();
                let _ = crate::zone_alloc::kept_with(|| tx.send(()));
                owner_loop();
            })
            .expect("makepad: cannot start the compiler's owner thread");
        let _ = rx.recv();
    });
}

/// Runs `f` on the owner thread and returns its result (its panic resumes here). On the owner it
/// runs at once (nested: constant evaluation's JIT code compiling what it calls). The caller
/// waits, so `f` may borrow its locals (as `std::thread::scope`'s threads do).
#[track_caller]
pub fn call<R>(class: Class, f: impl FnOnce() -> R) -> R {
    request(class, false, None, f).unwrap()
}

/// `call` for work that reads no slot identity (the startup identities stay encoded).
pub fn call_quick<R>(class: Class, f: impl FnOnce() -> R) -> R {
    request(class, true, None, f).unwrap()
}

/// `call`, unless the owner has not started it within `ms`: then it is withdrawn (None).
pub fn call_within<R>(class: Class, ms: u64, f: impl FnOnce() -> R) -> Option<R> {
    // (at least 2 ms: an idle owner picks a request up in microseconds; 0 means "only if the
    // owner is free now")
    request(class, false, Some(std::time::Instant::now() + std::time::Duration::from_millis(ms.max(2))), f)
}

fn request<R>(class: Class, quick: bool, deadline: Option<std::time::Instant>, f: impl FnOnce() -> R) -> Option<R> {
    if is_owner() || CHILD.load(Ordering::Relaxed) {
        return Some(f());
    }
    // a front reader that must not wait (lowering ahead) gives the work back first; a back
    // end's finish is never refused (its slots stay in flight until it is served)
    if class != Class::Finish {
        crate::jit::lower_ahead_needs_unit();
        crate::preempt::point();
    }
    start();
    let mut out: Option<R> = None;
    let outp: *mut Option<R> = &mut out;
    let mut f = Some(f);
    let mut run = move || unsafe { *outp = Some((f.take().unwrap())()) };
    let run: &mut dyn FnMut() = &mut run;
    // SAFETY: the request is done (or withdrawn) before this frame returns; the owner runs the
    // closure only between this push and the Done store
    let run: *mut (dyn FnMut() + 'static) = unsafe { std::mem::transmute(run) };
    let main_wait = class == Class::Main && crate::jit::FIRST_PRESENT.load(Ordering::Acquire) && !crate::jit::QUIT_SEEN.load(Ordering::Relaxed);
    // a thread waiting for its work on the owner is not a compile job meanwhile (an allocator
    // swap or fork waits for the jobs running; its work runs on the owner, which serves the
    // pauser): pure jobs (Job class) are, they hold front guards and queries
    let _idle = (class != Class::Job && class != Class::Finish).then(crate::jit::JobIdle::enter);
    let req = Req {
        run,
        class,
        from: me(),
        ctx: crate::query::context_ptr(),
        rec: crate::jit::main_rec_target(),
        quick,
        state: AtomicU8::new(QUEUED),
        panic: Cell::new(None),
        waited: true,
        done: Mutex::new(false),
        wake: Condvar::new(),
    };
    let behind = NOW_CLASS.load(Ordering::Relaxed);
    let t_req = std::time::Instant::now();
    let t0 = if main_wait { crate::jit::wall_ns() } else { 0 };
    in_call_add(1);
    push(&req as *const Req as usize, class);
    let withdrawn = {
        let _w = crate::waits::begin(crate::waits::Kind::Owner, class as u64, OWNER_TID.load(Ordering::Relaxed), CLASS_NAMES[class as usize]);
        wait(&req, deadline)
    };
    in_call_add(-1);
    // makepad inspect: a real-time thread (an audio callback) that waited for the compiler
    if crate::inspect::events::watched() {
        let waited = std::time::Instant::now().duration_since(t_req);
        if waited.as_millis() >= 2 && crate::os::thread::realtime() {
            crate::inspect::events::emit(
                "rtstall",
                &format!("{}", me()),
                &format!(
                    "\"thread\":{},\"waited_ms\":{:.1},\"class\":\"{}\",\"owner_serves\":{},\"more\":[\"why-slow\",\"timeline start\"]",
                    crate::inspect::json_str(&crate::os::thread::name(me())),
                    waited.as_secs_f64() * 1e3,
                    CLASS_NAMES[class as usize],
                    crate::inspect::json_str(&serving_note())
                ),
            );
        }
    }
    if main_wait {
        let w = crate::jit::wall_ns() - t0;
        if MAIN_WAIT_MAX.fetch_max(w, Ordering::Relaxed) < w {
            MAIN_WAIT_BEHIND.store(behind as u64, Ordering::Relaxed);
        }
    }
    if withdrawn {
        return None;
    }
    match req.panic.take() {
        // served nested and blocked by an earlier request on the owner: the requester's
        // work is cancelled as a preemption is (its queries too) and redone later. Only
        // Job requests are served nested, and those that can be blocked come from type
        // checks (a constant's value), which retry a preempted body.
        Some(p) if p.is::<crate::preempt::OwnerBusy>() => {
            std::panic::resume_unwind(Box::new(crate::preempt::Preempted));
        }
        Some(p) => std::panic::resume_unwind(p),
        None => Some(out.unwrap()),
    }
}

/// Posts `f` to the owner and returns at once (it runs later, in class order).
pub fn post(class: Class, f: impl FnOnce() + Send + 'static) {
    if is_owner() || CHILD.load(Ordering::Relaxed) {
        f();
        return;
    }
    start();
    let mut f = Some(f);
    let run: Box<dyn FnMut()> = Box::new(move || (f.take().unwrap())());
    let req = Box::new(Req {
        run: Box::into_raw(run),
        class,
        from: me(),
        ctx: std::ptr::null(),
        rec: std::ptr::null(),
        quick: false,
        state: AtomicU8::new(QUEUED),
        panic: Cell::new(None),
        waited: false,
        done: Mutex::new(false),
        wake: Condvar::new(),
    });
    push(Box::into_raw(req) as usize, class);
}

fn push(r: usize, class: Class) {
    let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
    q.lanes[class as usize].push_back(r);
    if matches!(class, Class::Main | Class::App) {
        FOREGROUND.fetch_add(1, Ordering::Relaxed);
    }
    if class == Class::Main {
        MAIN_QUEUED.fetch_add(1, Ordering::Relaxed);
    }
    QUEUED_N.fetch_add(1, Ordering::SeqCst);
    if OWNER_ASLEEP.load(Ordering::SeqCst) {
        WAKE.notify_all();
    }
}

fn dequeued(class: usize) {
    QUEUED_N.fetch_sub(1, Ordering::SeqCst);
    if class == Class::Main as usize || class == Class::App as usize {
        FOREGROUND.fetch_sub(1, Ordering::Relaxed);
    }
    if class == Class::Main as usize {
        MAIN_QUEUED.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Waits for `req` to be done; true: withdrawn at the deadline before it started.
fn wait(req: &Req, deadline: Option<std::time::Instant>) -> bool {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < SPIN {
        if req.state.load(Ordering::Acquire) == RELEASED {
            return false;
        }
        std::hint::spin_loop();
    }
    let mut done = req.done.lock().unwrap_or_else(|e| e.into_inner());
    while !*done {
        match deadline {
            Some(d) if req.state.load(Ordering::Acquire) == QUEUED => {
                let now = std::time::Instant::now();
                if now >= d {
                    drop(done);
                    let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
                    // the owner takes a request out of its lane under this mutex: still in the
                    // lane = not started
                    let lane = &mut q.lanes[req.class as usize];
                    if let Some(i) = lane.iter().position(|x| *x == req as *const Req as usize) {
                        lane.remove(i);
                        dequeued(req.class as usize);
                        return true;
                    }
                    drop(q);
                    done = req.done.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                done = req.wake.wait_timeout(done, d - now).unwrap_or_else(|e| e.into_inner()).0;
            }
            _ => done = req.wake.wait(done).unwrap_or_else(|e| e.into_inner()),
        }
    }
    drop(done);
    // the owner's last touch follows its unlock
    while req.state.load(Ordering::Acquire) != RELEASED {
        std::hint::spin_loop();
    }
    false
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// the owner's idle loop: everything, in class order
    Top,
    /// a wait inside a request: Job and Finish only (what the awaited thread may be waiting
    /// for; a program thread's request waits for the owner's current one to end)
    Pump,
}

fn pick(q: &mut Queue, mode: Mode) -> Option<usize> {
    // the process ends (only the exit set's requests), or the program swaps its allocator (only
    // the pauser's): other threads' foreground and background requests wait
    let only = match EXIT_TID.load(Ordering::Acquire) {
        0 => crate::jit::paused_for(),
        e => e,
    };
    let helper = EXIT_HELPER.load(Ordering::Acquire);
    for c in 0..CLASSES {
        if mode == Mode::Pump && c >= Class::Main as usize {
            break;
        }
        if c >= Class::Background as usize && FOREGROUND.load(Ordering::Relaxed) > 0 {
            break;
        }
        let lane = &mut q.lanes[c];
        let at = if only != 0 && c >= Class::Main as usize {
            lane.iter().position(|r| {
                let from = unsafe { (*(*r as *const Req)).from };
                from == only || from == helper && helper != 0
            })
        } else if lane.is_empty() {
            None
        } else {
            Some(0)
        };
        if let Some(i) = at {
            let r = lane.remove(i).unwrap();
            dequeued(c);
            return Some(r);
        }
    }
    None
}

fn serve(r: usize) {
    let req = unsafe { &*(r as *const Req) };
    req.state.store(RUNNING, Ordering::Release);
    let t0 = crate::jit::wall_ns();
    let prev_ctx = unsafe { crate::query::swap_context(req.ctx) };
    let prev = SERVING.with(|s| s.replace((req.class, req.from)));
    let prev_now = (NOW_CLASS.swap(req.class as u8, Ordering::Relaxed), NOW_FROM.swap(req.from, Ordering::Relaxed));
    DEPTH.with(|d| d.set(d.get() + 1));
    let frame = crate::jit::frame_enter(req.rec);
    // makepad inspect's timeline: the owner's service of this request is a span
    let traced = crate::inspect::timeline::recording();
    if traced {
        crate::inspect::timeline::push(crate::inspect::timeline::Kind::SpanBegin, SERVE_NAMES[req.class as usize], req.from, 0);
    }
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !req.quick {
            crate::jit::ids_on_serve();
        }
        unsafe { (*req.run)() }
    }));
    let busy = crate::jit::frame_leave(frame);
    if traced {
        crate::inspect::timeline::push(crate::inspect::timeline::Kind::SpanEnd, SERVE_NAMES[req.class as usize], req.from, 0);
    }
    // a compile the request's code reached was cancelled and nothing cancelled the request yet:
    // its result is not used
    let res = match res {
        Ok(()) if busy => Err(Box::new(crate::preempt::OwnerBusy) as Box<dyn Any + Send>),
        r => r,
    };
    // MAKEPAD_RUN_OWNER_TRACE: a request's panic, with where it happened on the owner (the compiler's
    // panic hook is silent there; the requester resumes it)
    if let Err(p) = &res {
        if std::env::var_os("MAKEPAD_RUN_OWNER_TRACE").is_some() {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "(cancellation)".to_string());
            eprintln!("makepad-run owner: a {:?} request of thread `{}` panicked on the owner: {}{}", req.class, crate::os::thread::name(req.from), msg, crate::jit::take_ice_location());
        }
    }
    DEPTH.with(|d| d.set(d.get() - 1));
    NOW_CLASS.store(prev_now.0, Ordering::Relaxed);
    NOW_FROM.store(prev_now.1, Ordering::Relaxed);
    SERVING.with(|s| s.set(prev));
    unsafe { crate::query::swap_context(prev_ctx) };
    BUSY_NS[req.class as usize].fetch_add(crate::jit::wall_ns().saturating_sub(t0), Ordering::Relaxed);
    // the process is ending on another thread: this request is never answered (its requester
    // waits until the process is gone)
    if res.as_ref().is_err_and(|p| p.is::<ExitParked>()) {
        return;
    }
    match req.waited {
        true => {
            if let Err(p) = res {
                req.panic.set(Some(p));
            }
            if traced {
                // the edge that ends the requester's wait
                crate::inspect::timeline::push(crate::inspect::timeline::Kind::Served, "", req.from, 0);
            }
            req.state.store(DONE, Ordering::Release);
            {
                let mut done = req.done.lock().unwrap_or_else(|e| e.into_inner());
                *done = true;
                req.wake.notify_one();
            }
            // the last touch: after this store the requester may return and `req` is gone
            req.state.store(RELEASED, Ordering::Release);
        }
        false => unsafe {
            drop(Box::from_raw(req.run));
            drop(Box::from_raw(r as *mut Req));
            if let Err(p) = res {
                if !crate::preempt::is_cancelled(&*p) {
                    let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                    eprintln!("makepad: a posted compiler task panicked: {}", msg);
                }
            }
        },
    }
}

fn owner_loop() -> ! {
    loop {
        // idle: a short spin catches the next request of a burst without a sleep and wake
        let t0 = std::time::Instant::now();
        while QUEUED_N.load(Ordering::Relaxed) == 0 && FORK.load(Ordering::Relaxed) != 1 && t0.elapsed() < SPIN {
            std::hint::spin_loop();
        }
        let r = {
            let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                // a fork: parked here, between two requests (no request's state half done)
                if FORK.load(Ordering::SeqCst) == 1 {
                    drop(q);
                    FORK.store(2, Ordering::SeqCst);
                    while FORK.load(Ordering::SeqCst) == 2 {
                        std::thread::park();
                    }
                    q = Q.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                if let Some(r) = pick(&mut q, Mode::Top) {
                    break r;
                }
                OWNER_ASLEEP.store(true, Ordering::SeqCst);
                q = WAKE.wait(q).unwrap_or_else(|e| e.into_inner());
                OWNER_ASLEEP.store(false, Ordering::SeqCst);
            }
        };
        serve(r);
        // between top-level requests: the vtables made for real-time trait-object types
        let frame = crate::jit::frame_enter(std::ptr::null());
        let _ = std::panic::catch_unwind(crate::jit::realtime_drain);
        crate::jit::frame_leave(frame);
    }
}

/// The owner waits for `done`, serving meanwhile what may be holding it up: Job and Finish
/// requests (a type check's constant, a back end's finish). On another thread it waits
/// plainly. `done` only reads state or takes leaf locks.
pub fn pump_wait(done: &dyn Fn() -> bool) {
    pump_wait_for(0, done)
}

/// `pump_wait` for a wait on thread `runner` (0: unknown), as the wait registry records it.
fn pump_wait_for(runner: u64, done: &dyn Fn() -> bool) {
    if done() {
        return;
    }
    let _w = crate::waits::begin(crate::waits::Kind::Pump, 0, runner, "pump");
    if !is_owner() || CHILD.load(Ordering::Relaxed) {
        let t0 = std::time::Instant::now();
        while !done() {
            if CHILD.load(Ordering::Relaxed) && t0.elapsed().as_secs() >= 10 {
                eprintln!("makepad: forked child waited 10 s for work of a thread of the parent -- exiting");
                crate::os::thread::exit_now(134);
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        return;
    }
    loop {
        if done() {
            return;
        }
        let r = {
            let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
            match pick(&mut q, Mode::Pump) {
                Some(r) => Some(r),
                None => {
                    // a latch completing or a request arriving wakes it; the timeout bounds a
                    // condition nothing announces (an in-flight flag cleared by a Finish)
                    OWNER_ASLEEP.store(true, Ordering::SeqCst);
                    let _ = WAKE.wait_timeout(q, std::time::Duration::from_millis(1));
                    OWNER_ASLEEP.store(false, Ordering::SeqCst);
                    None
                }
            }
        };
        if let Some(r) = r {
            serve(r);
        }
    }
}

/// The query engine's wait for a query another thread runs (or, nested, a lower frame of this
/// one): on the owner a pump; a query running lower on the owner's own stack cannot finish
/// before this request does, so a nested request is cancelled instead.
fn pump_latch(runner: u64, done: &dyn Fn() -> bool) {
    if runner != 0 && runner == OWNER_TID.load(Ordering::Relaxed) && !done() {
        if nested() {
            std::panic::resume_unwind(Box::new(crate::preempt::OwnerBusy));
        }
        panic!("makepad: a query waits for itself on the compiler's owner thread (a query cycle the engine did not see)");
    }
    pump_wait_for(runner, done);
}

/// A request served nested cannot do what only the owner's earlier request may finish (a load,
/// a front write, waiting for a back end): it is cancelled and posted again later.
pub fn busy_if_nested() {
    if is_owner() && nested() {
        std::panic::resume_unwind(Box::new(crate::preempt::OwnerBusy));
    }
}

/// Wakes a pumping owner (a query latch it may wait for completed).
pub fn wake() {
    if CHILD.load(Ordering::Relaxed) || !STARTED.is_completed() {
        return;
    }
    let _q = Q.lock().unwrap_or_else(|e| e.into_inner());
    WAKE.notify_all();
}

/// The process is ending on this thread: from now on the owner serves only its requests and its
/// exit helper's (and Job / Finish ones); other threads' requests stay queued until the process
/// is gone.
pub fn begin_exit() {
    let _ = EXIT_TID.compare_exchange(0, me(), Ordering::AcqRel, Ordering::Acquire);
    wake();
}

/// This thread works for the exiting one (run.rs's exit worker): its requests are served too.
pub fn exit_helper() {
    EXIT_HELPER.store(me(), Ordering::Release);
}

/// The thread ending the process (0: none).
pub fn exit_tid() -> u64 {
    EXIT_TID.load(Ordering::Acquire)
}

/// The request being served is the exit's (the exiting thread's or its helper's).
pub fn serving_exit() -> bool {
    let from = serving().1;
    from == EXIT_TID.load(Ordering::Acquire) || from != 0 && from == EXIT_HELPER.load(Ordering::Acquire)
}

/// The unwind payload of a request left unanswered because the process is ending.
pub struct ExitParked;

/// Ends the request being served without an answer: the process is ending on another thread.
pub fn park_for_exit() -> ! {
    std::panic::resume_unwind(Box::new(ExitParked))
}

/// The owner serves nothing and nothing is queued (the settle swap's drain).
pub fn idle() -> bool {
    NOW_CLASS.load(Ordering::Acquire) == u8::MAX && QUEUED_N.load(Ordering::SeqCst) == 0
}

/// The owner is serving a request of another thread than `tid` now.
pub fn busy_for_other(tid: u64) -> bool {
    NOW_CLASS.load(Ordering::Acquire) != u8::MAX && NOW_FROM.load(Ordering::Acquire) != tid
}

/// The allocator swap ended: the owner serves everyone again.
pub fn resume() {
    wake();
}

// ------------------------------------------------------------ fork

/// fork() in a program run by Relax: the owner is parked between two requests across the fork
/// (holding no lock), so the child -- which has no owner thread -- finds the units consistent
/// and runs every request on its one thread. 0: no fork, 1: asked, 2: owner parked, 3: released.
static FORK: AtomicU8 = AtomicU8::new(0);

/// One fork at a time (two threads forking at once would release each other's parked owner).
static FORKING: AtomicBool = AtomicBool::new(false);

pub fn fork_prepare() {
    if is_owner() || CHILD.load(Ordering::Relaxed) || !STARTED.is_completed() {
        return;
    }
    while FORKING.swap(true, Ordering::SeqCst) {
        std::thread::yield_now();
    }
    FORK.store(1, Ordering::SeqCst);
    wake();
    while FORK.load(Ordering::SeqCst) != 2 {
        std::thread::yield_now();
    }
}

pub fn fork_parent() {
    if FORK.load(Ordering::SeqCst) != 2 || CHILD.load(Ordering::Relaxed) {
        return;
    }
    FORK.store(3, Ordering::SeqCst);
    if let Some(t) = OWNER_THREAD.get() {
        t.unpark();
    }
    FORKING.store(false, Ordering::SeqCst);
}

pub fn fork_child() {
    CHILD.store(true, Ordering::SeqCst);
    FORKING.store(false, Ordering::SeqCst);
    IS_OWNER.with(|o| o.set(true));
}
