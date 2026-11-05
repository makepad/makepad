//! The GPU device: compute programs' GPU ops ([`GpuOp`]) run on a Cx, the app's own or a
//! thread's ([`spawn`]), in unattached compute passes on the plain HAL path.
//!
//! - **Order.** A session's ops run in the order it sent them. Its dispatches go into passes,
//!   which the HAL runs in submission order with a barrier between dispatches. An upload goes
//!   in before the passes not submitted yet; one into a buffer that a pass still in flight binds
//!   waits for the session's passes to finish, so it never lands under work sent before it.
//! - **Passes.** Each pass holds as much of one session's work as fits a cap of estimated GPU
//!   time: 32 ms with two passes in flight while the GPU is free, 4 ms with one while someone
//!   interacts with what this GPU draws: the app says so (`GpuLedger::pressure`), or a session
//!   someone waits on or watches is open (its app draws on this GPU too). Sessions take turns,
//!   a pass each.
//!   The next pass goes once one in flight finishes (its readback, which lands on the device's
//!   thread), never on frames or input.
//! - **Kernels** are built once a device: a session sending code the device has (another
//!   solve of the same program) gets the kernel it built before.
//! - **Estimates.** A dispatch's GPU time is its threads times its kernel's measured ns a
//!   thread at its size (its threads' power of two: a dispatch's fixed cost weighs on a small
//!   one's threads, so a multigrid's coarse levels never set the fine level's estimate): each
//!   kernel's first dispatches at a size, and one in 32 after, are timed, and the measurements
//!   carry over to every session running the same code. A size not measured yet counts 1 ms a
//!   dispatch.
//! - **Element kernels** run as range dispatches (`dispatch_range = (first, count)`), each of
//!   about 1 ms once the kernel is measured; elements never depend on the ranges, so the
//!   results are the same bits at any size.
//! - **Memory.** Every buffer is admitted on the GPU ledger (one document for all programs);
//!   one the ledger refuses fails the session's next answer.
//! - **Stops.** An interrupted or cancelled session's work sent before the stop is dropped as it
//!   comes up (passes in flight finish); its reads answer [`GpuError::Stopped`].

use crate::session::GpuSession;
use makepad_compute_program::gpu::{GpuError, GpuInfo, GpuLink, GpuMsg, GpuOp, GpuReads};
use makepad_platform::gpu_admission::{GpuDocument, GpuReservation};
use makepad_platform::makepad_script::*;
use makepad_platform::thread::SignalToUI;
use makepad_platform::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader

    mod.program_gpu = {}

    // Words first..first + count of `out` set to one word: range = (first, count, word, 0).
    mod.program_gpu.Fill = {
        workgroup_size: 64
        range: shader.uniform(vec4u)
        out: shader.buffer_rw(u32)
        main: fn() {
            let i = (group_id().x + group_id().y * group_count().x) * 64u + local_index()
            if i >= self.range.y { return }
            self.out[self.range.x + i] = self.range.z
        }
    }

    // Words first..first + count of `src` into `dst`: range = (first, count, 0, 0).
    mod.program_gpu.Copy = {
        workgroup_size: 64
        range: shader.uniform(vec4u)
        src: shader.buffer(u32)
        dst: shader.buffer_rw(u32)
        main: fn() {
            let i = (group_id().x + group_id().y * group_count().x) * 64u + local_index()
            if i >= self.range.y { return }
            self.dst[self.range.x + i] = self.src[self.range.x + i]
        }
    }
}

/// The most groups a dispatch takes on an axis.
const MAX_GROUPS: u32 = 65535;
/// Words a fill or copy dispatch covers at most.
const LIB_SLICE: u32 = 1 << 22;
/// Elements an element kernel's dispatch covers before the kernel is measured.
const FIRST_SLICE: u32 = 1 << 16;
/// GPU ms a dispatch aims at once its kernel is measured.
const SLICE_MS: f64 = 1.0;
/// A pass's estimated GPU ms and the passes in flight: while the GPU is free, and while the
/// person interacts.
const FREE: (f64, usize) = (32.0, 2);
const PRESSED: (f64, usize) = (4.0, 1);
/// What a dispatch of a kernel not measured yet counts.
const UNMEASURED_MS: f64 = 1.0;
/// Timed dispatches waiting for their measurement, at most.
const MAX_TIMED: usize = 256;
/// The name the device's passes and timed dispatches go by.
const PASS: &str = "compute programs";
const TIMER: &str = "compute program dispatch";
/// The library kernels' keys (a compiled kernel's key is a hash of its code).
const FILL_KEY: u64 = 1;
const COPY_KEY: u64 = 2;
/// Element kernels' threads a workgroup.
const ELEMENT_WORKGROUP: u32 = 64;

