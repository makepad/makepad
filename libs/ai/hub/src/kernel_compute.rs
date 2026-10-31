//! The node side of the `kernel` domain: Splash compute kernels from fleet
//! clients, run on this node's CPUs (wire: [`crate::kernel_wire`]).
//!
//! Safety comes from the language, not from trust in the sender: a kernel
//! is Splash compute (`platform/script/compute`), not Rust. It is
//! statically typed, its loops are bounded, its ops are total and exactly
//! specified, and every buffer access is clamped to the buffer the node
//! bound. It reaches nothing but its own buffers. On top of that:
//! - admission (`makepad_script_compute_sched::admission`, origin `Lan`)
//!   refuses a kernel whose worst case is absurd before anything runs, and
//!   gives every call a counted-ops budget it stops at;
//! - sizes are capped (source, tiles, elements, inputs, outputs), and each
//!   client may hold only a few open jobs;
//! - the workers run at the OS background priority (Linux SCHED_IDLE,
//!   macOS utility QoS, Windows idle), one per logical core, so a
//!   node's UI, window manager and GPU jobs are never starved;
//! - submits are parsed and compiled off the routing thread, two compiles
//!   at a time, kernels cached by the source's sha-256 (LRU);
//! - jobs belong to the credential that submitted them, and a job nobody
//!   polls is cancelled.
//!
//! No JIT is involved: kernels are lowered to AIR and run by the compute
//! crate's own native x64/arm64 backends (the interpreter where executable
//! memory is unavailable). Every backend is bit-identical to the
//! interpreter, so tiles do not depend on which node rendered them.

use crate::kernel_wire::*;
use makepad_micro_serde::{DeJson, SerJson};
use makepad_script_compute::kernel::{Access, KernelRef};
use makepad_script_compute::Backend;
use makepad_script_compute_sched::admission::{DeviceLimits, JobBudget, Ledger, Origin};
use makepad_script_compute_sched::kernel::{CancelToken, KernelError, KernelExt};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Open jobs one client may hold.
const MAX_JOBS_PER_CLIENT: usize = 16;
/// Open jobs on the node.
const MAX_JOBS: usize = 256;
/// Compiled kernels kept.
const CACHE_KERNELS: usize = 64;
/// Submits being parsed or compiled at once (each on a short-lived
/// thread); more are refused (503) until one ends.
const MAX_SUBMITS: usize = 32;
/// Of those, how many compile at once (the rest wait their turn).
const MAX_COMPILES: usize = 2;

/// Sends a finished response body (the routing layer turns it into HTTP).
pub type Reply = Box<dyn FnOnce(Result<Vec<u8>, (u16, String)>) + Send>;

pub struct KernelFacility {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    wake: Condvar,
    cache: Mutex<Vec<(String, KernelRef)>>,
    /// (submits in progress, compiles running).
    submits: Mutex<(usize, usize)>,
    compile_done: Condvar,
    /// Open streams: socket -> (owner, its sender).
    streams: Mutex<HashMap<u64, (String, Push)>>,
    ledger: Ledger,
    workers: usize,
    backend: Backend,
    tiles_done: AtomicU64,
    elements_done: AtomicU64,
    busy_nanos: AtomicU64,
}

#[derive(Default)]
struct State {
    queue: VecDeque<(u64, usize)>,
    jobs: HashMap<u64, Job>,
    next_id: u64,
    started: bool,
}

struct Job {
    owner: String,
    kernel: KernelRef,
    shared: Arc<Shared>,
    tiles: Vec<Arc<TileSpec>>,
    encoding: u32,
    /// Encoded tile records waiting for a poll.
    finished: Vec<Vec<u8>>,
    /// Tiles not yet finished (queued or running).
    remaining: usize,
    running: HashMap<usize, CancelToken>,
    cancelled: bool,
    waiter: Option<(Reply, Instant)>,
    last_seen: Instant,
    /// Submitted over a stream ([`KernelFacility::stream_message`]): tiles
    /// are pushed as they finish instead of waiting for a poll.
    push: Option<Push>,
}

