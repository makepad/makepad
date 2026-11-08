//! Exact bodies for the solid graph, on a thread of their own.
//!
//! The kernel recurses deeply, so one long-lived worker with a big stack runs every job: the
//! caller never does kernel work and can cancel. The worker keeps one body per step key,
//! whoever asked for it. No step depends on the chord, only a tessellation does: a drag's
//! preview, its commit and a slice at print detail build a body once and each cut their own
//! mesh from it, and a body's exact measure and entities are worked out once. An edit
//! rebuilds only the steps it changed.
//!
//! A body depends on its step and its inputs alone, never on what the worker did before:
//! each step builds on a renewed kernel (`Kernel::renewed`: nothing an earlier call left on
//! it, its random draws seeded from the step's key), from copies of its inputs as their steps
//! made them. A mesh or a query reads a copy of its own on a renewed kernel too: what it works
//! out on the body it reads (a tessellation keeps its trim curves there) reaches no other.
//!
//! The worker runs under the process's CPU budget at the class of the job it serves: the
//! highest class waiting first, the oldest of a class first, with checkpoints between steps
//! and inside the kernel. A Background job gives way at its next checkpoint to a job of a
//! higher class sent meanwhile: it unwinds as a cancel does, keeps the steps it finished and
//! waits again, first of its class. A caller lends its own token while it waits. The worker
//! starts at its first body: a document without B-rep parts never pays for one.

use crate::guard::{guarded, stack_size, BrepError, CancelToken};
use crate::ops::{self, Body};
use crate::profile::Frame;
use crate::query::{self, EdgeInfo, EdgeShape};
use crate::tess::{tessellate, TessOptions};
use makepad_csg_brep::kernel::Kernel;
use makepad_csg_brep::topology::{brep, FaceId};
use makepad_csg_graph::edges::{EdgeFilter, EdgeKind, EdgeSel};
use makepad_csg_graph::eval::plane_frame;
use makepad_csg_graph::{BoolMode, BrepEngine, BrepEntity, BrepFailure, BrepMeasure, BrepOp, BrepStep, Key, Open, TriMesh};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// How many bodies stay on the worker, least recently used out first: a drag revisits few.
const KEEP_BODIES: usize = 64;
/// How many meshes stay on the worker, likewise: a few bodies' display and print detail.
const KEEP_MESHES: usize = 32;
/// Chords below this are print detail (mm): a slice's mesh, which speaks for no job (the slice
/// says its own stages).
const PRINT_DETAIL: f64 = 0.02;
/// The longest kernel work goes between two checkpoints of its CPU permit while no one waits
/// for the permit: the budget keeps account of how often its holders check in.
const CHECKPOINT_EVERY: Duration = Duration::from_millis(5);

enum Want {
    Mesh(f64),
    Measure,
    Entities,
}

enum Answer {
    Mesh(Arc<TriMesh>),
    Measure(BrepMeasure),
    Entities(Vec<BrepEntity>, Vec<BrepEntity>),
}

struct Job {
    steps: Vec<BrepStep>,
    want: Want,
    cancel: CancelToken,
    /// The job the caller works for: the steps report on it.
    job: Option<makepad_jobs::JobReporter>,
    /// The caller's CPU budget class: the work runs at it.
    class: u8,
    reply: Sender<Result<Answer, BrepFailure>>,
}

/// The process's one engine. The thread starts here, at the first body.
pub fn engine() -> Arc<dyn BrepEngine> {
    static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
    ENGINE.get_or_init(|| Arc::new(Engine::default())).clone()
}

/// See [`engine`]. Its worker ends with it.
#[derive(Default)]
pub struct Engine {
    jobs: OnceLock<Mutex<Sender<Job>>>,
    shared: Arc<Shared>,
}

/// What the worker and its callers both read.
#[derive(Default)]
struct Shared {
    /// The step keys of the bodies the worker keeps.
    held: Mutex<HashSet<Key>>,
    /// Jobs of a class above Background sent and not started: a Background job running gives way
    /// while there are any. Counted once sent, so a job counted is in the channel or waiting; the
    /// worker may start one before it is counted (below zero meanwhile).
    ahead: AtomicIsize,
}

