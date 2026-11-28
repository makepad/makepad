use crate::{baked::MeshEntry, ObjectGeometry};
use crate::{
    canon::{CanonicalZeros, Reader, Writer},
    mesh,
    rig::{read_clip, read_skeleton, write_clip, write_skeleton},
    AnimationClip, Material, Operation, OperationResult, Skeleton,
};
use makepad_core_util::sha256::sha256;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Mesh(mesh::MeshError),
    Budget(&'static str),
    /// An edit or a compile needed more working memory than its limit (`mesh::Limits::max_bytes`)
    /// allows: the step refused, what that step needed (the whole may need more), and the limit,
    /// in bytes. A larger limit lets it through.
    WorkingBytes { what: &'static str, needed: usize, limit: usize },
    Invalid(&'static str),
    Corrupt(&'static str),
    UnsupportedVersion(u32),
    MissingObject(String),
    DuplicateObject(String),
    StaleHead { expected: Head, actual: Head },
    RequestIdReused,
    BakedObject { index: usize, op: &'static str, object: String },
}
impl Error {
    /// Whether the builder should retry this refusal under a larger working-memory limit.
    pub fn is_memory_refusal(&self) -> bool {
        matches!(self, Self::WorkingBytes { .. } | Self::Mesh(mesh::MeshError::Budget { resource: "bytes", .. }) | Self::Budget(_))
    }
}
impl From<mesh::MeshError> for Error {
    fn from(v: mesh::MeshError) -> Self {
        Self::Mesh(v)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BakedObject { index, op, object } => write!(f, "operation {index}: cannot {op} '{object}': the object is baked"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "model source version {version} is not supported (this build reads version {SOURCE_VERSION})"
            ),
            Self::WorkingBytes { what, needed, limit } => {
                let mib = |b: &usize| *b as f64 / 1048576.0;
                write!(f, "{what}: needs {:.1} MiB, limit {:.1} MiB", mib(needed), mib(limit))
            }
            _ => write!(f, "{self:?}"),
        }
    }
}
impl std::error::Error for Error {}

/// Generation is a monotonic concurrency token; hash identifies exact content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    pub generation: u64,
    pub content: [u8; 32],
}
#[derive(Clone, Debug)]
pub struct Limits {
    pub mesh: mesh::Limits,
    /// Undo retention window. When exceeded, the oldest half is checkpointed
    /// together, so retained undo depth ranges from half the window to the
    /// full window. Zero disables undo without refusing edits.
    pub max_history: usize,
    pub max_receipts: usize,
    pub max_texture_dimension: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            mesh: mesh::Limits::default(),
            max_history: 4096,
            max_receipts: 128,
            max_texture_dimension: 4096,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Transaction {
    pub request_id: String,
    pub expected: Head,
    pub operations: Vec<Operation>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub committed: Head,
    pub current: Head,
    pub replayed: bool,
    pub results: Vec<OperationResult>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    pub objects: BTreeMap<String, ObjectGeometry>,
    pub materials: BTreeMap<u32, Material>,
    pub skeleton: Option<Skeleton>,
    pub clips: BTreeMap<String, AnimationClip>,
    pub scene: crate::SceneState,
    pub selections: crate::SelectionState,
    pub rig: crate::RigState,
    pub surface: crate::SurfaceState,
    pub soft_body: Option<makepad_gltf::SoftBodyMetadata>,
}
impl State {
    pub(crate) fn editable(&self, name: &str) -> Option<&mesh::Mesh> {
        self.objects.get(name)?.editable()
    }
    pub(crate) fn editable_mut(&mut self, name: &str) -> Option<&mut mesh::Mesh> {
        self.objects.get_mut(name)?.editable_mut()
    }
    pub(crate) fn editable_objects(&self) -> impl Iterator<Item = (&String, &mesh::Mesh)> {
        self.objects.iter().filter_map(|(name, geometry)| Some((name, geometry.editable()?)))
    }
    fn memory_bytes(&self) -> usize {
        self.mesh_bytes().saturating_add(self.bytes_besides_meshes())
    }
    pub(crate) fn baked_meshes(&self) -> impl Iterator<Item = &std::sync::Arc<crate::BakedMesh>> {
        self.objects.values().filter_map(|g| match g { ObjectGeometry::Baked(m) => Some(m), _ => None })
    }
    pub(crate) fn baked_bytes(&self) -> usize {
        let mut seen = HashSet::new();
        self.baked_meshes().filter(|m| seen.insert(std::sync::Arc::as_ptr(m))).map(|m| m.memory_bytes()).sum()
    }
    // Frozen arrays in a replay candidate are already owned by its checkpoint or log.
    fn copy_bytes(&self) -> usize { self.mesh_copy_bytes().saturating_add(self.bytes_besides_meshes()) }
    fn copies_bytes(&self, copies: usize) -> usize {
        self.copy_bytes().saturating_mul(copies).saturating_add(self.baked_bytes())
    }
    fn mesh_bytes(&self) -> usize { self.mesh_copy_bytes().saturating_add(self.baked_bytes()) }
    fn mesh_copy_bytes(&self) -> usize {
        self.objects.iter().fold(0usize, |n, (name, geometry)| {
            n.saturating_add(geometry.editable().map_or(0, mesh::Mesh::memory_bytes))
                .saturating_add(name.len().saturating_mul(2)).saturating_add(128)
        })
    }
    fn bytes_besides_meshes(&self) -> usize {
        let materials = self.materials.values().fold(0usize, |n, m| {
            n.saturating_add(m.base_color_png.len().saturating_mul(2))
                .saturating_add(128)
        });
        let clips = self.clips.values().fold(0usize, |n, c| {
            n.saturating_add(c.memory_bytes()).saturating_add(128)
        });
        self.scene.memory_bytes()
            .saturating_add(self.selections.memory_bytes())
            .saturating_add(self.rig.memory_bytes())
            .saturating_add(self.surface.memory_bytes())
            .saturating_add(self.soft_body.as_ref().map_or(0, |m|m.to_value().to_json().len().saturating_mul(3)))
            .saturating_add(materials)
            .saturating_add(clips)
            .saturating_add(self.skeleton.as_ref().map_or(0, Skeleton::memory_bytes))
    }
    /// Everything but the meshes.
    fn clone_without_meshes(&self) -> Self {
        Self {
            objects: BTreeMap::new(),
            materials: self.materials.clone(),
            skeleton: self.skeleton.clone(),
            clips: self.clips.clone(),
            scene: self.scene.clone(),
            selections: self.selections.clone(),
            rig: self.rig.clone(),
            surface: self.surface.clone(),
            soft_body: self.soft_body.clone(),
        }
    }
}
#[derive(Clone, Debug)]
struct Receipt {
    id: String,
    fingerprint: [u8; 32],
    head: Head,
    results: Vec<OperationResult>,
    source_bytes: usize,
    retained_bytes: usize,
    resident_bytes: usize,
    previous: Option<[u8; 32]>,
    next: Option<[u8; 32]>,
}
impl Receipt {
    fn new(id: String, fingerprint: [u8; 32], head: Head, results: Vec<OperationResult>) -> Result<Self> {
        let retained_bytes = id.len()+128+results.iter().map(|r|r.object.len()+128+(r.faces.len()+r.vertices.len())*16+r.metrics.iter().map(|(k,_)|k.len()+96).sum::<usize>()).sum::<usize>();
        let resident_bytes = retained_bytes.saturating_add(128).saturating_add(id.len()).saturating_add(results.iter().map(|r|r.object.len()).sum::<usize>());
        let mut receipt = Self { id, fingerprint, head, results, source_bytes: 0, retained_bytes, resident_bytes, previous: None, next: None };
        let mut w = Writer::new();
        write_receipt(&mut w, &receipt)?;
        receipt.source_bytes = w.bytes.len();
        Ok(receipt)
    }
}
/// Insertion-ordered receipts with constant-time identity lookup and removal.
/// Each receipt links to its neighbors so removing it preserves the order
/// of the remaining receipts without scanning or shifting them.
#[derive(Clone, Debug, Default)]
struct Receipts {
    entries: HashMap<[u8; 32], Receipt>,
    first: Option<[u8; 32]>,
    last: Option<[u8; 32]>,
    retained_bytes: usize,
    resident_bytes: usize,
    source_bytes: usize,
}
impl Receipts {
    fn len(&self) -> usize { self.entries.len() }
    fn iter(&self) -> impl Iterator<Item = &Receipt> {
        let mut next = self.first;
        std::iter::from_fn(move || {
            let receipt = self.entries.get(&next?)?;
            next = receipt.next;
            Some(receipt)
        })
    }
    fn push_back(&mut self, mut receipt: Receipt) {
        let identity = sha256(receipt.id.as_bytes());
        receipt.previous = self.last;
        receipt.next = None;
        if let Some(last) = self.last { self.entries.get_mut(&last).unwrap().next = Some(identity); }
        else { self.first = Some(identity); }
        self.last = Some(identity);
        self.retained_bytes += receipt.retained_bytes;
        self.resident_bytes += receipt.resident_bytes;
        self.source_bytes += receipt.source_bytes;
        self.entries.insert(identity, receipt);
    }
    fn remove(&mut self, identity: &[u8; 32]) {
        if let Some(receipt) = self.entries.remove(identity) {
            if let Some(previous) = receipt.previous { self.entries.get_mut(&previous).unwrap().next = receipt.next; }
            else { self.first = receipt.next; }
            if let Some(next) = receipt.next { self.entries.get_mut(&next).unwrap().previous = receipt.previous; }
            else { self.last = receipt.previous; }
            self.retained_bytes -= receipt.retained_bytes;
            self.resident_bytes -= receipt.resident_bytes;
            self.source_bytes -= receipt.source_bytes;
        }
    }
    fn pop_front(&mut self) { if let Some(first) = self.first { self.remove(&first); } }
}
/// Version 4 persists request identities in acceptance order for oldest-first eviction.
const SOURCE_VERSION: u32 = 4;
const MAX_RECEIPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_REQUEST_IDENTITIES: usize = 65_536;
/// Serialized size and work of the checkpoint section of a source.
/// Measured from trusted execution whenever the checkpoint is established, so
/// admitting a transaction never re-serializes the checkpoint geometry.
#[derive(Clone, Copy, Debug, Default)]
struct SourceMeasure {
    bytes: usize,
    work: u64,
}

/// Single-owner state: run this on a long-lived worker or transfer ownership
/// between pool tasks. Element selectors are scoped to expected Head and object
/// name: undo branches/recreated objects may repeat numeric mesh IDs. Serialization includes the editable checkpoint, operation
/// log, undo cursor, monotonic generation and bounded idempotency receipts.
#[derive(Clone, Debug)]
pub struct Document {
    limits: Limits,
    checkpoint: State,
    log: Vec<Vec<Operation>>,
    log_memory: Vec<usize>,
    log_source: Vec<usize>,
    log_memory_bytes: usize,
    log_baked_bytes: usize,
    log_source_bytes: usize,
    checkpoint_source: SourceMeasure,
    cursor: usize,
    pub(crate) state: State,
    head: Head,
    receipts: Receipts,
    request_identities: HashSet<[u8; 32]>,
    request_order: VecDeque<[u8; 32]>,
}
/// A document at its last use ([`Document::into_last_use`]): it takes edits and compiles, with no
/// undo history or checkpoint beside its state. It is never saved: a save writes what it dropped.
#[derive(Debug)]
pub struct LastUse(Document);
impl LastUse {
    /// Applies `operations` as one transaction: all of them, or none and an error.
    pub fn apply(&mut self, request_id: &str, operations: Vec<Operation>, cancelled: Option<&dyn Fn() -> bool>) -> Result<()> {
        let expected = self.0.head;
        self.0.apply(Transaction { request_id: request_id.into(), expected, operations }, cancelled).map(|_| ())
    }
    /// Compiles the product. A refusal leaves it as it was, to compile again (under a larger
    /// limit when it was refused for working memory).
    pub fn compile_with_cache(&self, cache: &mut crate::CompileCache, cancelled: Option<&dyn Fn() -> bool>) -> Result<crate::CompiledModel> {
        self.0.compile_with_cache(cache, cancelled)
    }
    /// The working memory its edits and compiles may hold, in bytes (`mesh::Limits::max_bytes`).
    pub fn set_memory_limit(&mut self, bytes: usize) { self.0.set_memory_limit(bytes); }
}
/// An unlogged model state, for callers that keep their own program and undo
/// log and rebuild the model from it: operations execute in place, with none
/// of [`Document`]'s history, receipts, content hash or checkpoints (each of
/// which costs a pass over the whole state per transaction). A failed
/// `execute` may leave the workspace partly applied; callers discard it.
#[derive(Clone, Debug)]
pub struct Workspace {
    limits: Limits,
    state: State,
}

impl Workspace {
    pub(crate) fn check_geometry_op(&self, index: usize, op: &Operation) -> Result<()> {
        self.state.check_geometry_op(index, op)
    }
    pub(crate) fn check_editable(&self, index: usize, op: &'static str, name: &str) -> Result<()> {
        self.state.check_editable(index, op, name)
    }
    pub fn new(limits: Limits) -> Result<Self> {
        let mut state = State::default();
        state.materials.insert(0, Material::default());
        Ok(Self { limits, state })
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    /// Execute operations in order, in place (the same execution a
    /// [`Document`] transaction runs on its candidate state).
    pub fn execute(&mut self, ops: &[Operation], cancelled: Option<&dyn Fn() -> bool>) -> Result<Vec<OperationResult>> {
        let mut ctx = mesh::Context::new(self.limits.mesh.clone(), cancelled);
        ctx.checkpoint(1)?;
        execute_with(&mut self.state, ops, &self.limits, &mut ctx, Execution::Workspace, &mut 0)
    }
    /// Program batches are disposable on failure; retain the failing operation
    /// index instead of cloning the state for a successful batch's rollback.
    pub(crate) fn execute_program(&mut self, ops: &[Operation], retained_bytes: usize, borrowed_baked_bytes: usize, cancelled: Option<&dyn Fn() -> bool>) -> std::result::Result<(), (usize, Error)> {
        // The program has already reserved the immutable payloads. Validate each
        // source operation against the full limit, then charge only its private storage.
        let mut source_limits = self.limits.clone();
        source_limits.mesh.max_bytes += borrowed_baked_bytes;
        for (index, op) in ops.iter().enumerate() {
            if cancelled.is_some_and(|c| c()) { return Err((index, Error::Mesh(mesh::MeshError::Cancelled))); }
            op.check(&source_limits).map_err(|error| (index, error))?;
        }
        // Charge canonical transaction storage against the host memory budget.
        let resident = ops.iter().fold(retained_bytes, |n, op| n.saturating_add(op.memory_bytes() - op.baked_bytes()));
        {
            let mut encoded = Writer::measure();
            write_head(&mut encoded, Head { generation: 0, content: [0; 32] }).map_err(|e| (0, e))?;
            write_ops(&mut encoded, ops, &source_limits).map_err(|e| (0, e))?;
            let needed = self.state.copy_bytes().saturating_add(resident).saturating_add(encoded.len().saturating_sub(ops.iter().map(Operation::baked_source_bytes).sum()));
            if needed > self.limits.mesh.max_bytes {
                return Err((0, Error::WorkingBytes { what: "program working bytes", needed, limit: self.limits.mesh.max_bytes }));
            }
        }
        let mut ctx = mesh::Context::new(self.limits.mesh.clone(), cancelled);
        ctx.limits.max_bytes = ctx.limits.max_bytes.checked_sub(resident)
            .ok_or((0, Error::WorkingBytes { what: "program working bytes", needed: resident, limit: self.limits.mesh.max_bytes }))?;
        let mut index = 0;
        execute_with(&mut self.state, ops, &self.limits, &mut ctx, Execution::Program, &mut index)
            .map(|_| ()).map_err(|error| (index, error))
    }
    /// Establish the editable checkpoint once, after a successful program.
    /// The program itself owns its undo history; no intermediate receipts or
    /// snapshots are needed in the returned document.
    pub(crate) fn into_document(mut self, generation: u64, cancelled: Option<&dyn Fn() -> bool>) -> Result<Document> {
        use crate::build_detail::{GeometryPhase, GeometryTimer};
        let _history = GeometryTimer::new(GeometryPhase::History);
        // The document holds the values its source decodes to (a program's
        // mirrors leave -0.0), so it compiles the bytes a reopen compiles.
        self.state.canonicalize_zeros();
        // The returned checkpoint must remain serializable/reopenable too.
        // Admit its copies before measurement decodes any temporary state.
        // The checkpoint and compiler count their live baked arrays themselves.
        // Deleted geometry is still pinned by the calling program until compile returns.
        self.limits.mesh.max_bytes += self.state.baked_bytes();
        let needed = self.state.copies_bytes(3);
        if needed > self.limits.mesh.max_bytes {
            return Err(Error::WorkingBytes { what: "program checkpoint bytes", needed, limit: self.limits.mesh.max_bytes });
        }
        let mut ctx = mesh::Context::new(self.limits.mesh.clone(), cancelled);
        let content = state_hash(&self.state, &self.limits, &mut ctx)?;
        let checkpoint_source = verify_checkpoint(&self.state, &self.limits, &mut ctx)?;
        ctx.checkpoint(0)?;
        let checkpoint = {
            let _clone = GeometryTimer::new(GeometryPhase::Clone);
            self.state.clone()
        };
        let document = Document { limits: self.limits, state: self.state, checkpoint,
            log: Vec::new(), log_memory: Vec::new(), log_source: Vec::new(), log_memory_bytes: 0, log_baked_bytes: 0, log_source_bytes: 0,
            checkpoint_source, cursor: 0,
            head: Head { generation, content }, receipts: Receipts::default(), request_identities: HashSet::new(), request_order: VecDeque::new() };
        let mut source_ctx = mesh::Context::new(document.limits.mesh.clone(), cancelled);
        let envelope = encode_source(&SourceParts {
            head: document.head, checkpoint: CheckpointSource::Measured(checkpoint_source),
            cursor: 0, log: &[], receipts: &[], identities: &document.request_order, added_identity: None,
        }, &document.limits, &mut source_ctx)?;
        let source_bytes = checkpoint_source.bytes.saturating_add(envelope.len());
        let needed = document.state.copies_bytes(3).saturating_add(source_bytes);
        if needed > document.limits.mesh.max_bytes {
            return Err(Error::WorkingBytes { what: "program checkpoint bytes", needed, limit: document.limits.mesh.max_bytes });
        }
        Ok(document)
    }
    /// Add a new object holding `mesh`: what `Operation::ImportMesh` does,
    /// without encoding and re-decoding (and re-validating) the mesh.
    pub fn insert_mesh(&mut self, object: &str, mesh: mesh::Mesh) -> Result<()> {
        check_name(object, &self.limits)?;
        if self.state.objects.contains_key(object) {
            return Err(Error::DuplicateObject(object.to_string()));
        }
        self.state.objects.insert(object.to_string(), mesh.into());
        Ok(())
    }
    /// Replace an existing object's mesh, keeping its scene node.
    pub fn set_mesh(&mut self, object: &str, mesh: mesh::Mesh) -> Result<()> {
        self.state.check_editable(0, "set_mesh", object)?;
        let slot = self.state.editable_mut(object).ok_or_else(|| Error::MissingObject(object.to_string()))?;
        *slot = mesh;
        Ok(())
    }
    /// Objects with editable topology. Use `geometries` to enumerate every object.
    pub fn objects(&self) -> impl Iterator<Item = (&str, &mesh::Mesh)> {
        self.state.editable_objects().map(|(name, mesh)| (name.as_str(), mesh))
    }
    pub fn geometries(&self) -> impl Iterator<Item = (&str, &ObjectGeometry)> {
        self.state.objects.iter().map(|(name, g)| (name.as_str(), g))
    }
    pub fn object(&self, name: &str) -> Option<&mesh::Mesh> {
        self.state.editable(name)
    }
    pub fn materials(&self) -> &BTreeMap<u32, Material> {
        &self.state.materials
    }
    pub fn scene(&self) -> &crate::SceneState {
        &self.state.scene
    }
    pub fn skeleton(&self) -> Option<&Skeleton> { self.state.skeleton.as_ref() }
    pub fn surface(&self) -> &crate::SurfaceState { &self.state.surface }
}

impl Document {
    pub fn new(limits: Limits) -> Result<Self> {
        let mut state = State::default();
        state.materials.insert(0, Material::default());
        let mut ctx = mesh::Context::new(limits.mesh.clone(), None);
        let hash = state_hash(&state, &limits, &mut ctx)?;
        let checkpoint_source = verify_checkpoint(&state, &limits, &mut ctx)?;
        Ok(Self {
            limits,
            checkpoint: state.clone(),
            log: Vec::new(),
            log_memory: Vec::new(), log_source: Vec::new(), log_memory_bytes: 0, log_baked_bytes: 0, log_source_bytes: 0,
            checkpoint_source,
            cursor: 0,
            state,
            head: Head {
                generation: 0,
                content: hash,
            },
            receipts: Receipts::default(),
            request_identities: HashSet::new(), request_order: VecDeque::new(),
        })
    }
    pub(crate) fn set_memory_limit(&mut self, bytes: usize) { self.limits.mesh.max_bytes = bytes; }
    pub fn head(&self) -> Head {
        self.head
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    /// Objects with editable topology. Use `geometries` to enumerate every object.
    pub fn objects(&self) -> impl Iterator<Item = (&str, &mesh::Mesh)> {
        self.state.editable_objects().map(|(name, mesh)| (name.as_str(), mesh))
    }
    pub fn geometries(&self) -> impl Iterator<Item = (&str, &ObjectGeometry)> {
        self.state.objects.iter().map(|(name, g)| (name.as_str(), g))
    }
    pub fn object(&self, name: &str) -> Option<&mesh::Mesh> {
        self.state.editable(name)
    }
    pub fn materials(&self) -> &BTreeMap<u32, Material> {
        &self.state.materials
    }
    pub fn skeleton(&self) -> Option<&Skeleton> {
        self.state.skeleton.as_ref()
    }
    pub fn clips(&self) -> &BTreeMap<String, AnimationClip> {
        &self.state.clips
    }
    pub fn surface(&self) -> &crate::SurfaceState { &self.state.surface }
    pub fn rig(&self) -> &crate::RigState { &self.state.rig }
    pub fn selections(&self) -> &crate::SelectionState { &self.state.selections }
    pub fn scene(&self) -> &crate::SceneState { &self.state.scene }
    pub fn soft_body(&self) -> Option<&makepad_gltf::SoftBodyMetadata> { self.state.soft_body.as_ref() }
    /// A working copy to compile from: no undo history, and none of the state's meshes, which
    /// the caller replaces with the ones it evaluated. `working` is what the caller holds beside
    /// it (those meshes and their working copies).
    pub(crate) fn evaluation_copy(&self, working:usize) -> Result<Self> {
        self.admit(working.saturating_add(self.state.bytes_besides_meshes()))?;
        Ok(Self { limits:self.limits.clone(), checkpoint:State::default(), log:Vec::new(), log_memory:Vec::new(), log_source:Vec::new(), log_memory_bytes:0, log_baked_bytes:0, log_source_bytes:0, checkpoint_source:SourceMeasure::default(),
            cursor:0, state:self.state.clone_without_meshes(), head:self.head, receipts:Receipts::default(), request_identities:HashSet::new(), request_order:VecDeque::new() })
    }
    /// The document for its last use, by a host that derives one more product from a document
    /// whose source it has kept already (a model's distance levels, added after its full detail
    /// was saved). Its undo history and checkpoint go, so an edit and a compile hold one copy of
    /// the state where a document with its history holds two.
    pub fn into_last_use(self) -> LastUse {
        LastUse(Self { limits:self.limits, checkpoint:State::default(), log:Vec::new(), log_memory:Vec::new(), log_source:Vec::new(), log_memory_bytes:0, log_baked_bytes:0, log_source_bytes:0, checkpoint_source:SourceMeasure::default(),
            cursor:0, state:self.state, head:self.head, receipts:Receipts::default(), request_identities:HashSet::new(), request_order:VecDeque::new() })
    }
    pub fn history_position(&self) -> (usize, usize) {
        (self.cursor, self.log.len())
    }
    fn memory_bytes(&self) -> usize {
        let mut seen = HashSet::new();
        let mut unique_bytes = 0usize;
        let mut include = |mesh: &std::sync::Arc<crate::BakedMesh>| {
            if seen.insert(std::sync::Arc::as_ptr(mesh)) { unique_bytes = unique_bytes.saturating_add(mesh.memory_bytes()); }
        };
        for mesh in self.checkpoint.baked_meshes().chain(self.state.baked_meshes()) { include(mesh); }
        if self.log_baked_bytes != 0 {
            for op in self.log.iter().flatten() { if let Operation::Baked { mesh, .. } = op { include(mesh); } }
        }
        self.checkpoint.copy_bytes().saturating_add(self.state.copy_bytes())
            .saturating_add(self.log_memory_bytes - self.log_baked_bytes).saturating_add(unique_bytes)
            .saturating_add(self.receipts.resident_bytes)
            .saturating_add(self.request_identities.len().saturating_mul(128))
            .saturating_add(self.request_order.len().saturating_mul(32))
    }
    fn drain_history(&mut self, count: usize) {
        self.log_baked_bytes -= self.log[..count].iter().flatten().map(Operation::baked_bytes).sum::<usize>();
        self.log.drain(..count);
        self.log_memory_bytes -= self.log_memory.drain(..count).sum::<usize>();
        self.log_source_bytes -= self.log_source.drain(..count).sum::<usize>();
    }
    fn admit(&self, extra: usize) -> Result<()> {
        let needed = self.memory_bytes().saturating_add(extra);
        if needed > self.limits.mesh.max_bytes {
            Err(Error::WorkingBytes { what: "document working bytes", needed, limit: self.limits.mesh.max_bytes })
        } else {
            Ok(())
        }
    }
    pub fn apply(
        &mut self,
        tx: Transaction,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> Result<Applied> {
        check_name(&tx.request_id, &self.limits)?;
        if tx.operations.is_empty() {
            return Err(Error::Invalid("empty transaction"));
        }
        for op in &tx.operations {
            op.check(&self.limits)?;
        }
        // The fingerprint encoding is released before admission; it is not
        // resident while the transaction executes. The edit runs and logs the
        // operations it decodes to (a -0.0 literal reads as +0.0), the values
        // a reopen replays.
        let (fingerprint, operation_source_bytes, operations) = if tx.operations.iter().any(|op| matches!(op, Operation::Baked { .. })) {
            let mut hash = Writer::hashing();
            write_head(&mut hash, tx.expected)?;
            let start = hash.len();
            write_ops(&mut hash, &tx.operations, &self.limits)?;
            let size = hash.len() - start;
            let mut operations = Vec::with_capacity(tx.operations.len());
            for op in tx.operations {
                if matches!(op, Operation::Baked { .. }) { operations.push(op); }
                else {
                    let mut w = Writer::new();
                    op.write(&mut w)?;
                    let mut r = Reader::new(&w.bytes);
                    operations.push(Operation::read(&mut r, &self.limits)?);
                    r.end()?;
                }
            }
            (hash.finish_hash(), size, operations)
        } else {
            let mut encoded = Writer::new();
            write_head(&mut encoded, tx.expected)?;
            let start = encoded.bytes.len();
            write_ops(&mut encoded, &tx.operations, &self.limits)?;
            let mut decoded = Reader::new(&encoded.bytes[start..]);
            let operations = read_ops(&mut decoded, &self.limits)?;
            decoded.end()?;
            (sha256(&encoded.bytes), encoded.bytes.len() - start, operations)
        };
        let tx = Transaction { operations, ..tx };
        let request_identity = sha256(tx.request_id.as_bytes());
        if let Some(receipt) = self.receipts.entries.get(&request_identity).filter(|r|r.id == tx.request_id) {
            if receipt.fingerprint != fingerprint {
                return Err(Error::RequestIdReused);
            }
            return Ok(Applied {
                committed: receipt.head,
                current: self.head,
                replayed: true,
                results: receipt.results.clone(),
            });
        }
        // Idempotency is a retention window: remember recent requests in acceptance order.
        if self.request_identities.contains(&request_identity) {
            return Err(Error::RequestIdReused);
        }
        self.expect(tx.expected)?;
        let mut ctx = mesh::Context::new(self.limits.mesh.clone(), cancelled);
        ctx.checkpoint(1)?;
        let operation_bytes = tx
            .operations
            .iter()
            .fold(0usize, |n, op| n.saturating_add(op.memory_bytes()));
        // Resident while a transaction is admitted: the retained document, the
        // transaction and one candidate copy of the state. Retained source
        // sizes are cached; only this edit is encoded. Leading
        // whole-object deletions never read the mesh they remove, so the
        // candidate carries an empty stand-in for that geometry instead of a
        // copy; execute still performs and validates the deletion itself.
        let retained = self.memory_bytes();
        let state_bytes = self.state.memory_bytes();
        let baked_ops = tx.operations.iter().map(Operation::baked_bytes).sum::<usize>();
        let history_bytes = retained
            .saturating_sub(state_bytes)
            .saturating_sub(self.checkpoint.memory_bytes());
        let released = released_objects(&self.state, &tx.operations);
        let placeholder_bytes = mesh::Mesh::new().memory_bytes();
        let candidate_bytes = released.iter().fold(self.state.copy_bytes(), |n, name| {
            n.saturating_sub(self.state.editable(name).map_or(0, mesh::Mesh::memory_bytes))
                .saturating_add(placeholder_bytes)
        });
        let transaction_bytes = operation_bytes
            .saturating_mul(2).saturating_sub(baked_ops)
            .saturating_add(history_bytes);
        self.admit(candidate_bytes.saturating_add(transaction_bytes))?;
        // Mesh operations work beside the retained document and the
        // transaction; execute pins the candidate itself per operation.
        ctx.limits.max_bytes = ctx
            .limits
            .max_bytes
            .saturating_sub(retained)
            .saturating_sub(operation_bytes);
        let mut candidate = candidate_state(&self.state, &released);
        let results = execute(&mut candidate, &tx.operations, &self.limits, &mut ctx)?;
        self.admit(candidate.copy_bytes().saturating_add(transaction_bytes))?;
        let content = state_hash(&candidate, &self.limits, &mut ctx)?;
        let head = Head {
            generation: self
                .head
                .generation
                .checked_add(1)
                .ok_or(Error::Budget("generation"))?,
            content,
        };
        let folded = history_fold_count(self.cursor + 1, self.limits.max_history);
        let automatic_checkpoint = if folded == 0 { None } else {
            self.admit(candidate.copy_bytes().saturating_add(self.checkpoint.copy_bytes().saturating_mul(2)).saturating_add(transaction_bytes))?;
            let mut checkpoint_ctx = mesh::Context::new(mesh::Limits {
                max_bytes: ctx.limits.max_bytes.saturating_sub(candidate.copy_bytes()),
            }, cancelled);
            let checkpoint = if folded == self.cursor + 1 {
                self.admit(candidate.copy_bytes().saturating_mul(3).saturating_add(transaction_bytes))?;
                candidate.clone()
            } else {
                let mut checkpoint = self.checkpoint.clone();
                for ops in &self.log[..folded] {
                    execute(&mut checkpoint, ops, &self.limits, &mut checkpoint_ctx)?;
                }
                checkpoint
            };
            let source = checkpoint_source_measure(&checkpoint, &self.limits, &mut checkpoint_ctx)?;
            Some((checkpoint, source))
        };
        let identity_evictions = (self.request_order.len() + 1).saturating_sub(MAX_REQUEST_IDENTITIES);
        let evicted_identities = self.request_order.iter().take(identity_evictions).copied().collect::<Vec<_>>();
        // Use the same writer as to_bytes for the new receipt, even when the
        // retention policy will immediately evict it.
        let receipt = Receipt::new(tx.request_id, fingerprint, head, results.clone())?;
        let (mut evicted, retain_receipt) = plan_receipt_eviction(&self.receipts, &receipt, self.limits.max_receipts);
        for identity in &evicted_identities {
            if self.receipts.entries.contains_key(identity) { evicted.insert(*identity); }
        }
        let evicted_source = evicted.iter().map(|id|self.receipts.entries[id].source_bytes).sum::<usize>();
        let removed_log_source = self.log_source[self.cursor..].iter().sum::<usize>()
            + self.log_source[..folded.min(self.cursor)].iter().sum::<usize>();
        let log_source_bytes = self.log_source_bytes - removed_log_source
            + if folded <= self.cursor { operation_source_bytes } else { 0 };
        let checkpoint_source = automatic_checkpoint.as_ref().map_or(self.checkpoint_source, |(_, source)| *source);
        // Validate the changed envelope counts and identity with the canonical
        // writers. The other sections were validated when first installed.
        let mut envelope = Writer::new();
        envelope.raw(b"MPMODEL\0")?;
        envelope.u32(SOURCE_VERSION)?;
        write_head(&mut envelope, head)?;
        envelope.count(self.cursor + 1 - folded)?;
        envelope.count(self.cursor + 1 - folded)?;
        envelope.count(self.receipts.len() - evicted.len() + usize::from(retain_receipt))?;
        envelope.count(self.request_order.len() + 1 - identity_evictions)?;
        let envelope_bytes = envelope.bytes.len();
        envelope.raw(&request_identity)?;
        let source_bytes = envelope_bytes.saturating_add(checkpoint_source.bytes)
            .saturating_add(log_source_bytes)
            .saturating_add(self.receipts.source_bytes - evicted_source)
            .saturating_add(if retain_receipt { receipt.source_bytes } else { 0 })
            .saturating_add((self.request_order.len() + 1 - identity_evictions).saturating_mul(32));
        self.admit(candidate.copy_bytes().saturating_add(operation_bytes.saturating_mul(2).saturating_sub(baked_ops))
            .saturating_add(source_bytes.saturating_sub(checkpoint_source.bytes)))?;
        // Preserve source work accounting without copying or traversing history.
        ctx.checkpoint(checkpoint_source.work)?;
        ctx.checkpoint(source_bytes as u64)?;
        ctx.checkpoint((self.cursor + 1 - folded) as u64)?;
        ctx.checkpoint(1)?;
        // Install in place: nothing below can fail, so the previous state, the
        // redo branch and evicted receipts are released only after every check.
        self.log_baked_bytes -= self.log[self.cursor..].iter().flatten().map(Operation::baked_bytes).sum::<usize>();
        self.log_baked_bytes += baked_ops;
        self.log.truncate(self.cursor);
        self.log_memory_bytes -= self.log_memory[self.cursor..].iter().sum::<usize>();
        self.log_source_bytes -= self.log_source[self.cursor..].iter().sum::<usize>();
        self.log_memory.truncate(self.cursor);
        self.log_source.truncate(self.cursor);
        self.log.push(tx.operations);
        self.log_memory.push(operation_bytes);
        self.log_source.push(operation_source_bytes);
        self.log_memory_bytes += operation_bytes;
        self.log_source_bytes += operation_source_bytes;
        for identity in &evicted { self.receipts.remove(identity); }
        if retain_receipt {
            self.receipts.push_back(receipt);
        }
        if !evicted.is_empty() || !retain_receipt {
            eprintln!("model receipts: evicted {} oldest receipts", evicted.len() + usize::from(!retain_receipt));
        }
        if identity_evictions > 0 {
            for identity in self.request_order.drain(..identity_evictions) { self.request_identities.remove(&identity); }
            eprintln!("model request identities: evicted {identity_evictions} oldest identities");
        }
        self.request_identities.insert(request_identity);
        self.request_order.push_back(request_identity);
        self.state = candidate;
        self.head = head;
        self.cursor += 1;
        if let Some((checkpoint, source)) = automatic_checkpoint {
            self.checkpoint = checkpoint;
            self.checkpoint_source = source;
            self.drain_history(folded);
            self.cursor -= folded;
            eprintln!("model history: checkpointed {folded} oldest edits");
        }
        Ok(Applied {
            committed: head,
            current: head,
            replayed: false,
            results,
        })
    }
    fn expect(&self, expected: Head) -> Result<()> {
        if expected == self.head {
            Ok(())
        } else {
            Err(Error::StaleHead {
                expected,
                actual: self.head,
            })
        }
    }
    pub fn undo(&mut self, expected: Head, cancelled: Option<&dyn Fn() -> bool>) -> Result<Head> {
        self.expect(expected)?;
        if self.cursor == 0 {
            return Err(Error::Invalid("nothing to undo"));
        }
        self.move_cursor(self.cursor - 1, cancelled)
    }
    pub fn redo(&mut self, expected: Head, cancelled: Option<&dyn Fn() -> bool>) -> Result<Head> {
        self.expect(expected)?;
        if self.cursor == self.log.len() {
            return Err(Error::Invalid("nothing to redo"));
        }
        self.move_cursor(self.cursor + 1, cancelled)
    }
    fn move_cursor(&mut self, cursor: usize, cancelled: Option<&dyn Fn() -> bool>) -> Result<Head> {
        let mut ctx = mesh::Context::new(self.limits.mesh.clone(), cancelled);
        ctx.checkpoint(1)?;
        self.admit(self.checkpoint.copy_bytes().saturating_mul(2))?;
        ctx.limits.max_bytes = ctx.limits.max_bytes.saturating_sub(self.memory_bytes());
        let folded = history_fold_count(cursor, self.limits.max_history);
        let mut state = self.checkpoint.clone();
        let mut automatic_checkpoint = None;
        for (index, ops) in self.log[..cursor].iter().enumerate() {
            execute(&mut state, ops, &self.limits, &mut ctx)?;
            // Redo can cross a smaller retention window used to reopen a source.
            if index + 1 == folded {
                self.admit(state.copy_bytes().saturating_mul(3))?;
                let source = checkpoint_source_measure(&state, &self.limits, &mut ctx)?;
                automatic_checkpoint = Some((state.clone(), source));
                ctx.limits.max_bytes = ctx.limits.max_bytes.saturating_sub(state.copy_bytes());
            }
        }
        let head = Head {
            generation: self
                .head
                .generation
                .checked_add(1)
                .ok_or(Error::Budget("generation"))?,
            content: state_hash(&state, &self.limits, &mut ctx)?,
        };
        ctx.checkpoint(1)?;
        self.state = state;
        self.cursor = cursor;
        self.head = head;
        if let Some((checkpoint, source)) = automatic_checkpoint {
            self.checkpoint = checkpoint;
            self.checkpoint_source = source;
            self.drain_history(folded);
            self.cursor -= folded;
            eprintln!("model history: checkpointed {folded} oldest edits");
        }
        Ok(head)
    }
    /// Explicitly discard undo history. Receipts remain valid across compaction.
    pub fn checkpoint(&mut self, expected: Head) -> Result<()> {
        self.expect(expected)?;
        let generation = self
            .head
            .generation
            .checked_add(1)
            .ok_or(Error::Budget("generation"))?;
        self.admit(self.state.copy_bytes())?;
        let source = verify_checkpoint(&self.state, &self.limits, &mut mesh::Context::new(self.limits.mesh.clone(), None))?;
        self.checkpoint = self.state.clone();
        self.checkpoint_source=source;
        self.log.clear();
        self.log_memory.clear();
        self.log_source.clear();
        self.log_memory_bytes = 0;
        self.log_baked_bytes = 0;
        self.log_source_bytes = 0;
        self.cursor = 0;
        // The content is unchanged, but the serialized source revision changed.
        // A queued publisher must not cross this compaction boundary unnoticed.
        self.head.generation = generation;
        Ok(())
    }
    pub fn to_bytes(&self, cancelled: Option<&dyn Fn() -> bool>) -> Result<Vec<u8>> {
        self.encode(&mut mesh::Context::new(self.limits.mesh.clone(), cancelled))
    }
    /// Portable publication source. The current canonical state is the
    /// checkpoint, so clients never replay floating-point modeling operations.
    /// Local undo history remains intact; identity receipts and Head persist.
    pub fn to_snapshot_bytes(&self, cancelled: Option<&dyn Fn() -> bool>) -> Result<Vec<u8>> {
        self.encode_parts(&mut mesh::Context::new(self.limits.mesh.clone(), cancelled), true)
    }
    fn encode(&self, ctx: &mut mesh::Context) -> Result<Vec<u8>> {
        self.encode_parts(ctx, false)
    }
    fn encode_parts(&self, ctx: &mut mesh::Context, snapshot: bool) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        if self.state.baked_bytes() != 0 || self.checkpoint.baked_bytes() != 0 ||
            self.log_baked_bytes != 0 {
            let mut size = Writer::measure();
            self.write_parts(&mut size, ctx, snapshot)?;
            self.admit(size.len())?;
            w.bytes.reserve_exact(size.len());
        }
        self.write_parts(&mut w, ctx, snapshot)?;
        Ok(w.bytes)
    }
    fn write_parts(&self, w: &mut Writer, ctx: &mut mesh::Context, snapshot: bool) -> Result<()> {
        self.admit(
            self.memory_bytes()
                .saturating_sub(self.state.memory_bytes()),
        )?;
        let log = if snapshot {
            Vec::new()
        } else {
            self.log.iter().map(Vec::as_slice).collect::<Vec<_>>()
        };
        let receipts = self.receipts.iter().collect::<Vec<_>>();
        write_source(w,
            &SourceParts {
                head: self.head,
                checkpoint: CheckpointSource::State(if snapshot { &self.state } else { &self.checkpoint }),
                cursor: if snapshot { 0 } else { self.cursor },
                log: &log,
                receipts: &receipts,
                identities: &self.request_order,
                added_identity: None,
            },
            &self.limits,
            ctx,
        )
    }
    pub fn from_bytes(
        bytes: &[u8],
        limits: Limits,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let mut ctx = mesh::Context::new(limits.mesh.clone(), cancelled);
        ctx.checkpoint(1)?;
        ctx.limits.max_bytes = ctx
            .limits
            .max_bytes
            .checked_sub(bytes.len())
            .ok_or(Error::WorkingBytes { what: "decode working bytes", needed: bytes.len(), limit: ctx.limits.max_bytes })?;
        if r.raw(8)? != b"MPMODEL\0" {
            return Err(Error::Corrupt("not a model source"));
        }
        let version = r.u32()?;
        if version != SOURCE_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let head = read_head(&mut r)?;
        let checkpoint = read_state(&mut r, &limits, &mut ctx, None)?;
        let needed = checkpoint.copies_bytes(3);
        if needed > ctx.limits.max_bytes {
            return Err(Error::WorkingBytes { what: "decode checkpoint copies", needed, limit: ctx.limits.max_bytes });
        }
        let checkpoint_source=checkpoint_source_measure(&checkpoint,&limits,&mut ctx)?;
        let cursor = r.u32()? as usize;
        let count = r.count(4)?;
        if cursor > count {
            return Err(Error::Corrupt("undo cursor"));
        }
        let mut log = Vec::new();
        let mut log_memory = Vec::new();
        let mut log_source = Vec::new();
        let mut log_source_bytes = 0usize;
        let mut state = checkpoint.clone();
        let mut current = checkpoint.clone();
        let decode_budget = ctx.limits.max_bytes;
        let mut log_bytes = 0usize;
        let mut log_baked_bytes = 0usize;
        for i in 0..count {
            let start = r.position();
            let retained = checkpoint.memory_bytes().saturating_add(current.copy_bytes()).saturating_add(log_bytes);
            let available = decode_budget.saturating_sub(retained).saturating_sub(state.copy_bytes());
            let ops = read_ops_budgeted(&mut r, &limits, available)?;
            let source_bytes = r.position() - start;
            log_source.push(source_bytes);
            log_source_bytes += source_bytes;
            let op_bytes = ops.iter().fold(0usize, |n, op| n.saturating_add(op.memory_bytes()));
            log_baked_bytes += ops.iter().map(Operation::baked_bytes).sum::<usize>();
            log_memory.push(op_bytes);
            log_bytes = log_bytes.saturating_add(op_bytes);
            let pinned = retained.saturating_add(op_bytes);
            ctx.limits.max_bytes = decode_budget
                .checked_sub(pinned)
                .ok_or(Error::WorkingBytes { what: "decode history working bytes", needed: pinned, limit: decode_budget })?;
            execute(&mut state, &ops, &limits, &mut ctx)?;
            if i + 1 == cursor {
                let needed = pinned.saturating_add(state.copy_bytes().saturating_mul(2));
                if needed > decode_budget {
                    return Err(Error::WorkingBytes { what: "decode cursor copy", needed, limit: decode_budget });
                }
                current = state.clone();
            }
            log.push(ops);
        }
        ctx.limits.max_bytes = decode_budget;
        if state_hash(&current, &limits, &mut ctx)? != head.content {
            return Err(Error::Corrupt("content hash"));
        }
        let mut receipts = Receipts::default();
        for _ in 0..r.count(80)? {
            let id = r.string()?;
            check_name(&id, &limits)?;
            if receipts.entries.contains_key(&sha256(id.as_bytes())) {
                return Err(Error::Corrupt("duplicate receipt"));
            }
            let fingerprint = r.raw(32)?.try_into().unwrap();
            let receipt_head = read_head(&mut r)?;
            if receipt_head.generation > head.generation {
                return Err(Error::Corrupt("future receipt"));
            }
            let mut results = Vec::new();
            for _ in 0..r.count(16)? {
                let object = r.string()?;
                let mut faces = Vec::new();
                for _ in 0..r.count(8)? {
                    faces.push(mesh::FaceId(r.u64()?));
                }
                let mut vertices = Vec::new();
                for _ in 0..r.count(8)? {
                    vertices.push(mesh::VertexId(r.u64()?));
                }
                let mut metrics = BTreeMap::new();
                for _ in 0..r.count(12)? {
                    let name = r.string()?;
                    let value = usize::try_from(r.u64()?).map_err(|_|Error::Budget("operation metric"))?;
                    if metrics.insert(name, value).is_some() { return Err(Error::Corrupt("duplicate operation metric")); }
                }
                results.push(OperationResult {
                    object,
                    faces,
                    vertices,
                    metrics,
                });
            }
            receipts.push_back(Receipt::new(id, fingerprint, receipt_head, results)?);
        }
        let mut request_identities = HashSet::new();
        let mut request_order = VecDeque::new();
        for _ in 0..r.count(32)? {
            let identity = r.raw(32)?.try_into().unwrap();
            request_order.push_back(identity);
            if !request_identities.insert(identity) {
                return Err(Error::Corrupt("duplicate request identity"));
            }
        }
        for receipt in receipts.iter() {
            if !request_identities.contains(&sha256(receipt.id.as_bytes())) {
                return Err(Error::Corrupt("receipt lacks request identity"));
            }
        }
        r.end()?;
        let mut out = Self {
            limits,
            checkpoint,
            log,
            log_memory, log_source, log_memory_bytes: log_bytes, log_baked_bytes, log_source_bytes,
            checkpoint_source,
            cursor,
            state: current,
            head,
            receipts,
            request_identities, request_order,
        };
        // Exact re-encoding rejects ambiguous orderings and noncanonical inputs.
        let mut canonical = Writer::compare(bytes);
        out.write_parts(&mut canonical, &mut ctx, false)?;
        if canonical.len() != bytes.len() {
            return Err(Error::Corrupt("noncanonical document"));
        }
        // A source may have been saved with larger retention windows. Decode
        // and validate it first, then compact only the applied oldest history.
        let folded = history_fold_count(out.log.len(), out.limits.max_history).min(out.cursor);
        if folded != 0 {
            let mut checkpoint = out.checkpoint.clone();
            for ops in &out.log[..folded] { execute(&mut checkpoint, ops, &out.limits, &mut ctx)?; }
            out.checkpoint_source = checkpoint_source_measure(&checkpoint, &out.limits, &mut ctx)?;
            out.checkpoint = checkpoint;
            out.drain_history(folded); out.cursor -= folded;
            eprintln!("model history: checkpointed {folded} oldest edits");
        }
        let identity_evictions = out.request_order.len().saturating_sub(MAX_REQUEST_IDENTITIES);
        let old_receipts = out.receipts.len();
        for identity in out.request_order.drain(..identity_evictions) { out.request_identities.remove(&identity); out.receipts.remove(&identity); }
        if identity_evictions != 0 { eprintln!("model request identities: evicted {identity_evictions} oldest identities"); }
        while out.receipts.len() > out.limits.max_receipts || out.receipts.retained_bytes > MAX_RECEIPT_BYTES {
            out.receipts.pop_front();
        }
        let evicted = old_receipts - out.receipts.len();
        if evicted != 0 { eprintln!("model receipts: evicted {evicted} oldest receipts"); }
        Ok(out)
    }
}

/// Objects removed by the leading whole-object deletions of a transaction.
/// Execute never reads such a mesh before removing it, except as vertex colour
/// provenance, which it snapshots before the first operation; coloured objects
/// therefore stay excluded so replay observes identical inputs.
fn released_objects<'a>(state: &State, ops: &'a [Operation]) -> BTreeSet<&'a str> {
    let mut released = BTreeSet::new();
    for op in ops {
        let Operation::DeleteObject { object } = op else { break };
        let coloured = state.surface.vertex_colors.keys().any(|(name, _)| name == object);
        if state.objects.contains_key(object) && !coloured {
            released.insert(object.as_str());
        }
    }
    released
}
/// Copy of the state for execution, with released objects replaced by empty
/// stand-ins so deleted heavy geometry is never duplicated.
fn candidate_state(state: &State, released: &BTreeSet<&str>) -> State {
    State {
        objects: state
            .objects
            .iter()
            .map(|(name, mesh)| {
                let mesh = if released.contains(name.as_str()) { mesh::Mesh::new().into() } else { mesh.clone() };
                (name.clone(), mesh)
            })
            .collect(),
        materials: state.materials.clone(),
        skeleton: state.skeleton.clone(),
        clips: state.clips.clone(),
        scene: state.scene.clone(),
        selections: state.selections.clone(),
        rig: state.rig.clone(),
        surface: state.surface.clone(),
        soft_body: state.soft_body.clone(),
    }
}
/// Eviction that appending `added` would cause: oldest receipts first while
/// over the count or byte bound, then the new receipt itself. Returns the
/// identities of leading receipts evicted and whether `added` is retained.
fn plan_receipt_eviction(receipts: &Receipts, added: &Receipt, max_receipts: usize) -> (HashSet<[u8; 32]>, bool) {
    let mut bytes = receipts.retained_bytes.saturating_add(added.retained_bytes);
    let mut count = receipts.len() + 1;
    let mut evicted = HashSet::new();
    let mut oldest = receipts.iter();
    while count > max_receipts || bytes > MAX_RECEIPT_BYTES {
        if let Some(receipt) = oldest.next() {
            bytes -= receipt.retained_bytes;
            evicted.insert(sha256(receipt.id.as_bytes()));
        } else {
            return (evicted, false);
        }
        count -= 1;
    }
    (evicted, true)
}
/// Round excess history up to whole half-window batches. Reopening a larger
/// source can combine several batches in one replay and checkpoint measure.
fn history_fold_count(applied: usize, window: usize) -> usize {
    let excess = applied.saturating_sub(window);
    let batch = (window / 2).max(1);
    excess.div_ceil(batch).saturating_mul(batch).min(applied)
}
/// Checkpoint section of a source: serialized in full, or stood in for by its
/// measurement when only the bound on the remaining sections is needed.
/// For measured sections the returned bytes omit the
/// checkpoint, so callers use them for admission rather than as a document.
enum CheckpointSource<'a> {
    State(&'a State),
    Measured(SourceMeasure),
}
struct SourceParts<'a> {
    head: Head,
    checkpoint: CheckpointSource<'a>,
    cursor: usize,
    log: &'a [&'a [Operation]],
    receipts: &'a [&'a Receipt],
    identities: &'a VecDeque<[u8; 32]>,
    /// Appended in acceptance order, after eviction of the oldest identities.
    added_identity: Option<[u8; 32]>,
}
fn encode_source(parts: &SourceParts<'_>, limits: &Limits, ctx: &mut mesh::Context) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    write_source(&mut w, parts, limits, ctx)?;
    Ok(w.bytes)
}
fn write_source(w: &mut Writer, parts: &SourceParts<'_>, limits: &Limits, ctx: &mut mesh::Context) -> Result<()> {
    let measured = match &parts.checkpoint {
        CheckpointSource::Measured(m) => m.bytes,
        CheckpointSource::State(_) => 0,
    };
    // Measured checkpoint bytes participate in work accounting without a copy.
    w.raw(b"MPMODEL\0")?;
    w.u32(SOURCE_VERSION)?;
    write_head(w, parts.head)?;
    match &parts.checkpoint {
        CheckpointSource::State(state) => write_state(w, state, limits, ctx, true)?,
        CheckpointSource::Measured(m) => ctx.checkpoint(m.work)?,
    }
    w.count(parts.cursor)?;
    w.count(parts.log.len())?;
    for ops in parts.log {
        ctx.checkpoint(1)?;
        write_ops(w, ops, limits)?;
    }
    w.count(parts.receipts.len())?;
    for receipt in parts.receipts {
        write_receipt(w, receipt)?;
    }
    let identity_count = parts.identities.len().saturating_add(parts.added_identity.is_some() as usize);
    // Apply eviction in the candidate encoding too, without mutating the document.
    let evicted = if parts.added_identity.is_some() { identity_count.saturating_sub(MAX_REQUEST_IDENTITIES) } else { 0 };
    w.count(identity_count - evicted)?;
    for identity in parts.identities.iter().skip(evicted) { w.raw(identity)?; }
    if let Some(identity) = parts.added_identity { w.raw(&identity)?; }
    ctx.checkpoint(w.len().saturating_add(measured) as u64)?;
    Ok(())
}