/// A client's stream (`GET /kernel/stream`, a websocket): its frames go out
/// through `tx`, never blocking the sender.
#[derive(Clone)]
pub struct Push {
    pub socket: u64,
    pub tx: std::sync::mpsc::Sender<Vec<u8>>,
}

impl Push {
    fn send_batch(&self, job_id: u64, batch: Vec<u8>) {
        let mut frame = Vec::with_capacity(9 + batch.len());
        frame.push(STREAM_BATCH);
        frame.extend_from_slice(&job_id.to_le_bytes());
        frame.extend_from_slice(&batch);
        let _ = self.tx.send(frame);
    }
}

/// What every call of a job shares.
struct Shared {
    params: Vec<(String, f32)>,
    inputs: Vec<(String, Vec<u32>)>,
    /// (name, words per element) of every buffer the kernel writes.
    outputs: Vec<(String, u32)>,
    seed: u32,
    /// Calls per tile.
    passes: usize,
    /// (output, input): what one pass hands the next.
    feedback: Vec<(String, String)>,
}

struct TileSpec {
    id: u64,
    count: usize,
    params: Vec<(String, f32)>,
    inputs: Vec<(String, Vec<u32>)>,
}

/// What a node admits: every call may run [`TILE_WORK`] counted ops (a
/// 256x256 tile at 16k iterations per texel fits), and every worker may hold
/// two such calls (one running, one about to).
fn node_limits(workers: usize) -> DeviceLimits {
    let in_flight = TILE_WORK.saturating_mul(2 * workers.max(1) as u64);
    DeviceLimits {
        max_in_flight: in_flight,
        max_untrusted_in_flight: in_flight,
        max_jobs: u32::MAX,
        max_untrusted_jobs: u32::MAX,
        untrusted_job_work: TILE_WORK,
        ..DeviceLimits::default()
    }
}

/// Counted ops one tile may run.
pub const TILE_WORK: u64 = 1 << 37;

/// The node's one facility (its workers start with the first job).
pub fn facility() -> &'static KernelFacility {
    static F: OnceLock<KernelFacility> = OnceLock::new();
    F.get_or_init(KernelFacility::new)
}