impl Shared {
    /// The held keys: an insert or a removal leaves the set whole, whatever unwound meanwhile.
    fn held(&self) -> MutexGuard<'_, HashSet<Key>> {
        self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Engine {
    fn send(&self, job: Job) -> Result<(), String> {
        let jobs = self.jobs.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new().name("brep".into()).stack_size(stack_size()).spawn(move || serve(rx, shared));
            if let Err(e) = spawned {
                eprintln!("[brep] the worker thread did not start: {e}");
            }
            Mutex::new(tx)
        });
        let ahead = job.class < makepad_cpu_budget::BACKGROUND;
        jobs.lock().map_err(|_| "the B-rep worker is gone".to_string())?.send(job).map_err(|_| "the B-rep worker is gone".to_string())?;
        if ahead {
            self.shared.ahead.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    fn ask(&self, steps: Vec<BrepStep>, want: Want, cancelled: Option<&dyn Fn() -> bool>) -> Result<Answer, BrepFailure> {
        let (reply, answer) = channel();
        let cancel = CancelToken::new();
        let class = makepad_cpu_budget::thread_class().unwrap_or(makepad_cpu_budget::VISIBLE);
        self.send(Job { steps, want, cancel: cancel.clone(), job: makepad_jobs::current(), class, reply }).map_err(|message| BrepFailure { step: None, message })?;
        // Waiting is no work: the caller's token is the budget's until the answer is in.
        let _lent = Lent::new();
        loop {
            match answer.recv_timeout(Duration::from_millis(5)) {
                Ok(a) => return a,
                Err(RecvTimeoutError::Disconnected) => return Err(gone()),
                Err(RecvTimeoutError::Timeout) => {
                    if cancelled.is_some_and(|c| c()) {
                        cancel.cancel();
                    }
                }
            }
        }
    }
}

fn gone() -> BrepFailure {
    BrepFailure { step: None, message: "the B-rep worker is gone".into() }
}

/// The calling thread's CPU token, lent to the budget while this lives.
struct Lent(bool);

impl Lent {
    fn new() -> Lent {
        Lent(makepad_cpu_budget::lend())
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        if self.0 {
            makepad_cpu_budget::take_back();
        }
    }
}

impl BrepEngine for Engine {
    fn mesh(&self, steps: Vec<BrepStep>, chord: f64, cancelled: Option<&dyn Fn() -> bool>) -> Result<Arc<TriMesh>, BrepFailure> {
        match self.ask(steps, Want::Mesh(chord), cancelled)? {
            Answer::Mesh(m) => Ok(m),
            _ => Err(gone()),
        }
    }
    fn measure(&self, steps: Vec<BrepStep>) -> Result<BrepMeasure, BrepFailure> {
        match self.ask(steps, Want::Measure, None)? {
            Answer::Measure(m) => Ok(m),
            _ => Err(gone()),
        }
    }
    fn entities(&self, steps: Vec<BrepStep>) -> Result<(Vec<BrepEntity>, Vec<BrepEntity>), BrepFailure> {
        match self.ask(steps, Want::Entities, None)? {
            Answer::Entities(e, f) => Ok((e, f)),
            _ => Err(gone()),
        }
    }
    fn built(&self, keys: &[Key]) -> bool {
        let held = self.shared.held();
        keys.iter().all(|k| held.contains(k))
    }
}

/// The worker's loop: of the jobs waiting, the one of the highest class first (the budget's
/// classes count down), the oldest of it.
fn serve(jobs: Receiver<Job>, shared: Arc<Shared>) {
    let mut w = Worker::new(shared.clone());
    let mut waiting: Vec<Job> = Vec::new();
    loop {
        if waiting.is_empty() {
            match jobs.recv() {
                Ok(job) => waiting.push(job),
                Err(_) => return,
            }
        }
        waiting.extend(jobs.try_iter());
        let Some(next) = (0..waiting.len()).min_by_key(|&i| waiting[i].class) else { continue };
        let job = waiting.remove(next);
        if job.class < makepad_cpu_budget::BACKGROUND {
            shared.ahead.fetch_sub(1, Ordering::AcqRel);
        }
        match w.serve(&job) {
            Served::Answer(answer) => {
                let _ = job.reply.send(answer);
            }
            // It goes on where it was once the jobs it gave way to are done.
            Served::GaveWay => waiting.insert(0, job),
        }
    }
}

/// What came of serving a job.
enum Served {
    Answer(Result<Answer, BrepFailure>),
    /// A Background job gave way to a job of a higher class: the steps it finished are kept.
    GaveWay,
}

thread_local! {
    /// The worker's CPU permit while it runs a job, and when it last checked in with it.
    static PERMIT: Cell<Option<(u32, Instant)>> = const { Cell::new(None) };
    /// While the worker runs a Background job: the count of jobs it gives way to.
    static GIVES_WAY: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
    /// The job running gave way: its run unwound at a checkpoint.
    static GAVE_WAY: Cell<bool> = const { Cell::new(false) };
}

/// A permit of the CPU budget at a job's class, held while the worker runs the job.
struct Permit;

impl Permit {
    fn admit(class: u8, shared: &Arc<Shared>) -> Permit {
        makepad_cpu_budget::set_thread_class(class);
        let permit = makepad_cpu_budget::admit(class, makepad_cpu_budget::APP, "B-rep");
        PERMIT.with(|p| p.set(Some((permit, Instant::now()))));
        GIVES_WAY.with(|g| *g.borrow_mut() = (class >= makepad_cpu_budget::BACKGROUND).then(|| shared.clone()));
        GAVE_WAY.with(|g| g.set(false));
        Permit
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        GIVES_WAY.with(|g| g.borrow_mut().take());
        if let Some((permit, _)) = PERMIT.with(Cell::take) {
            makepad_cpu_budget::release(permit);
        }
    }
}

/// The worker's checkpoint, between steps and from inside the kernel: a Background job gives way
/// to a job of a higher class sent meanwhile (it unwinds as a cancel does: the kernel is renewed
/// for whatever runs next); the permit goes to a higher class waiting for one and comes back when
/// there is room.
fn checkpoint() {
    if GIVES_WAY.with(|g| g.borrow().as_ref().is_some_and(|s| s.ahead.load(Ordering::Acquire) > 0)) {
        GAVE_WAY.with(|g| g.set(true));
        std::panic::resume_unwind(Box::new(makepad_csg_brep::limits::Unwound::Cancelled));
    }
    PERMIT.with(|p| {
        if let Some((permit, at)) = p.get() {
            if makepad_cpu_budget::should_yield(permit) || at.elapsed() >= CHECKPOINT_EVERY {
                p.set(Some((makepad_cpu_budget::checkpoint(permit, 0, ""), Instant::now())));
            }
        }
    });
}

/// A body the worker keeps.
struct Kept {
    /// As its step made it: the steps, meshes and queries that read it copy it. Nothing
    /// changes it.
    body: Body,
    used: u64,
    measure: Option<BrepMeasure>,
    entities: Option<(Vec<BrepEntity>, Vec<BrepEntity>)>,
}

struct Worker {
    shared: Arc<Shared>,
    /// The step being built: a failure belongs to it.
    failing: Option<Key>,
    /// The kernel of the step or read under way, renewed for each.
    kernel: Kernel,
    /// Copies of inputs the step being built took: each its own key domain.
    serial: u64,
    bodies: HashMap<Key, Kept>,
    meshes: HashMap<(Key, u64), (Arc<TriMesh>, u64)>,
    clock: u64,
}

/// The body's entity for each of a copy's: `copy` and `body` list them in the same order.
fn as_listed<T: Copy + Eq + std::hash::Hash>(copy: Vec<T>, body: Vec<T>) -> Result<impl Fn(T) -> Result<T, BrepError>, BrepError> {
    if copy.len() != body.len() {
        return Err(BrepError::Kernel("a copy lists other entities than its body".into()));
    }
    let of: HashMap<T, T> = copy.into_iter().zip(body).collect();
    Ok(move |id| of.get(&id).copied().ok_or_else(|| BrepError::Kernel("an entity the copy does not list".into())))
}

/// A step's kernel draws from its key: the same step draws the same.
fn seed(key: &Key) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&key[..8]);
    u64::from_le_bytes(word)
}