/// A session's reads for one message, filled as they come back (on the worker pool).
struct Collect {
    reads: Vec<Option<Vec<u8>>>,
    /// Reads issued and not back yet.
    left: usize,
    /// The message's end came: it answers once `left` is 0.
    ended: bool,
    error: Option<GpuError>,
    reply: Option<mpsc::Sender<GpuReads>>,
}

impl Collect {
    fn answer_if_done(&mut self) {
        if !self.ended || self.left > 0 {
            return;
        }
        if let Some(reply) = self.reply.take() {
            let answer = match self.error.take() {
                Some(e) => Err(e),
                None => Ok(self.reads.iter_mut().map(|r| r.take().unwrap_or_default()).collect()),
            };
            let _ = reply.send(answer);
        }
    }
}

enum Item {
    Op { epoch: u64, op: GpuOp },
    /// An element kernel's run, `done` of its elements in passes already.
    Run { epoch: u64, kernel: usize, count: u32, args: Vec<u32>, buffers: Vec<Option<usize>>, done: u32, args_buffer: Option<GpuBuffer> },
    Read { epoch: u64, buf: usize, offset: u64, bytes: u64, collect: Arc<Mutex<Collect>>, index: usize },
    /// A message's end.
    End { epoch: u64, collect: Arc<Mutex<Collect>> },
}

impl Item {
    fn epoch(&self) -> u64 {
        match self {
            Item::Op { epoch, .. } | Item::Run { epoch, .. } | Item::Read { epoch, .. } | Item::End { epoch, .. } => *epoch,
        }
    }
}

struct Kernel {
    kernel: ComputeKernel,
    key: u64,
    /// Threads a workgroup.
    group: u64,
}

struct Sess {
    id: u64,
    stop: Arc<AtomicU64>,
    watched: bool,
    buffers: HashMap<usize, (GpuBuffer, GpuReservation)>,
    kernels: HashMap<usize, Kernel>,
    queue: VecDeque<Item>,
    /// The first error of its work since its last answer.
    error: Option<GpuError>,
    in_flight: usize,
    /// Buffers its passes in flight bind.
    busy: HashSet<usize>,
    /// Args buffers its passes in flight read, and free ones by size.
    args_in_flight: Vec<GpuBuffer>,
    args_free: HashMap<u64, Vec<GpuBuffer>>,
    closed: bool,
}

impl Sess {
    fn stale(&self, epoch: u64) -> bool {
        epoch < self.stop.load(Ordering::Relaxed)
    }

    fn fail(&mut self, e: GpuError) {
        self.error.get_or_insert(e);
    }
}

/// A pass being built: its dispatches' estimated ms, the buffers they bind, the last bound one
/// (read back to see the pass finish).
struct Building {
    pass: ComputePass,
    ms: f64,
    bound: HashSet<usize>,
    last: Option<GpuBuffer>,
}

pub struct GpuDevice {
    rx: mpsc::Receiver<GpuMsg>,
    done_tx: mpsc::Sender<u64>,
    done_rx: mpsc::Receiver<u64>,
    wake: Arc<dyn Fn() + Send + Sync>,
    sessions: Vec<Sess>,
    turn: usize,
    in_flight: usize,
    fill: ComputeKernel,
    copy: ComputeKernel,
    doc: GpuDocument,
    /// Measured ns a thread, per kernel key and [`size`].
    speed: HashMap<(u64, u32), f64>,
    /// Every kernel installed, by key: a session sending one the device has gets it (its
    /// pipeline built once).
    compiled: HashMap<u64, ComputeKernel>,
    /// Dispatches timed per kernel key and [`size`].
    timed_count: HashMap<(u64, u32), u64>,
    /// Timed dispatches not measured yet: their key, size and threads, in submission order.
    timed: VecDeque<((u64, u32), u64)>,
    lost: bool,
    linked: bool,
}