impl KernelFacility {
    fn new() -> Self {
        let workers = std::env::var("MAKEPAD_AI_KERNEL_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
        let backend = match std::env::var("MAKEPAD_AI_KERNEL_BACKEND").as_deref() {
            Ok("interp") => Backend::Interp,
            _ => Backend::Native,
        };
        let limits = node_limits(workers);
        KernelFacility {
            inner: Arc::new(Inner {
                state: Mutex::new(State { next_id: 1, ..State::default() }),
                wake: Condvar::new(),
                cache: Mutex::new(Vec::new()),
                submits: Mutex::new((0, 0)),
                compile_done: Condvar::new(),
                streams: Mutex::new(HashMap::new()),
                ledger: Ledger::new(limits),
                workers,
                backend,
                tiles_done: AtomicU64::new(0),
                elements_done: AtomicU64::new(0),
                busy_nanos: AtomicU64::new(0),
            }),
        }
    }

    pub fn info(&self) -> KernelInfoJson {
        let i = &self.inner;
        let s = i.state.lock().unwrap();
        KernelInfoJson {
            api: KERNEL_API_VERSION,
            cores: i.workers as u32,
            arch: std::env::consts::ARCH.to_string(),
            backend: if i.backend == Backend::Native { "native" } else { "interp" }.to_string(),
            kernels_cached: i.cache.lock().unwrap().len() as u32,
            jobs: s.jobs.len() as u32,
            tiles_queued: s.queue.len() as u32,
            tiles_done: i.tiles_done.load(Ordering::Relaxed),
            elements_done: i.elements_done.load(Ordering::Relaxed),
            busy_ms: i.busy_nanos.load(Ordering::Relaxed) / 1_000_000,
        }
    }

    /// `POST /kernel/jobs`: parses, compiles (cached) and queues off the
    /// caller's thread; `reply` gets a [`KernelSubmittedJson`].
    pub fn submit(&self, owner: &str, body: Vec<u8>, reply: Reply) {
        self.submit_with(owner, body, None, reply)
    }

    fn submit_with(&self, owner: &str, body: Vec<u8>, push: Option<Push>, reply: Reply) {
        {
            let mut c = self.inner.submits.lock().unwrap();
            if c.0 >= MAX_SUBMITS {
                drop(c);
                reply(Err((503, "too many kernel submits in progress; retry".into())));
                return;
            }
            c.0 += 1;
        }
        let inner = self.inner.clone();
        let owner = owner.to_string();
        let spawned = std::thread::Builder::new().name("kernel-submit".into()).spawn(move || {
            let result = submit_job(&inner, &owner, &body, push);
            inner.submits.lock().unwrap().0 -= 1;
            reply(result.map(|s| s.serialize_json().into_bytes()));
        });
        if spawned.is_err() {
            self.inner.submits.lock().unwrap().0 -= 1;
        }
    }

    /// `GET /kernel/job/<id>/tiles`: answers now when tiles are waiting (or
    /// the job is over), else when the next one finishes or `timeout` ends.
    pub fn poll(&self, owner: &str, job_id: u64, timeout: Duration, reply: Reply) {
        let mut s = self.inner.state.lock().unwrap();
        let Some(job) = s.jobs.get_mut(&job_id).filter(|j| j.owner == owner) else {
            drop(s);
            reply(Err((404, "no such kernel job".into())));
            return;
        };
        job.last_seen = Instant::now();
        if let Some((old, _)) = job.waiter.take() {
            // one poll at a time: the older one answers empty
            old(Ok(batch_bytes(false, &[])));
        }
        if !job.finished.is_empty() || job.remaining == 0 || timeout.is_zero() {
            let (bytes, gone) = take_batch(job);
            if gone {
                s.jobs.remove(&job_id);
            }
            drop(s);
            reply(Ok(bytes));
            return;
        }
        job.waiter = Some((reply, Instant::now() + timeout.min(Duration::from_millis(MAX_POLL_MS))));
    }

    /// `POST /kernel/job/<id>/cancel`.
    pub fn cancel(&self, owner: &str, job_id: u64) -> bool {
        let mut s = self.inner.state.lock().unwrap();
        if !s.jobs.get(&job_id).is_some_and(|j| j.owner == owner) {
            return false;
        }
        let reply = cancel_locked(&mut s, job_id);
        drop(s);
        if let Some((w, bytes)) = reply {
            w(Ok(bytes));
        }
        true
    }

    /// One frame from a client's stream (see `kernel_wire`'s stream frames):
    /// a submit (answered with [`STREAM_SUBMITTED`]; its tiles are then
    /// pushed as [`STREAM_BATCH`] frames) or a cancel.
    /// A stream opened by `owner`; its frames go out through `tx`.
    pub fn stream_open(&self, socket: u64, owner: &str, tx: std::sync::mpsc::Sender<Vec<u8>>) {
        self.inner.streams.lock().unwrap().insert(socket, (owner.to_string(), Push { socket, tx }));
    }

    /// Whether `socket` is one of this facility's streams.
    pub fn is_stream(&self, socket: u64) -> bool {
        self.inner.streams.lock().unwrap().contains_key(&socket)
    }

    /// A frame from a stream.
    pub fn stream_frame(&self, socket: u64, data: &[u8]) {
        let Some((owner, push)) = self.inner.streams.lock().unwrap().get(&socket).cloned() else { return };
        self.stream_message(&owner, push, data);
    }

    fn stream_message(&self, owner: &str, push: Push, data: &[u8]) {
        if data.len() < 9 {
            return;
        }
        let request = u64::from_le_bytes(data[1..9].try_into().unwrap());
        match data[0] {
            STREAM_SUBMIT => {
                let tx = push.tx.clone();
                self.submit_with(
                    owner,
                    data[9..].to_vec(),
                    Some(push),
                    Box::new(move |r| {
                        let (status, body) = match r {
                            Ok(body) => (200u16, body),
                            Err((status, message)) => (status, message.into_bytes()),
                        };
                        let mut frame = Vec::with_capacity(11 + body.len());
                        frame.push(STREAM_SUBMITTED);
                        frame.extend_from_slice(&request.to_le_bytes());
                        frame.extend_from_slice(&status.to_le_bytes());
                        frame.extend_from_slice(&body);
                        let _ = tx.send(frame);
                    }),
                );
            }
            STREAM_CANCEL => {
                self.cancel(owner, request);
            }
            _ => {}
        }
    }

    /// A stream closed: its jobs are cancelled and dropped.
    pub fn stream_closed(&self, socket: u64) {
        if self.inner.streams.lock().unwrap().remove(&socket).is_none() {
            return;
        }
        let mut s = self.inner.state.lock().unwrap();
        let ids: Vec<u64> = s.jobs.iter().filter(|(_, j)| j.push.as_ref().is_some_and(|p| p.socket == socket)).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(job) = s.jobs.get_mut(&id) {
                job.push = None;
            }
            cancel_locked(&mut s, id);
            s.jobs.remove(&id);
        }
    }
}