impl Worker {
    fn new(shared: Arc<Shared>) -> Worker {
        Worker { shared, failing: None, kernel: ops::new_kernel(), serial: 0, bodies: HashMap::new(), meshes: HashMap::new(), clock: 0 }
    }

    fn serve(&mut self, job: &Job) -> Served {
        if job.cancel.is_cancelled() {
            return Served::Answer(Err(BrepFailure { step: None, message: BrepError::Cancelled.to_string() }));
        }
        let _permit = Permit::admit(job.class, &self.shared);
        self.failing = None;
        // A call that failed or unwound leaves no half-made body: a step's body is kept once
        // it is whole, and the next step or read renews the kernel it left.
        let answer = makepad_jobs::with_current(job.job.clone(), || guarded(stack_size(), &job.cancel, Some(checkpoint), || self.run(&job.steps, &job.want)));
        if GAVE_WAY.with(Cell::take) {
            return Served::GaveWay;
        }
        Served::Answer(answer.map_err(|e| BrepFailure { step: self.failing, message: e.to_string() }))
    }

    fn run(&mut self, steps: &[BrepStep], want: &Want) -> Result<Answer, BrepError> {
        let Some(last) = steps.last() else { return Err(BrepError::Input("an empty recipe".into())) };
        // What is left to build, by kind: the job says "fillets 3/7".
        let job = if matches!(want, Want::Mesh(chord) if *chord < PRINT_DETAIL) { None } else { makepad_jobs::current() };
        let todo: Vec<&BrepStep> = steps.iter().filter(|s| !self.bodies.contains_key(&s.key)).collect();
        let total = todo.len();
        let mut kinds: HashMap<&'static str, (u32, u32)> = HashMap::new();
        for s in &todo {
            kinds.entry(op_kind(&s.op)).or_default().1 += 1;
        }
        let mut built = 0usize;
        for step in steps {
            self.clock += 1;
            if let Some(b) = self.bodies.get_mut(&step.key) {
                b.used = self.clock;
                continue;
            }
            checkpoint();
            if let Some(job) = &job {
                let kind = op_kind(&step.op);
                let count = kinds.entry(kind).or_default();
                count.0 += 1;
                let (i, n) = *count;
                job.stage(kind, (n > 1).then_some((i, n)), Some(built as f32 / total.max(1) as f32));
            }
            built += 1;
            self.failing = Some(step.key);
            let body = self.step(step)?;
            self.failing = None;
            self.bodies.insert(step.key, Kept { body, used: self.clock, measure: None, entities: None });
            self.shared.held().insert(step.key);
        }
        while self.bodies.len() > KEEP_BODIES {
            let oldest = self.bodies.iter().min_by_key(|(_, kept)| kept.used).map(|(k, _)| *k);
            if let Some(k) = oldest {
                self.bodies.remove(&k);
                self.shared.held().remove(&k);
            }
        }
        let Worker { kernel, bodies, meshes, clock, .. } = self;
        let kept = bodies.get_mut(&last.key).ok_or_else(|| BrepError::Input("a recipe's body is gone".into()))?;
        match want {
            Want::Mesh(chord) => {
                if let Some(m) = meshes.get_mut(&(last.key, chord.to_bits())) {
                    m.1 = *clock;
                    return Ok(Answer::Mesh(m.0.clone()));
                }
            }
            Want::Measure => {
                if let Some(m) = kept.measure {
                    return Ok(Answer::Measure(m));
                }
            }
            Want::Entities => {
                if let Some((e, f)) = &kept.entities {
                    return Ok(Answer::Entities(e.clone(), f.clone()));
                }
            }
        }
        checkpoint();
        // A read of its own, like a step: what it works out reaches no other.
        *kernel = kernel.renewed(seed(&last.key));
        let copy = &mut ops::copy_body(kernel, &kept.body, 1)?;
        let k = &*kernel;
        let answer = match want {
            Want::Mesh(chord) => {
                if let Some(job) = &job {
                    job.stage("meshing", None, None);
                }
                let mesh = Arc::new(tessellate(k, copy, &TessOptions { chord: *chord, ..TessOptions::default() })?.mesh);
                meshes.insert((last.key, chord.to_bits()), (mesh.clone(), *clock));
                while meshes.len() > KEEP_MESHES {
                    let oldest = meshes.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| *k);
                    if let Some(k) = oldest {
                        meshes.remove(&k);
                    }
                }
                Answer::Mesh(mesh)
            }
            Want::Measure => {
                if let Some(job) = &job {
                    job.stage("measuring", None, None);
                }
                let m = query::measure(k, copy)?;
                let m = BrepMeasure { volume: m.volume, area: m.area, centroid: m.centroid, min: m.min, max: m.max, faces: m.faces, edges: m.edges };
                kept.measure = Some(m);
                Answer::Measure(m)
            }
            Want::Entities => {
                // Named as the body names them: the copy numbers its edges and faces afresh, and
                // lists them in the body's order.
                let edge = as_listed(brep::p1::edges(copy, None)?, brep::p1::edges(&kept.body, None)?)?;
                let face = as_listed(brep::p1::faces(copy, None)?, brep::p1::faces(&kept.body, None)?)?;
                let edges = query::edges(k, copy)?
                    .into_iter()
                    .map(|e| {
                        Ok(BrepEntity {
                            id: edge(e.id)?.index(),
                            kind: shape_name(e.shape),
                            size: e.length,
                            at: e.mid,
                            normal: None,
                            convex: e.convex,
                            smooth: e.smooth,
                            radius: e.radius,
                        })
                    })
                    .collect::<Result<Vec<BrepEntity>, BrepError>>()?;
                let faces = query::faces(k, copy)?
                    .into_iter()
                    .map(|f| Ok(BrepEntity { id: face(f.id)?.index(), kind: f.kind, size: f.area, at: f.centroid, normal: f.normal, convex: false, smooth: false, radius: None }))
                    .collect::<Result<Vec<BrepEntity>, BrepError>>()?;
                kept.entities = Some((edges.clone(), faces.clone()));
                Answer::Entities(edges, faces)
            }
        };
        Ok(answer)
    }

    /// A private copy of an input, in a key domain of its own within the step.
    fn input(&mut self, key: &Key) -> Result<Body, BrepError> {
        self.serial += 1;
        let kept = self.bodies.get(key).ok_or_else(|| BrepError::Input("a step's input is missing".into()))?;
        ops::copy_body(&mut self.kernel, &kept.body, self.serial)
    }

    fn step(&mut self, step: &BrepStep) -> Result<Body, BrepError> {
        // A kernel of its own: what the step builds depends on its inputs alone.
        self.kernel = self.kernel.renewed(seed(&step.key));
        self.serial = 0;
        let name = op_name(&step.op);
        self.build(step).map_err(|e| match e {
            BrepError::Kernel(m) => BrepError::Kernel(format!("{name}: {m}")),
            BrepError::Input(m) => BrepError::Input(format!("{name}: {m}")),
            other => other,
        })
    }

    fn build(&mut self, step: &BrepStep) -> Result<Body, BrepError> {
        let first = |w: &mut Worker| -> Result<Body, BrepError> {
            let key = step.inputs.first().ok_or_else(|| BrepError::Input("needs a body".into()))?;
            w.input(key)
        };
        match &step.op {
            BrepOp::Box { size } => ops::make_box(&mut self.kernel, *size),
            BrepOp::Sphere { radius } => ops::make_sphere(&mut self.kernel, *radius),
            BrepOp::Cylinder { radius, top_radius, height } if radius == top_radius => ops::make_cylinder(&mut self.kernel, *radius, *height),
            BrepOp::Cylinder { radius, top_radius, height } => ops::make_cone(&mut self.kernel, *radius, *top_radius, *height),
            BrepOp::Torus { radius, tube } => ops::make_torus(&mut self.kernel, *radius, *tube),
            BrepOp::Extrude { sketch, plane, height, both } => {
                let (u, v, w) = plane_frame(*plane);
                let start = if *both { -height * 0.5 } else { 0.0 };
                let frame = Frame { origin: [w.x * start, w.y * start, w.z * start], u: [u.x, u.y, u.z], v: [v.x, v.y, v.z] };
                let b = ops::extrude(&mut self.kernel, sketch, &frame, [w.x, w.y, w.z], *height)?;
                ops::check_solid(&b, "extrude")?;
                Ok(b)
            }
            BrepOp::Revolve { sketch, angle } => {
                let b = ops::revolve(&mut self.kernel, sketch, *angle)?;
                ops::check_solid(&b, "revolve")?;
                Ok(b)
            }
            BrepOp::Sweep { sketch, path, scale } => {
                let b = ops::sweep(&mut self.kernel, sketch, path, scale)?;
                ops::check_solid(&b, "sweep")?;
                Ok(b)
            }
            BrepOp::Loft { sections, plane } => {
                let (u, v, w) = plane_frame(*plane);
                let list: Vec<_> = sections
                    .iter()
                    .map(|s| (s.sketch.clone(), Frame { origin: [w.x * s.at, w.y * s.at, w.z * s.at], u: [u.x, u.y, u.z], v: [v.x, v.y, v.z] }))
                    .collect();
                let b = ops::loft(&mut self.kernel, &list, 3.min(list.len() - 1).max(1))?;
                ops::check_solid(&b, "loft")?;
                Ok(b)
            }
            BrepOp::Boolean { mode } => {
                let mode = match mode {
                    BoolMode::Union => ops::BoolMode::Union,
                    BoolMode::Difference => ops::BoolMode::Difference,
                    BoolMode::Intersection => ops::BoolMode::Intersection,
                };
                let mut out = first(self)?;
                for key in &step.inputs[1..] {
                    let b = self.input(key)?;
                    out = ops::boolean(&mut self.kernel, mode, out, b)?;
                }
                Ok(out)
            }
            BrepOp::Fillet { radius, edges } => {
                let mut b = first(self)?;
                let picked = select_edges(&self.kernel, &mut b, edges)?;
                ops::fillet(&mut self.kernel, &mut b, &picked, *radius)?;
                Ok(b)
            }
            BrepOp::Chamfer { size, edges } => {
                let mut b = first(self)?;
                let picked = select_edges(&self.kernel, &mut b, edges)?;
                ops::chamfer(&mut self.kernel, &mut b, &picked, *size)?;
                Ok(b)
            }
            BrepOp::Shell { thickness, open } => {
                let mut b = first(self)?;
                let faces = open_faces(&self.kernel, &mut b, *open)?;
                ops::shell(&mut self.kernel, &b, *thickness, &faces)
            }
            BrepOp::Offset { distance } => {
                let mut b = first(self)?;
                ops::offset(&mut self.kernel, &mut b, *distance)
            }
            BrepOp::Place { at, rotate, scale } => {
                let mut b = first(self)?;
                if *scale != [1.0; 3] {
                    ops::scale(&mut self.kernel, &mut b, *scale)?;
                }
                ops::place(&mut self.kernel, &mut b, *at, *rotate)?;
                Ok(b)
            }
        }
    }
}

