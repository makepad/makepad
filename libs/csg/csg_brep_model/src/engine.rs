//! Exact bodies for the solid graph, on a thread of their own.
//!
//! The kernel recurses deeply, so long-lived workers with big stacks run every job: the
//! caller never does kernel work and can cancel. Bodies stay on the worker, by key, so an
//! edit rebuilds only the steps it changed. Display and print detail have a worker each: a
//! slice's fine tessellation never holds up a drag. A worker starts at its first body: a
//! document without B-rep parts never pays for one.

use crate::guard::{guarded, stack_size, BrepError, CancelToken};
use crate::ops::{self, Body};
use crate::profile::Frame;
use crate::query::{self, EdgeInfo, EdgeShape};
use crate::tess::{tessellate, TessOptions};
use makepad_csg_brep::kernel::Kernel;
use makepad_csg_brep::topology::FaceId;
use makepad_csg_graph::edges::{EdgeFilter, EdgeKind, EdgeSel};
use makepad_csg_graph::eval::plane_frame;
use makepad_csg_graph::{BoolMode, BrepEngine, BrepEntity, BrepFailure, BrepMeasure, BrepOp, BrepStep, Key, Open, TriMesh};
use std::collections::HashMap;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// How many bodies stay on a worker, least recently used out first: a drag revisits few.
const KEEP_BODIES: usize = 64;
/// How many meshes stay on a worker, likewise.
const KEEP_MESHES: usize = 16;
/// Chords below this are print detail (mm).
const PRINT_DETAIL: f64 = 0.02;

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
    reply: Sender<Result<Answer, BrepFailure>>,
}

/// The process's one engine. The thread starts here, at the first body.
pub fn engine() -> Arc<dyn BrepEngine> {
    static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
    ENGINE.get_or_init(|| Arc::new(Engine::default())).clone()
}

/// See [`engine`].
#[derive(Default)]
pub struct Engine {
    display: Lane,
    print: Lane,
}

#[derive(Default)]
struct Lane {
    jobs: OnceLock<std::sync::Mutex<Sender<Job>>>,
}

impl Lane {
    fn send(&self, job: Job) -> Result<(), String> {
        let jobs = self.jobs.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            let spawned = std::thread::Builder::new().name("brep".into()).stack_size(stack_size()).spawn(move || {
                let mut w = Worker::new();
                while let Ok(job) = rx.recv() {
                    w.failing = None;
                    let answer = makepad_jobs::with_current(job.job.clone(), || guarded(stack_size(), &job.cancel, || w.run(&job.steps, &job.want)));
                    let step = w.failing;
                    if matches!(answer, Err(BrepError::Internal(_))) {
                        // An unwound kernel may hold half-made state: start clean.
                        w = Worker::new();
                    }
                    let _ = job.reply.send(answer.map_err(|e| BrepFailure { step, message: e.to_string() }));
                }
            });
            if let Err(e) = spawned {
                eprintln!("[brep] the worker thread did not start: {e}");
            }
            std::sync::Mutex::new(tx)
        });
        jobs.lock().map_err(|_| "the B-rep worker is gone".to_string())?.send(job).map_err(|_| "the B-rep worker is gone".to_string())
    }
}

impl Engine {
    fn ask(&self, steps: Vec<BrepStep>, want: Want, cancelled: Option<&dyn Fn() -> bool>) -> Result<Answer, BrepFailure> {
        let (reply, answer) = channel();
        let cancel = CancelToken::new();
        let lane = if matches!(want, Want::Mesh(chord) if chord < PRINT_DETAIL) { &self.print } else { &self.display };
        lane.send(Job { steps, want, cancel: cancel.clone(), job: makepad_jobs::current(), reply }).map_err(|message| BrepFailure { step: None, message })?;
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
}

struct Worker {
    /// The step being built: a failure belongs to it.
    failing: Option<Key>,
    kernel: Kernel,
    bodies: HashMap<Key, (Body, u64)>,
    meshes: HashMap<(Key, u64), (Arc<TriMesh>, u64)>,
    clock: u64,
    serial: u64,
}

impl Worker {
    fn new() -> Worker {
        Worker { failing: None, kernel: ops::new_kernel(), bodies: HashMap::new(), meshes: HashMap::new(), clock: 0, serial: 0 }
    }

    fn run(&mut self, steps: &[BrepStep], want: &Want) -> Result<Answer, BrepError> {
        let Some(last) = steps.last() else { return Err(BrepError::Input("an empty recipe".into())) };
        // What is left to build, by kind: the job says "fillets 3/7". The print-detail lane runs
        // beside the display one for the same job: only the display lane speaks for it (the two
        // would take turns on its one stage line).
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
                b.1 = self.clock;
                continue;
            }
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
            self.bodies.insert(step.key, (body, self.clock));
        }
        while self.bodies.len() > KEEP_BODIES {
            let oldest = self.bodies.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| *k);
            if let Some(k) = oldest {
                self.bodies.remove(&k);
            }
        }
        if let Want::Mesh(chord) = want {
            if let Some(m) = self.meshes.get_mut(&(last.key, chord.to_bits())) {
                m.1 = self.clock;
                return Ok(Answer::Mesh(m.0.clone()));
            }
        }
        let k = &self.kernel;
        let (body, _) = self.bodies.get_mut(&last.key).ok_or_else(|| BrepError::Input("a recipe's body is gone".into()))?;
        Ok(match want {
            Want::Mesh(chord) => {
                if let Some(job) = &job {
                    job.stage("meshing", None, None);
                }
                let mesh = Arc::new(tessellate(k, body, &TessOptions { chord: *chord, ..TessOptions::default() })?.mesh);
                self.meshes.insert((last.key, chord.to_bits()), (mesh.clone(), self.clock));
                while self.meshes.len() > KEEP_MESHES {
                    let oldest = self.meshes.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| *k);
                    if let Some(k) = oldest {
                        self.meshes.remove(&k);
                    }
                }
                Answer::Mesh(mesh)
            }
            Want::Measure => {
                if let Some(job) = &job {
                    job.stage("measuring", None, None);
                }
                let m = query::measure(k, body)?;
                Answer::Measure(BrepMeasure { volume: m.volume, area: m.area, centroid: m.centroid, min: m.min, max: m.max, faces: m.faces, edges: m.edges })
            }
            Want::Entities => {
                let edges = query::edges(k, body)?
                    .into_iter()
                    .map(|e| BrepEntity {
                        id: e.id.index() as u32,
                        kind: shape_name(e.shape),
                        size: e.length,
                        at: e.mid,
                        normal: None,
                        convex: e.convex,
                        smooth: e.smooth,
                        radius: e.radius,
                    })
                    .collect();
                let faces = query::faces(k, body)?
                    .into_iter()
                    .map(|f| BrepEntity { id: f.id.index() as u32, kind: f.kind, size: f.area, at: f.centroid, normal: f.normal, convex: false, smooth: false, radius: None })
                    .collect();
                Answer::Entities(edges, faces)
            }
        })
    }

    /// A private copy of an input.
    fn input(&mut self, key: &Key) -> Result<Body, BrepError> {
        self.serial += 1;
        let serial = self.serial;
        let (body, _) = self.bodies.get(key).ok_or_else(|| BrepError::Input("a step's input is missing".into()))?;
        ops::copy_body(&mut self.kernel, body, serial)
    }

    fn step(&mut self, step: &BrepStep) -> Result<Body, BrepError> {
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
            BrepOp::Sweep { sketch, path } => {
                let b = ops::sweep(&mut self.kernel, sketch, path)?;
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