fn write_receipt(w: &mut Writer, receipt: &Receipt) -> Result<()> {
        w.string(&receipt.id)?;
        w.raw(&receipt.fingerprint)?;
        write_head(w, receipt.head)?;
        w.count(receipt.results.len())?;
        for result in &receipt.results {
            w.string(&result.object)?;
            w.count(result.faces.len())?;
            for face in &result.faces {
                w.u64(face.0)?;
            }
            w.count(result.vertices.len())?;
            for vertex in &result.vertices {
                w.u64(vertex.0)?;
            }
            w.count(result.metrics.len())?;
            for (name, value) in &result.metrics { w.string(name)?; w.u64(*value as u64)?; }
        }
    Ok(())
}

fn check_name(name: &str, _limits: &Limits) -> Result<()> {
    if name.is_empty() || name.chars().any(char::is_control) {
        Err(Error::Invalid(
            "name is empty or contains control characters",
        ))
    } else {
        Ok(())
    }
}
fn check_material(value: &Material, limits: &Limits) -> Result<()> {
    if value
        .color
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err(Error::Invalid("material color"));
    }
    if !value.base_color_png.is_empty() {
        let b = &value.base_color_png;
        if b.len() < 33 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
            return Err(Error::Invalid("base color must be a PNG"));
        }
        let width = u32::from_be_bytes(b[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(b[20..24].try_into().unwrap());
        if width == 0
            || height == 0
            || width > limits.max_texture_dimension
            || height > limits.max_texture_dimension
        {
            return Err(Error::Budget("texture dimensions"));
        }
    }
    Ok(())
}
pub(crate) fn execute(
    state: &mut State,
    ops: &[Operation],
    limits: &Limits,
    ctx: &mut mesh::Context,
) -> Result<Vec<OperationResult>> {
    let results = execute_with(state, ops, limits, ctx, Execution::Transaction, &mut 0)?;
    // A document's state holds the values its source decodes to: an edit, an
    // undo/redo replay and a reopen then compile the same bytes.
    state.canonicalize_zeros();
    Ok(results)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Execution { Transaction, Workspace, Program }

/// A transaction holds each operation to the execution budget minus the rest
/// of the state (a [`Document`] transaction's admission). A [`Workspace`]
/// skips that per-operation pass over the state; operations still check
/// their own working budget.
fn execute_with(
    state: &mut State,
    ops: &[Operation],
    limits: &Limits,
    ctx: &mut mesh::Context,
    execution: Execution,
    failed_operation: &mut usize,
) -> Result<Vec<OperationResult>> {
    if ops.is_empty() {
        return Err(Error::Budget("transaction operations"));
    }
    let mut out = Vec::new();
    let execution_budget = ctx.limits.max_bytes;
    let accounted = execution == Execution::Transaction;
    // Stable, fully painted vertices need no provenance copy. Sparse paint
    // still needs the original interpolation even in an attribute-only batch.
    let changes_vertices = ops.iter().any(|op| !matches!(op,
        Operation::Surface(_) | Operation::Selection(_) | Operation::SetMaterial { .. }
        | Operation::TextureSolid { .. } | Operation::PaintTexture { .. }
        | Operation::SetClip { .. } | Operation::DeleteClip { .. } | Operation::SetSkeleton { .. }
        | Operation::AutoWeights { .. } | Operation::SetWeights { .. } | Operation::Transform { .. }
        | Operation::Smooth { .. } | Operation::CornerNormal { .. } | Operation::EdgeAttributes { .. }
        | Operation::SetUv { .. } | Operation::ProjectUv { .. } | Operation::AssignMaterial { .. }
        | Operation::MeshEditing(crate::MeshEditingOperation { action: crate::Action::Normals { .. }, .. })
    ));
    let mut color_counts = BTreeMap::new();
    for (name, _) in state.surface.vertex_colors.keys() { *color_counts.entry(name).or_insert(0usize) += 1; }
    let colored_objects = color_counts.into_iter().filter_map(|(name, count)|
        (changes_vertices || state.editable(name).is_some_and(|m| count < m.vertices().len())).then_some(name)
    ).collect::<BTreeSet<_>>();
    let color_source_bytes=state.editable_objects().filter(|(name,_)|colored_objects.contains(name)).map(|(n,m)|n.len()+m.memory_bytes()+128).sum::<usize>();
    if accounted{let needed=color_source_bytes.saturating_add(state.copy_bytes());if needed>execution_budget{return Err(Error::WorkingBytes{what:"vertex color provenance snapshot",needed,limit:execution_budget});}}
    let color_sources = {
        let _clone = crate::build_detail::GeometryTimer::new(crate::build_detail::GeometryPhase::Clone);
        if colored_objects.is_empty(){None}else{Some(state.editable_objects().filter(|(name,_)|colored_objects.contains(name)).map(|(n,m)|(n.clone(),m.clone())).collect::<BTreeMap<_,_>>())}
    };
    drop(colored_objects);
    let mut construction_cache = crate::construction::ConstructionCache::new(ops);
    for (index, op) in ops.iter().enumerate() {
        *failed_operation = index;
        state.check_geometry_op(index, op)?;
        let _operation_timer = crate::build_detail::OperationTimer::new(op);
        if accounted {
            let target = op
                .object_name()
                .and_then(|name| state.editable(name))
                .map_or(0, mesh::Mesh::memory_bytes);
            let pinned = state.copy_bytes().saturating_sub(target).saturating_add(color_source_bytes);
            ctx.limits.max_bytes = execution_budget
                .checked_sub(pinned)
                .ok_or(Error::WorkingBytes { what: "candidate working bytes", needed: pinned, limit: execution_budget })?;
        }
        ctx.checkpoint(1)?;
        match op {
            Operation::Baked { object, mesh } => {
                check_name(object, limits)?;
                if state.objects.contains_key(object) { return Err(Error::DuplicateObject(object.clone())); }
                mesh.view().validate()?;
                state.objects.insert(object.clone(), ObjectGeometry::Baked(mesh.clone()));
                out.push(OperationResult { object: object.clone(), ..Default::default() });
                continue;
            }
            Operation::SoftBodyUnbind => {
                out.push(crate::soft_body::unbind(state, ctx)?);
                continue;
            }
            Operation::SoftBody(op) => {
                out.push(op.apply(state, limits, ctx)?);
                continue;
            }
            Operation::Selection(op) => {
                out.push(op.apply(state, limits, ctx)?);
                continue;
            }
            Operation::Construction(op) => {
                out.push(op.apply_cached_in_scene(&mut state.objects, &state.scene, limits, ctx, &mut construction_cache)?);
                if matches!(op.kind, crate::Construction::Decal { .. }) { state.scene.paint.insert(op.object.clone()); }
                continue;
            }
            Operation::Rig(op) => {
                out.push(crate::RigState::apply(op, state, limits, ctx)?);
                continue;
            }
            Operation::MeshEditing(op) => {
                out.push(op.apply_in_scene(&mut state.objects, &state.scene, limits, ctx)?);
                continue;
            }
            Operation::Surface(op) => {
                let texture_start = (!matches!(op, crate::SurfaceOperation::VertexPaint { .. } | crate::SurfaceOperation::VertexColors { .. })).then(crate::program::ProgramInstant::now);
                if matches!(op,crate::SurfaceOperation::Bake{..}|crate::SurfaceOperation::ProjectedStroke{..}) {
                    let cache=state.scene.evaluated_meshes(state,ctx)?;
                    let mut world=std::collections::BTreeMap::new();
                    for (name,source) in &cache{ctx.checkpoint(1)?;let mut mesh=(**source).clone();let matrix=state.scene.world_matrix(name)?;let ids=mesh.vertices().iter().map(|v|v.id).collect::<Vec<_>>();mesh.transform(&ids,matrix,ctx)?;world.insert(name.clone(),mesh);}
                    state.surface.apply_disposable(op,&world,&state.materials,limits,ctx)?;
                } else {state.surface.apply_disposable(op, &state.objects, &state.materials, limits, ctx)?;}
                if let Some(start) = texture_start {
                    let elapsed = start.elapsed();
                    crate::program::record_texture_time(elapsed);
                    crate::build_detail::record_surface(op, &state.surface, elapsed);
                }
                out.push(OperationResult::default());
                continue;
            }
            Operation::Scene(op) => {
                out.push(crate::SceneState::apply(op, state, limits, ctx)?);
                continue;
            }
            Operation::TextureSolid {
                material,
                width,
                height,
                color,
            } => {
                if state.surface.materials.contains_key(material) { return Err(Error::Invalid("layered material requires surface layer operations")); }
                if !state.materials.contains_key(material) {
                    return Err(Error::Invalid("unknown texture material"));
                }
                texture_admission(*width, *height, 0, ctx)?;
                let texture_start = crate::program::ProgramInstant::now();
                let texture = crate::Texture::solid(*width, *height, *color, limits)?;
                ctx.checkpoint(texture.rgb.len() as u64)?;
                let png = texture.to_png(limits)?;
                state.materials.get_mut(material).unwrap().base_color_png = png;
                crate::program::record_texture_time(texture_start.elapsed());
                crate::build_detail::record_texture(Some(*material), crate::SurfaceChannel::BaseColor, "solid", *width, *height, texture_start.elapsed());
                out.push(OperationResult::default());
                continue;
            }
            Operation::PaintTexture {
                material,
                center,
                radius,
                color,
            } => {
                if state.surface.materials.contains_key(material) { return Err(Error::Invalid("layered material requires surface layer operations")); }
                let material = state
                    .materials
                    .get_mut(material)
                    .ok_or(Error::Invalid("unknown texture material"))?;
                if material.base_color_png.is_empty() {
                    return Err(Error::Invalid(
                        "paint requires an existing material texture",
                    ));
                }
                png_admission(&material.base_color_png, ctx)?;
                let texture_start = crate::program::ProgramInstant::now();
                let mut texture = crate::Texture::from_png(&material.base_color_png, limits)?;
                texture.paint_disk_ctx(*center, *radius, *color, limits, ctx)?;
                material.base_color_png = texture.to_png(limits)?;
                crate::program::record_texture_time(texture_start.elapsed());
                out.push(OperationResult::default());
                continue;
            }
            Operation::SetSkeleton { skeleton } => {
                skeleton.validate(limits, ctx)?;
                if let Some(old) = &state.skeleton {
                    for (_, mesh) in state.editable_objects() {
                        for v in mesh.vertices() {
                            for weight in &v.weights {
                                let index = weight.joint as usize;
                                if old.joints.get(index).map(|j| j.name.as_str())
                                    != skeleton.joints.get(index).map(|j| j.name.as_str())
                                {
                                    return Err(Error::Invalid("skeleton replacement would retarget weighted joint identities"));
                                }
                            }
                        }
                    }
                }
                state.skeleton = Some(skeleton.clone());
                out.push(OperationResult::default());
                continue;
            }
            Operation::SetClip { clip } => {
                let skeleton = state
                    .skeleton
                    .as_ref()
                    .ok_or(Error::Invalid("animation requires a skeleton"))?;
                clip.validate(skeleton, limits, ctx)?;
                state.clips.insert(clip.name.clone(), clip.clone());
                out.push(OperationResult::default());
                continue;
            }
            Operation::DeleteClip { name } => {
                if state.clips.remove(name).is_none() {
                    return Err(Error::Invalid("unknown animation clip"));
                }
                state.rig.clip_options.remove(name);
                out.push(OperationResult::default());
                continue;
            }
            _ => {}
        }
        if let Operation::SetMaterial { material, value } = op {
            check_material(value, limits)?;
            if !value.base_color_png.is_empty() {
                png_admission(&value.base_color_png, ctx)?;
                crate::Texture::from_png(&value.base_color_png, limits)?;
            }
            if state.surface.materials.contains_key(material) { return Err(Error::Invalid("rich material requires surface_material; legacy material would discard layers")); }
            state.materials.insert(*material, value.clone());
            out.push(OperationResult::default());
            continue;
        }
        let object = match op {
            Operation::Cube { object, .. }
            | Operation::Plane { object, .. }
            | Operation::ImportMesh { object, .. }
            | Operation::DeleteObject { object }
            | Operation::Transform { object, .. }
            | Operation::Extrude { object, .. }
            | Operation::DeleteFaces { object, .. }
            | Operation::Mirror { object, .. }
            | Operation::SetUv { object, .. }
            | Operation::SetWeights { object, .. }
            | Operation::AssignMaterial { object, .. }
            | Operation::Inset { object, .. }
            | Operation::Weld { object, .. }
            | Operation::EdgeAttributes { object, .. }
            | Operation::CornerNormal { object, .. }
            | Operation::AutoWeights { object }
            | Operation::Smooth { object, .. }
            | Operation::Subdivide { object, .. }
            | Operation::ProjectUv { object, .. }
            | Operation::Brush { object, .. } => object,
            _ => unreachable!(),
        };
        check_name(object, limits)?;
        let created = matches!(
            op,
            Operation::Cube { .. } | Operation::Plane { .. } | Operation::ImportMesh { .. }
        );
        if created {
            if state.objects.contains_key(object) {
                return Err(Error::DuplicateObject(object.clone()));
            }
            let mesh = match op {
                Operation::Cube { size, .. } => mesh::Mesh::cube(*size, ctx)?,
                Operation::Plane { size, .. } => mesh::Mesh::plane(*size, ctx)?,
                Operation::ImportMesh { source, .. } => mesh::Mesh::from_bytes(source, ctx)?,
                _ => unreachable!(),
            };
            let result = OperationResult {
                object: object.clone(),
                faces: mesh.faces().iter().map(|f| f.id).collect(),
                vertices: mesh.vertices().iter().map(|v| v.id).collect(),
                ..Default::default()
            };
            state.objects.insert(object.clone(), mesh.into());
            out.push(result);
            continue;
        }

        let mut result = OperationResult {
            object: object.clone(),
            ..Default::default()
        };
        if matches!(op, Operation::DeleteObject { .. }) {
            if !state.objects.contains_key(object) { return Err(Error::MissingObject(object.clone())); }
            state.objects.remove(object);
            state.scene.nodes.remove(object);
            state.scene.modifiers.remove(object);
            state.scene.lods.remove(object);
            state.scene.colliders.remove(object);
            state.scene.paint.remove(object);
            state.surface.vertex_colors.retain(|(name,_),_| name != object);
            // A replacement with the same name is a new object, even if
            // its mesh allocator happens to reuse the old element IDs.
            state.selections.groups.retain(|(name,_),_| name != object);
            out.push(result);
            continue;
        }
        let mesh = state.objects.get_mut(object).and_then(MeshEntry::editable_mut).ok_or_else(|| Error::MissingObject(object.clone()))?;
        match op {
            Operation::Transform {
                vertices, matrix, ..
            } => {
                mesh.transform(vertices, *matrix, ctx)?;
                result.vertices = vertices.clone();
            }
            Operation::Extrude { face, offset, .. } => {
                let change = mesh.extrude_face(*face, *offset, ctx)?;
                result.faces.push(change.cap);
                result.faces.extend(change.side_faces);
            }
            Operation::DeleteFaces { faces, .. } => {
                mesh.delete_faces(faces, ctx)?;
            }
            Operation::Mirror { axis, offset, .. } => {
                mesh.mirror(*axis as usize, *offset, ctx)?;
            }
            Operation::SetUv { corner, uv, .. } => {
                mesh.set_corner_uv(*corner, *uv, ctx)?;
            }
            Operation::SetWeights {
                vertex, weights, ..
            } => {
                mesh.set_vertex_weights(*vertex, weights, ctx)?;
                result.vertices.push(*vertex);
            }
            Operation::Inset { face, distance, .. } => {
                let inset = mesh.inset_face(*face, *distance, ctx)?;
                result.faces.push(inset.inset);
                result.faces.extend(inset.ring_faces);
            }
            Operation::Weld {
                vertices, distance, ..
            } => {
                mesh.weld(vertices, *distance, ctx)?;
                result.vertices = vertices
                    .iter()
                    .copied()
                    .filter(|id| mesh.vertex(*id).is_some())
                    .collect();
            }
            Operation::EdgeAttributes {
                edge, attributes, ..
            } => {
                mesh.set_edge_attributes(*edge, *attributes, ctx)?;
            }
            Operation::CornerNormal { corner, normal, .. } => {
                mesh.set_corner_normal(*corner, *normal, ctx)?;
            }
            Operation::AutoWeights { .. } => {
                let skeleton = state
                    .skeleton
                    .as_ref()
                    .ok_or(Error::Invalid("auto weights require a skeleton"))?;
                let globals = state.rig.global_rest(skeleton)?;
                let heads = globals.iter().map(|m| crate::transform::transform_point(*m, [0.;3])).collect::<Vec<_>>();
                let owner = state.scene.world_matrix(object)?;
                let weights = skeleton.nearest_weights(mesh, &heads, owner, ctx)?;
                mesh.set_weights_bulk(&weights, ctx)?;
                result.vertices = weights.iter().map(|(id, _)| *id).collect();
            }
            Operation::Smooth {
                vertices,
                iterations,
                factor,
                preserve_boundary,
                ..
            } => {
                mesh.smooth(vertices, *iterations, *factor, *preserve_boundary, ctx)?;
                result.vertices = vertices.clone();
            }
            Operation::Subdivide { levels, .. } => {
                mesh.subdivide(*levels, ctx)?;
                result.faces = mesh.faces().iter().map(|f| f.id).collect();
                result.vertices = mesh.vertices().iter().map(|v| v.id).collect();
            }
            Operation::ProjectUv {
                faces,
                axis,
                scale,
                offset,
                ..
            } => {
                mesh.project_uv(faces, *axis as usize, *scale, *offset, ctx)?;
                result.faces = faces.clone();
            }
            Operation::Brush {
                vertices,
                center,
                radius,
                delta,
                max_displacement,
                ..
            } => {
                mesh.brush(vertices, *center, *radius, *delta, *max_displacement, ctx)?;
                result.vertices = vertices.clone();
            }
            Operation::AssignMaterial {
                faces, material, ..
            } => {
                if !state.materials.contains_key(material) {
                    return Err(Error::Invalid("unknown material"));
                }
                mesh.set_face_materials(faces, *material, ctx)?;
                result.faces = faces.clone();
            }
            _ => unreachable!(),
        }
        out.push(result);
    }
    let _validate = crate::build_detail::GeometryTimer::new(crate::build_detail::GeometryPhase::Validate);
    let needed = if execution == Execution::Workspace { state.memory_bytes() } else { state.copy_bytes() };
    if needed > execution_budget {
        return Err(Error::WorkingBytes { what: "candidate bytes", needed, limit: execution_budget });
    }
    ctx.limits.max_bytes = execution_budget;
    if let Some(before) = color_sources { state.surface.reconcile_objects(&before, &state.objects, limits, ctx)?; }
    state.selections.reconcile(&state.objects);
    state.selections.validate(state, limits)?;
    validate_surface_refs(state)?;
    for (_, mesh) in state.editable_objects() {
        for face in mesh.faces() {
            if !state.materials.contains_key(&face.material) {
                return Err(Error::Invalid("unknown face material"));
            }
        }
    }
    if let Err(error) = state.validate_baked_references() {
        // Scene records can refer to an object created later in the transaction.
        if let Error::BakedObject { object, .. } = &error {
            for (index, op) in ops.iter().enumerate().rev() {
                if matches!(op, Operation::Scene(_) | Operation::Rig(_)) {
                    if let Err(refused @ Error::BakedObject { .. }) = state.check_geometry_op(index, op) {
                        if matches!(&refused, Error::BakedObject { object: target, .. } if target == object) {
                            *failed_operation = index;
                            return Err(refused);
                        }
                    }
                }
            }
        }
        return Err(error);
    }
    validate_rig(state, limits, ctx)?;
    state.scene.validate(state, limits, ctx)?;
    state.rig.validate(state, limits, ctx)?;
    state.surface.validate(limits, ctx)?;
    crate::soft_body::validate(state, limits, ctx)?;
    Ok(out)
}

fn validate_surface_refs(state:&State)->Result<()> {
    if state.surface.materials.keys().any(|id|!state.materials.contains_key(id)) { return Err(Error::Invalid("surface references unknown material")); }
    if state.surface.vertex_colors.keys().any(|(object,id)|state.editable(object).and_then(|m|m.vertex(*id)).is_none()) { return Err(Error::Invalid("surface references unknown vertex")); }
    Ok(())
}
fn texture_admission(
    width: u32,
    height: u32,
    encoded: usize,
    ctx: &mesh::Context<'_>,
) -> Result<()> {
    let working = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(16)
        .saturating_add(encoded.saturating_mul(2));
    if working > ctx.limits.max_bytes {
        Err(Error::WorkingBytes { what: "texture working bytes", needed: working, limit: ctx.limits.max_bytes })
    } else {
        Ok(())
    }
}
fn png_admission(bytes: &[u8], ctx: &mesh::Context<'_>) -> Result<()> {
    if bytes.len() < 24 {
        return Err(Error::Invalid("PNG header"));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    texture_admission(width, height, bytes.len(), ctx)
}

fn validate_rig(state: &State, limits: &Limits, ctx: &mut mesh::Context<'_>) -> Result<()> {
    if let Some(skeleton) = &state.skeleton {
        skeleton.validate(limits, ctx)?;
        for (_, mesh) in state.editable_objects() {
            for vertex in mesh.vertices() {
                ctx.checkpoint(1)?;
                if vertex
                    .weights
                    .iter()
                    .any(|w| w.joint as usize >= skeleton.joints.len())
                {
                    return Err(Error::Invalid("weight references unknown skeleton joint"));
                }
            }
        }
        for clip in state.clips.values() {
            clip.validate(skeleton, limits, ctx)?;
        }
    } else if !state.clips.is_empty() {
        return Err(Error::Invalid("animation requires a skeleton"));
    }
    Ok(())
}

fn write_head(w: &mut Writer, head: Head) -> Result<()> {
    w.u64(head.generation)?;
    w.raw(&head.content)
}
fn read_head(r: &mut Reader) -> Result<Head> {
    Ok(Head {
        generation: r.u64()?,
        content: r.raw(32)?.try_into().unwrap(),
    })
}
// Measure the checkpoint using the model source encoding.
// The measured section stands in for the checkpoint when a transaction bounds
// the source it would produce, so this is the single source of that measure.
fn checkpoint_source_measure(state: &State, limits: &Limits, ctx: &mut mesh::Context) -> Result<SourceMeasure> {
    let mut writer = Writer::measure();
    let start = ctx.work_used();
    write_state(&mut writer, state, limits, ctx, true)?;
    Ok(SourceMeasure { bytes: writer.len(), work: ctx.work_used() - start })
}

fn verify_checkpoint(state: &State, limits: &Limits, ctx: &mut mesh::Context) -> Result<SourceMeasure> {
    let mut w = Writer::new();
    if state.baked_bytes() != 0 {
        let size = checkpoint_source_measure(state, limits, ctx)?.bytes;
        let needed = state.copies_bytes(2).saturating_add(size);
        if needed > limits.mesh.max_bytes {
            return Err(Error::WorkingBytes { what: "checkpoint verification", needed, limit: limits.mesh.max_bytes });
        }
        w.bytes.reserve_exact(size);
    }
    let start = ctx.work_used();
    write_state(&mut w, state, limits, ctx, true)?;
    let measure = SourceMeasure { bytes: w.len(), work: ctx.work_used() - start };
    let mut r = Reader::new(&w.bytes);
    read_state(&mut r, limits, ctx, Some(state))?;
    r.end()?;
    Ok(measure)
}

fn state_hash(state: &State, limits: &Limits, ctx: &mut mesh::Context) -> Result<[u8; 32]> {
    let mut w = Writer::hashing();
    write_state(&mut w, state, limits, ctx, false)?;
    ctx.checkpoint(w.len() as u64)?;
    Ok(w.finish_hash())
}
fn write_material(w: &mut Writer, value: &Material) -> Result<()> {
    for v in value.color {
        w.f64(v)?;
    }
    w.blob(&value.base_color_png)
}
fn read_material(r: &mut Reader, limits: &Limits) -> Result<Material> {
    let value = Material {
        color: [r.f64()?, r.f64()?, r.f64()?],
        base_color_png: r.blob()?.to_vec(),
    };
    check_material(&value, limits)?;
    if !value.base_color_png.is_empty() {
        crate::Texture::from_png(&value.base_color_png, limits)?;
    }
    Ok(value)
}
fn write_state(
    w: &mut Writer,
    state: &State,
    limits: &Limits,
    ctx: &mut mesh::Context,
    include_empty_extensions: bool,
) -> Result<()> {
    state.validate_baked_references()?;
    validate_rig(state, limits, ctx)?;
    state.scene.validate(state, limits, ctx)?;
    state.rig.validate(state, limits, ctx)?;
    state.surface.validate(limits, ctx)?;
    crate::soft_body::validate(state, limits, ctx)?;
    w.count(state.materials.len())?;
    for (id, value) in &state.materials {
        ctx.checkpoint(1)?;
        check_material(value, limits)?;
        w.u32(*id)?;
        write_material(w, value)?;
    }
    w.count(state.objects.len())?;
    for (name, mesh) in &state.objects {
        ctx.checkpoint(1)?;
        w.string(name)?;
        match mesh {
            ObjectGeometry::Editable(m) => {
                let bytes = m.to_bytes(ctx)?;
                w.count(bytes.len())?;
                for piece in bytes.chunks(1 << 20) { ctx.checkpoint(0)?; w.raw(piece)?; }
            }
            ObjectGeometry::Baked(m) => {
                ctx.checkpoint(m.encoded_size() as u64)?;
                w.count(m.encoded_size())?;
                m.view().write_checked(w, || Ok(ctx.checkpoint(0)?))?;
            }
        }
    }
    w.u8(state.skeleton.is_some() as u8)?;
    if let Some(skeleton) = &state.skeleton {
        write_skeleton(w, skeleton)?;
    }
    w.count(state.clips.len())?;
    for clip in state.clips.values() {
        write_clip(w, clip)?;
    }
    let mut extensions = Vec::new();
    if let Some(metadata)=&state.soft_body {
        let mut payload=Writer::new();
        crate::soft_body::write(&mut payload,metadata)?;
        extensions.push(("soft_body",payload.bytes));
    }
    if !state.scene.is_empty() {
        let mut payload = Writer::new();
        state.scene.write(&mut payload)?;
        extensions.push(("scene", payload.bytes));
    }
    if !state.surface.materials.is_empty() || !state.surface.vertex_colors.is_empty() {
        let mut payload = Writer::new();
        crate::surface::write_surface(&mut payload, &state.surface)?;
        extensions.push(("surface", payload.bytes));
    }
    if !state.selections.groups.is_empty() {
        let mut payload=Writer::new();state.selections.write(&mut payload)?;extensions.push(("selections",payload.bytes));
    }
    if !state.rig.is_empty() {
        let mut payload = Writer::new();
        state.rig.write(&mut payload)?;
        extensions.push(("rig", payload.bytes));
    }
    if include_empty_extensions || !extensions.is_empty() {
        w.raw(b"MPEXT\0")?;
        w.count(extensions.len())?;
        for (name, bytes) in extensions { w.string(name)?; w.blob(&bytes)?; }
    }
    Ok(())
}
fn read_state(r: &mut Reader, limits: &Limits, ctx: &mut mesh::Context, known: Option<&State>) -> Result<State> {
    let mut state = State::default();
    for _ in 0..r.count(32)? {
        let id = r.u32()?;
        let value = read_material(r, limits)?;
        if state.materials.insert(id, value).is_some() {
            return Err(Error::Corrupt("duplicate material"));
        }
    }
    let object_count = r.count(40)?;
    if !state.materials.contains_key(&0) {
        return Err(Error::Corrupt("missing default material"));
    }
    for _ in 0..object_count {
        let name = r.string()?;
        check_name(&name, limits)?;
        let bytes = r.blob()?;
        if bytes.starts_with(crate::baked::MAGIC) {
            ctx.checkpoint(bytes.len() as u64)?;
            let mesh = match known.and_then(|s| s.objects.get(&name)) {
                Some(ObjectGeometry::Baked(m)) => m.clone(),
                _ => std::sync::Arc::new(crate::BakedMesh::read(bytes, ctx.limits.max_bytes.saturating_sub(state.memory_bytes()))?),
            };
            if state.objects.insert(name, ObjectGeometry::Baked(mesh)).is_some() { return Err(Error::Corrupt("duplicate object")); }
            continue;
        }
        let budget = ctx.limits.max_bytes;
        ctx.limits.max_bytes = budget.saturating_sub(state.baked_bytes());
        let decoded = mesh::Mesh::from_bytes(bytes, ctx);
        ctx.limits.max_bytes = budget;
        let mesh = decoded?;
        if mesh
            .faces()
            .iter()
            .any(|f| !state.materials.contains_key(&f.material))
        {
            return Err(Error::Corrupt("face material"));
        }
        if state.objects.insert(name, mesh.into()).is_some() {
            return Err(Error::Corrupt("duplicate object"));
        }
    }
    state.skeleton = match r.u8()? {
        0 => None,
        1 => Some(read_skeleton(r, limits)?),
        _ => return Err(Error::Corrupt("skeleton flag")),
    };
    for _ in 0..r.count(8)? {
        let clip = read_clip(r, limits)?;
        if state.clips.insert(clip.name.clone(), clip).is_some() {
            return Err(Error::Corrupt("duplicate animation clip"));
        }
    }
    if r.raw(6)? != b"MPEXT\0" { return Err(Error::Corrupt("state extension marker")); }
    let mut seen = BTreeSet::new();
    for _ in 0..r.count(8)? {
        let name = r.string()?;
        if !seen.insert(name.clone()) { return Err(Error::Corrupt("duplicate state extension")); }
        let mut payload = Reader::new(r.blob()?);
        match name.as_str() {
            "selections" => state.selections=crate::SelectionState::read(&mut payload,limits)?,
            "rig" => state.rig = crate::RigState::read(&mut payload, limits)?,
            "scene" => state.scene = crate::SceneState::read(&mut payload, limits)?,
            "surface" => state.surface = crate::surface::read_surface(&mut payload, limits)?,
            "soft_body" => state.soft_body = Some(crate::soft_body::read(&mut payload, limits)?),
            _ => return Err(Error::Corrupt("unsupported state extension")),
        }
        payload.end()?;
    }
    state.validate_baked_references()?;
    state.selections.validate(&state, limits)?;
    validate_surface_refs(&state)?;
    validate_rig(&state, limits, ctx)?;
    state.scene.validate(&state, limits, ctx)?;
    state.rig.validate(&state, limits, ctx)?;
    crate::soft_body::validate(&state, limits, ctx)?;
    Ok(state)
}

fn write_ops(w: &mut Writer, ops: &[Operation], _limits: &Limits) -> Result<()> {
    w.count(ops.len())?;
    for op in ops {
        op.write(w)?;
    }
    Ok(())
}
impl Operation {
    /// The canonical encoding: the document log, transaction fingerprints, program hashes.
    pub(crate) fn write(&self, w: &mut Writer) -> Result<()> {
        let op = self;
        match op {
            Operation::Baked { object, mesh } => { w.u8(34)?; w.string(object)?; w.count(mesh.encoded_size())?; mesh.view().write(w)?; }
            Operation::SoftBodyUnbind => { w.u8(33)?; }
            Operation::SoftBody(op) => { w.u8(32)?; op.write(w)?; }
            Operation::Selection(op) => { w.u8(31)?; op.write(w)?; }
            Operation::Construction(op) => { w.u8(30)?; op.write(w)?; }
            Operation::Rig(op) => { w.u8(29)?; op.write(w)?; }
            Operation::MeshEditing(op) => { w.u8(28)?; op.write(w)?; }
            Operation::Surface(op) => { w.u8(27)?; crate::surface::write_surface_operation(w, op)?; }
            Operation::Scene(op) => { w.u8(26)?; op.write(w)?; }
            Operation::Cube { object, size } => {
                w.u8(0)?;
                w.string(object)?;
                for v in size {
                    w.f64(*v)?;
                }
            }
            Operation::Plane { object, size } => {
                w.u8(1)?;
                w.string(object)?;
                for v in size {
                    w.f64(*v)?;
                }
            }
            Operation::ImportMesh { object, source } => {
                w.u8(2)?;
                w.string(object)?;
                w.blob(source)?;
            }
            Operation::DeleteObject { object } => {
                w.u8(3)?;
                w.string(object)?;
            }
            Operation::Transform {
                object,
                vertices,
                matrix,
            } => {
                w.u8(4)?;
                w.string(object)?;
                w.count(vertices.len())?;
                for v in vertices {
                    w.u64(v.0)?;
                }
                for row in matrix {
                    for v in row {
                        w.f64(*v)?;
                    }
                }
            }
            Operation::Extrude {
                object,
                face,
                offset,
            } => {
                w.u8(5)?;
                w.string(object)?;
                w.u64(face.0)?;
                for v in offset {
                    w.f64(*v)?;
                }
            }
            Operation::DeleteFaces { object, faces } => {
                w.u8(6)?;
                w.string(object)?;
                w.count(faces.len())?;
                for v in faces {
                    w.u64(v.0)?;
                }
            }
            Operation::Mirror {
                object,
                axis,
                offset,
            } => {
                w.u8(7)?;
                w.string(object)?;
                w.u8(*axis)?;
                w.f64(*offset)?;
            }
            Operation::SetUv { object, corner, uv } => {
                w.u8(8)?;
                w.string(object)?;
                w.u64(corner.0)?;
                for v in uv {
                    w.f64(*v)?;
                }
            }
            Operation::SetWeights {
                object,
                vertex,
                weights,
            } => {
                w.u8(9)?;
                w.string(object)?;
                w.u64(vertex.0)?;
                w.count(weights.len())?;
                for weight in weights {
                    w.u32(weight.joint)?;
                    w.f64(weight.weight)?;
                }
            }
            Operation::AssignMaterial {
                object,
                faces,
                material,
            } => {
                w.u8(10)?;
                w.string(object)?;
                w.count(faces.len())?;
                for v in faces {
                    w.u64(v.0)?;
                }
                w.u32(*material)?;
            }
            Operation::SetMaterial { material, value } => {
                w.u8(11)?;
                w.u32(*material)?;
                write_material(w, value)?;
            }
            Operation::Inset {
                object,
                face,
                distance,
            } => {
                w.u8(12)?;
                w.string(object)?;
                w.u64(face.0)?;
                w.f64(*distance)?;
            }
            Operation::Weld {
                object,
                vertices,
                distance,
            } => {
                w.u8(13)?;
                w.string(object)?;
                w.count(vertices.len())?;
                for v in vertices {
                    w.u64(v.0)?;
                }
                w.f64(*distance)?;
            }
            Operation::EdgeAttributes {
                object,
                edge,
                attributes,
            } => {
                w.u8(14)?;
                w.string(object)?;
                w.u64(edge.0 .0)?;
                w.u64(edge.1 .0)?;
                w.u8(attributes.seam as u8)?;
                w.f64(attributes.crease)?;
            }
            Operation::CornerNormal {
                object,
                corner,
                normal,
            } => {
                w.u8(15)?;
                w.string(object)?;
                w.u64(corner.0)?;
                w.u8(normal.is_some() as u8)?;
                if let Some(normal) = normal {
                    for n in normal {
                        w.f64(*n)?;
                    }
                }
            }
            Operation::TextureSolid {
                material,
                width,
                height,
                color,
            } => {
                w.u8(16)?;
                w.u32(*material)?;
                w.u32(*width)?;
                w.u32(*height)?;
                w.raw(color)?;
            }
            Operation::PaintTexture {
                material,
                center,
                radius,
                color,
            } => {
                w.u8(17)?;
                w.u32(*material)?;
                for c in center {
                    w.f64(*c)?;
                }
                w.f64(*radius)?;
                w.raw(color)?;
            }
            Operation::SetSkeleton { skeleton } => {
                w.u8(18)?;
                write_skeleton(w, skeleton)?;
            }
            Operation::SetClip { clip } => {
                w.u8(19)?;
                write_clip(w, clip)?;
            }
            Operation::DeleteClip { name } => {
                w.u8(20)?;
                w.string(name)?;
            }
            Operation::AutoWeights { object } => {
                w.u8(21)?;
                w.string(object)?;
            }
            Operation::Smooth {
                object,
                vertices,
                iterations,
                factor,
                preserve_boundary,
            } => {
                w.u8(22)?;
                w.string(object)?;
                w.count(vertices.len())?;
                for v in vertices {
                    w.u64(v.0)?;
                }
                w.u32(*iterations)?;
                w.f64(*factor)?;
                w.u8(*preserve_boundary as u8)?;
            }
            Operation::Subdivide { object, levels } => {
                w.u8(23)?;
                w.string(object)?;
                w.u32(*levels)?;
            }
            Operation::ProjectUv {
                object,
                faces,
                axis,
                scale,
                offset,
            } => {
                w.u8(24)?;
                w.string(object)?;
                w.count(faces.len())?;
                for f in faces {
                    w.u64(f.0)?;
                }
                w.u8(*axis)?;
                for v in scale {
                    w.f64(*v)?;
                }
                for v in offset {
                    w.f64(*v)?;
                }
            }
            Operation::Brush {
                object,
                vertices,
                center,
                radius,
                delta,
                max_displacement,
            } => {
                w.u8(25)?;
                w.string(object)?;
                w.count(vertices.len())?;
                for v in vertices {
                    w.u64(v.0)?;
                }
                for v in center {
                    w.f64(*v)?;
                }
                w.f64(*radius)?;
                for v in delta {
                    w.f64(*v)?;
                }
                w.f64(*max_displacement)?;
            }
        }
        Ok(())
    }
}
fn read_ops(r: &mut Reader, limits: &Limits) -> Result<Vec<Operation>> {
    read_ops_budgeted(r, limits, limits.mesh.max_bytes)
}
fn read_ops_budgeted(r: &mut Reader, limits: &Limits, mut available: usize) -> Result<Vec<Operation>> {
    let mut ops = Vec::new();
    for _ in 0..r.count(1)? {
        let op = Operation::read_budgeted(r, limits, available)?;
        available = available.saturating_sub(op.memory_bytes());
        ops.push(op);
    }
    Ok(ops)
}
impl Operation {
    /// Decode one operation of the canonical encoding (`write`).
    pub(crate) fn read(r: &mut Reader, limits: &Limits) -> Result<Self> {
        Self::read_budgeted(r, limits, limits.mesh.max_bytes)
    }
    fn read_budgeted(r: &mut Reader, limits: &Limits, available: usize) -> Result<Self> {
        let tag = r.u8()?;
        if tag == 11 {
            let material = r.u32()?;
            let value = read_material(r, limits)?;
            return Ok(Operation::SetMaterial { material, value });
        }
        let global = match tag {
            34 => Some(Operation::Baked { object: r.string()?, mesh: std::sync::Arc::new(crate::BakedMesh::read(r.blob()?, available)?) }),
            33 => Some(Operation::SoftBodyUnbind),
            32 => Some(Operation::SoftBody(crate::SoftBodyBind::read(r,limits)?)),
            31 => Some(Operation::Selection(crate::SelectionOperation::read(r,limits)?)),
            30 => Some(Operation::Construction(crate::ConstructionOperation::read(r,limits)?)),
            29 => Some(Operation::Rig(crate::RigOperation::read(r,limits)?)),
            28 => Some(Operation::MeshEditing(crate::MeshEditingOperation::read(r,limits)?)),
            27 => Some(Operation::Surface(crate::surface::read_surface_operation(r, limits)?)),
            26 => Some(Operation::Scene(crate::SceneOperation::read(r,limits)?)),
            16 => Some(Operation::TextureSolid {
                material: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
                color: r.raw(3)?.try_into().unwrap(),
            }),
            17 => Some(Operation::PaintTexture {
                material: r.u32()?,
                center: [r.f64()?, r.f64()?],
                radius: r.f64()?,
                color: r.raw(3)?.try_into().unwrap(),
            }),
            18 => Some(Operation::SetSkeleton {
                skeleton: read_skeleton(r, limits)?,
            }),
            19 => Some(Operation::SetClip {
                clip: read_clip(r, limits)?,
            }),
            20 => Some(Operation::DeleteClip {
                name: r.string()?,
            }),
            _ => None,
        };
        if let Some(op) = global {
            return Ok(op);
        }
        let object = r.string()?;
        let op = match tag {
            0 => Operation::Cube {
                object,
                size: [r.f64()?, r.f64()?, r.f64()?],
            },
            1 => Operation::Plane {
                object,
                size: [r.f64()?, r.f64()?],
            },
            2 => Operation::ImportMesh {
                object,
                source: r.blob()?.to_vec(),
            },
            3 => Operation::DeleteObject { object },
            4 => {
                let mut vertices = Vec::new();
                for _ in 0..r.count(8)? {
                    vertices.push(mesh::VertexId(r.u64()?));
                }
                let mut matrix = [[0.0; 4]; 4];
                for row in &mut matrix {
                    for v in row {
                        *v = r.f64()?;
                    }
                }
                Operation::Transform {
                    object,
                    vertices,
                    matrix,
                }
            }
            5 => Operation::Extrude {
                object,
                face: mesh::FaceId(r.u64()?),
                offset: [r.f64()?, r.f64()?, r.f64()?],
            },
            6 => {
                let mut faces = Vec::new();
                for _ in 0..r.count(8)? {
                    faces.push(mesh::FaceId(r.u64()?));
                }
                Operation::DeleteFaces { object, faces }
            }
            7 => Operation::Mirror {
                object,
                axis: r.u8()?,
                offset: r.f64()?,
            },
            8 => Operation::SetUv {
                object,
                corner: mesh::CornerId(r.u64()?),
                uv: [r.f64()?, r.f64()?],
            },
            9 => {
                let vertex = mesh::VertexId(r.u64()?);
                let mut weights = Vec::new();
                for _ in 0..r.count(12)? {
                    weights.push(mesh::JointWeight {
                        joint: r.u32()?,
                        weight: r.f64()?,
                    });
                }
                Operation::SetWeights {
                    object,
                    vertex,
                    weights,
                }
            }
            10 => {
                let mut faces = Vec::new();
                for _ in 0..r.count(8)? {
                    faces.push(mesh::FaceId(r.u64()?));
                }
                Operation::AssignMaterial {
                    object,
                    faces,
                    material: r.u32()?,
                }
            }
            12 => Operation::Inset {
                object,
                face: mesh::FaceId(r.u64()?),
                distance: r.f64()?,
            },
            13 => {
                let mut vertices = Vec::new();
                for _ in 0..r.count(8)? {
                    vertices.push(mesh::VertexId(r.u64()?));
                }
                Operation::Weld {
                    object,
                    vertices,
                    distance: r.f64()?,
                }
            }
            14 => {
                let edge = mesh::EdgeKey(mesh::VertexId(r.u64()?), mesh::VertexId(r.u64()?));
                let seam = match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Corrupt("seam flag")),
                };
                Operation::EdgeAttributes {
                    object,
                    edge,
                    attributes: mesh::EdgeAttributes {
                        seam,
                        crease: r.f64()?,
                    },
                }
            }
            15 => {
                let corner = mesh::CornerId(r.u64()?);
                let normal = match r.u8()? {
                    0 => None,
                    1 => Some([r.f64()?, r.f64()?, r.f64()?]),
                    _ => return Err(Error::Corrupt("normal flag")),
                };
                Operation::CornerNormal {
                    object,
                    corner,
                    normal,
                }
            }
            21 => Operation::AutoWeights { object },
            22 => {
                let mut vertices = Vec::new();
                for _ in 0..r.count(8)? {
                    vertices.push(mesh::VertexId(r.u64()?));
                }
                let iterations = r.u32()?;
                let factor = r.f64()?;
                let preserve_boundary = match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Corrupt("boundary flag")),
                };
                Operation::Smooth {
                    object,
                    vertices,
                    iterations,
                    factor,
                    preserve_boundary,
                }
            }
            23 => Operation::Subdivide {
                object,
                levels: r.u32()?,
            },
            24 => {
                let mut faces = Vec::new();
                for _ in 0..r.count(8)? {
                    faces.push(mesh::FaceId(r.u64()?));
                }
                Operation::ProjectUv {
                    object,
                    faces,
                    axis: r.u8()?,
                    scale: [r.f64()?, r.f64()?],
                    offset: [r.f64()?, r.f64()?],
                }
            }
            25 => {
                let mut vertices = Vec::new();
                for _ in 0..r.count(8)? {
                    vertices.push(mesh::VertexId(r.u64()?));
                }
                Operation::Brush {
                    object,
                    vertices,
                    center: [r.f64()?, r.f64()?, r.f64()?],
                    radius: r.f64()?,
                    delta: [r.f64()?, r.f64()?, r.f64()?],
                    max_displacement: r.f64()?,
                }
            }
            _ => return Err(Error::Corrupt("unknown operation")),
        };
        Ok(op)
    }
}