/// A dispatch's size: its threads' power of two.
fn size(threads: u64) -> u32 {
    u64::BITS - threads.max(1).leading_zeros()
}

fn bytes_of(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Groups for `groups` workgroups on one axis: rows of [`MAX_GROUPS`] past it.
fn rows(groups: u32) -> [u32; 3] {
    match groups <= MAX_GROUPS {
        true => [groups.max(1), 1, 1],
        false => [MAX_GROUPS, groups.div_ceil(MAX_GROUPS), 1],
    }
}

impl GpuDevice {
    /// The device on `cx` (its GPU has compute), and the link sessions use it through.
    /// `handle` runs what they send: call it on the Cx's thread on every signal.
    pub fn new(cx: &mut Cx) -> Result<(GpuDevice, GpuLink), String> {
        let target = cx.compute_target()?;
        let (fill, copy) = cx.with_vm(|vm| {
            script_mod(vm);
            let module = vm.heap().module(id!(program_gpu));
            let fill = vm.value(module, id!(Fill)).as_object().ok_or("the program GPU library has no Fill")?;
            let copy = vm.value(module, id!(Copy)).as_object().ok_or("the program GPU library has no Copy")?;
            Ok::<_, String>((ComputeKernel::new(vm, fill, "program fill")?, ComputeKernel::new(vm, copy, "program copy")?))
        })?;
        let budget = cx.gpu_admission().budget();
        let doc = cx.gpu_admission().register(PASS);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| SignalToUI::set_ui_signal());
        let (link, rx) = GpuLink::new(GpuInfo { target, budget_bytes: budget.document_bytes.min(budget.device_bytes) }, wake.clone());
        let (done_tx, done_rx) = mpsc::channel();
        let device = GpuDevice {
            rx,
            done_tx,
            done_rx,
            wake,
            sessions: Vec::new(),
            turn: 0,
            in_flight: 0,
            fill,
            copy,
            doc,
            speed: HashMap::new(),
            compiled: HashMap::new(),
            timed_count: HashMap::new(),
            timed: VecDeque::new(),
            lost: false,
            linked: true,
        };
        Ok((device, link))
    }

    /// Sessions open on the device.
    pub fn sessions(&self) -> usize {
        self.sessions.len()
    }

    /// Takes what came (messages, finished passes, measurements) and submits the passes that fit.
    /// False once every link is gone and no session is left.
    pub fn handle(&mut self, cx: &mut Cx) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(msg) => self.take(msg),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.linked = false;
                    break;
                }
            }
        }
        while let Ok(session) = self.done_rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) {
                s.in_flight = s.in_flight.saturating_sub(1);
                if s.in_flight == 0 {
                    s.busy.clear();
                    for b in s.args_in_flight.drain(..) {
                        s.args_free.entry(b.size()).or_default().push(b);
                    }
                }
            }
        }
        self.measure();
        if cx.gpu_device_lost() && !self.lost {
            self.lose();
        }
        let pressed = cx.gpu_admission().pressure() || self.sessions.iter().any(|s| s.watched && !s.closed);
        let (cap, most) = if pressed { PRESSED } else { FREE };
        // sessions take turns, a pass each, while passes may go
        let mut idle = 0;
        while !self.sessions.is_empty() && idle < self.sessions.len() {
            let k = self.turn % self.sessions.len();
            self.turn = self.turn.wrapping_add(1);
            let room = self.in_flight < most;
            match self.advance(cx, k, cap, room) {
                true => idle = 0,
                false => idle += 1,
            }
        }
        self.sessions.retain(|s| !(s.closed && s.queue.is_empty() && s.in_flight == 0));
        self.linked || !self.sessions.is_empty()
    }

    fn take(&mut self, msg: GpuMsg) {
        match msg {
            GpuMsg::Open { session, label: _, stop, watched } => self.sessions.push(Sess {
                id: session,
                stop,
                watched,
                buffers: HashMap::new(),
                kernels: HashMap::new(),
                queue: VecDeque::new(),
                error: None,
                in_flight: 0,
                busy: HashSet::new(),
                args_in_flight: Vec::new(),
                args_free: HashMap::new(),
                closed: false,
            }),
            GpuMsg::Ops { session, epoch, ops, reply } => {
                let lost = self.lost;
                let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) else {
                    if let Some(reply) = reply {
                        let _ = reply.send(Err(GpuError::DeviceLost));
                    }
                    return;
                };
                let reads = ops.iter().filter(|op| matches!(op, GpuOp::Read { .. })).count();
                let collect = Arc::new(Mutex::new(Collect { reads: vec![None; reads], left: 0, ended: false, error: None, reply }));
                let mut index = 0;
                for op in ops {
                    s.queue.push_back(match op {
                        GpuOp::Read { buf, offset, bytes } => {
                            index += 1;
                            Item::Read { epoch, buf, offset, bytes, collect: collect.clone(), index: index - 1 }
                        }
                        GpuOp::Run { kernel, count, args, buffers } => Item::Run { epoch, kernel, count, args, buffers, done: 0, args_buffer: None },
                        op => Item::Op { epoch, op },
                    });
                }
                s.queue.push_back(Item::End { epoch, collect });
                if lost {
                    s.fail(GpuError::DeviceLost);
                }
            }
            GpuMsg::Close { session } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session) {
                    s.closed = true;
                }
            }
        }
    }

    /// The timed dispatches measured since the last look.
    fn measure(&mut self) {
        for ms in gpu_frame_timer::take_watched(TIMER) {
            let Some((at, threads)) = self.timed.pop_front() else { break };
            let ns = ms as f64 * 1e6 / threads.max(1) as f64;
            let s = self.speed.entry(at).or_insert(ns);
            *s = *s * 0.75 + ns * 0.25;
        }
    }

    /// The GPU was lost: every session's work ends with [`GpuError::DeviceLost`].
    fn lose(&mut self) {
        self.lost = true;
        for s in &mut self.sessions {
            s.fail(GpuError::DeviceLost);
            s.buffers.clear();
            s.kernels.clear();
            s.busy.clear();
            s.args_in_flight.clear();
            s.args_free.clear();
            s.in_flight = 0;
        }
        self.in_flight = 0;
    }

    fn estimate(&self, key: u64, threads: u64) -> f64 {
        match self.speed.get(&(key, size(threads))) {
            Some(ns) => ns * threads as f64 / 1e6,
            None => UNMEASURED_MS,
        }
    }

    /// One dispatch into the pass: timed while its kernel is new at its size, and one in 32
    /// after.
    #[allow(clippy::too_many_arguments)]
    fn dispatch(&mut self, b: &mut Building, kernel: &ComputeKernel, key: u64, groups: [u32; 3], threads: u64, params: &ComputeParams, buffers: &[GpuBuffer]) -> Result<(), String> {
        let at = (key, size(threads));
        let n = self.timed_count.entry(at).or_insert(0);
        *n += 1;
        if (*n <= 2 || *n % 32 == 0) && self.timed.len() < MAX_TIMED {
            b.pass.dispatch_timed(TIMER, kernel, groups, params, buffers, &[])?;
            self.timed.push_back((at, threads));
        } else {
            b.pass.dispatch(kernel, groups, params, buffers, &[])?;
        }
        b.ms += self.estimate(key, threads);
        b.last = buffers.last().cloned();
        Ok(())
    }

    /// A fill or copy of `words` words, sliced.
    fn library(&mut self, b: &mut Building, fill: Option<u32>, src: Option<&GpuBuffer>, dst: &GpuBuffer, words: u32) -> Result<(), String> {
        let (kernel, key) = match fill {
            Some(_) => (self.fill.clone(), FILL_KEY),
            None => (self.copy.clone(), COPY_KEY),
        };
        let mut first = 0;
        while first < words {
            let count = (words - first).min(LIB_SLICE);
            let mut params = kernel.params();
            params.set(id!(range), &bytes_of(&[first, count, fill.unwrap_or(0), 0]))?;
            // bound in the layout's order: `src` (a copy's), then `out` or `dst`
            let bound: Vec<GpuBuffer> = kernel.layout().buffers.iter().map(|l| if l.name == id!(src) { src.unwrap_or(dst).clone() } else { dst.clone() }).collect();
            self.dispatch(b, &kernel, key, rows(count.div_ceil(64)), count as u64, &params, &bound)?;
            first += count;
        }
        Ok(())
    }

    /// Builds and submits session `k`'s next pass (and runs what needs none: kernels, buffers,
    /// uploads, reads). True when it did anything.
    fn advance(&mut self, cx: &mut Cx, k: usize, cap: f64, room: bool) -> bool {
        let mut building: Option<Building> = None;
        let mut did = self.step(cx, k, cap, room, &mut building);
        if let Some(b) = building.take() {
            self.submit(cx, k, b);
            did = true;
        }
        did
    }

    /// Runs session `k`'s items until one stops it (it waits, or the pass is to go). True when
    /// it ran any.
    fn step(&mut self, cx: &mut Cx, k: usize, cap: f64, room: bool, building: &mut Option<Building>) -> bool {
        let mut did = false;
        loop {
            let s = &mut self.sessions[k];
            let Some(item) = s.queue.front() else { return did };
            let stale = s.stale(item.epoch());
            // what computes waits for room for a pass, and fills the one being built to its
            // cap (a new buffer's zeros too, stale or not)
            let computes = match item {
                Item::Op { op: GpuOp::Alloc { .. }, .. } => true,
                Item::Run { .. } | Item::Op { op: GpuOp::Fill { .. } | GpuOp::Copy { .. } | GpuOp::Dispatch { .. }, .. } => !stale,
                _ => false,
            };
            if computes {
                match building {
                    None if !room => return did,
                    Some(b) if b.ms >= cap => return did,
                    _ => {}
                }
            }
            let item = s.queue.pop_front().unwrap();
            match self.item(cx, k, item, stale, cap, building) {
                Ok(None) => did = true,
                Ok(Some(item)) => {
                    self.sessions[k].queue.push_front(item);
                    return did;
                }
                Err(e) => {
                    did = true;
                    self.sessions[k].fail(GpuError::Failed(e));
                }
            }
        }
    }

    /// One item. Some(item): it goes back to the queue's front, to wait (for the session's
    /// passes in flight), or to go after the pass being built (a read, an upload into what the
    /// pass binds, the rest of a run).
    fn item(&mut self, cx: &mut Cx, k: usize, item: Item, stale: bool, cap: f64, building: &mut Option<Building>) -> Result<Option<Item>, String> {
        let new_pass = |b: &mut Option<Building>| {
            b.get_or_insert_with(|| Building { pass: ComputePass::new(PASS, None), ms: 0.0, bound: HashSet::new(), last: None });
        };
        match item {
            Item::End { collect, epoch } => {
                let s = &mut self.sessions[k];
                let mut c = collect.lock().unwrap();
                if let Some(e) = s.error.take() {
                    c.error.get_or_insert(e);
                }
                if s.stale(epoch) {
                    c.error.get_or_insert(GpuError::Stopped);
                }
                c.ended = true;
                c.answer_if_done();
                Ok(None)
            }
            Item::Read { buf, offset, bytes, collect, index, epoch } => {
                let s = &self.sessions[k];
                if stale || s.error.is_some() {
                    collect.lock().unwrap().error.get_or_insert(if stale { GpuError::Stopped } else { s.error.clone().unwrap() });
                    return Ok(None);
                }
                // the read goes after the work before it: the pass being built first
                if building.is_some() {
                    return Ok(Some(Item::Read { buf, offset, bytes, collect, index, epoch }));
                }
                let buffer = s.buffers.get(&buf).ok_or("a read of a buffer the session does not hold")?.0.clone();
                collect.lock().unwrap().left += 1;
                let into = collect.clone();
                let read = cx.read(&[(&buffer, offset..offset + bytes)], move |_, r| {
                    let mut c = into.lock().unwrap();
                    match r {
                        Ok(mut ranges) => c.reads[index] = Some(ranges.remove(0).to_vec()),
                        Err(_) => {
                            c.error.get_or_insert(GpuError::DeviceLost);
                        }
                    }
                    c.left -= 1;
                    c.answer_if_done();
                });
                if read.is_err() {
                    let mut c = collect.lock().unwrap();
                    c.left -= 1;
                    c.error.get_or_insert(GpuError::DeviceLost);
                }
                Ok(None)
            }
            Item::Op { op, epoch } => self.op(cx, k, op, epoch, stale, building, new_pass),
            Item::Run { epoch, kernel, count, args, buffers, done, args_buffer } => {
                if stale {
                    return Ok(None);
                }
                let s = &mut self.sessions[k];
                let kn = s.kernels.get(&kernel).ok_or("a run of a kernel the session did not send")?;
                let (compute, key) = (kn.kernel.clone(), kn.key);
                let args_buffer = match args_buffer {
                    Some(a) => a,
                    None => {
                        let size = args.len() as u64 * 4;
                        let a = match s.args_free.get_mut(&size).and_then(|v| v.pop()) {
                            Some(a) => a,
                            None => GpuBuffer::new(cx, size, GpuBufferFlags::default())?,
                        };
                        a.update_owned(cx, 0, bytes_of(&args))?;
                        a
                    }
                };
                let mut bound = Vec::with_capacity(buffers.len());
                for b in &buffers {
                    bound.push(match b {
                        Some(b) => s.buffers.get(b).ok_or("a run binds a buffer the session does not hold")?.0.clone(),
                        None => args_buffer.clone(),
                    });
                }
                new_pass(building);
                let b = building.as_mut().unwrap();
                b.bound.extend(buffers.iter().flatten());
                // sized from the first slices' measurement
                let slice = match self.speed.get(&(key, size(FIRST_SLICE as u64))) {
                    Some(ns) if *ns > 0.0 => ((SLICE_MS * 1e6 / ns) as u32).clamp(ELEMENT_WORKGROUP, MAX_GROUPS * ELEMENT_WORKGROUP),
                    _ => FIRST_SLICE,
                } / ELEMENT_WORKGROUP * ELEMENT_WORKGROUP;
                let mut first = done;
                while first < count {
                    let n = (count - first).min(slice);
                    let mut params = compute.params();
                    params.set(id!(dispatch_range), &bytes_of(&[first, n, 0, 0]))?;
                    self.dispatch(b, &compute, key, [n.div_ceil(ELEMENT_WORKGROUP), 1, 1], n as u64, &params, &bound)?;
                    first += n;
                    if b.ms >= cap && first < count {
                        // the rest in the next pass, on the same args (they go back to the
                        // pool once the run's last pass finishes)
                        return Ok(Some(Item::Run { epoch, kernel, count, args, buffers, done: first, args_buffer: Some(args_buffer) }));
                    }
                }
                self.sessions[k].args_in_flight.push(args_buffer);
                Ok(None)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn op(&mut self, cx: &mut Cx, k: usize, op: GpuOp, epoch: u64, stale: bool, building: &mut Option<Building>, new_pass: impl Fn(&mut Option<Building>)) -> Result<Option<Item>, String> {
        match op {
            GpuOp::Kernel { kernel, key, compiled, label } => {
                let group = compiled.layout.workgroup.iter().map(|n| *n as u64).product();
                let compute = match self.compiled.get(&key) {
                    Some(compute) => compute.clone(),
                    None => {
                        let compute = ComputeKernel::from_compiled(cx, compiled, &label)?;
                        self.compiled.insert(key, compute.clone());
                        compute
                    }
                };
                self.sessions[k].kernels.insert(kernel, Kernel { kernel: compute, key, group });
                Ok(None)
            }
            GpuOp::Alloc { buf, bytes } => {
                let reservation = self.doc.admit(bytes).map_err(|e| e.to_string())?;
                let buffer = GpuBuffer::new(cx, bytes, GpuBufferFlags { indirect: false, readback: true })?;
                self.sessions[k].buffers.insert(buf, (buffer.clone(), reservation));
                // a new buffer holds zeros
                new_pass(building);
                let mut b = building.take().unwrap();
                let filled = self.library(&mut b, Some(0), None, &buffer, (bytes / 4) as u32);
                b.bound.insert(buf);
                *building = Some(b);
                filled.map(|_| None)
            }
            GpuOp::Free { buf } => {
                self.sessions[k].buffers.remove(&buf);
                Ok(None)
            }
            GpuOp::Upload { buf, offset, bytes } => {
                if stale {
                    return Ok(None);
                }
                let s = &self.sessions[k];
                if building.as_ref().is_some_and(|b| b.bound.contains(&buf)) {
                    return Ok(Some(Item::Op { epoch, op: GpuOp::Upload { buf, offset, bytes } }));
                }
                if s.busy.contains(&buf) {
                    return Ok(Some(Item::Op { epoch, op: GpuOp::Upload { buf, offset, bytes } }));
                }
                let buffer = s.buffers.get(&buf).ok_or("an upload into a buffer the session does not hold")?.0.clone();
                buffer.update_owned(cx, offset, bytes)?;
                Ok(None)
            }
            GpuOp::Fill { buf, word } => {
                if stale {
                    return Ok(None);
                }
                let buffer = self.sessions[k].buffers.get(&buf).ok_or("a fill of a buffer the session does not hold")?.0.clone();
                new_pass(building);
                let mut b = building.take().unwrap();
                let done = self.library(&mut b, Some(word), None, &buffer, (buffer.size() / 4) as u32);
                b.bound.insert(buf);
                *building = Some(b);
                done.map(|_| None)
            }
            GpuOp::Copy { src, dst } => {
                if stale {
                    return Ok(None);
                }
                let s = &self.sessions[k];
                let from = s.buffers.get(&src).ok_or("a copy from a buffer the session does not hold")?.0.clone();
                let to = s.buffers.get(&dst).ok_or("a copy into a buffer the session does not hold")?.0.clone();
                new_pass(building);
                let mut b = building.take().unwrap();
                let done = self.library(&mut b, None, Some(&from), &to, (to.size() / 4) as u32);
                b.bound.extend([src, dst]);
                *building = Some(b);
                done.map(|_| None)
            }
            GpuOp::Dispatch { kernel, groups, params, buffers } => {
                if stale {
                    return Ok(None);
                }
                let s = &self.sessions[k];
                let kn = s.kernels.get(&kernel).ok_or("a dispatch of a kernel the session did not send")?;
                let (compute, key, group) = (kn.kernel.clone(), kn.key, kn.group);
                let mut set = compute.params();
                for (name, bytes) in &params {
                    set.set(*name, bytes)?;
                }
                let mut bound = Vec::with_capacity(buffers.len());
                for b in &buffers {
                    bound.push(s.buffers.get(b).ok_or("a dispatch binds a buffer the session does not hold")?.0.clone());
                }
                new_pass(building);
                let mut b = building.take().unwrap();
                b.bound.extend(buffers.iter().copied());
                let threads = groups.iter().map(|n| *n as u64).product::<u64>() * group;
                let done = self.dispatch(&mut b, &compute, key, groups, threads, &set, &bound);
                *building = Some(b);
                done.map(|_| None)
            }
            GpuOp::Run { .. } | GpuOp::Read { .. } => unreachable!("runs and reads come in as their own items"),
        }
    }

    /// Submits session `k`'s pass, and reads a word of what it binds last to see it finish.
    fn submit(&mut self, cx: &mut Cx, k: usize, b: Building) {
        let Building { pass, bound, last, .. } = b;
        pass.submit(cx);
        let s = &mut self.sessions[k];
        s.busy.extend(bound);
        let Some(last) = last else { return };
        s.in_flight += 1;
        self.in_flight += 1;
        let (done, wake, id) = (self.done_tx.clone(), self.wake.clone(), s.id);
        let read = cx.read(&[(&last, 0..4)], move |_, _| {
            let _ = done.send(id);
            wake();
        });
        if read.is_err() {
            let _ = self.done_tx.send(id);
        }
    }
}

/// A GPU device on its own thread, on an offscreen session of this machine's GPU: for a hub
/// with no window of its own, and for tests. The thread ends once every link is gone and its
/// sessions ended.
pub fn spawn() -> Result<GpuLink, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("compute-program-gpu".into())
        .spawn(move || {
            let mut session = match GpuSession::open() {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let (mut device, link) = match GpuDevice::new(&mut session.cx) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let _ = tx.send(Ok(link));
            while session.turn() && device.handle(&mut session.cx) {}
        })
        .map_err(|e| format!("the GPU device's thread: {e}"))?;
    rx.recv().map_err(|_| "the GPU device's thread ended".to_string())?
}