/// Cancels a job: its queued tiles are dropped, its running ones stopped at
/// their next element (they come back as cancelled). A waiting poll is
/// answered (returned, to be sent outside the lock).
fn cancel_locked(s: &mut State, job_id: u64) -> Option<(Reply, Vec<u8>)> {
    let before = s.queue.len();
    s.queue.retain(|(j, _)| *j != job_id);
    let dropped = before - s.queue.len();
    let job = s.jobs.get_mut(&job_id)?;
    job.cancelled = true;
    job.remaining = job.remaining.saturating_sub(dropped);
    for t in job.running.values() {
        t.cancel();
    }
    if let Some(push) = job.push.clone() {
        let (bytes, over) = take_batch(job);
        push.send_batch(job_id, bytes);
        if over {
            s.jobs.remove(&job_id);
        }
        return None;
    }
    let (w, _) = job.waiter.take()?;
    let (bytes, over) = take_batch(job);
    if over {
        s.jobs.remove(&job_id);
    }
    Some((w, bytes))
}

/// The waiting records as a batch; true when the job is over (all sent).
fn take_batch(job: &mut Job) -> (Vec<u8>, bool) {
    let done = job.remaining == 0;
    let tiles = std::mem::take(&mut job.finished);
    (batch_bytes(done, &tiles), done)
}

fn parse_params(list: &[KernelParamJson], kernel: &KernelRef) -> Result<Vec<(String, f32)>, String> {
    list.iter()
        .map(|p| {
            if kernel.param_index(&p.name).is_none() {
                return Err(format!("the kernel has no param `{}`", p.name));
            }
            Ok((p.name.clone(), p.value))
        })
        .collect()
}

fn parse_inputs(list: &[KernelBufferJson], kernel: &KernelRef) -> Result<Vec<(String, Vec<u32>)>, String> {
    let mut out = Vec::with_capacity(list.len());
    for b in list {
        match kernel.buffer_index(&b.name).filter(|k| *k > 0).map(|k| kernel.buffer_view(k).access) {
            Some(Access::Read) => {}
            Some(_) => return Err(format!("buffer `{}` is written by the kernel; it is an output, not an input", b.name)),
            None => return Err(format!("the kernel has no buffer `{}`", b.name)),
        }
        let words = b.decode(MAX_INPUT_WORDS)?;
        if words.is_empty() {
            return Err(format!("input `{}` is empty", b.name));
        }
        out.push((b.name.clone(), words));
    }
    Ok(out)
}

fn compile_cached(inner: &Inner, source: &str) -> Result<(String, KernelRef, bool), String> {
    let key = crate::sha256::sha256_hex(source.as_bytes());
    {
        let mut cache = inner.cache.lock().unwrap();
        if let Some(at) = cache.iter().position(|(k, _)| *k == key) {
            let entry = cache.remove(at);
            let kernel = entry.1.clone();
            cache.push(entry);
            return Ok((key, kernel, false));
        }
    }
    {
        let mut c = inner.submits.lock().unwrap();
        while c.1 >= MAX_COMPILES {
            c = inner.compile_done.wait(c).unwrap();
        }
        c.1 += 1;
    }
    let kernel = compile(source, inner.backend);
    inner.submits.lock().unwrap().1 -= 1;
    inner.compile_done.notify_one();
    let kernel = kernel?;
    let mut cache = inner.cache.lock().unwrap();
    if !cache.iter().any(|(k, _)| *k == key) {
        if cache.len() >= CACHE_KERNELS {
            cache.remove(0);
        }
        cache.push((key.clone(), kernel.clone()));
    }
    Ok((key, kernel, true))
}

