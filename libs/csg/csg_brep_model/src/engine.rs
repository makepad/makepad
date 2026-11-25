//! Exact bodies for the solid graph, on a thread of their own.
//!
//! The kernel recurses deeply, so one long-lived worker with a big stack runs every job: the
//! caller never does kernel work and can cancel. The worker keeps bodies by step key.
//! An identity placement shares its input's body and keeps its own face names.
//! A drag's preview, its commit and a slice at print detail build a body once and each cut
//! their own mesh at the requested chord. Each key caches its exact measure and entities.
//! An edit rebuilds only the steps it changed.
//!
//! A body depends on its step and its inputs alone, never on what the worker did before:
//! each new body builds on a renewed kernel (`Kernel::renewed`: nothing an earlier call left on
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

use crate::guard::{caught, guarded, stack_size, BrepError, CancelToken};
use crate::names::{self, Names};
use crate::ops::{self, Body};
use crate::profile::Frame;
use crate::query::{self, EdgeInfo, EdgeShape};
use crate::tess::{tessellate, TessOptions};
use makepad_csg_brep::kernel::Kernel;
use makepad_csg_brep::math::{Point3, Vec3, EFF_ZERO};
use makepad_csg_brep::query::api_queries::face_internal_point;
use makepad_csg_brep::topology::{brep, FaceId};
use makepad_csg_cad::split;
use makepad_csg_graph::edges::{EdgeFilter, EdgeKind, EdgeSel, Face, FaceName, Role};
use makepad_csg_graph::eval::plane_frame;
use makepad_csg_graph::{BoolMode, BrepAt, BrepEngine, BrepEntity, BrepFailure, BrepMeasure, BrepOp, BrepStep, BrepTaken, HoleKind, Key, MirrorPlane, Open, SplitPiece, SplitPins, StepNote, TriMesh};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// How many distinct bodies stay on the worker. The committed build's steps go out last.
const KEEP_BODIES: usize = 64;
/// How many meshes stay on the worker, likewise: a few bodies' display and print detail.
const KEEP_MESHES: usize = 32;
/// How many steps' notes stay, the least recently built or asked for out first: a host asks for
/// the notes of the steps it shows at each build, so theirs stay however many it made since.
const KEEP_NOTES: usize = 256;
/// Chords below this are print detail (mm): a slice's mesh, which speaks for no job (the slice
/// says its own stages).
const PRINT_DETAIL: f64 = 0.02;
/// The longest kernel work goes between two checkpoints of its CPU permit while no one waits
/// for the permit: the budget keeps account of how often its holders check in.
const CHECKPOINT_EVERY: Duration = Duration::from_millis(5);

/// The chord a point query meshes a body at when nothing meshed it yet (mm).
const AT_CHORD: f64 = 0.05;
/// The chord of the mesh a split plans its pins on (mm): their room to a tenth of a millimetre.
const SPLIT_CHORD: f64 = 0.1;

enum Want {
    Mesh(f64),
    Measure,
    Entities,
    At([f64; 3], f64),
}

enum Answer {
    Mesh(Arc<TriMesh>),
    Measure(BrepMeasure),
    Entities(Vec<BrepEntity>, Vec<BrepEntity>),
    At(BrepAt),
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
    /// The step keys of the bodies the worker keeps, with what each step's written selections
    /// took ([`BrepEngine::taken`]).
    held: Mutex<HashMap<Key, Vec<BrepTaken>>>,
    /// The bed's committed step keys. The worker takes a snapshot when it needs to evict.
    committed: Mutex<Arc<HashSet<Key>>>,
    /// What steps noted when built ([`BrepEngine::note`]), with when each was last built or asked
    /// for, and the clock of those: kept past the step's body, since a mesh made from it may
    /// still show.
    notes: Mutex<(HashMap<Key, (StepNote, u64)>, u64)>,
    /// Jobs of a class above Background sent and not started: a Background job running gives way
    /// while there are any. Counted once sent, so a job counted is in the channel or waiting; the
    /// worker may start one before it is counted (below zero meanwhile).
    ahead: AtomicIsize,
}