/// A step's kind as a job stage names it, counted: "fillets", "booleans".
fn op_kind(op: &BrepOp) -> &'static str {
    match op {
        BrepOp::Box { .. } | BrepOp::Sphere { .. } | BrepOp::Cylinder { .. } | BrepOp::Torus { .. } => "primitives",
        BrepOp::Extrude { .. } => "extrusions",
        BrepOp::Revolve { .. } => "revolves",
        BrepOp::Sweep { .. } => "sweeps",
        BrepOp::Loft { .. } => "lofts",
        BrepOp::Boolean { .. } => "booleans",
        BrepOp::Fillet { .. } => "fillets",
        BrepOp::Chamfer { .. } => "chamfers",
        BrepOp::Shell { .. } => "shells",
        BrepOp::Offset { .. } => "offsets",
        BrepOp::Place { .. } => "placements",
    }
}

fn op_name(op: &BrepOp) -> &'static str {
    match op {
        BrepOp::Box { .. } => "box",
        BrepOp::Sphere { .. } => "sphere",
        BrepOp::Cylinder { .. } => "cylinder",
        BrepOp::Torus { .. } => "torus",
        BrepOp::Extrude { .. } => "extrude",
        BrepOp::Revolve { .. } => "revolve",
        BrepOp::Sweep { .. } => "sweep",
        BrepOp::Loft { .. } => "loft",
        BrepOp::Boolean { .. } => "boolean",
        BrepOp::Fillet { .. } => "fillet",
        BrepOp::Chamfer { .. } => "chamfer",
        BrepOp::Shell { .. } => "shell",
        BrepOp::Offset { .. } => "offset",
        BrepOp::Place { .. } => "placement",
    }
}

