//! What a document declares (`Physics{}`, `Body{}`, `Spawn{}`, `Sim{}`),
//! read by the host's document reader into plain values. Kernels arrive
//! compiled (the host compiles them with [`crate::layouts`] and its own
//! modules) together with a hash of their program text for the
//! checkpoints' version stamp.

use crate::layouts::BodyRecord;
use crate::time::{SimTime, StepDt};
use makepad_render_kernels::compute::kernel::{Kernel, Layout};
use std::sync::Arc;

/// A collider.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A box `size` across.
    Box,
    /// A ball `max(size)` across.
    Sphere,
    /// A capsule along y, `size.x` across and `size.y` tall.
    Capsule,
    /// An endless floor through the body's position, facing +y (a thick
    /// slab 2000 across whose top is at `pos.y`).
    Plane,
    /// The convex hull of points (body-local).
    Hull(Arc<Vec<[f32; 3]>>),
    /// A triangle mesh (body-local; static or kinematic bodies).
    Mesh { vertices: Arc<Vec<[f32; 3]>>, indices: Arc<Vec<u32>> },
    /// A height grid `count_x` × `count_z` (row-major along x), `cell`
    /// apart, from the body's position along +x and +z.
    HeightField { heights: Arc<Vec<f32>>, count_x: u32, count_z: u32, cell: [f32; 2] },
}

/// How a declared body moves.
#[derive(Clone, Debug, PartialEq)]
pub enum Motion {
    /// Never moves (floors, walls, colliders from generated geometry).
    Fixed,
    /// Moved by the solver.
    Dynamic,
    /// Follows a pose the host evaluates at each step's time (animated
    /// geometry: `Drive::pose(key, t)`), pushing dynamic bodies.
    Kinematic { key: String },
}

/// A body the document declares by name.
#[derive(Clone, Debug)]
pub struct BodyDesc {
    pub name: String,
    /// The group `bodies("…")` reads; the name when empty.
    pub group: String,
    pub shape: Shape,
    pub motion: Motion,
    /// Pose, size, velocity, material and render fields.
    pub record: BodyRecord,
}

/// Where a spawn's bodies come from.
#[derive(Clone, Debug)]
pub enum SpawnFrom {
    /// Records given in place (or read by the host from a document list).
    Records(Arc<Vec<BodyRecord>>),
    /// A kernel run once at the spawn's time: its `output(Body)` (one
    /// record per element) or `emit_buffer(Body, n)` (what it emits)
    /// buffer `buffer`, with `count` elements.
    Kernel(KernelCall),
}

/// A kernel as the stepper runs it.
#[derive(Clone)]
pub struct KernelCall {
    pub kernel: Arc<Kernel>,
    /// The buffer the stepper reads back (a spawn's Body records, a sim's
    /// state).
    pub buffer: String,
    /// Params by name (values; keyable params are evaluated by the host
    /// before it builds the description).
    pub params: Vec<(String, f32)>,
    /// Buffers bound from the sim's state at run time.
    pub inputs: Vec<(String, SimInput)>,
    /// A hash of the kernel's program text and layouts (the version stamp).
    pub program: u64,
}

impl std::fmt::Debug for KernelCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KernelCall({:?} -> {}, program {:016x})", self.kernel, self.buffer, self.program)
    }
}

/// A buffer a kernel reads from the running sim.
#[derive(Clone, Debug, PartialEq)]
pub enum SimInput {
    /// `Rigid` records of a group's live bodies (every body when empty).
    Bodies(String),
    /// `Contact` records inside the retention window involving a group.
    Contacts(String),
    /// `Event` records inside the retention window for a group.
    Events(String),
    /// Another sim's state.
    Sim(String),
}

/// Bodies created at a time.
#[derive(Clone, Debug)]
pub struct SpawnDesc {
    /// When: the first step at or after it (`beat(16)` resolved through
    /// the tempo map by the host).
    pub at: SimTime,
    pub group: String,
    /// Kernel elements (records given in place ignore it).
    pub count: u32,
    pub shape: Shape,
    /// `Fixed` for colliders, `Dynamic` otherwise (kinematic spawns are
    /// not a thing: animate a declared body).
    pub fixed: bool,
    /// Size when a record leaves it 0.
    pub size: [f32; 3],
    pub from: SpawnFrom,
}

/// A stateful kernel (KERNELS.md §3.5.6): `count` records of `state`,
/// written by `init` at step 0 and by `update` (reading the previous state
/// as `prev`) every `every` steps; both write `next` (see
/// [`crate::sim_sources`]).
#[derive(Clone, Debug)]
pub struct SimDesc {
    pub name: String,
    pub state: Layout,
    pub count: u32,
    /// Physics steps per sim step (a sim's own step must be a whole
    /// multiple of the stepper's).
    pub every: u32,
    /// Writes `buffer` (count records).
    pub init: KernelCall,
    /// Reads `prev` (the previous state), writes `buffer`.
    pub update: KernelCall,
    /// The name of the update kernel's previous-state input.
    pub prev: String,
    /// Bodies the update emits into physics: `emit_buffer(Body, n)` buffer
    /// name, the group they join and their shape.
    pub spawn: Option<(String, String, Shape)>,
}

/// The whole of a document's effect physics.
#[derive(Clone, Debug)]
pub struct PhysicsDesc {
    pub dt: StepDt,
    /// box3d sub-steps per step.
    pub substeps: u32,
    pub seed: u64,
    pub gravity: [f32; 3],
    /// How long contacts and events stay readable (`retain`).
    pub retain: SimTime,
    /// The slowest approach that still makes a contact record (m/s).
    pub hit_speed: f32,
    /// Resting bodies sleep (cheaper; a sleep is an event).
    pub sleep: bool,
    pub bodies: Vec<BodyDesc>,
    pub spawns: Vec<SpawnDesc>,
    pub sims: Vec<SimDesc>,
    /// The document's own identity (text hash) and its external input
    /// revisions (tempo map, inputs): part of the version stamp.
    pub doc_hash: u64,
    pub revisions: u64,
    /// In-memory checkpoint budget in bytes.
    pub checkpoint_bytes: usize,
}

impl PhysicsDesc {
    /// A description with the defaults (1/240 s, 2 sub-steps, earth
    /// gravity, 0.3 s retention, 64 MiB of checkpoints) and nothing in it.
    pub fn new(doc_hash: u64) -> Self {
        Self {
            dt: StepDt::new(1, 240).unwrap(),
            substeps: 2,
            seed: 0,
            gravity: [0.0, -9.8, 0.0],
            retain: SimTime::from_ratio(3, 10),
            hit_speed: 0.05,
            sleep: true,
            bodies: Vec::new(),
            spawns: Vec::new(),
            sims: Vec::new(),
            doc_hash,
            revisions: 0,
            checkpoint_bytes: 64 << 20,
        }
    }
}