impl Shared {
    fn committed(&self) -> MutexGuard<'_, Arc<HashSet<Key>>> {
        self.committed.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The held keys: an insert or a removal leaves the set whole, whatever unwound meanwhile.
    fn held(&self) -> MutexGuard<'_, HashMap<Key, Vec<BrepTaken>>> {
        self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Step `key`'s note, as used now. O(1); O(KEEP_NOTES) when one goes out.
    fn note(&self, key: Key, note: Option<StepNote>) -> Option<StepNote> {
        let mut guard = self.notes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (notes, clock) = &mut *guard;
        *clock += 1;
        let now = *clock;
        if let Some(note) = note {
            notes.insert(key, (note, now));
            if notes.len() > KEEP_NOTES {
                if let Some(oldest) = notes.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| *k) {
                    notes.remove(&oldest);
                }
            }
        }
        notes.get_mut(&key).map(|(note, used)| {
            *used = now;
            *note
        })
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
        self.send(Job { steps, want, cancel: cancel.clone(), job: makepad_jobs::current(), class, reply }).map_err(|message| BrepFailure { step: None, lost: None, message })?;
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
    BrepFailure { step: None, lost: None, message: "the B-rep worker is gone".into() }
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
    fn at(&self, steps: Vec<BrepStep>, p: [f64; 3], reach: f64) -> Result<BrepAt, BrepFailure> {
        match self.ask(steps, Want::At(p, reach), None)? {
            Answer::At(a) => Ok(a),
            _ => Err(gone()),
        }
    }
    fn built(&self, keys: &[Key]) -> bool {
        let held = self.shared.held();
        keys.iter().all(|k| held.contains_key(k))
    }
    fn keep(&self, keys: Vec<Key>) {
        let mut next = Arc::new(keys.into_iter().collect());
        std::mem::swap(&mut *self.shared.committed(), &mut next);
    }
    fn taken(&self, key: Key) -> Option<Vec<BrepTaken>> {
        self.shared.held().get(&key).cloned()
    }
    fn note(&self, key: Key) -> Option<StepNote> {
        self.shared.note(key, None)
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

/// The meshes the worker keeps, by body and chord: each with the face (as the body lists its
/// faces) of every triangle, and when it was last used.
type Meshes = HashMap<(Key, u64), (Arc<TriMesh>, Arc<Vec<u32>>, u64)>;

/// A step the worker keeps.
struct Kept {
    /// Shared with identity placements. Kernel work that changes a body takes a private copy.
    body: Rc<Body>,
    used: u64,
    measure: Option<BrepMeasure>,
    entities: Option<(Vec<BrepEntity>, Vec<BrepEntity>)>,
    /// Its faces' names in the body's face order, once something asked ([`Worker::names_of`]).
    names: Option<Arc<Vec<Names>>>,
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
    /// Distinct allocations in `bodies`. Only kept keys own references between steps.
    body_count: usize,
    meshes: Meshes,
    clock: u64,
    /// The names of the first input of the step being built, when its selection names faces.
    input_names: Option<Arc<Vec<Names>>>,
    /// What the written selections of the step being built took.
    taken: Vec<BrepTaken>,
    /// What the step being built noted.
    note: Option<StepNote>,
}

/// The body's entity for each of a copy's: `copy` and `body` list them in the same order.
fn as_listed<T: Copy + Eq + std::hash::Hash>(copy: Vec<T>, body: Vec<T>) -> Result<impl Fn(T) -> Result<T, BrepError>, BrepError> {
    if copy.len() != body.len() {
        return Err(BrepError::Kernel("a copy lists other entities than its body".into()));
    }
    let of: HashMap<T, T> = copy.into_iter().zip(body).collect();
    Ok(move |id| of.get(&id).copied().ok_or_else(|| BrepError::Kernel("an entity the copy does not list".into())))
}

/// Body `body`'s mesh within `chord`, from `copy` (a copy of it), with the face of each
/// triangle as the body lists its faces.
fn mesh_of(k: &Kernel, copy: &mut Body, body: &Body, chord: f64) -> Result<(Arc<TriMesh>, Arc<Vec<u32>>), BrepError> {
    let t = tessellate(k, copy, &TessOptions { chord, ..TessOptions::default() })?;
    let listed = brep::p1::faces(copy, None)?;
    let face = as_listed(listed.clone(), brep::p1::faces(body, None)?)?;
    let of: HashMap<u32, u32> = listed.into_iter().map(|f| Ok((f.index() as u32, face(f)?.index() as u32))).collect::<Result<_, BrepError>>()?;
    let tri_face = t.tri_face.iter().map(|f| of.get(f).copied().unwrap_or(u32::MAX)).collect();
    Ok((Arc::new(t.mesh), Arc::new(tri_face)))
}

/// Body `body`'s (edges, faces) from `copy` (a copy of it), named as the body names them (`names`:
/// its faces' names in its face order, when they could be worked out).
fn entities_of(k: &Kernel, copy: &mut Body, body: &Body, names: Option<&Vec<Names>>) -> Result<(Vec<BrepEntity>, Vec<BrepEntity>), BrepError> {
    // The copy numbers its edges and faces afresh, and lists them in the body's order.
    let edge = as_listed(brep::p1::edges(copy, None)?, brep::p1::edges(body, None)?)?;
    let face = as_listed(brep::p1::faces(copy, None)?, brep::p1::faces(body, None)?)?;
    let edges = query::edges(k, copy)?
        .into_iter()
        .map(|e| {
            let points = match e.shape {
                EdgeShape::Line => vec![e.start, e.end],
                _ => query::sampled(k, copy, e.id, query::OUTLINE)?.unwrap_or(e.points),
            };
            Ok(BrepEntity {
                id: edge(e.id)?.index(),
                kind: shape_name(e.shape),
                size: e.length,
                at: e.mid,
                normal: None,
                convex: e.convex,
                smooth: e.smooth,
                radius: e.radius,
                names: Vec::new(),
                faces: names::edge_faces(copy, e.id)?.into_iter().map(|f| face(f).map(|f| f.index())).collect::<Result<_, _>>()?,
                points,
            })
        })
        .collect::<Result<Vec<BrepEntity>, BrepError>>()?;
    let face_order = names::faces_of(copy)?;
    let named = |f: FaceId| -> Vec<FaceName> {
        match (names, face_order.iter().position(|g| *g == f)) {
            (Some(n), Some(i)) => n.get(i).map(|s| s.iter().cloned().collect()).unwrap_or_default(),
            _ => Vec::new(),
        }
    };
    let faces = query::faces(k, copy)?
        .into_iter()
        .map(|f| Ok(BrepEntity { id: face(f.id)?.index(), kind: f.kind, size: f.area, at: f.centroid, normal: f.normal, convex: false, smooth: false, radius: None, names: named(f.id), faces: Vec::new(), points: Vec::new() }))
        .collect::<Result<Vec<BrepEntity>, BrepError>>()?;
    Ok((edges, faces))
}

/// What of body `key` is at `p`: the face of the nearest triangle of its meshes, and the sharp
/// edge within `reach` nearest it (`edges`: its entities) with its tangent chain. A smooth edge
/// shows no line, so the face is what is there.
fn at(meshes: &Meshes, key: &Key, edges: &[BrepEntity], p: [f64; 3], reach: f64) -> BrepAt {
    let found = meshes.iter().filter(|((k, _), _)| k == key).filter_map(|(_, (mesh, faces, _))| query::nearest_face(mesh, faces, p)).filter(|(f, _)| *f != u32::MAX).min_by(|a, b| a.1.total_cmp(&b.1));
    let edge = edges.iter().filter(|e| !e.smooth).map(|e| (e.id, names::polyline_distance(&e.points, p))).filter(|(_, d)| *d <= reach).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(id, _)| id);
    let chain = match edge {
        Some(e) => {
            let sampled: Vec<(u32, &[[f64; 3]])> = edges.iter().map(|e| (e.id, &e.points[..])).collect();
            names::chain(&sampled, &[e])
        }
        None => Vec::new(),
    };
    BrepAt { face: found.map(|f| f.0), off: found.map_or(f64::MAX, |f| f.1), edge, chain }
}

/// A step's kernel draws from its key: the same step draws the same.
fn seed(key: &Key) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&key[..8]);
    u64::from_le_bytes(word)
}