/// A job checked against its kernel: what the node queues, and what a
/// client runs on its own CPUs with [`PreparedJob::run_tile`] (the same
/// kernel gives the same bits there).
pub struct PreparedJob {
    kernel: KernelRef,
    shared: Arc<Shared>,
    tiles: Vec<Arc<TileSpec>>,
    encoding: u32,
}

impl PreparedJob {
    /// Checks `req` against `kernel` (sizes, names, bindings, admission's
    /// worst case for untrusted code).
    pub fn new(kernel: KernelRef, req: &KernelJobJson) -> Result<PreparedJob, String> {
        if req.tiles.is_empty() || req.tiles.len() > MAX_TILES {
            return Err(format!("{} tiles; a job has 1..={MAX_TILES}", req.tiles.len()));
        }
        let encoding = encoding_of(&req.encoding).ok_or_else(|| format!("unknown encoding `{}`", req.encoding))?;
        let mut outputs = Vec::new();
        for k in 1..kernel.buffer_count() {
            let b = kernel.buffer_view(k);
            match b.access {
                Access::Read => {}
                Access::Write => outputs.push((b.name.to_string(), b.stride.max(1))),
                Access::Emit { .. } | Access::EmitCount => {
                    return Err(format!("buffer `{}`: emit kernels are not served", b.name));
                }
            }
        }
        let passes = req.passes.unwrap_or(1) as usize;
        if passes == 0 || passes > MAX_PASSES {
            return Err(format!("{passes} passes; a tile runs 1..={MAX_PASSES}"));
        }
        let mut feedback = Vec::new();
        for f in req.feedback.iter().flatten() {
            let out = kernel.buffer_index(&f.output).filter(|k| *k > 0).map(|k| kernel.buffer_view(k));
            let input = kernel.buffer_index(&f.input).filter(|k| *k > 0).map(|k| kernel.buffer_view(k));
            match (out, input) {
                (Some(o), Some(i)) if o.access == Access::Write && i.access == Access::Read && o.stride == i.stride => {}
                _ => return Err(format!("feedback `{}` -> `{}`: needs an output and an input of the same stride", f.output, f.input)),
            }
            if req.inputs.iter().chain(req.tiles.iter().flat_map(|t| t.inputs.iter())).any(|b| b.name == f.input) {
                return Err(format!("input `{}` is fed back; it is not also sent", f.input));
            }
            feedback.push((f.output.clone(), f.input.clone()));
        }
        let shared = Shared { params: parse_params(&req.params, &kernel)?, inputs: parse_inputs(&req.inputs, &kernel)?, outputs, seed: req.seed, passes, feedback };
        let per_element: usize = shared.outputs.iter().map(|(_, w)| *w as usize).sum();
        let mut total_out = 0usize;
        let mut tiles = Vec::with_capacity(req.tiles.len());
        for t in &req.tiles {
            let count = t.count as usize;
            if count == 0 || count > MAX_TILE_ELEMENTS {
                return Err(format!("tile {}: {count} elements; a tile has 1..={MAX_TILE_ELEMENTS}", t.id));
            }
            let words = per_element.saturating_mul(count);
            if words > MAX_TILE_OUTPUT_WORDS {
                return Err(format!("tile {}: {words} output words; the limit is {MAX_TILE_OUTPUT_WORDS}", t.id));
            }
            total_out = total_out.saturating_add(words);
            let spec = TileSpec { id: t.id, count, params: parse_params(&t.params, &kernel)?, inputs: parse_inputs(&t.inputs, &kernel)? };
            for k in 1..kernel.buffer_count() {
                let b = kernel.buffer_view(k);
                if b.access == Access::Read && !spec.inputs.iter().chain(shared.inputs.iter()).any(|(n, _)| n == b.name) && !shared.feedback.iter().any(|(_, i)| i == b.name) {
                    return Err(format!("tile {}: input `{}` is not bound", t.id, b.name));
                }
            }
            tiles.push(Arc::new(spec));
        }
        if total_out > MAX_JOB_OUTPUT_WORDS {
            return Err(format!("the job produces {total_out} words; the limit is {MAX_JOB_OUTPUT_WORDS}"));
        }
        // The worst case of the heaviest tile, refused up front (each tile
        // is admitted again when it runs, for its budget).
        let heaviest = tiles.iter().map(|t| t.count).max().unwrap_or(1);
        let probe = Ledger::new(node_limits(1));
        probe.admit(&kernel, heaviest, Origin::Lan, &JobBudget::UNLIMITED).map_err(|r| format!("refused: {r}"))?;
        Ok(PreparedJob { kernel, shared: Arc::new(shared), tiles, encoding })
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Runs tile `index` on this thread.
    pub fn run_tile(&self, index: usize, backend: Backend) -> TileResult {
        let ledger = Ledger::new(node_limits(1));
        let t0 = Instant::now();
        let r = run_tile(&ledger, backend, &self.kernel, &self.shared, &self.tiles[index], &|_| true);
        tile_result(self.tiles[index].id, r, t0.elapsed().as_nanos() as u64)
    }
}

/// Compiles a kernel the way a node does.
pub fn compile(source: &str, backend: Backend) -> Result<KernelRef, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!("kernel source of {} bytes; the limit is {MAX_SOURCE_BYTES}", source.len()));
    }
    makepad_script_compute::kernel::compile_with(source, &[], backend).map_err(|errors| {
        let text: Vec<String> = errors
            .iter()
            .take(8)
            .map(|e| {
                let (line, col) = e.line_col(source);
                format!("{line}:{col}: {}", e.message)
            })
            .collect();
        format!("the kernel does not compile:\n{}", text.join("\n"))
    })
}