#[cfg(test)]
mod history_budget_tests {
    use super::*;
    fn apply(doc:&mut Document,id:&str,operations:Vec<Operation>)->Result<Applied>{doc.apply(Transaction{request_id:id.into(),expected:doc.head(),operations},None)}
    fn edit(doc:&Document,index:usize)->Operation{Operation::SetUv{object:"body".into(),corner:doc.object("body").unwrap().corners()[0].id,uv:[index as f64*0.01,0.25]}}
    #[test]
    fn indexed_material_runs_reopen_with_only_their_referenced_vertices() {
        use crate::{BakedMesh, MeshPrimitive, ProgramOp};
        use makepad_gltf::{decode_mesh_primitive, load_gltf_from_bytes};
        let positions: Vec<_> = (0..96).map(|i| [(i / 3) as f32, (i % 3 == 1) as u8 as f32, (i % 3 == 2) as u8 as f32]).collect();
        let uvs: Vec<_> = positions.iter().map(|p| [p[0], p[2]]).collect();
        let mut indices: Vec<_> = (0..96).rev().collect();
        indices[60] = indices[59];
        let normals = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]].into_iter().cycle().take(positions.len()).collect();
        let mut mesh = BakedMesh { normals, positions, uvs: Some(uvs), material_bytes: None,
            indices, runs: vec![(7, 0..60), (0, 60..81), (7, 81..96)] };
        let payload = 96 * (12 + 12 + 8 + 4) + 3 * 12;
        assert_eq!(mesh.memory_bytes(), payload);
        // The original UV-only blob used 1 here and had no stream between UVs and indices.
        let mut legacy = b"MPBAKE01".to_vec();
        for count in [96u32, 96, 3] { legacy.extend_from_slice(&count.to_le_bytes()); }
        legacy.push(1);
        for v in mesh.positions.iter().chain(&mesh.normals).flatten().chain(mesh.uvs.as_ref().unwrap().iter().flatten()) {
            legacy.extend_from_slice(&v.to_le_bytes());
        }
        for word in mesh.indices.iter().copied().chain(mesh.runs.iter().flat_map(|(m, r)| [*m, r.start, r.end])) {
            legacy.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(BakedMesh::read(&legacy, payload).unwrap(), mesh, "the UV-only source remains readable");
        let mut encoded = Writer::new();
        mesh.view().write(&mut encoded).unwrap();
        assert_eq!(encoded.bytes, legacy, "absent material bytes preserve the original source encoding");
        legacy[20] |= 4;
        assert!(matches!(BakedMesh::read(&legacy, payload), Err(Error::Corrupt(_))), "unknown attribute flags are corrupt");
        let patterns = [[0; 4], [255; 4], [0, 0, 0xc0, 0x7f], [1, 0, 0x80, 0x7f],
            [0, 0, 0, 0x80], [1, 0, 0, 0], [17, 31, 173, 240]];
        mesh.material_bytes = Some(patterns.into_iter().cycle().take(mesh.positions.len()).collect());
        let uvs = mesh.uvs.take();
        let mut bytes_only = Writer::new();
        mesh.view().write(&mut bytes_only).unwrap();
        assert_eq!(bytes_only.bytes[20], 2, "material bytes have their own flag without UVs");
        assert_eq!(BakedMesh::read(&bytes_only.bytes, mesh.memory_bytes()).unwrap(), mesh, "byte-only geometry reopens");
        mesh.uvs = uvs;
        let payload = payload + 4 * mesh.positions.len();
        assert_eq!(mesh.memory_bytes(), payload, "material byte capacity is admitted");
        let mesh = std::sync::Arc::new(mesh);
        let primitive = MeshPrimitive::Baked { object: "strip".into(), mesh: mesh.clone() };
        let program = [ProgramOp::Op(Operation::SetMaterial { material: 7, value: Material { color: [0.25, 0.5, 0.75], base_color_png: vec![] } }),
            ProgramOp::Mesh(primitive)];
        let admitted = crate::program_bytes(&program).unwrap().len();
        assert!((payload..2 * payload).contains(&admitted), "program admission {admitted} for {payload} bytes of arrays");
        let built = crate::build_program(&program, Limits::default(), &mut |_| unreachable!(), None).unwrap();
        assert!(matches!(&built.document.state.objects["strip"], ObjectGeometry::Baked(stored) if std::sync::Arc::ptr_eq(stored, &mesh)),
            "the compiled document shares the program's allocation");
        let mut spare = (*mesh).clone();
        spare.indices.reserve_exact(512);
        spare.material_bytes.as_mut().unwrap().reserve_exact(16_384);
        let spare = std::sync::Arc::new(spare);
        let op = Operation::Baked { object: "other".into(), mesh: spare.clone() };
        assert!(op.memory_bytes() >= payload + 512 * 4 + 16_384 * 4, "the operation charges spare index and attribute capacity");
        let shared = [program[0].clone(), ProgramOp::Mesh(MeshPrimitive::Baked { object: "strip".into(), mesh: spare.clone() }),
            ProgramOp::Op(op)];
        let shared_limit = Limits { mesh: mesh::Limits { max_bytes: spare.memory_bytes() + 8 * payload }, ..Default::default() };
        crate::build_program(&shared, shared_limit, &mut |_| unreachable!(), None)
            .expect("two names and their lowered operations charge the shared buffers once");
        let resident = built.document.memory_bytes();
        assert!((payload..2 * payload).contains(&resident), "resident checkpoint and current geometry cost {resident}");
        let loaded = load_gltf_from_bytes(&built.compiled.glb, None).unwrap();
        let prims = &loaded.document.meshes_slice()[0].primitives;
        assert_eq!(prims.len(), mesh.runs.len());
        assert_eq!(built.compiled.vertices, 96, "the shared corner is exported twice and unused vertex 35 is omitted");
        assert_ne!(prims[0].material, prims[1].material);
        assert_eq!(prims[0].material, prims[2].material, "a repeated material keeps its own run");
        for (i, (material, range)) in mesh.runs.iter().enumerate() {
            let primitive = decode_mesh_primitive(&loaded, 0, i).unwrap();
            let corners = &mesh.indices[range.start as usize..range.end as usize];
            let referenced: Vec<_> = corners.iter().copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            for &accessor in prims[i].attributes.values() {
                assert_eq!(loaded.document.accessors_slice()[accessor].count, referenced.len(), "run {i} accessor count");
            }
            for (local, &original) in referenced.iter().enumerate() {
                let j = original as usize;
                assert_eq!(primitive.positions[local], mesh.positions[j], "ascending positions for run {i}");
                assert_eq!(primitive.normals.as_ref().unwrap()[local], mesh.normals[j], "normal for run {i}");
                assert_eq!(primitive.texcoords0.as_ref().unwrap()[local], mesh.uvs.as_ref().unwrap()[j], "UV for run {i}");
            }
            let decoded: Vec<_> = primitive.indices.iter().map(|&j| referenced[j as usize]).collect();
            assert_eq!(decoded, corners, "triangle order and corner identity for run {i}");
            let bytes = &loaded.document.accessors_slice()[prims[i].attributes["_MATERIAL_BYTES"]];
            let view = &loaded.document.buffer_views_slice()[bytes.buffer_view.unwrap()];
            let start = view.byte_offset.unwrap_or(0) + bytes.byte_offset.unwrap_or(0);
            let expected: Vec<_> = referenced.iter().flat_map(|&j| mesh.material_bytes.as_ref().unwrap()[j as usize]).collect();
            assert_eq!(&loaded.buffers[view.buffer][start..start + expected.len()], expected, "material bytes for run {i}");
            assert_eq!(built.compiled.primitives[i].material, *material);
            let expected = &built.document.materials()[material].color;
            let factor = loaded.document.materials_slice()[primitive.material.unwrap()].pbr_metallic_roughness.as_ref().unwrap().base_color_factor.unwrap_or([1.;4]);
            assert_eq!(&factor[..3], &expected.map(|v| v as f32));
        }
        let row = built.document.to_bytes(None).unwrap();
        let restored = Document::from_bytes(&row, Limits::default(), None).unwrap();
        assert_eq!(restored.state.objects["strip"], ObjectGeometry::Baked(mesh.clone()), "full-detail source preserves each material byte");
        let reopened_glb = restored.compile(None).unwrap().glb;
        assert_eq!(reopened_glb, built.compiled.glb);
        let reopened = load_gltf_from_bytes(&reopened_glb, None).unwrap();
        let run = &reopened.document.meshes_slice()[0].primitives[0];
        let accessor = &reopened.document.accessors_slice()[run.attributes["_MATERIAL_BYTES"]];
        assert_eq!((accessor.accessor_type.as_str(), accessor.component_type, accessor.normalized), ("VEC4", 5121, None),
            "material byte accessor is raw VEC4 UNSIGNED_BYTE");
        let view = &reopened.document.buffer_views_slice()[accessor.buffer_view.unwrap()];
        assert_eq!((view.byte_stride, view.target), (Some(4), Some(34962)), "four byte vertex stride and ARRAY_BUFFER target");
        let mut capped = restored.clone().into_last_use();
        capped.set_memory_limit(4 * payload);
        assert_eq!(capped.compile_with_cache(&mut crate::CompileCache::default(), None).unwrap().glb, built.compiled.glb,
            "the writer admits its array and binary buffer footprint");
        let mut placed = restored.clone();
        let transform = crate::Transform { translation: [3., -2., 1.], ..Default::default() };
        apply(&mut placed, "place", vec![
            Operation::Scene(crate::SceneOperation::Node { object: "strip".into(), node: crate::SceneNode { transform, ..Default::default() } }),
            Operation::Scene(crate::SceneOperation::Socket(crate::Socket { name: "tip".into(), attachment: crate::Attachment::Object("strip".into()), transform: Default::default() })),
        ]).unwrap();
        let compiled = placed.compile(None).unwrap();
        assert_eq!(compiled.bounds[0], [3., -2., 1.]);
        assert_eq!(placed.state.objects["strip"], restored.state.objects["strip"]);
        let mut corrupt = row.clone();
        let counts = corrupt.windows(8).position(|bytes| bytes == crate::baked::MAGIC).unwrap() + 8;
        corrupt[counts..counts+4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(Document::from_bytes(&corrupt, Limits::default(), None),
            Err(Error::Corrupt("baked geometry counts" | "baked geometry size overflow"))));
        let tight = Limits { mesh: mesh::Limits { max_bytes: row.len() + payload - 1 }, ..Default::default() };
        assert!(matches!(Document::from_bytes(&row, tight, None), Err(Error::WorkingBytes { .. })), "decode charges both the source and the live arrays");
    }

    #[test]
    fn program_workspace_preserves_product_checkpoint_and_error_attribution() {
        let limits = Limits::default();
        let mut operations = vec![Operation::Cube { object: "body".into(), size: [1.; 3] }];
        // Cross the runner's batch boundary, with surface state already present.
        let mesh = mesh::Mesh::cube([1.; 3], &mut mesh::Context::default()).unwrap();
        let vertices = mesh.vertices().iter().map(|v| v.id).collect::<Vec<_>>();
        operations.push(Operation::Surface(crate::SurfaceOperation::VertexColors {
            object: "body".into(), vertices: vertices.clone(), colors: vec![[0.2, 0.4, 0.8, 1.]; vertices.len()],
        }));
        for i in 0..65 {
            operations.push(Operation::Surface(crate::SurfaceOperation::VertexPaint {
                object: "body".into(), vertices: vec![vertices[i % vertices.len()]], color: [0.8, 0.6, 0.4, 1.], opacity: 0.125,
            }));
        }
        operations.push(Operation::Construction(crate::ConstructionOperation {
            object: "stripe".into(),
            kind: crate::Construction::Decal { material: 0, decal: crate::Decal {
                target: "body".into(), center: [0., 0., 0.5], normal: [0., 0., 1.],
                up: [0., 1., 0.], size: [0.3, 0.2], depth: 0.1, offset: 0.0015,
            } },
        }));
        let mut reference = Document::new(limits.clone()).unwrap();
        for (i, batch) in operations.chunks(64).enumerate() {
            apply(&mut reference, &format!("program-{}", i + 1), batch.to_vec()).unwrap();
            reference.checkpoint(reference.head()).unwrap();
        }
        let program = operations.into_iter().map(crate::ProgramOp::Op).collect::<Vec<_>>();
        let built = crate::build_program(&program, limits.clone(), &mut |_| unreachable!(), None).unwrap();
        assert_eq!(built.document.state, reference.state, "unlogged batches preserve authored state");
        assert_eq!(built.document.head(), reference.head(), "content and generation remain stable");
        assert_eq!(built.compiled.glb, reference.compile(None).unwrap().glb, "render bytes remain identical");
        let glb = makepad_gltf::parse_glb_bytes(&built.compiled.glb).unwrap();
        let paint = |name: &str| glb.document.nodes_slice().iter().find(|n| n.name.as_deref() == Some(name))
            .unwrap().extras.as_ref().and_then(|e| e.key("MAKEPAD_paint")).and_then(|v| v.as_bool());
        assert_eq!(paint("stripe"), Some(true), "a decal construction exports paint even without a builder parent");
        assert_eq!(paint("body"), None, "vertex colours alone do not mark a mesh as a decal");
        let source = built.document.to_bytes(None).unwrap();
        // A reopen under too small a limit says what it needed: here, its source.
        let tight = Limits { mesh: mesh::Limits { max_bytes: source.len() - 1 }, ..limits.clone() };
        assert_eq!(Document::from_bytes(&source, tight, None).unwrap_err(),
            Error::WorkingBytes { what: "decode working bytes", needed: source.len(), limit: source.len() - 1 }, "a refused reopen names its step and its numbers");
        let mut reopened = Document::from_bytes(&source, limits.clone(), None).unwrap();
        assert_eq!(reopened.head(), built.document.head());
        assert_eq!(reopened.state, built.document.state);
        assert_eq!(reopened.history_position(), (0, 0));
        let op = edit(&reopened, 2);
        apply(&mut reopened, "after-build", vec![op]).unwrap();
        reopened.undo(reopened.head(), None).unwrap();
        assert_eq!(reopened.state, built.document.state, "the final checkpoint supports later editing");
        apply(&mut reopened, "copy-paint", vec![
            Operation::Scene(crate::SceneOperation::Instance { object: "linked".into(), source: "stripe".into(), transform: Default::default() }),
            Operation::Scene(crate::SceneOperation::Duplicate { object: "copy".into(), source: "linked".into() }),
            Operation::DeleteObject { object: "linked".into() },
            Operation::DeleteObject { object: "stripe".into() },
            Operation::Cube { object: "stripe".into(), size: [1.; 3] },
        ]).unwrap();
        assert_eq!(reopened.scene().paint, BTreeSet::from(["copy".into()]), "paint survives copying and clears on deletion and name reuse");

        let bad = vec![program[0].clone(), crate::ProgramOp::Op(Operation::Scene(crate::SceneOperation::Node {
            object: "body".into(), node: crate::SceneNode { parent: Some("missing".into()), ..Default::default() },
        })), crate::ProgramOp::Op(Operation::SetMaterial { material: 0, value: Material::default() })];
        let failure = crate::build_program(&bad, limits.clone(), &mut |_| unreachable!(), None).err().unwrap();
        assert_eq!(failure.index, 1, "batch validation must identify the introducing operation");
        let baked = Operation::Baked {
            object: "card".into(),
            mesh: std::sync::Arc::new(crate::BakedMesh {
                positions: vec![[0.; 3], [1., 0., 0.], [0., 1., 0.]], normals: vec![[0., 0., 1.]; 3],
                uvs: None, material_bytes: None, indices: vec![0, 1, 2], runs: vec![(0, 0..3)],
            }),
        };
        apply(&mut reopened, "card", vec![baked.clone()]).unwrap();
        let reopen = |d: &Document| Document::from_bytes(&d.to_bytes(None).unwrap(), limits.clone(), None).unwrap();
        let mut mixed = reopen(&reopened);
        assert_eq!(mixed.state, reopened.state, "recorded baked object replays beside editable body");
        mixed.checkpoint(mixed.head()).unwrap();
        let checkpoint = reopen(&mixed);
        assert_eq!(checkpoint.state, mixed.state, "baked checkpoint restores both kinds");
        let replace = vec![Operation::DeleteObject { object: "card".into() }, Operation::Plane { object: "card".into(), size: [1.; 2] }];
        apply(&mut mixed, "replace", replace).unwrap();
        assert!(mixed.object("card").is_some(), "name reused for an editable object");
        mixed.undo(mixed.head(), None).unwrap();
        assert_eq!(mixed.state, checkpoint.state, "undo restores the baked geometry");
        let indirect = Operation::Construction(crate::ConstructionOperation { object: "derived".into(),
            kind: crate::Construction::Boolean { a: "body".into(), b: "card".into(), mode: crate::BooleanMode::Union } });
        let head = mixed.head();
        let err = apply(&mut mixed, "indirect", vec![Operation::Cube { object: "new".into(), size: [1.;3] }, indirect.clone()]).unwrap_err();
        assert!(matches!(err, Error::BakedObject { index: 1, ref object, .. } if object == "card"), "{err}");
        assert_eq!(mixed.head(), head, "indirect refusal rolls back the transaction");
        let mut forward = Document::new(limits.clone()).unwrap();
        let err = apply(&mut forward, "forward-link", vec![Operation::Plane { object: "copy".into(), size: [1.; 2] },
            Operation::Scene(crate::SceneOperation::Node { object: "copy".into(), node: crate::SceneNode { linked_to: Some("card".into()), ..Default::default() } }),
            baked.clone()]).unwrap_err();
        assert!(matches!(err, Error::BakedObject { index: 1, ref object, .. } if object == "card"), "{err}");
        let err = crate::build_program(&[crate::ProgramOp::Op(baked), crate::ProgramOp::Op(indirect)], limits.clone(),
            &mut |_| unreachable!(), None).err().unwrap();
        assert_eq!(err.index, 1);
        assert!(err.message.contains("'card': the object is baked"), "{err}");
        assert!(mixed.apply(Transaction { request_id: "cancel".into(), expected: head,
            operations: vec![Operation::DeleteObject { object: "card".into() }] }, Some(&|| true)).is_err());
        assert_eq!(mixed.state, checkpoint.state, "cancel preserves baked buffers");
        let valid_import = [crate::ProgramOp::Op(Operation::ImportMesh {
            object: "body".into(), source: mesh.to_bytes(&mut mesh::Context::default()).unwrap(),
        })];
        let imported = crate::build_program(&valid_import, limits.clone(), &mut |_| unreachable!(), None).unwrap();
        assert_eq!(imported.document.object("body"), Some(&mesh));
        assert!(crate::build_program(&program[..1], limits, &mut |_| unreachable!(), None).is_ok());
    }

    #[test]
    fn disposable_vertex_colors_match_atomic_surface_edits() {
        let limits = Limits::default();
        let mut doc = Document::new(limits.clone()).unwrap();
        apply(&mut doc, "body", vec![Operation::Cube { object: "body".into(), size: [1.; 3] }]).unwrap();
        let ids = doc.object("body").unwrap().vertices().iter().map(|v| v.id).collect::<Vec<_>>();
        let mut actual = crate::SurfaceState::default();
        let mut expected = actual.clone();
        for op in [
            crate::SurfaceOperation::VertexColors { object: "body".into(), vertices: ids.clone(), colors: vec![[0.2, 0.4, 0.6, 1.]; ids.len()] },
            crate::SurfaceOperation::VertexColors { object: "body".into(), vertices: vec![ids[0], ids[0]], colors: vec![[0.1, 0.3, 0.5, 0.7], [0.9, 0.7, 0.5, 0.3]] },
            crate::SurfaceOperation::VertexPaint { object: "body".into(), vertices: vec![ids[1], ids[1]], color: [0.5, 0.25, 0.75, 1.], opacity: 0.25 },
        ] {
            actual.apply_disposable(&op, &doc.state.objects, &doc.state.materials, &limits, &mut mesh::Context::default()).unwrap();
            expected.apply(&op, &BTreeMap::from([("body".into(), doc.object("body").unwrap().clone())]), &doc.state.materials, &limits, &mut mesh::Context::default()).unwrap();
            assert_eq!(actual, expected);
            let bits = |surface: &crate::SurfaceState| surface.vertex_colors.iter().map(|(key, color)| (key.clone(), color.map(f64::to_bits))).collect::<Vec<_>>();
            assert_eq!(bits(&actual), bits(&expected), "every vertex color bit is preserved");
        }
    }
    #[test]
    fn a_last_use_compiles_its_distance_levels_beside_one_copy_of_the_state_and_again_after_a_refusal() {
        // A dense part whose undo history was compacted into its checkpoint (a full copy of the
        // state, as a built program's document holds it), then distance levels and a compile, as
        // a host derives them after saving the full detail.
        let mut doc = Document::new(Limits::default()).unwrap();
        let sphere = Operation::sphere("body", 1.0, 48, 32, true, doc.limits()).unwrap();
        apply(&mut doc, "shape", vec![sphere]).unwrap();
        doc.checkpoint(doc.head()).unwrap();
        let levels = || vec![Operation::Scene(crate::SceneOperation::Lods { object: "body".into(), levels: vec![
            crate::LodLevel { target_faces: 400, max_error: 0.01, distance: 10. },
            crate::LodLevel { target_faces: 0, max_error: 0.1, distance: 40. },
        ] })];
        let state = doc.state.memory_bytes();
        let mut edited = doc.clone();
        apply(&mut edited, "levels", levels()).unwrap();
        let product = edited.compile_with_cache(&mut crate::CompileCache::default(), None).unwrap();
        // Under 4.5 x the state: the state, its evaluated meshes and their copies fit (4 x) ...
        let limit = state * 9 / 2;
        let mut capped = doc.clone();
        capped.limits.mesh.max_bytes = limit;
        let mut last = capped.clone().into_last_use();
        last.set_memory_limit(state * 3);
        last.apply("levels", levels(), None).expect("the last use's edit holds the state and one candidate (2 x)");
        match last.compile_with_cache(&mut crate::CompileCache::default(), None) {
            Err(Error::WorkingBytes { what, needed, limit: l }) => {
                assert_eq!((what, l), ("document working bytes", state * 3), "the refusal names its step and the last use's limit");
                assert!(needed > l && needed <= limit, "the last use's compile needs about 4 x the state: {needed} for {state}");
            }
            other => panic!("the last use's compile is over 3 x the state: {:?}", other.map(|p| p.glb.len())),
        }
        // The scene's evaluation refuses with its numbers too: three copies of the part's mesh.
        last.set_memory_limit(state * 5 / 2);
        match last.compile_with_cache(&mut crate::CompileCache::default(), None) {
            Err(Error::WorkingBytes { what, needed, limit: l }) => {
                assert_eq!((what, l), ("modifier source copies", state * 5 / 2), "the evaluation's refusal names its step and the limit");
                assert!(needed > l && needed <= state * 3, "the evaluation needs three copies of the mesh: {needed} for {state}");
            }
            other => panic!("the evaluation is over 2.5 x the state: {:?}", other.map(|p| p.glb.len())),
        }
        last.set_memory_limit(limit);
        let last = last.compile_with_cache(&mut crate::CompileCache::default(), None)
            .expect("a refused last use compiles once its limit allows it, beside one copy of the state");
        assert!(last.glb == product.glb, "the last use makes the product an edit and a compile make");
        // ... and the same edit beside the checkpoint is refused at the compile (5 x), saying by
        // how much: the compile's copy holds none of the meshes it replaces (6 x before).
        let mut beside = capped.clone();
        apply(&mut beside, "levels", levels()).unwrap();
        match beside.compile_with_cache(&mut crate::CompileCache::default(), None) {
            Err(Error::WorkingBytes { needed, limit: l, .. }) => {
                assert_eq!(l, limit, "the refusal names the limit");
                assert!(needed > limit && needed < state * 11 / 2, "the compile beside a checkpoint needs about 5 x the state, not 6 x: {needed} for {state}");
            }
            other => panic!("the compile beside the checkpoint is over 4.5 x the state: {:?}", other.map(|p| p.glb.len())),
        }
    }
    #[test]
    fn heavy_history_checkpoints_oldest_edits_and_keeps_each_retained_cursor_replayable(){
        let limits = Limits { max_history: 8, max_receipts: 1, ..Limits::default() };
        let mut doc = Document::new(limits.clone()).unwrap();
        let mut explicit = Document::new(Limits { max_history: usize::MAX, ..limits.clone() }).unwrap();
        for d in [&mut doc, &mut explicit] {
            apply(d, "seed", vec![Operation::Cube { object: "body".into(), size: [1.; 3] }]).unwrap();
        }
        let mut folded = 0;
        for i in 0..12 {
            for d in [&mut doc, &mut explicit] {
                let op = edit(d, i);
                apply(d, &format!("edit-{i}"), vec![op]).unwrap();
            }
            let total = i + 2;
            if total == 9 || total == 13 { folded += 4; }
            assert_eq!(doc.history_position(), (total - folded, total - folded), "fold only each fourth edit after filling the window");
            if folded != 0 {
                assert_eq!(doc.checkpoint.editable("body").unwrap().corners()[0].uv, [(folded - 2) as f64 * 0.01, 0.25], "checkpoint includes exactly the oldest half-window");
            }
            // Explicit checkpoints advance the public concurrency token. For
            // byte comparison keep the same generation and request fingerprints
            // as automatic folding, which preserves that token.
            if total == 4 || total == 8 {
                let head = explicit.head();
                explicit.checkpoint(head).unwrap();
                explicit.head = head;
            }
        }
        let source = doc.to_bytes(None).unwrap();
        assert_eq!(source, explicit.to_bytes(None).unwrap(), "batch folding matches explicit checkpoints at the same edits");
        let opened = Document::from_bytes(&source, limits.clone(), None).unwrap();
        assert_eq!(opened.head(), doc.head());
        assert_eq!(opened.to_bytes(None).unwrap(), source);
        assert_eq!(doc.log_memory_bytes, doc.log_memory.iter().sum::<usize>());
        assert_eq!(doc.log_source_bytes, doc.log_source.iter().sum::<usize>());
        assert_eq!(source.len(), 68 + doc.checkpoint_source.bytes + doc.log_source_bytes + doc.receipts.source_bytes + doc.request_order.len() * 32);
        let saved = doc.head().content;
        let mut legacy = source.clone();
        legacy[8..12].copy_from_slice(&3u32.to_le_bytes());
        let error = Document::from_bytes(&legacy, limits.clone(), None).unwrap_err();
        assert_eq!(error, Error::UnsupportedVersion(3));
        assert!(error.to_string().contains("model source version 3 is not supported"), "{error}");
        let retained = doc.history_position().0;
        for _ in 0..retained {
            doc.undo(doc.head(), None).unwrap();
            let opened = Document::from_bytes(&doc.to_bytes(None).unwrap(), limits.clone(), None).unwrap();
            assert_eq!(opened.head(), doc.head());
            assert_eq!(opened.state, doc.state);
        }
        assert_eq!(doc.undo(doc.head(), None).unwrap_err(), Error::Invalid("nothing to undo"));
        assert_eq!(doc.object("body").unwrap().corners()[0].uv, [0.06, 0.25]);
        let mut no_undo = Document::from_bytes(&doc.to_bytes(None).unwrap(), Limits { max_history: 0, ..limits.clone() }, None).unwrap();
        for _ in 0..retained { no_undo.redo(no_undo.head(), None).unwrap(); }
        assert_eq!(no_undo.history_position(), (0, 0));
        assert_eq!(no_undo.head().content, saved);
        for _ in 0..retained { doc.redo(doc.head(), None).unwrap(); }
        assert_eq!(doc.head().content, saved);
        // Loading with a smaller window folds a whole batch, and redo across
        // that window uses the same rule without dropping unapplied entries.
        let smaller = Limits { max_history: 4, ..limits.clone() };
        let compacted = Document::from_bytes(&source, smaller.clone(), None).unwrap();
        assert_eq!(compacted.history_position(), (3, 3));
        assert_eq!(compacted.head().content, saved);
        doc.undo(doc.head(), None).unwrap();
        doc.undo(doc.head(), None).unwrap();
        let mut redo = Document::from_bytes(&doc.to_bytes(None).unwrap(), smaller, None).unwrap();
        while redo.history_position().0 < redo.history_position().1 { redo.redo(redo.head(), None).unwrap(); }
        assert_eq!(redo.head().content, saved);
        assert_eq!(redo.history_position(), (3, 3));
        // Force the identity window full without executing thousands of edits.
        doc.request_identities.clear(); doc.request_order.clear(); doc.receipts = Receipts::default();
        let oldest = sha256(b"expired");
        doc.request_identities.insert(oldest); doc.request_order.push_back(oldest);
        for i in 1..MAX_REQUEST_IDENTITIES {
            let mut identity = [0; 32]; identity[..8].copy_from_slice(&(i as u64).to_le_bytes());
            doc.request_identities.insert(identity); doc.request_order.push_back(identity);
        }
        let op = edit(&doc, 9); apply(&mut doc, "new", vec![op]).unwrap();
        assert!(!doc.request_identities.contains(&oldest));
        assert_eq!(doc.request_order.len(), MAX_REQUEST_IDENTITIES);
        let mut opened = Document::from_bytes(&doc.to_bytes(None).unwrap(), limits, None).unwrap();
        assert_eq!(opened.request_order, doc.request_order);
        let op = edit(&opened, 10); apply(&mut opened, "expired", vec![op]).unwrap();
        opened.limits.max_history = 0;
        let op = edit(&opened, 11); apply(&mut opened, "no-undo", vec![op]).unwrap();
        assert_eq!(opened.history_position(), (0, 0));
        assert_eq!(opened.state, opened.checkpoint);
    }
}