impl Worker {
    fn new(shared: Arc<Shared>) -> Worker {
        Worker { shared, failing: None, kernel: ops::new_kernel(), serial: 0, bodies: HashMap::new(), body_count: 0, meshes: HashMap::new(), clock: 0, input_names: None, taken: Vec::new(), note: None }
    }

    fn serve(&mut self, job: &Job) -> Served {
        if job.cancel.is_cancelled() {
            return Served::Answer(Err(BrepFailure { step: None, lost: None, message: BrepError::Cancelled.to_string() }));
        }
        let _permit = Permit::admit(job.class, &self.shared);
        self.failing = None;
        // A call that failed or unwound leaves no half-made body: a step's body is kept once
        // it is whole, and the next step or read renews the kernel it left.
        let answer = makepad_jobs::with_current(job.job.clone(), || guarded(stack_size(), &job.cancel, Some(checkpoint), || self.run(&job.steps, &job.want)));
        if GAVE_WAY.with(Cell::take) {
            return Served::GaveWay;
        }
        Served::Answer(answer.map_err(|e| BrepFailure { step: self.failing, lost: match &e { BrepError::Lost { index, .. } => Some(*index), _ => None }, message: e.to_string() }))
    }

    fn run(&mut self, steps: &[BrepStep], want: &Want) -> Result<Answer, BrepError> {
        let Some(last) = steps.last() else { return Err(BrepError::Input("an empty recipe".into())) };
        // What is left to build, by kind: the job says "fillets 3/7".
        let job = if matches!(want, Want::Mesh(chord) if *chord < PRINT_DETAIL) { None } else { makepad_jobs::current() };
        let mut total = 0usize;
        let mut kinds: HashMap<&'static str, (u32, u32)> = HashMap::new();
        for s in steps.iter().filter(|s| !self.bodies.contains_key(&s.key)) {
            if let Some(kind) = op_kind(&s.op) {
                kinds.entry(kind).or_default().1 += 1;
                total += 1;
            }
        }
        let mut built = 0usize;
        for step in steps {
            self.clock += 1;
            if let Some(b) = self.bodies.get_mut(&step.key) {
                b.used = self.clock;
                continue;
            }
            checkpoint();
            if let (Some(job), Some(kind)) = (&job, op_kind(&step.op)) {
                let count = kinds.entry(kind).or_default();
                count.0 += 1;
                let (i, n) = *count;
                job.stage(kind, (n > 1).then_some((i, n)), Some(built as f32 / total.max(1) as f32));
                built += 1;
            }
            self.failing = Some(step.key);
            // A selection that names faces reads its input's names, built first on their own kernel.
            self.input_names = match (names_by(&step.op), step.inputs.first()) {
                (true, Some(input)) => Some(self.names_of(steps, input)?),
                _ => None,
            };
            self.taken.clear();
            self.note = None;
            let body = self.step(step)?;
            if let Some(note) = self.note.take() {
                self.shared.note(step.key, Some(note));
            }
            self.input_names = None;
            self.failing = None;
            if Rc::strong_count(&body) == 1 {
                self.body_count += 1;
            }
            self.bodies.insert(step.key, Kept { body, used: self.clock, measure: None, entities: None, names: None });
            self.shared.held().insert(step.key, std::mem::take(&mut self.taken));
        }
        // A tool reading the entities reads the faces' names too: built while every step's body
        // of the recipe is still kept. Names that cannot be worked out leave the listing without
        // them, not failed.
        let entity_names = match want {
            Want::Entities | Want::At(..) if self.bodies.get(&last.key).is_some_and(|k| k.entities.is_none()) => match caught(|| self.names_of(steps, &last.key)) {
                Ok(names) => Some(names),
                Err(BrepError::Cancelled) => return Err(BrepError::Cancelled),
                Err(e) => {
                    eprintln!("[brep] the faces' names: {e}; the entities are listed without them");
                    None
                }
            },
            _ => None,
        };
        if self.body_count > KEEP_BODIES {
            let committed = self.shared.committed().clone();
            let mut warned = false;
            while self.body_count > KEEP_BODIES {
                let oldest = self.bodies.iter()
                    .filter(|(key, _)| *key != &last.key)
                    .min_by_key(|(key, kept)| (committed.contains(*key), kept.used));
                let Some((&key, kept)) = oldest else { break };
                if !warned && committed.contains(&key) {
                    eprintln!("[brep] the committed build and current answer exceed the {KEEP_BODIES}-body budget; evicting committed steps");
                    warned = true;
                }
                if Rc::strong_count(&kept.body) == 1 {
                    self.body_count -= 1;
                }
                self.bodies.remove(&key);
                self.shared.held().remove(&key);
            }
        }
        let Worker { kernel, bodies, meshes, clock, .. } = self;
        let kept = bodies.get_mut(&last.key).ok_or_else(|| BrepError::Input("a recipe's body is gone".into()))?;
        let meshed = |meshes: &Meshes| meshes.keys().any(|(k, _)| *k == last.key);
        match want {
            Want::Mesh(chord) => {
                if let Some(m) = meshes.get_mut(&(last.key, chord.to_bits())) {
                    m.2 = *clock;
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
            Want::At(p, reach) => {
                if let (Some((edges, _)), true) = (&kept.entities, meshed(meshes)) {
                    return Ok(Answer::At(at(meshes, &last.key, edges, *p, *reach)));
                }
            }
        }
        checkpoint();
        // A read of its own, like a step: what it works out reaches no other.
        *kernel = kernel.renewed(seed(&last.key));
        let copy = &mut ops::copy_body(kernel, &kept.body, 1)?;
        let k = &*kernel;
        let keep = |meshes: &mut Meshes, chord: f64, mesh: (Arc<TriMesh>, Arc<Vec<u32>>)| {
            meshes.insert((last.key, chord.to_bits()), (mesh.0, mesh.1, *clock));
            while meshes.len() > KEEP_MESHES {
                let oldest = meshes.iter().min_by_key(|(_, (_, _, used))| *used).map(|(k, _)| *k);
                if let Some(k) = oldest {
                    meshes.remove(&k);
                }
            }
        };
        let answer = match want {
            Want::Mesh(chord) => {
                if let Some(job) = &job {
                    job.stage("meshing", None, None);
                }
                let mesh = mesh_of(k, copy, &kept.body, *chord)?;
                let out = mesh.0.clone();
                keep(meshes, *chord, mesh);
                Answer::Mesh(out)
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
                let (edges, faces) = entities_of(k, copy, &kept.body, entity_names.as_deref())?;
                kept.entities = Some((edges.clone(), faces.clone()));
                Answer::Entities(edges, faces)
            }
            Want::At(p, reach) => {
                if kept.entities.is_none() {
                    kept.entities = Some(entities_of(k, copy, &kept.body, entity_names.as_deref())?);
                }
                // A point is found on a mesh of the body: the one the view shows when there is one.
                if !meshed(meshes) {
                    let mesh = mesh_of(k, copy, &kept.body, AT_CHORD)?;
                    keep(meshes, AT_CHORD, mesh);
                }
                let edges = kept.entities.as_ref().map(|e| &e.0[..]).unwrap_or_default();
                Answer::At(at(meshes, &last.key, edges, *p, *reach))
            }
        };
        Ok(answer)
    }

    /// The names of a kept step's faces, in its body's face order (`steps`: the recipe it is in).
    /// Built once and kept with the body, on a kernel of their own: never the one a step is
    /// being built in, whose input copies live there. Every body of the recipe is kept while it
    /// runs.
    fn names_of(&mut self, steps: &[BrepStep], key: &Key) -> Result<Arc<Vec<Names>>, BrepError> {
        if let Some(n) = self.bodies.get(key).and_then(|k| k.names.clone()) {
            return Ok(n);
        }
        let step = steps.iter().find(|s| s.key == *key).ok_or_else(|| BrepError::Input("names: a step outside its recipe".into()))?;
        let in_names: Vec<Arc<Vec<Names>>> = step.inputs.iter().map(|i| self.names_of(steps, i)).collect::<Result<_, _>>()?;
        checkpoint();
        fn body<'a>(w: &'a Worker, key: &Key) -> Result<&'a Body, BrepError> {
            w.bodies.get(key).map(|k| k.body.as_ref()).ok_or_else(|| BrepError::Input("names: a step's body is gone".into()))
        }
        let mut k = self.kernel.renewed(seed(key));
        let mut out = ops::copy_body(&mut k, body(self, key)?, 1)?;
        let names = match &step.op {
            BrepOp::Box { .. } | BrepOp::Sphere { .. } | BrepOp::Cylinder { .. } | BrepOp::Torus { .. } | BrepOp::Extrude { .. } | BrepOp::Revolve { .. } | BrepOp::Sweep { .. } | BrepOp::Loft { .. } => {
                names::classify(&k, &mut out, *key, &step.op)?
            }
            // The faces keep their order and names. The placement also names its input's own roles.
            BrepOp::Place { .. } | BrepOp::Mirror { keep: false, .. } => {
                let input = in_names.first().ok_or_else(|| BrepError::Input("names: a step without its input".into()))?;
                if names::faces_of(&out)?.len() != input.len() {
                    return Err(BrepError::Kernel("names: a moved body lists other faces than its input".into()));
                }
                let input_key = step.inputs[0];
                let mut moved = (**input).clone();
                for (before, after) in input.iter().zip(&mut moved) {
                    for name in before {
                        if name.origin == input_key {
                            after.insert(FaceName { origin: *key, role: name.role.clone() });
                        }
                    }
                }
                moved
            }
            op => {
                let mut inputs = Vec::new();
                let mut lists: Vec<Vec<Names>> = Vec::new();
                for (n, i) in step.inputs.iter().enumerate() {
                    inputs.push(ops::copy_body(&mut k, body(self, i)?, 2 + n as u64)?);
                    lists.push((*in_names[n]).clone());
                }
                let picked: Vec<names::Picked>;
                let mut rec = names::Step { key: *key, offset: 0.0, shell: 0.0, blend: None };
                match op {
                    // The image keeps the original's names: both sides answer them.
                    BrepOp::Mirror { plane, center, .. } => {
                        let mut image = ops::copy_body(&mut k, body(self, &step.inputs[0])?, 2 + step.inputs.len() as u64)?;
                        ops::mirrored(&mut k, &mut image, *center, mirror_normal(*plane))?;
                        inputs.push(image);
                        lists.push((*in_names[0]).clone());
                    }
                    BrepOp::Fillet { radius: size, edges } | BrepOp::Chamfer { size, edges } => {
                        let ids = select_edges(&k, &mut inputs[0], edges, Some(&lists[0]), &mut Vec::new())?;
                        picked = names::picked(&k, &mut inputs[0], &lists[0], &ids)?;
                        rec.blend = Some((&picked[..], *size, matches!(op, BrepOp::Fillet { .. })));
                    }
                    BrepOp::Shell { thickness, .. } => rec.shell = *thickness,
                    BrepOp::Offset { distance } => rec.offset = *distance,
                    // The moved face lies on its prism's far cap: the cap carries the pulled
                    // face's names; the prism's sides are the pull's own and go unnamed (where
                    // they lie on a face beside it, that face's names hold).
                    BrepOp::Pull { face, by } => {
                        let found = names::faces_named(&lists[0], face);
                        let order = names::faces_of(&inputs[0])?;
                        let faces: Vec<FaceId> = found.iter().map(|i| order[*i]).collect();
                        let pulled: Names = found.iter().flat_map(|i| lists[0][*i].iter().cloned()).collect();
                        let prisms = ops::pull_prisms(&mut k, &mut inputs[0], &faces, *by)?;
                        for (mut prism, normal, on) in prisms {
                            lists.push(names::pull_caps(&k, &mut prism, normal, on, *by, &pulled)?);
                            inputs.push(prism);
                        }
                    }
                    BrepOp::Hole { diameter, depth, at, dir, on, .. } => {
                        let (points, axis) = hole_starts(&k, &mut inputs[0], Some(&lists[0]), on.as_ref(), at, *dir, None)?;
                        let shafts = ops::hole_cutters(&mut k, &inputs[0], HoleKind::Simple, *diameter, *depth, &points, axis)?;
                        for mut shaft in shafts {
                            lists.push(names::hole_faces(&k, &mut shaft, *key, axis, depth.is_some())?);
                            inputs.push(shaft);
                        }
                    }
                    // The cut's faces lie on the slab's face on the plane: the slab's names, `cut`.
                    // A pin's faces, and a hole's, are the split's own and go unnamed.
                    BrepOp::Split { center, normal, pins, .. } => {
                        let plan = split_plan(&mut k, body(self, &step.inputs[0])?, 2 + step.inputs.len() as u64, *center, *normal, pins)?;
                        let slab = split_slab(&mut k, &plan)?;
                        let cut = Names::from([FaceName { origin: *key, role: Role::Named(Face::Cut) }]);
                        lists.push(vec![cut; names::faces_of(&slab)?.len()]);
                        inputs.push(slab);
                    }
                    _ => {}
                }
                names::carry(&k, &mut out, &mut inputs, &lists, &rec)?
            }
        };
        let names = Arc::new(names);
        if let Some(kept) = self.bodies.get_mut(key) {
            kept.names = Some(names.clone());
        }
        Ok(names)
    }