fn tile_result(id: u64, r: Result<TileOk, (u32, String)>, nanos: u64) -> TileResult {
    match r {
        Ok((work, elements, reduced, outputs)) => TileResult { id, status: STATUS_OK, elements: elements as u32, work, nanos, reduced, outputs, message: String::new() },
        Err((status, message)) => TileResult { id, status, elements: 0, work: 0, nanos, reduced: Vec::new(), outputs: Vec::new(), message },
    }
}

fn submit_job(inner: &Arc<Inner>, owner: &str, body: &[u8], push: Option<Push>) -> Result<KernelSubmittedJson, (u16, String)> {
    let bad = |m: String| (400u16, m);
    let text = std::str::from_utf8(body).map_err(|_| bad("the job is not utf-8 JSON".into()))?;
    let req = KernelJobJson::deserialize_json(text).map_err(|e| bad(format!("bad kernel job: {e:?}")))?;
    {
        let s = inner.state.lock().unwrap();
        if s.jobs.len() >= MAX_JOBS || s.jobs.values().filter(|j| j.owner == owner).count() >= MAX_JOBS_PER_CLIENT {
            return Err((503, "too many open kernel jobs; poll or cancel the open ones".into()));
        }
    }
    let (key, kernel, compiled) = compile_cached(inner, &req.source).map_err(bad)?;
    let PreparedJob { kernel, shared, tiles, encoding } = PreparedJob::new(kernel, &req).map_err(bad)?;
    let n = tiles.len();
    let mut s = inner.state.lock().unwrap();
    let job_id = s.next_id;
    s.next_id += 1;
    for k in 0..n {
        s.queue.push_back((job_id, k));
    }
    s.jobs.insert(
        job_id,
        Job { owner: owner.to_string(), kernel, shared, tiles, encoding, finished: Vec::new(), remaining: n, running: HashMap::new(), cancelled: false, waiter: None, last_seen: Instant::now(), push },
    );
    if !s.started {
        s.started = true;
        start_workers(inner);
    }
    drop(s);
    inner.wake.notify_all();
    Ok(KernelSubmittedJson { job_id, tiles: n as u32, kernel: key, compiled })
}

fn start_workers(inner: &Arc<Inner>) {
    for k in 0..inner.workers {
        let inner = inner.clone();
        let _ = std::thread::Builder::new().name(format!("kernel-worker-{k}")).spawn(move || {
            background_priority();
            worker(&inner);
        });
    }
    let inner = inner.clone();
    let _ = std::thread::Builder::new().name("kernel-sweeper".into()).spawn(move || sweeper(&inner));
}

