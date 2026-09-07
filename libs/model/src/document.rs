use crate::{
    canon::{Reader, Writer},
    mesh,
    rig::{read_clip, read_skeleton, write_clip, write_skeleton},
    AnimationClip, Material, Operation, OperationResult, Skeleton,
};
use makepad_asset_data::sha256;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Mesh(mesh::MeshError),
    Budget(&'static str),
    Invalid(&'static str),
    MissingField { field: String, operation: Option<String> },
    Corrupt(&'static str),
    MissingObject(String),
    DuplicateObject(String),
    StaleHead { expected: Head, actual: Head },
    RequestIdReused,
}
impl From<mesh::MeshError> for Error {
    fn from(v: mesh::MeshError) -> Self {
        Self::Mesh(v)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
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
    pub max_objects: usize,
    pub max_materials: usize,
    pub max_operations: usize,
    pub max_history: usize,
    pub max_receipts: usize,
    pub max_name_bytes: usize,
    pub max_source_bytes: usize,
    pub max_transaction_bytes: usize,
    pub max_texture_bytes: usize,
    pub max_texture_dimension: u32,
    pub max_joints: usize,
    pub max_clips: usize,
    pub max_keyframes: usize,
    pub max_clip_duration: f64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            mesh: mesh::Limits::default(),
            max_objects: 2048,
            max_materials: 512,
            max_operations: 256,
            max_history: 4096,
            max_receipts: 128,
            max_name_bytes: 96,
            max_source_bytes: 256 * 1024 * 1024,
            max_transaction_bytes: 64 * 1024 * 1024,
            max_texture_bytes: 64 * 1024 * 1024,
            max_texture_dimension: 2048,
            max_joints: 64,
            max_clips: 64,
            max_keyframes: 65_536,
            max_clip_duration: 3600.0,
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
    pub objects: BTreeMap<String, mesh::Mesh>,
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
    fn memory_bytes(&self) -> usize {
        let meshes = self.objects.iter().fold(0usize, |n, (name, m)| {
            n.saturating_add(m.memory_bytes())
                .saturating_add(name.len().saturating_mul(2))
                .saturating_add(128)
        });
        let materials = self.materials.values().fold(0usize, |n, m| {
            n.saturating_add(m.base_color_png.len().saturating_mul(2))
                .saturating_add(128)
        });
        let clips = self.clips.values().fold(0usize, |n, c| {
            n.saturating_add(c.memory_bytes()).saturating_add(128)
        });
        meshes
            .saturating_add(self.scene.memory_bytes())
            .saturating_add(self.selections.memory_bytes())
            .saturating_add(self.rig.memory_bytes())
            .saturating_add(self.surface.memory_bytes())
            .saturating_add(self.soft_body.as_ref().map_or(0, |m|m.to_value().to_json().len().saturating_mul(3)))
            .saturating_add(materials)
            .saturating_add(clips)
            .saturating_add(self.skeleton.as_ref().map_or(0, Skeleton::memory_bytes))
    }
}
#[derive(Clone, Debug)]
struct Receipt {
    id: String,
    fingerprint: [u8; 32],
    head: Head,
    results: Vec<OperationResult>,
}
impl Receipt {
    fn memory_bytes(&self) -> usize {
        self.id.len()+128+self.results.iter().map(|r|r.object.len()+128+(r.faces.len()+r.vertices.len())*16+r.metrics.iter().map(|(k,_)|k.len()+96).sum::<usize>()).sum::<usize>()
    }
}
const MAX_RECEIPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_REQUEST_IDENTITIES: usize = 65_536;
/// Serialized size and work of the checkpoint section of a version 2 source.
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
    // Measured work is derived from trusted execution, never read from source.
    // Include each transaction's execute + hash, matching history decoding.
    replay_work: Vec<u64>,
    hash_work: Vec<u64>,
    checkpoint_load_work: u64,
    checkpoint_hash_work: u64,
    checkpoint_source: SourceMeasure,
    cursor: usize,
    pub(crate) state: State,
    head: Head,
    receipts: VecDeque<Receipt>,
    request_identities: BTreeSet<[u8; 32]>,
}
impl Document {
    pub fn new(limits: Limits) -> Result<Self> {
        if limits.max_materials == 0 || limits.max_receipts == 0 {
            return Err(Error::Invalid("zero material or receipt limit"));
        }
        let mut state = State::default();
        state.materials.insert(0, Material::default());
        let mut ctx = mesh::Context::new(limits.mesh.clone(), None);
        let hash = state_hash(&state, &limits, &mut ctx)?;
        let checkpoint_hash_work=ctx.work_used();
        let (checkpoint_load_work,checkpoint_source)=checkpoint_load_work(&state,&limits)?;
        Ok(Self {
            limits,
            checkpoint: state.clone(),
            log: Vec::new(),
            log_memory: Vec::new(),
            replay_work: Vec::new(), hash_work: Vec::new(),
            checkpoint_load_work, checkpoint_hash_work, checkpoint_source,
            cursor: 0,
            state,
            head: Head {
                generation: 0,
                content: hash,
            },
            receipts: VecDeque::new(),
            request_identities: BTreeSet::new(),
        })
    }
    pub fn head(&self) -> Head {
        self.head
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    pub fn objects(&self) -> impl Iterator<Item = (&str, &mesh::Mesh)> {
        self.state
            .objects
            .iter()
            .map(|(name, mesh)| (name.as_str(), mesh))
    }
    pub fn object(&self, name: &str) -> Option<&mesh::Mesh> {
        self.state.objects.get(name)
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
    pub(crate) fn render_copy(&self, working:usize) -> Result<Self> {
        self.admit(working.saturating_add(self.state.memory_bytes()))?;
        Ok(Self { limits:self.limits.clone(), checkpoint:State::default(), log:Vec::new(), log_memory:Vec::new(), replay_work:Vec::new(), hash_work:Vec::new(), checkpoint_load_work:0, checkpoint_hash_work:0, checkpoint_source:SourceMeasure::default(),
            cursor:0, state:self.state.clone(), head:self.head, receipts:VecDeque::new(), request_identities:BTreeSet::new() })
    }
    pub fn history_position(&self) -> (usize, usize) {
        (self.cursor, self.log.len())
    }
    fn memory_bytes(&self) -> usize {
        let log = self.log_memory.iter().fold(0usize, |n, v| n.saturating_add(*v));
        let receipts = self.receipts.iter().fold(0usize, |n, r| {
            n.saturating_add(r.id.len().saturating_mul(2))
                .saturating_add(128)
                .saturating_add(r.results.iter().fold(0usize, |n, r| {
                    n.saturating_add(r.object.len().saturating_mul(2))
                        .saturating_add((r.faces.len() + r.vertices.len()).saturating_mul(16))
                        .saturating_add(r.metrics.iter().map(|(k,_)|k.len()+96).sum::<usize>())
                        .saturating_add(128)
                }))
        });
        self.checkpoint
            .memory_bytes()
            .saturating_add(self.state.memory_bytes())
            .saturating_add(log)
            .saturating_add((self.replay_work.len()+self.hash_work.len()).saturating_mul(8))
            .saturating_add(receipts)
            .saturating_add(self.request_identities.len().saturating_mul(128))
    }
    fn admit(&self, extra: usize) -> Result<()> {
        if self.memory_bytes().saturating_add(extra) > self.limits.mesh.max_bytes {
            Err(Error::Budget("document working bytes"))
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
        // The fingerprint encoding is released before admission; it is not
        // resident while the transaction executes.
        let fingerprint = {
            let mut encoded = Writer::new(self.limits.max_transaction_bytes);
            write_head(&mut encoded, tx.expected)?;
            write_ops(&mut encoded, &tx.operations, &self.limits)?;
            sha256(&encoded.bytes)
        };
        if let Some(receipt) = self.receipts.iter().find(|r| r.id == tx.request_id) {
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
        let request_identity = sha256(tx.request_id.as_bytes());
        // Eviction drops cached results, never permission to execute an old ID
        // again. Exhaustion is explicit; checkpoint preserves these tombstones.
        if self.request_identities.contains(&request_identity) {
            return Err(Error::RequestIdReused);
        }
        if self.request_identities.len() >= MAX_REQUEST_IDENTITIES {
            return Err(Error::Budget("request identity retention"));
        }
        self.expect(tx.expected)?;
        if self.cursor >= self.limits.max_history {
            return Err(Error::Budget(
                "history; checkpoint explicitly before editing further",
            ));
        }
        let mut ctx = mesh::Context::new(self.limits.mesh.clone(), cancelled);
        ctx.checkpoint(1)?;
        let operation_bytes = tx
            .operations
            .iter()
            .fold(0usize, |n, op| n.saturating_add(op.memory_bytes()));
        // Resident while a transaction is admitted: the retained document, the
        // transaction, one candidate copy of the state and the re-serialized
        // history that bounds the source. Nothing else is cloned. Leading
        // whole-object deletions never read the mesh they remove, so the
        // candidate carries an empty stand-in for that geometry instead of a
        // copy; execute still performs and validates the deletion itself.
        let retained = self.memory_bytes();
        let state_bytes = self.state.memory_bytes();
        let history_bytes = retained
            .saturating_sub(state_bytes)
            .saturating_sub(self.checkpoint.memory_bytes());
        let released = released_objects(&self.state, &tx.operations);
        let placeholder_bytes = mesh::Mesh::new().memory_bytes();
        let candidate_bytes = released.iter().fold(state_bytes, |n, name| {
            n.saturating_sub(self.state.objects[*name].memory_bytes())
                .saturating_add(placeholder_bytes)
        });
        let transaction_bytes = operation_bytes
            .saturating_mul(2)
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
        let replay_start=ctx.work_used();
        let results = execute(&mut candidate, &tx.operations, &self.limits, &mut ctx)?;
        self.admit(candidate.memory_bytes().saturating_add(transaction_bytes))?;
        let hash_start=ctx.work_used();
        let content = state_hash(&candidate, &self.limits, &mut ctx)?;
        let hash_work=ctx.work_used()-hash_start;
        let replay_work=ctx.work_used()-replay_start;
        let head = Head {
            generation: self
                .head
                .generation
                .checked_add(1)
                .ok_or(Error::Budget("generation"))?,
            content,
        };
        let receipt = Receipt {
            id: tx.request_id,
            fingerprint,
            head,
            results: results.clone(),
        };
        let (evicted, retain_receipt) =
            plan_receipt_eviction(&self.receipts, &receipt, self.limits.max_receipts);
        // Bound the source and history work of the document that would result
        // before installing anything. The redo branch is dropped only on
        // success; the checkpoint section is measured, not re-serialized.
        let log = self.log[..self.cursor]
            .iter()
            .map(Vec::as_slice)
            .chain(std::iter::once(tx.operations.as_slice()))
            .collect::<Vec<_>>();
        let receipts = self
            .receipts
            .iter()
            .skip(evicted)
            .chain(retain_receipt.then_some(&receipt))
            .collect::<Vec<_>>();
        let encode_start=ctx.work_used();
        encode_source(
            &SourceParts {
                head,
                checkpoint: CheckpointSource::Measured(self.checkpoint_source),
                cursor: self.cursor + 1,
                log: &log,
                receipts: &receipts,
                identities: &self.request_identities,
                added_identity: Some(request_identity),
            },
            &self.limits,
            &mut ctx,
            2,
        )?;
        let needed = history_work(
            self.replay_work[..self.cursor]
                .iter()
                .copied()
                .chain(std::iter::once(replay_work)),
            self.hash_work[..self.cursor]
                .iter()
                .copied()
                .chain(std::iter::once(hash_work)),
            self.checkpoint_load_work,
            self.checkpoint_hash_work,
            ctx.work_used()-encode_start,
        );
        if needed > self.limits.mesh.max_work {
            return Err(Error::Budget("history replay work; checkpoint explicitly before editing further"));
        }
        ctx.checkpoint(1)?;
        // Install in place: nothing below can fail, so the previous state, the
        // redo branch and evicted receipts are released only after every check.
        self.log.truncate(self.cursor);
        self.log_memory.truncate(self.cursor);
        self.replay_work.truncate(self.cursor);
        self.hash_work.truncate(self.cursor);
        self.log.push(tx.operations);
        self.log_memory.push(operation_bytes);
        self.replay_work.push(replay_work);
        self.hash_work.push(hash_work);
        for _ in 0..evicted {
            self.receipts.pop_front();
        }
        if retain_receipt {
            self.receipts.push_back(receipt);
        }
        self.request_identities.insert(request_identity);
        self.state = candidate;
        self.head = head;
        self.cursor += 1;
        Ok(Applied {
            committed: head,
            current: head,
            replayed: false,
            results,
        })
    }
    /// Retained history must fit the same aggregate budget used to decode it,
    /// including checkpoint decoding, every accepted transaction, the most
    /// expensive possible undo cursor hash, and canonical re-encoding. Reject
    /// before installing the new state; explicit checkpointing is the user's
    /// choice, so admission never silently removes undo entries.
    fn history_work_needed(&self,encode_work:u64)->u64 {
        history_work(
            self.replay_work.iter().copied(),
            self.hash_work.iter().copied(),
            self.checkpoint_load_work,
            self.checkpoint_hash_work,
            encode_work,
        )
    }
    fn admit_history_work(&self,encode_work:u64)->Result<()> {
        if self.history_work_needed(encode_work)>self.limits.mesh.max_work{return Err(Error::Budget("history replay work; checkpoint explicitly before editing further"));}
        Ok(())
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
        self.admit(self.checkpoint.memory_bytes().saturating_mul(2))?;
        ctx.limits.max_bytes = ctx.limits.max_bytes.saturating_sub(self.memory_bytes());
        let mut state = self.checkpoint.clone();
        for ops in &self.log[..cursor] {
            execute(&mut state, ops, &self.limits, &mut ctx)?;
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
        self.admit(self.state.memory_bytes())?;
        let (load_work,source)=checkpoint_load_work(&self.state,&self.limits)?;
        let hash_work=if self.cursor==0{self.checkpoint_hash_work}else{self.hash_work[self.cursor-1]};
        self.checkpoint = self.state.clone();
        self.checkpoint_load_work=load_work;
        self.checkpoint_hash_work=hash_work;
        self.checkpoint_source=source;
        self.log.clear();
        self.log_memory.clear();
        self.replay_work.clear();self.hash_work.clear();
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
        self.encode_parts(&mut mesh::Context::new(self.limits.mesh.clone(), cancelled), 2, true)
    }
    fn encode(&self, ctx: &mut mesh::Context) -> Result<Vec<u8>> {
        self.encode_version(ctx, 2)
    }
    fn encode_version(&self, ctx: &mut mesh::Context, version:u32) -> Result<Vec<u8>> {
        self.encode_parts(ctx, version, false)
    }
    fn encode_parts(&self, ctx: &mut mesh::Context, version:u32, snapshot:bool) -> Result<Vec<u8>> {
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
        encode_source(
            &SourceParts {
                head: self.head,
                checkpoint: CheckpointSource::State(if snapshot { &self.state } else { &self.checkpoint }),
                cursor: if snapshot { 0 } else { self.cursor },
                log: &log,
                receipts: &receipts,
                identities: &self.request_identities,
                added_identity: None,
            },
            &self.limits,
            ctx,
            version,
        )
    }
    pub fn from_bytes(
        bytes: &[u8],
        limits: Limits,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> Result<Self> {
        if bytes.len() > limits.max_source_bytes {
            return Err(Error::Budget("source bytes"));
        }
        let mut r = Reader::new(bytes);
        let mut ctx = mesh::Context::new(limits.mesh.clone(), cancelled);
        ctx.checkpoint(1)?;
        ctx.limits.max_bytes = ctx
            .limits
            .max_bytes
            .checked_sub(bytes.len())
            .ok_or(Error::Budget("decode working bytes"))?;
        if r.raw(8)? != b"MPMODEL\0" {
            return Err(Error::Corrupt("unsupported model source version"));
        }
        let version = r.u32()?;
        if !(1..=2).contains(&version) { return Err(Error::Corrupt("unsupported model source version")); }
        let head = read_head(&mut r)?;
        let checkpoint = read_state(&mut r, &limits, &mut ctx, version)?;
        if checkpoint.memory_bytes().saturating_mul(3) > ctx.limits.max_bytes {
            return Err(Error::Budget("decode checkpoint copies"));
        }
        let checkpoint_source=checkpoint_source_measure(&checkpoint,&limits,&mut ctx)?.1;
        let checkpoint_load_work=ctx.work_used()-1;
        let cursor = r.u32()? as usize;
        let count = r.count(limits.max_history)?;
        let mut checkpoint_hash_work=0;
        if count!=0 {let start=ctx.work_used();state_hash(&checkpoint,&limits,&mut ctx)?;checkpoint_hash_work=ctx.work_used()-start;}
        if cursor > count {
            return Err(Error::Corrupt("undo cursor"));
        }
        let mut log = Vec::new();
        let mut log_memory = Vec::new();
        let mut replay_work=Vec::new();let mut hash_work=Vec::new();
        let mut state = checkpoint.clone();
        let mut current = checkpoint.clone();
        let decode_budget = ctx.limits.max_bytes;
        let mut log_bytes = 0usize;
        for i in 0..count {
            let ops = read_ops(&mut r, &limits)?;
            let op_bytes = ops.iter().fold(0usize, |n, op| n.saturating_add(op.memory_bytes()));
            log_memory.push(op_bytes);
            log_bytes = log_bytes.saturating_add(op_bytes);
            let pinned = checkpoint
                .memory_bytes()
                .saturating_add(current.memory_bytes())
                .saturating_add(log_bytes);
            ctx.limits.max_bytes = decode_budget
                .checked_sub(pinned)
                .ok_or(Error::Budget("decode history working bytes"))?;
            let replay_start=ctx.work_used();
            execute(&mut state, &ops, &limits, &mut ctx)?;
            let hash_start=ctx.work_used();
            state_hash(&state, &limits, &mut ctx)?;
            replay_work.push(ctx.work_used()-replay_start);
            hash_work.push(ctx.work_used()-hash_start);
            if i + 1 == cursor {
                if pinned.saturating_add(state.memory_bytes().saturating_mul(2)) > decode_budget {
                    return Err(Error::Budget("decode cursor copy"));
                }
                current = state.clone();
            }
            log.push(ops);
        }
        ctx.limits.max_bytes = decode_budget;
        let cursor_hash_start=ctx.work_used();
        if state_hash(&current, &limits, &mut ctx)? != head.content {
            return Err(Error::Corrupt("content hash"));
        }
        if count==0{checkpoint_hash_work=ctx.work_used()-cursor_hash_start;}
        let mut receipts = VecDeque::new();
        for _ in 0..r.count(limits.max_receipts)? {
            let id = r.string(limits.max_name_bytes)?;
            check_name(&id, &limits)?;
            if receipts.iter().any(|v: &Receipt| v.id == id) {
                return Err(Error::Corrupt("duplicate receipt"));
            }
            let fingerprint = r.raw(32)?.try_into().unwrap();
            let receipt_head = read_head(&mut r)?;
            if receipt_head.generation > head.generation {
                return Err(Error::Corrupt("future receipt"));
            }
            let mut results = Vec::new();
            for _ in 0..r.count(limits.max_operations)? {
                let object = r.string(limits.max_name_bytes)?;
                let mut faces = Vec::new();
                for _ in 0..r.count(limits.mesh.max_faces)? {
                    faces.push(mesh::FaceId(r.u64()?));
                }
                let mut vertices = Vec::new();
                for _ in 0..r.count(limits.mesh.max_vertices)? {
                    vertices.push(mesh::VertexId(r.u64()?));
                }
                let mut metrics = BTreeMap::new();
                if version >= 2 {
                    for _ in 0..r.count(16)? {
                        let name = r.string(limits.max_name_bytes)?;
                        let value = usize::try_from(r.u64()?).map_err(|_|Error::Budget("operation metric"))?;
                        if metrics.insert(name, value).is_some() { return Err(Error::Corrupt("duplicate operation metric")); }
                    }
                }
                results.push(OperationResult {
                    object,
                    faces,
                    vertices,
                    metrics,
                });
            }
            receipts.push_back(Receipt {
                id,
                fingerprint,
                head: receipt_head,
                results,
            });
        }
        let mut request_identities = BTreeSet::new();
        for _ in 0..r.count(MAX_REQUEST_IDENTITIES)? {
            let identity = r.raw(32)?.try_into().unwrap();
            if !request_identities.insert(identity) {
                return Err(Error::Corrupt("duplicate request identity"));
            }
        }
        for receipt in &receipts {
            if !request_identities.contains(&sha256(receipt.id.as_bytes())) {
                return Err(Error::Corrupt("receipt lacks request identity"));
            }
        }
        r.end()?;
        let out = Self {
            limits,
            checkpoint,
            log,
            log_memory,
            replay_work,hash_work,checkpoint_load_work,checkpoint_hash_work,checkpoint_source,
            cursor,
            state: current,
            head,
            receipts,
            request_identities,
        };
        // Exact re-encoding rejects ambiguous orderings and noncanonical inputs.
        let encode_start=ctx.work_used();
        if out.encode_version(&mut ctx, version)? != bytes {
            return Err(Error::Corrupt("noncanonical document"));
        }
        out.admit_history_work(ctx.work_used()-encode_start)?;
        Ok(out)
    }
}

/// Retained history must fit the same aggregate budget used to decode it,
/// including checkpoint decoding, every accepted transaction, the most
/// expensive possible undo cursor hash, and canonical re-encoding.
fn history_work(
    replay_work: impl Iterator<Item = u64>,
    hash_work: impl Iterator<Item = u64>,
    checkpoint_load_work: u64,
    checkpoint_hash_work: u64,
    encode_work: u64,
) -> u64 {
    let mut entries = 0usize;
    let replay = replay_work.fold(2u64, |n, w| {
        entries += 1;
        n.saturating_add(w)
    });
    let cursor_hash = hash_work.fold(checkpoint_hash_work, u64::max);
    replay
        .saturating_add(checkpoint_load_work)
        .saturating_add(if entries == 0 { 0 } else { checkpoint_hash_work })
        .saturating_add(cursor_hash)
        .saturating_add(encode_work)
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
                let mesh = if released.contains(name.as_str()) { mesh::Mesh::new() } else { mesh.clone() };
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
/// number of leading receipts evicted and whether `added` is retained.
fn plan_receipt_eviction(receipts: &VecDeque<Receipt>, added: &Receipt, max_receipts: usize) -> (usize, bool) {
    let mut bytes = receipts
        .iter()
        .fold(added.memory_bytes(), |n, r| n.saturating_add(r.memory_bytes()));
    let mut count = receipts.len() + 1;
    let mut evicted = 0usize;
    let mut retain_added = true;
    while count > max_receipts || bytes > MAX_RECEIPT_BYTES {
        if evicted < receipts.len() {
            bytes = bytes.saturating_sub(receipts[evicted].memory_bytes());
            evicted += 1;
        } else if retain_added {
            bytes = bytes.saturating_sub(added.memory_bytes());
            retain_added = false;
        } else {
            break;
        }
        count -= 1;
    }
    (evicted, retain_added)
}
/// Checkpoint section of a source: serialized in full, or stood in for by its
/// measurement when only the bound on the remaining sections is needed.
/// Measured sections are version 2 only and the returned bytes then omit the
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
    identities: &'a BTreeSet<[u8; 32]>,
    /// Written in canonical order as if it were already a member of `identities`.
    added_identity: Option<[u8; 32]>,
}
fn encode_source(parts: &SourceParts<'_>, limits: &Limits, ctx: &mut mesh::Context, version: u32) -> Result<Vec<u8>> {
    let measured = match &parts.checkpoint {
        CheckpointSource::Measured(m) => m.bytes,
        CheckpointSource::State(_) => 0,
    };
    // Reserving the measured section keeps the source bound identical.
    let mut w = Writer::new(limits.max_source_bytes.saturating_sub(measured));
    w.raw(b"MPMODEL\0")?;
    w.u32(version)?;
    write_head(&mut w, parts.head)?;
    match &parts.checkpoint {
        CheckpointSource::State(state) => write_state(&mut w, state, limits, ctx, version >= 2)?,
        CheckpointSource::Measured(m) => ctx.checkpoint(m.work)?,
    }
    w.count(parts.cursor)?;
    w.count(parts.log.len())?;
    for ops in parts.log {
        ctx.checkpoint(1)?;
        write_ops(&mut w, ops, limits)?;
    }
    w.count(parts.receipts.len())?;
    for receipt in parts.receipts {
        w.string(&receipt.id)?;
        w.raw(&receipt.fingerprint)?;
        write_head(&mut w, receipt.head)?;
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
            if version >= 2 {
                w.count(result.metrics.len())?;
                for (name, value) in &result.metrics { w.string(name)?; w.u64(*value as u64)?; }
            }
        }
    }
    w.count(parts.identities.len().saturating_add(parts.added_identity.is_some() as usize))?;
    let mut added = parts.added_identity;
    for identity in parts.identities {
        if let Some(a) = added.filter(|a| a < identity) {
            w.raw(&a)?;
            added = None;
        }
        w.raw(identity)?;
    }
    if let Some(a) = added {
        w.raw(&a)?;
    }
    ctx.checkpoint(w.bytes.len().saturating_add(measured) as u64)?;
    Ok(w.bytes)
}

fn check_name(name: &str, limits: &Limits) -> Result<()> {
    if name.is_empty() || name.len() > limits.max_name_bytes || name.chars().any(char::is_control) {
        Err(Error::Invalid(
            "name is empty, too long or contains control characters",
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
    if value.base_color_png.len() > limits.max_texture_bytes {
        return Err(Error::Budget("texture bytes"));
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
    if ops.is_empty() || ops.len() > limits.max_operations {
        return Err(Error::Budget("transaction operations"));
    }
    let mut out = Vec::new();
    let execution_budget = ctx.limits.max_bytes;
    let colored_objects=state.surface.vertex_colors.keys().map(|(name,_)|name).collect::<BTreeSet<_>>();
    let color_source_bytes=state.objects.iter().filter(|(name,_)|colored_objects.contains(name)).map(|(n,m)|n.len()+m.memory_bytes()+128).sum::<usize>();
    if color_source_bytes.saturating_add(state.memory_bytes())>execution_budget{return Err(Error::Budget("vertex color provenance snapshot"));}
    let color_sources=if colored_objects.is_empty(){None}else{Some(state.objects.iter().filter(|(name,_)|colored_objects.contains(name)).map(|(n,m)|(n.clone(),m.clone())).collect::<BTreeMap<_,_>>())};
    drop(colored_objects);
    for op in ops {
        let target = op
            .object_name()
            .and_then(|name| state.objects.get(name))
            .map_or(0, mesh::Mesh::memory_bytes);
        let pinned = state.memory_bytes().saturating_sub(target).saturating_add(color_source_bytes);
        ctx.limits.max_bytes = execution_budget
            .checked_sub(pinned)
            .ok_or(Error::Budget("candidate working bytes"))?;
        ctx.checkpoint(1)?;
        match op {
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
                out.push(op.apply_in_scene(&mut state.objects, &state.scene, limits, ctx)?);
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
                if matches!(op,crate::SurfaceOperation::Bake{..}|crate::SurfaceOperation::ProjectedStroke{..}) {
                    let cache=state.scene.evaluated_meshes(state,ctx)?;
                    let mut world=std::collections::BTreeMap::new();
                    for (name,source) in &cache{ctx.checkpoint(1)?;let mut mesh=(**source).clone();let matrix=state.scene.world_matrix(name)?;let ids=mesh.vertices().iter().map(|v|v.id).collect::<Vec<_>>();mesh.transform(&ids,matrix,ctx)?;world.insert(name.clone(),mesh);}
                    state.surface.apply(op,&world,&state.materials,limits,ctx)?;
                } else {state.surface.apply(op, &state.objects, &state.materials, limits, ctx)?;}
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
                let texture = crate::Texture::solid(*width, *height, *color, limits)?;
                ctx.checkpoint(texture.rgb.len() as u64)?;
                let png = texture.to_png(limits)?;
                state.materials.get_mut(material).unwrap().base_color_png = png;
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
                let mut texture = crate::Texture::from_png(&material.base_color_png, limits)?;
                texture.paint_disk_ctx(*center, *radius, *color, limits, ctx)?;
                material.base_color_png = texture.to_png(limits)?;
                out.push(OperationResult::default());
                continue;
            }
            Operation::SetSkeleton { skeleton } => {
                skeleton.validate(limits, ctx)?;
                if let Some(old) = &state.skeleton {
                    for mesh in state.objects.values() {
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
                if !state.clips.contains_key(&clip.name) && state.clips.len() >= limits.max_clips {
                    return Err(Error::Budget("animation clips"));
                }
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
            if !state.materials.contains_key(material)
                && state.materials.len() >= limits.max_materials
            {
                return Err(Error::Budget("materials"));
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
            if state.objects.len() >= limits.max_objects {
                return Err(Error::Budget("objects"));
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
            state.objects.insert(object.clone(), mesh);
            out.push(result);
            continue;
        }
        let mesh = state
            .objects
            .get_mut(object)
            .ok_or_else(|| Error::MissingObject(object.clone()))?;
        let mut result = OperationResult {
            object: object.clone(),
            ..Default::default()
        };
        match op {
            Operation::DeleteObject { .. } => {
                state.objects.remove(object);
                state.scene.nodes.remove(object);
                state.scene.modifiers.remove(object);
                state.scene.lods.remove(object);
                state.scene.colliders.remove(object);
                state.surface.vertex_colors.retain(|(name,_),_| name != object);
                // A replacement with the same name is a new object, even if
                // its mesh allocator happens to reuse the old element IDs.
                state.selections.groups.retain(|(name,_),_| name != object);
            }
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
    if state.memory_bytes() > execution_budget {
        return Err(Error::Budget("candidate bytes"));
    }
    ctx.limits.max_bytes = execution_budget;
    if let Some(before) = color_sources { state.surface.reconcile(&before, &state.objects, limits, ctx)?; }
    state.selections.reconcile(&state.objects);
    state.selections.validate(state, limits)?;
    validate_surface_refs(state)?;
    for mesh in state.objects.values() {
        for face in mesh.faces() {
            if !state.materials.contains_key(&face.material) {
                return Err(Error::Invalid("unknown face material"));
            }
        }
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
    if state.surface.vertex_colors.keys().any(|(object,id)|state.objects.get(object).and_then(|m|m.vertex(*id)).is_none()) { return Err(Error::Invalid("surface references unknown vertex")); }
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
        Err(Error::Budget("texture working bytes"))
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
        for mesh in state.objects.values() {
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
        let mut keys = 0usize;
        if state.clips.len() > limits.max_clips {
            return Err(Error::Budget("animation clips"));
        }
        for clip in state.clips.values() {
            clip.validate(skeleton, limits, ctx)?;
            keys = keys.saturating_add(clip.channels.iter().map(|c| c.keys.len()).sum::<usize>());
            if keys > limits.max_keyframes {
                return Err(Error::Budget("total animation keyframes"));
            }
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
// Serialize the checkpoint exactly as a version 2 source does and measure it.
// The measured section stands in for the checkpoint when a transaction bounds
// the source it would produce, so this is the single source of that measure.
fn checkpoint_source_measure(state:&State,limits:&Limits,ctx:&mut mesh::Context)->Result<(Vec<u8>,SourceMeasure)>{
    let mut writer=Writer::new(limits.max_source_bytes);
    let start=ctx.work_used();
    write_state(&mut writer,state,limits,ctx,true)?;
    let measure=SourceMeasure{bytes:writer.bytes.len(),work:ctx.work_used()-start};
    Ok((writer.bytes,measure))
}
// Compute once per checkpoint (and derive directly while decoding). The
// serialized work counters are deliberately not trusted or persisted.
fn checkpoint_load_work(state:&State,limits:&Limits)->Result<(u64,SourceMeasure)>{
    let mut ctx=mesh::Context::new(limits.mesh.clone(),None);
    let (bytes,source)=checkpoint_source_measure(state,limits,&mut ctx)?;
    let mut reader=Reader::new(&bytes);
    read_state(&mut reader,limits,&mut ctx,2)?;reader.end()?;
    Ok((ctx.work_used(),source))
}
fn state_hash(state: &State, limits: &Limits, ctx: &mut mesh::Context) -> Result<[u8; 32]> {
    let mut w = Writer::new(limits.max_source_bytes);
    write_state(&mut w, state, limits, ctx, false)?;
    ctx.checkpoint(w.bytes.len() as u64)?;
    Ok(sha256(&w.bytes))
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
        base_color_png: r.blob(limits.max_texture_bytes)?.to_vec(),
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
        w.blob(&mesh.to_bytes(ctx)?)?;
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
        let mut payload=Writer::new(limits.max_source_bytes);
        crate::soft_body::write(&mut payload,metadata)?;
        extensions.push(("soft_body",payload.bytes));
    }
    if !state.scene.is_empty() {
        let mut payload = Writer::new(limits.max_source_bytes);
        state.scene.write(&mut payload)?;
        extensions.push(("scene", payload.bytes));
    }
    if !state.surface.materials.is_empty() || !state.surface.vertex_colors.is_empty() {
        let mut payload = Writer::new(limits.max_source_bytes);
        crate::surface::write_surface(&mut payload, &state.surface)?;
        extensions.push(("surface", payload.bytes));
    }
    if !state.selections.groups.is_empty() {
        let mut payload=Writer::new(limits.max_source_bytes);state.selections.write(&mut payload)?;extensions.push(("selections",payload.bytes));
    }
    if !state.rig.is_empty() {
        let mut payload = Writer::new(limits.max_source_bytes);
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
fn read_state(r: &mut Reader, limits: &Limits, ctx: &mut mesh::Context, version:u32) -> Result<State> {
    let mut state = State::default();
    for _ in 0..r.count(limits.max_materials)? {
        let id = r.u32()?;
        let value = read_material(r, limits)?;
        if state.materials.insert(id, value).is_some() {
            return Err(Error::Corrupt("duplicate material"));
        }
    }
    if !state.materials.contains_key(&0) {
        return Err(Error::Corrupt("missing default material"));
    }
    for _ in 0..r.count(limits.max_objects)? {
        let name = r.string(limits.max_name_bytes)?;
        check_name(&name, limits)?;
        let mesh = mesh::Mesh::from_bytes(r.blob(limits.mesh.max_bytes)?, ctx)?;
        if mesh
            .faces()
            .iter()
            .any(|f| !state.materials.contains_key(&f.material))
        {
            return Err(Error::Corrupt("face material"));
        }
        if state.objects.insert(name, mesh).is_some() {
            return Err(Error::Corrupt("duplicate object"));
        }
    }
    state.skeleton = match r.u8()? {
        0 => None,
        1 => Some(read_skeleton(r, limits)?),
        _ => return Err(Error::Corrupt("skeleton flag")),
    };
    for _ in 0..r.count(limits.max_clips)? {
        let clip = read_clip(r, limits)?;
        if state.clips.insert(clip.name.clone(), clip).is_some() {
            return Err(Error::Corrupt("duplicate animation clip"));
        }
    }
    if version >= 2 {
        if r.raw(6)? != b"MPEXT\0" { return Err(Error::Corrupt("state extension marker")); }
        let mut seen = BTreeSet::new();
        for _ in 0..r.count(16)? {
            let name = r.string(64)?;
            if !seen.insert(name.clone()) { return Err(Error::Corrupt("duplicate state extension")); }
            let mut payload = Reader::new(r.blob(limits.max_source_bytes)?);
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
    }
    state.selections.validate(&state, limits)?;
    validate_surface_refs(&state)?;
    validate_rig(&state, limits, ctx)?;
    state.scene.validate(&state, limits, ctx)?;
    state.rig.validate(&state, limits, ctx)?;
    crate::soft_body::validate(&state, limits, ctx)?;
    Ok(state)
}

fn write_ops(w: &mut Writer, ops: &[Operation], limits: &Limits) -> Result<()> {
    if ops.len() > limits.max_operations {
        return Err(Error::Budget("operations"));
    }
    w.count(ops.len())?;
    for op in ops {
        match op {
            Operation::SoftBodyUnbind => { w.u8(33)?; }
            Operation::SoftBody(op) => { w.u8(32)?; crate::schema::write_value(w,op.value())?; }
            Operation::Selection(op) => { w.u8(31)?; crate::schema::write_value(w, op.value())?; }
            Operation::Construction(op) => { w.u8(30)?; crate::schema::write_value(w, op.value())?; }
            Operation::Rig(op) => { w.u8(29)?; crate::schema::write_value(w, op.value())?; }
            Operation::MeshEditing(op) => { w.u8(28)?; crate::schema::write_value(w, op.value())?; }
            Operation::Surface(op) => { w.u8(27)?; crate::surface::write_surface_operation(w, op)?; }
            Operation::Scene(op) => { w.u8(26)?; crate::schema::write_value(w, op.value())?; }
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
    }
    Ok(())
}
fn read_ops(r: &mut Reader, limits: &Limits) -> Result<Vec<Operation>> {
    let mut ops = Vec::new();
    for _ in 0..r.count(limits.max_operations)? {
        let tag = r.u8()?;
        if tag == 11 {
            let material = r.u32()?;
            let value = read_material(r, limits)?;
            ops.push(Operation::SetMaterial { material, value });
            continue;
        }
        let global = match tag {
            33 => Some(Operation::SoftBodyUnbind),
            32 => Some(Operation::SoftBody(crate::SoftBodyBind::parse(&crate::schema::read_value(r,limits)?,limits)?.ok_or(Error::Corrupt("soft-body operation"))?)),
            31 => Some(Operation::Selection(crate::SelectionOperation::parse(&crate::schema::read_value(r,limits)?,limits)?.ok_or(Error::Corrupt("selection operation"))?)),
            30 => Some(Operation::Construction(crate::ConstructionOperation::parse(&crate::schema::read_value(r,limits)?,limits)?.ok_or(Error::Corrupt("construction operation"))?)),
            29 => Some(Operation::Rig(crate::RigOperation::parse(&crate::schema::read_value(r,limits)?,limits)?.ok_or(Error::Corrupt("rig operation"))?)),
            28 => Some(Operation::MeshEditing(crate::MeshEditingOperation::parse(&crate::schema::read_value(r,limits)?,limits)?.ok_or(Error::Corrupt("mesh editing operation"))?)),
            27 => Some(Operation::Surface(crate::surface::read_surface_operation(r, limits)?)),
            26 => Some(Operation::Scene(crate::SceneOperation::parse(&crate::schema::read_value(r, limits)?, limits)?
                .ok_or(Error::Corrupt("scene operation"))?)),
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
                name: r.string(limits.max_name_bytes)?,
            }),
            _ => None,
        };
        if let Some(op) = global {
            ops.push(op);
            continue;
        }
        let object = r.string(limits.max_name_bytes)?;
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
                source: r.blob(limits.mesh.max_bytes)?.to_vec(),
            },
            3 => Operation::DeleteObject { object },
            4 => {
                let mut vertices = Vec::new();
                for _ in 0..r.count(limits.mesh.max_vertices)? {
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
                for _ in 0..r.count(limits.mesh.max_faces)? {
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
                for _ in 0..r.count(limits.mesh.max_weights_per_vertex)? {
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
                for _ in 0..r.count(limits.mesh.max_faces)? {
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
                for _ in 0..r.count(limits.mesh.max_vertices)? {
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
                for _ in 0..r.count(limits.mesh.max_vertices)? {
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
                for _ in 0..r.count(limits.mesh.max_faces)? {
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
                for _ in 0..r.count(limits.mesh.max_vertices)? {
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
        ops.push(op);
    }
    Ok(ops)
}

#[cfg(test)]
mod history_budget_tests {
    use super::*;
    fn apply(doc:&mut Document,id:&str,operations:Vec<Operation>)->Result<Applied>{doc.apply(Transaction{request_id:id.into(),expected:doc.head(),operations},None)}
    fn edit(doc:&Document,index:usize)->Operation{Operation::SetUv{object:"body".into(),corner:doc.object("body").unwrap().corners()[0].id,uv:[index as f64*0.01,0.25]}}
    #[test]
    fn admitted_heavy_history_keeps_every_cursor_replayable_and_refuses_growth_atomically(){
        let mut base=Document::new(Limits::default()).unwrap();
        let mut seed=17u32;let rgb=(0..256*256*3).map(|_|{seed^=seed<<13;seed^=seed>>17;seed^=seed<<5;seed as u8}).collect();
        let png=crate::Texture{width:256,height:256,rgb}.to_png(base.limits()).unwrap();assert!(png.len()>190_000);
        apply(&mut base,"seed",vec![Operation::Cube{object:"body".into(),size:[1.;3]},Operation::SetMaterial{material:0,value:Material{color:[1.;3],base_color_png:png}}]).unwrap();
        base.checkpoint(base.head()).unwrap();
        // Calibrate from actually admitted operations, not a guessed model size.
        let mut probe=base.clone();
        for i in 0..8{let op=edit(&probe,i);apply(&mut probe,&format!("edit-{i}"),vec![op]).unwrap();}
        let mut ctx=mesh::Context::default();probe.encode(&mut ctx).unwrap();
        let budget=probe.history_work_needed(ctx.work_used());
        let mut limits=base.limits.clone();limits.mesh.max_work=budget;
        let mut doc=Document::from_bytes(&base.to_bytes(None).unwrap(),limits.clone(),None).unwrap();
        for i in 0..8{let op=edit(&doc,i);apply(&mut doc,&format!("edit-{i}"),vec![op]).unwrap();}
        let before=doc.to_bytes(None).unwrap();let ninth=edit(&doc,9);
        assert_eq!(apply(&mut doc,"edit-9",vec![ninth.clone()]).unwrap_err(),Error::Budget("history replay work; checkpoint explicitly before editing further"));
        assert_eq!(doc.to_bytes(None).unwrap(),before);
        let saved=doc.head();
        for _ in 0..8{doc.undo(doc.head(),None).unwrap();let bytes=doc.to_bytes(None).unwrap();let opened=Document::from_bytes(&bytes,limits.clone(),None).unwrap();assert_eq!(opened.head(),doc.head());}
        for _ in 0..8{doc.redo(doc.head(),None).unwrap();}
        assert_eq!(doc.head().content,saved.content);
        doc.checkpoint(doc.head()).unwrap();apply(&mut doc,"edit-9",vec![ninth]).unwrap();
        Document::from_bytes(&doc.to_bytes(None).unwrap(),limits,None).unwrap();
    }
}