    /// A private copy of an input, in a key domain of its own within the step.
    fn input(&mut self, key: &Key) -> Result<Body, BrepError> {
        self.serial += 1;
        let kept = self.bodies.get(key).ok_or_else(|| BrepError::Input("a step's input is missing".into()))?;
        ops::copy_body(&mut self.kernel, &kept.body, self.serial)
    }

    fn step(&mut self, step: &BrepStep) -> Result<Rc<Body>, BrepError> {
        let body = if identity_place(&step.op) {
            match step.inputs.first() {
                Some(key) => self.bodies.get(key)
                    .map(|kept| Rc::clone(&kept.body))
                    .ok_or_else(|| BrepError::Input("a step's input is missing".into())),
                None => Err(BrepError::Input("needs a body".into())),
            }
        } else {
            // A kernel of its own: what the step builds depends on its inputs alone.
            self.kernel = self.kernel.renewed(seed(&step.key));
            self.serial = 0;
            self.build(step).map(Rc::new)
        };
        let name = op_name(&step.op);
        body.map_err(|e| match e {
            BrepError::Kernel(m) => BrepError::Kernel(format!("{name}: {m}")),
            BrepError::Input(m) => BrepError::Input(format!("{name}: {m}")),
            BrepError::Lost { index, message } => BrepError::Lost { index, message: format!("{name}: {message}") },
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
                let picked = select_edges(&self.kernel, &mut b, edges, self.input_names.as_deref().map(|n| &n[..]), &mut self.taken)?;
                ops::fillet(&mut self.kernel, &mut b, &picked, *radius)?;
                Ok(b)
            }
            BrepOp::Chamfer { size, edges } => {
                let mut b = first(self)?;
                let picked = select_edges(&self.kernel, &mut b, edges, self.input_names.as_deref().map(|n| &n[..]), &mut self.taken)?;
                ops::chamfer(&mut self.kernel, &mut b, &picked, *size)?;
                Ok(b)
            }
            BrepOp::Shell { thickness, open } => {
                let mut b = first(self)?;
                let faces = open_faces(&self.kernel, &mut b, open, self.input_names.as_deref().map(|n| &n[..]), &mut self.taken)?;
                ops::shell(&mut self.kernel, &b, *thickness, &faces)
            }
            BrepOp::Offset { distance } => {
                let mut b = first(self)?;
                ops::offset(&mut self.kernel, &mut b, *distance)
            }
            BrepOp::Pull { face, by } => {
                let mut b = first(self)?;
                let faces = select_faces(&self.kernel, &mut b, self.input_names.as_deref().map(Vec::as_slice), face, "pull", Some(&mut self.taken))?;
                ops::pull(&mut self.kernel, b, &faces, *by)
            }
            BrepOp::Hole { kind, diameter, depth, at, dir, on } => {
                let mut b = first(self)?;
                let (points, dir) = hole_starts(&self.kernel, &mut b, self.input_names.as_deref().map(Vec::as_slice), on.as_ref(), at, *dir, Some(&mut self.taken))?;
                ops::hole(&mut self.kernel, b, *kind, *diameter, *depth, &points, dir)
            }
            BrepOp::Place { at, rotate, scale } => {
                let mut b = first(self)?;
                if *scale != [1.0; 3] {
                    ops::scale(&mut self.kernel, &mut b, *scale)?;
                }
                ops::place(&mut self.kernel, &mut b, *at, *rotate)?;
                Ok(b)
            }
            BrepOp::Mirror { plane, center, keep } => {
                let mut image = first(self)?;
                ops::mirrored(&mut self.kernel, &mut image, *center, mirror_normal(*plane))?;
                if !*keep {
                    return Ok(image);
                }
                let original = first(self)?;
                ops::boolean(&mut self.kernel, ops::BoolMode::Union, original, image)
            }
            BrepOp::Split { center, normal, piece, pins } => {
                let key = step.inputs.first().ok_or_else(|| BrepError::Input("needs a body".into()))?;
                self.serial += 1;
                let input = &self.bodies.get(key).ok_or_else(|| BrepError::Input("a step's input is missing".into()))?.body;
                let plan = split_plan(&mut self.kernel, input, self.serial, *center, *normal, pins)?;
                if *piece == SplitPiece::Pins && plan.spots.is_empty() {
                    return Err(BrepError::Input("no place on the cut has room for a pin: a smaller 'pin' radius fits a thinner cut".into()));
                }
                self.note = plan.no_room.map(StepNote::SplitWithoutPins);
                let mut out = match piece {
                    SplitPiece::Below | SplitPiece::Above => {
                        let b = first(self)?;
                        let slab = split_slab(&mut self.kernel, &plan)?;
                        let mode = if *piece == SplitPiece::Below { ops::BoolMode::Difference } else { ops::BoolMode::Intersection };
                        Some(ops::boolean(&mut self.kernel, mode, b, slab)?)
                    }
                    SplitPiece::Pins => None,
                };
                let (gain, lose) = plan.rods(*piece);
                for rod in &gain {
                    let r = split_rod(&mut self.kernel, rod)?;
                    out = Some(match out {
                        Some(o) => ops::boolean(&mut self.kernel, ops::BoolMode::Union, o, r)?,
                        None => r,
                    });
                }
                let mut out = out.ok_or_else(|| BrepError::Input("a split's pins piece needs pins".into()))?;
                for rod in &lose {
                    let r = split_rod(&mut self.kernel, rod)?;
                    out = ops::boolean(&mut self.kernel, ops::BoolMode::Difference, out, r)?;
                }
                Ok(out)
            }
        }
    }
}