/// Answers polls whose wait ran out and drops jobs nobody polls.
fn sweeper(inner: &Inner) {
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let now = Instant::now();
        let mut replies = Vec::new();
        let mut s = inner.state.lock().unwrap();
        let mut gone = Vec::new();
        let mut idle = Vec::new();
        for (id, job) in s.jobs.iter_mut() {
            if job.waiter.as_ref().is_some_and(|(_, deadline)| *deadline <= now) {
                let (w, _) = job.waiter.take().unwrap();
                job.last_seen = now;
                let (bytes, over) = take_batch(job);
                if over {
                    gone.push(*id);
                }
                replies.push((w, bytes));
            } else if job.waiter.is_none() && job.push.is_none() && now.duration_since(job.last_seen) > Duration::from_secs(JOB_IDLE_SECS) {
                idle.push(*id);
            }
        }
        for id in &gone {
            s.jobs.remove(id);
        }
        for id in idle {
            cancel_locked(&mut s, id);
            if s.jobs.get(&id).is_some_and(|j| j.remaining == 0) {
                s.jobs.remove(&id);
            }
        }
        drop(s);
        for (w, bytes) in replies {
            w(Ok(bytes));
        }
    }
}

fn worker(inner: &Inner) {
    loop {
        let (job_id, index, kernel, shared, spec, encoding) = {
            let mut s = inner.state.lock().unwrap();
            loop {
                match s.queue.pop_front() {
                    Some((job_id, index)) => {
                        let Some(job) = s.jobs.get(&job_id) else { continue };
                        break (job_id, index, job.kernel.clone(), job.shared.clone(), job.tiles[index].clone(), job.encoding);
                    }
                    None => s = inner.wake.wait(s).unwrap(),
                }
            }
        };
        let t0 = Instant::now();
        let publish = |token: CancelToken| {
            let mut s = inner.state.lock().unwrap();
            match s.jobs.get_mut(&job_id) {
                Some(job) if !job.cancelled => {
                    job.running.insert(index, token);
                    true
                }
                _ => false,
            }
        };
        let result = run_tile(&inner.ledger, inner.backend, &kernel, &shared, &spec, &publish);
        let nanos = t0.elapsed().as_nanos() as u64;
        inner.busy_nanos.fetch_add(nanos, Ordering::Relaxed);
        if let Ok((_, elements, _, _)) = &result {
            inner.tiles_done.fetch_add(1, Ordering::Relaxed);
            inner.elements_done.fetch_add(*elements as u64, Ordering::Relaxed);
        }
        let tile = tile_result(spec.id, result, nanos);
        let mut record = Vec::new();
        encode_tile(&tile, encoding, &mut record);
        let mut s = inner.state.lock().unwrap();
        let mut reply = None;
        if let Some(job) = s.jobs.get_mut(&job_id) {
            job.running.remove(&index);
            job.remaining = job.remaining.saturating_sub(1);
            job.finished.push(record);
            if let Some(push) = job.push.clone() {
                let (bytes, over) = take_batch(job);
                push.send_batch(job_id, bytes);
                if over {
                    s.jobs.remove(&job_id);
                }
            } else if let Some((w, _)) = job.waiter.take() {
                let (bytes, over) = take_batch(job);
                reply = Some((w, bytes, over));
            }
        }
        if let Some((_, _, true)) = reply {
            s.jobs.remove(&job_id);
        }
        drop(s);
        if let Some((w, bytes, _)) = reply {
            w(Ok(bytes));
        }
    }
}

type TileOk = (u64, usize, Vec<f32>, Vec<TileOutput>);