fn shape_name(s: EdgeShape) -> &'static str {
    match s {
        EdgeShape::Line => "line",
        EdgeShape::Circle => "circle",
        EdgeShape::Curve => "curve",
    }
}

/// The faces a shell leaves open: the flat ones at the body's top or bottom.
fn open_faces(k: &Kernel, b: &mut Body, open: Open) -> Result<Vec<FaceId>, BrepError> {
    if open == Open::Closed {
        return Ok(Vec::new());
    }
    let want = if open == Open::Top { 1.0 } else { -1.0 };
    let faces = query::faces(k, b)?;
    let side: Vec<_> = faces.iter().filter(|f| f.normal.is_some_and(|n| n[1] * want > 0.999)).collect();
    let extreme = side.iter().map(|f| f.centroid[1] * want).fold(f64::MIN, f64::max);
    let picked: Vec<FaceId> = side.iter().filter(|f| (f.centroid[1] * want - extreme).abs() < 1e-6).map(|f| f.id).collect();
    if picked.is_empty() {
        return Err(BrepError::Input(format!("no flat face at the {} to leave open", if want > 0.0 { "top" } else { "bottom" })));
    }
    Ok(picked)
}

fn matches_filter(e: &EdgeInfo, f: &EdgeFilter, lo_y: f64, hi_y: f64) -> bool {
    const EPS: f64 = 1e-6;
    if f.top && e.points.iter().any(|p| (p[1] - hi_y).abs() > EPS) {
        return false;
    }
    if f.bottom && e.points.iter().any(|p| (p[1] - lo_y).abs() > EPS) {
        return false;
    }
    if let Some((a, b)) = f.y {
        if e.points.iter().any(|p| p[1] < a - EPS || p[1] > b + EPS) {
            return false;
        }
    }
    if let Some((a, b)) = f.within {
        if e.points.iter().any(|p| (0..3).any(|i| p[i] < a[i] - EPS || p[i] > b[i] + EPS)) {
            return false;
        }
    }
    if let Some(d) = f.dir {
        if e.shape != EdgeShape::Line {
            return false;
        }
        let t = [e.end[0] - e.start[0], e.end[1] - e.start[1], e.end[2] - e.start[2]];
        let (lt, ld) = ((t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt(), (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
        let cos = (t[0] * d[0] + t[1] * d[1] + t[2] * d[2]) / (lt * ld).max(1e-30);
        if cos.abs() < 2f64.to_radians().cos() {
            return false;
        }
    }
    if let Some(kind) = f.kind {
        let same = matches!((kind, e.shape), (EdgeKind::Line, EdgeShape::Line) | (EdgeKind::Circle, EdgeShape::Circle) | (EdgeKind::Curve, EdgeShape::Curve));
        if !same {
            return false;
        }
    }
    if f.convex && !e.convex {
        return false;
    }
    if f.concave && e.convex {
        return false;
    }
    if f.min_length.is_some_and(|m| e.length < m) {
        return false;
    }
    true
}

/// The sharp edges a selector picks.
pub fn select_edges(k: &Kernel, b: &mut Body, sel: &EdgeSel) -> Result<Vec<makepad_csg_brep::EdgeId>, BrepError> {
    let edges: Vec<EdgeInfo> = query::edges(k, b)?.into_iter().filter(|e| !e.smooth).collect();
    if sel.any.is_empty() {
        return Ok(edges.iter().map(|e| e.id).collect());
    }
    let ys = edges.iter().flat_map(|e| e.points.iter().map(|p| p[1]));
    let (lo, hi) = ys.fold((f64::MAX, f64::MIN), |(l, h), y| (l.min(y), h.max(y)));
    let mut out = Vec::new();
    for f in &sel.any {
        let hits: Vec<&EdgeInfo> = edges.iter().filter(|e| matches_filter(e, f, lo, hi)).collect();
        let picked: Vec<&EdgeInfo> = match f.near {
            Some(q) => {
                let d = |e: &EdgeInfo| e.points.iter().map(|p| (0..3).map(|i| (p[i] - q[i]).powi(2)).sum::<f64>()).fold(f64::MAX, f64::min);
                hits.iter().copied().min_by(|a, b| d(a).total_cmp(&d(b))).into_iter().collect()
            }
            None => hits,
        };
        for e in picked {
            if !out.contains(&e.id) {
                out.push(e.id);
            }
        }
    }
    if out.is_empty() {
        return Err(BrepError::Input(format!("no edge matches this selection ({} sharp edges on the body)", edges.len())));
    }
    Ok(out)
}