/// A split's plan, from a mesh of its input (`body`, copied into key domain `serial`): every
/// piece of the split, and the names of each, plan on the same mesh. O(the input's faces) to
/// mesh, then the plan's section and pin search.
fn split_plan(k: &mut Kernel, body: &Body, serial: u64, center: [f64; 3], normal: [f64; 3], pins: &SplitPins) -> Result<split::Plan, BrepError> {
    let mut copy = ops::copy_body(k, body, serial)?;
    let t = tessellate(k, &mut copy, &TessOptions { chord: SPLIT_CHORD, ..TessOptions::default() })?;
    split::plan(&t.mesh, center, normal, pins).map_err(BrepError::Input)
}

/// The cube on the plan's normal side, one face on its plane.
fn split_slab(k: &mut Kernel, plan: &split::Plan) -> Result<Body, BrepError> {
    let [u, n, _] = plan.turn();
    let mut b = ops::make_box(k, [plan.slab; 3])?;
    let c = plan.slab_center;
    ops::place_frame(k, &mut b, [c.x, c.y, c.z], [u.x, u.y, u.z], [n.x, n.y, n.z])?;
    Ok(b)
}

/// A pin's or a hole's rod.
fn split_rod(k: &mut Kernel, rod: &split::Rod) -> Result<Body, BrepError> {
    let n = (rod.b - rod.a).normalize();
    let u = if n.y.abs() < 0.9 { makepad_csg_math::dvec3(0.0, 1.0, 0.0).cross(n).normalize() } else { makepad_csg_math::dvec3(1.0, 0.0, 0.0).cross(n).normalize() };
    // the cylinder stands along Y: Y to the rod, X square to it
    let mut b = ops::make_cylinder(k, rod.radius, rod.length())?;
    let c = rod.middle();
    ops::place_frame(k, &mut b, [c.x, c.y, c.z], [u.x, u.y, u.z], [n.x, n.y, n.z])?;
    Ok(b)
}