/// One tile: `shared.passes` calls of the kernel, each bounded by
/// admission, the feedback outputs of one call bound as inputs of the next
/// (zeros for the first), `pass` set to the call's index. `publish` gets each
/// call's cancel token before it runs and says whether to run at all (false:
/// the job was cancelled meanwhile).
fn run_tile(ledger: &Ledger, backend: Backend, kernel: &KernelRef, shared: &Shared, spec: &TileSpec, publish: &dyn Fn(CancelToken) -> bool) -> Result<TileOk, (u32, String)> {
    let failed = |e: KernelError| (STATUS_FAILED, e.to_string());
    let words_of = |name: &str| shared.outputs.iter().find(|(n, _)| n == name).map_or(0, |(_, w)| *w as usize * spec.count);
    let mut fed: Vec<(String, Vec<u32>)> = shared.feedback.iter().map(|(out, input)| (input.clone(), vec![0u32; words_of(out)])).collect();
    let has_pass = kernel.param_index("pass").is_some();
    let mut work = 0u64;
    for pass in 0..shared.passes {
        let ticket = ledger.admit(kernel, spec.count, Origin::Lan, &JobBudget::UNLIMITED).map_err(|r| (STATUS_FAILED, format!("refused: {r}")))?;
        let mut outputs: Vec<(String, Vec<u32>)> = shared.outputs.iter().map(|(n, w)| (n.clone(), vec![0u32; *w as usize * spec.count])).collect();
        let mut call = kernel.call();
        for (name, value) in shared.params.iter().chain(spec.params.iter()) {
            call.set_param(name, *value);
        }
        if has_pass {
            call.set_param("pass", pass as f32);
        }
        call.set_seed(shared.seed);
        call.set_work_limit(ticket.work_limit().min(TILE_WORK.saturating_sub(work)).max(1));
        call.set_host_call_limit(ticket.host_call_limit());
        let bound = |name: &str, list: &[(String, Vec<u32>)]| list.iter().any(|(n, _)| n == name);
        for (name, words) in &shared.inputs {
            if !bound(name, &spec.inputs) && !bound(name, &fed) {
                call.input_u32(name, words).map_err(failed)?;
            }
        }
        for (name, words) in &spec.inputs {
            if !bound(name, &fed) {
                call.input_u32(name, words).map_err(failed)?;
            }
        }
        for (name, words) in &fed {
            call.input_u32(name, words).map_err(failed)?;
        }
        for (name, words) in outputs.iter_mut() {
            call.output_u32(name, words).map_err(failed)?;
        }
        // a cancel that came before the token was published is seen here
        if !publish(call.cancel_token()) {
            return Err((STATUS_CANCELLED, "cancelled".into()));
        }
        let stats = match backend {
            Backend::Interp => call.run_interp(spec.count),
            Backend::Native => call.run(spec.count),
        };
        drop(call);
        let stats = match stats {
            Ok(stats) => stats,
            Err(KernelError::Cancelled) => return Err((STATUS_CANCELLED, "cancelled".into())),
            Err(e @ KernelError::OverBudget { .. }) => return Err((STATUS_OVER_BUDGET, e.to_string())),
            Err(e) => return Err(failed(e)),
        };
        work = work.saturating_add(stats.work);
        if pass + 1 < shared.passes {
            for (out, input) in &shared.feedback {
                let at = outputs.iter().position(|(n, _)| n == out).expect("feedback outputs are checked");
                let words = std::mem::take(&mut outputs[at].1);
                if let Some(slot) = fed.iter_mut().find(|(n, _)| n == input) {
                    slot.1 = words;
                }
            }
            continue;
        }
        let results = outputs
            .into_iter()
            .filter(|(name, _)| !shared.feedback.iter().any(|(out, _)| out == name))
            .map(|(name, words)| TileOutput { name, words })
            .collect();
        return Ok((work, stats.elements, stats.reduced, results));
    }
    Err((STATUS_FAILED, "a tile runs at least one pass".into()))
}

/// The OS background class for the calling thread.
fn background_priority() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        #[repr(C)]
        struct SchedParam {
            priority: i32,
        }
        extern "C" {
            fn sched_setscheduler(pid: i32, policy: i32, param: *const SchedParam) -> i32;
        }
        const SCHED_IDLE: i32 = 5;
        // pid 0: the calling thread
        unsafe {
            sched_setscheduler(0, SCHED_IDLE, &SchedParam { priority: 0 });
        }
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_UTILITY: u32 = 0x11;
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
    }
    #[cfg(windows)]
    {
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        const THREAD_PRIORITY_IDLE: i32 = -15;
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_IDLE);
        }
    }
}