/// The pieces left of a named face, recording the selection when building the body.
fn select_faces(k: &Kernel, b: &mut Body, names: Option<&[Names]>, face: &FaceName, verb: &str, taken: Option<&mut Vec<BrepTaken>>) -> Result<Vec<FaceId>, BrepError> {
    let names = names.ok_or_else(|| BrepError::Input(format!("the face to {verb} has no names to be found by")))?;
    let order = names::faces_of(b)?;
    let faces: Vec<_> = names::faces_named(names, face).into_iter().map(|i| order[i]).collect();
    if faces.is_empty() {
        return Err(BrepError::Lost { index: 0, message: format!("no face named {} is left on this body to {verb}", names::describe(face)) });
    }
    if let Some(taken) = taken {
        let mut kinds = Vec::with_capacity(faces.len());
        for &f in &faces {
            kinds.push(crate::tess::face_kind(k, b, f)?.0);
        }
        kinds.sort_unstable();
        taken.push(BrepTaken { named: names::describe(face), kinds });
    }
    Ok(faces)
}

/// A named face supplies one starting plane even when the face has split into pieces.
fn hole_starts<'a>(
    k: &Kernel, b: &mut Body, names: Option<&[Names]>, on: Option<&FaceName>,
    at: &'a [[f64; 3]], dir: Option<[f64; 3]>, taken: Option<&mut Vec<BrepTaken>>,
) -> Result<(Cow<'a, [[f64; 3]]>, [f64; 3]), BrepError> {
    let Some(face) = on else { return Ok((Cow::Borrowed(at), dir.unwrap_or([0.0, -1.0, 0.0]))) };
    let faces = select_faces(k, b, names, face, "drill", taken)?;
    let mut plane = |f| {
        let (_, n) = crate::tess::face_kind(k, b, f)?;
        let n = n.ok_or_else(|| BrepError::Input("a hole starts on a flat face: the selected face is curved".into()))?;
        Ok::<_, BrepError>((face_internal_point(k, b, f)?, Vec3::from_array(n)))
    };
    let (origin, normal) = plane(faces[0])?;
    for &f in &faces[1..] {
        let (p, n) = plane(f)?;
        let coplanar = normal.cross(n).length() <= EFF_ZERO && (p - origin).dot(normal).abs() <= k.tolerances.zone_tol_3d;
        if !coplanar || (dir.is_none() && n.dot(normal) < 0.0) {
            return Err(BrepError::Input("the named faces do not give this hole one starting plane and direction".into()));
        }
    }
    let axis = ops::drill_direction(dir.unwrap_or((-normal).to_array()))?;
    let along = axis.dot(normal);
    if along.abs() <= EFF_ZERO {
        return Err(BrepError::Input("the hole direction is parallel to its starting face".into()));
    }
    let points = at.iter().map(|p| {
        let p = Point3::from_array(*p);
        (p + axis * ((origin - p).dot(normal) / along)).to_array()
    }).collect();
    Ok((Cow::Owned(points), axis.to_array()))
}

fn mirror_normal(plane: MirrorPlane) -> [f64; 3] {
    match plane {
        MirrorPlane::Yz => [1.0, 0.0, 0.0],
        MirrorPlane::Xz => [0.0, 1.0, 0.0],
        MirrorPlane::Xy => [0.0, 0.0, 1.0],
    }
}

/// The step's selection names faces: its first input's names are built before it.
fn names_by(op: &BrepOp) -> bool {
    match op {
        BrepOp::Fillet { edges, .. } | BrepOp::Chamfer { edges, .. } => !edges.named.is_empty(),
        BrepOp::Shell { open: Open::Faces(_), .. } => true,
        BrepOp::Pull { .. } | BrepOp::Hole { on: Some(_), .. } => true,
        _ => false,
    }
}

fn identity_place(op: &BrepOp) -> bool {
    matches!(op, BrepOp::Place { at, rotate, scale } if *at == [0.0; 3] && *rotate == [0.0; 3] && *scale == [1.0; 3])
}

/// A step's visible job stage. An identity placement shares an existing body.
fn op_kind(op: &BrepOp) -> Option<&'static str> {
    Some(match op {
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
        BrepOp::Pull { .. } => "pulls",
        BrepOp::Hole { .. } => "drills",
        BrepOp::Place { .. } if identity_place(op) => return None,
        BrepOp::Place { .. } => "placements",
        BrepOp::Mirror { .. } => "mirrors",
        BrepOp::Split { .. } => "splits",
    })
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
        BrepOp::Pull { .. } => "pull",
        BrepOp::Hole { .. } => "hole",
        BrepOp::Place { .. } => "placement",
        BrepOp::Mirror { .. } => "mirror",
        BrepOp::Split { .. } => "split",
    }
}

fn shape_name(s: EdgeShape) -> &'static str {
    match s {
        EdgeShape::Line => "line",
        EdgeShape::Circle => "circle",
        EdgeShape::Curve => "curve",
    }
}

/// The faces a shell leaves open: the flat ones at the body's top or bottom, or the faces named
/// (what each name took goes in `taken`).
fn open_faces(k: &Kernel, b: &mut Body, open: &Open, names: Option<&[Names]>, taken: &mut Vec<BrepTaken>) -> Result<Vec<FaceId>, BrepError> {
    let want = match open {
        Open::Closed => return Ok(Vec::new()),
        Open::Top => 1.0,
        Open::Bottom => -1.0,
        Open::Faces(list) => {
            let names = names.ok_or_else(|| BrepError::Input("the faces to leave open have no names to be found by".into()))?;
            let order = names::faces_of(b)?;
            let mut out = Vec::new();
            for (index, n) in list.iter().enumerate() {
                let found = names::faces_named(names, n);
                if found.is_empty() {
                    return Err(BrepError::Lost { index, message: format!("no face named {} is left on this body to leave open", names::describe(n)) });
                }
                let mut kinds = Vec::with_capacity(found.len());
                for i in &found {
                    kinds.push(crate::tess::face_kind(k, b, order[*i])?.0);
                }
                kinds.sort_unstable();
                taken.push(BrepTaken { named: names::describe(n), kinds });
                for i in found {
                    if !out.contains(&order[i]) {
                        out.push(order[i]);
                    }
                }
            }
            return Ok(out);
        }
    };
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

/// The edges of `b` that `sel` picks: by its rules among the sharp edges, and by its names (a
/// body's face names, `names`, in its face order) among all. What each name took goes in `taken`.
pub fn select_edges(k: &Kernel, b: &mut Body, sel: &EdgeSel, names: Option<&[Names]>, taken: &mut Vec<BrepTaken>) -> Result<Vec<makepad_csg_brep::EdgeId>, BrepError> {
    let all = query::edges(k, b)?;
    let mut out = Vec::new();
    for (index, n) in sel.named.iter().enumerate() {
        let names = names.ok_or_else(|| BrepError::Input("edges named by their faces need the body's names".into()))?;
        let mut pieces = names::edges_named(b, names, &n.a, &n.b)?;
        if pieces.is_empty() {
            return Err(BrepError::Lost { index, message: format!("no edge between {} and {} is left on this body (the faces no longer meet)", names::describe(&n.a), names::describe(&n.b)) });
        }
        if let Some(q) = n.near {
            let d = |id: &makepad_csg_brep::EdgeId| all.iter().find(|e| e.id == *id).map_or(f64::MAX, |e| e.points.iter().map(|p| (0..3).map(|i| (p[i] - q[i]).powi(2)).sum::<f64>()).fold(f64::MAX, f64::min));
            pieces.sort_by(|a, b| d(a).total_cmp(&d(b)));
            pieces.truncate(1);
        }
        if n.chain {
            let sampled: Vec<_> = all.iter().map(|e| (e.id, &e.points[..])).collect();
            pieces = names::chain(&sampled, &pieces);
        }
        let mut kinds: Vec<&'static str> = pieces.iter().filter_map(|id| all.iter().find(|e| e.id == *id)).map(|e| shape_name(e.shape)).collect();
        kinds.sort_unstable();
        let how = if n.chain { " chain" } else if n.near.is_some() { " near" } else { "" };
        taken.push(BrepTaken { named: format!("edge({}, {}){how}", names::describe(&n.a), names::describe(&n.b)), kinds });
        for e in pieces {
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    let edges: Vec<EdgeInfo> = all.into_iter().filter(|e| !e.smooth).collect();
    if sel.is_all() {
        return Ok(edges.iter().map(|e| e.id).collect());
    }
    let ys = edges.iter().flat_map(|e| e.points.iter().map(|p| p[1]));
    let (lo, hi) = ys.fold((f64::MAX, f64::MIN), |(l, h), y| (l.min(y), h.max(y)));
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
