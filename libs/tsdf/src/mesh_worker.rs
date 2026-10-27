//! Meshes field snapshots on its own thread (ported from the XR example's
//! `depth_debug_mesh_worker.rs`): the caller hands it the newest snapshot and a selection
//! (latest wins; a request still being meshed is given up for the newer one), the worker
//! re-meshes only the mesh chunks whose source chunks changed (their signature) and queues what
//! the caller must change: the visible set, upserted meshes, removed meshes. The caller never
//! waits; it takes results when it draws.
//!
//! Every vertex carries, in its position's w, when the brick holding it last changed (seconds
//! since [`mesh_time_base`]), so a shader can tint fresh updates.
use crate::mesh::{
    debug_depth_mesh_all_plan, debug_depth_mesh_focus_cube_plan, debug_depth_mesh_view_plan, snapshot_debug_mesh_layout,
    DebugDepthMeshChunk, DebugDepthMeshChunkSignature, DebugDepthMeshTriangulator, XR_DEBUG_DEPTH_FLOATS_PER_VERTEX,
};
use crate::{ChunkKey, SparseTsdGridReadSnapshot, SparseTsdReadChunk, TsdfPublishedSnapshot};
use makepad_math::{Pose, Vec3f};
use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread::{self, JoinHandle},
    time::Instant,
};

const XR_DEPTH_DEBUG_MESH_RESULT_BATCH_CHUNKS: usize = 4;
const XR_DEPTH_DEBUG_MESH_RESULT_BATCH_REMOVALS: usize = 16;

/// The clock the vertex time stamps count from (seconds as f32; shared by the worker and the
/// shader's "now").
pub fn mesh_time_base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

/// Seconds since [`mesh_time_base`].
pub fn mesh_time_now() -> f32 {
    mesh_time_base().elapsed().as_secs_f32()
}

struct DepthDebugMeshWorkerRequest {
    request_id: u64,
    snapshot: Arc<TsdfPublishedSnapshot>,
    selection: XrDepthDebugMeshSelection,
}

#[derive(Default)]
struct DepthDebugMeshWorkerMailbox {
    version: u64,
    shutdown: bool,
    pending_request: Option<DepthDebugMeshWorkerRequest>,
}

pub struct XrDepthDebugMeshVisibleSet {
    pub request_id: u64,
    pub generation: u64,
    pub update_sequence: u64,
    pub snapshot_grid: Arc<SparseTsdGridReadSnapshot>,
    pub visible_chunk_keys: Vec<ChunkKey>,
}

pub enum XrDepthDebugMeshWorkerResult {
    VisibleSet(XrDepthDebugMeshVisibleSet),
    ChunkUpserts { request_id: u64, chunks: Vec<DebugDepthMeshChunk> },
    ChunkRemovals { request_id: u64, chunk_keys: Vec<ChunkKey> },
}

#[derive(Clone, Copy, Debug)]
pub enum XrDepthDebugMeshSelection {
    HeadView { head_pose: Pose },
    FocusCube { center: Vec3f, cube_size_meters: f32 },
    /// The whole field, nearest to `focus` first.
    All { focus: Vec3f },
}

#[derive(Default)]
struct CachedDebugDepthMeshChunk {
    signature: DebugDepthMeshChunkSignature,
    mesh: Option<CachedDebugDepthMeshChunkData>,
    sent_to_ui: bool,
}

struct CachedDebugDepthMeshChunkData {
    fingerprint: u64,
    indices: Vec<u32>,
    vertices: Vec<f32>,
}

impl CachedDebugDepthMeshChunkData {
    fn from_chunk(chunk: DebugDepthMeshChunk) -> Self {
        Self { fingerprint: chunk.fingerprint, indices: chunk.indices, vertices: chunk.vertices }
    }

    fn to_chunk(&self, chunk_key: ChunkKey) -> DebugDepthMeshChunk {
        DebugDepthMeshChunk { chunk_key, fingerprint: self.fingerprint, indices: self.indices.clone(), vertices: self.vertices.clone() }
    }
}

pub struct XrDepthDebugMeshWorker {
    mailbox: Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>,
    pending_results: Arc<Mutex<VecDeque<XrDepthDebugMeshWorkerResult>>>,
    join_handle: Option<JoinHandle<()>>,
    next_request_id: u64,
}

impl XrDepthDebugMeshWorker {
    /// A worker meshing in chunks about `chunk_world_size_meters` across with cells of
    /// `stride_voxels` voxels ([`crate::mesh::XR_DEBUG_TSDF_MESH_CHUNK_WORLD_SIZE_METERS`] and
    /// [`crate::mesh::XR_DEBUG_TSDF_MESH_CELL_STRIDE_VOXELS`] for the XR example's view).
    pub fn new(chunk_world_size_meters: f32, stride_voxels: i32) -> Self {
        let mailbox = Arc::new((Mutex::new(DepthDebugMeshWorkerMailbox::default()), Condvar::new()));
        let pending_results = Arc::new(Mutex::new(VecDeque::new()));
        let mailbox_thread = mailbox.clone();
        let results_thread = pending_results.clone();
        mesh_time_base();
        let join_handle = thread::Builder::new()
            .name("makepad-xr-depth-debug-mesh".to_string())
            .spawn(move || {
                crate::depth_writer::background_priority();
                depth_debug_mesh_worker_loop(mailbox_thread, results_thread, chunk_world_size_meters, stride_voxels)
            })
            .ok();

        Self { mailbox, pending_results, join_handle, next_request_id: 0 }
    }

    pub fn request_snapshot(&mut self, snapshot: Arc<TsdfPublishedSnapshot>, selection: XrDepthDebugMeshSelection) {
        self.next_request_id = self.next_request_id.wrapping_add(1);
        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }

        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_request = Some(DepthDebugMeshWorkerRequest { request_id: self.next_request_id, snapshot, selection });
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }

    pub fn take_next_result(&mut self) -> Option<XrDepthDebugMeshWorkerResult> {
        self.pending_results.lock().ok().and_then(|mut results| results.pop_front())
    }
}

impl Drop for XrDepthDebugMeshWorker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.shutdown = true;
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn push_worker_result(pending_results: &Mutex<VecDeque<XrDepthDebugMeshWorkerResult>>, result: XrDepthDebugMeshWorkerResult) -> bool {
    if let Ok(mut pending_results) = pending_results.lock() {
        pending_results.push_back(result);
        true
    } else {
        false
    }
}

fn request_obsolete(mailbox: &Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>, active_version: u64) -> bool {
    let (lock, _) = &**mailbox;
    lock.lock().map(|guard| guard.shutdown || guard.version != active_version).unwrap_or(true)
}

/// When each source chunk last changed (the TSDF replaced it), on the worker's clock.
#[derive(Default)]
struct SourceAges {
    seen: HashMap<ChunkKey, (Arc<SparseTsdReadChunk>, f32)>,
}

impl SourceAges {
    fn note(&mut self, grid: &SparseTsdGridReadSnapshot, now: f32) {
        for (key, chunk) in &grid.chunks {
            match self.seen.get_mut(key) {
                Some(entry) if Arc::ptr_eq(&entry.0, chunk) => {}
                Some(entry) => *entry = (chunk.clone(), now),
                None => {
                    self.seen.insert(*key, (chunk.clone(), now));
                }
            }
        }
        if self.seen.len() > grid.chunks.len() {
            self.seen.retain(|key, _| grid.chunks.contains_key(key));
        }
    }

    /// Writes each vertex's brick time into its position's w (the newest of the mesh's sources
    /// for a vertex whose brick holds no voxels) and folds the times into the fingerprint.
    fn stamp(&self, chunk: &mut DebugDepthMeshChunk, sources: &DebugDepthMeshChunkSignature, voxel_size: f32, chunk_edge: i32) {
        let newest = sources.sources.iter().filter_map(|(key, _)| self.seen.get(key).map(|s| s.1)).fold(0.0f32, f32::max);
        let voxel = voxel_size.max(1.0e-5);
        let edge = chunk_edge.max(1);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        chunk.fingerprint.hash(&mut hasher);
        newest.to_bits().hash(&mut hasher);
        for v in chunk.vertices.chunks_exact_mut(XR_DEBUG_DEPTH_FLOATS_PER_VERTEX) {
            // vertices sit at voxel centres ((i + 0.5) * voxel)
            let at = |p: f32| ((p / voxel - 0.5).round() as i32).div_euclid(edge);
            let key = ChunkKey::new(at(v[0]), at(v[1]), at(v[2]));
            v[3] = self.seen.get(&key).map_or(newest, |s| s.1);
        }
        chunk.fingerprint = hasher.finish();
    }
}

fn depth_debug_mesh_worker_loop(
    mailbox: Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>,
    pending_results: Arc<Mutex<VecDeque<XrDepthDebugMeshWorkerResult>>>,
    chunk_world_size_meters: f32,
    stride_voxels: i32,
) {
    let mut seen_version = 0u64;
    let mut triangulator = DebugDepthMeshTriangulator::default();
    let mut cache = HashMap::<ChunkKey, CachedDebugDepthMeshChunk>::new();
    let mut ages = SourceAges::default();
    let mut ages_of: Option<Arc<SparseTsdGridReadSnapshot>> = None;

    loop {
        let (active_version, request) = {
            let (lock, wake) = &*mailbox;
            let mut guard = match lock.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            while !guard.shutdown && guard.version == seen_version {
                guard = match wake.wait(guard) {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
            }
            if guard.shutdown {
                return;
            }
            seen_version = guard.version;
            (seen_version, guard.pending_request.take())
        };

        let Some(request) = request else {
            continue;
        };
        if !ages_of.as_ref().is_some_and(|g| Arc::ptr_eq(g, &request.snapshot.grid)) {
            ages.note(&request.snapshot.grid, mesh_time_now());
            ages_of = Some(request.snapshot.grid.clone());
        }
        let layout = snapshot_debug_mesh_layout(request.snapshot.as_ref(), chunk_world_size_meters, stride_voxels);
        let view_plan = match request.selection {
            XrDepthDebugMeshSelection::HeadView { head_pose } => debug_depth_mesh_view_plan(request.snapshot.as_ref(), layout, head_pose),
            XrDepthDebugMeshSelection::FocusCube { center, cube_size_meters } => {
                debug_depth_mesh_focus_cube_plan(request.snapshot.as_ref(), layout, center, cube_size_meters)
            }
            XrDepthDebugMeshSelection::All { focus } => debug_depth_mesh_all_plan(request.snapshot.as_ref(), layout, focus),
        };
        if request_obsolete(&mailbox, active_version) {
            continue;
        }

        // The caller drops the meshes that left the visible set, so they go again when they come
        // back (without this a chunk that left the view and returned unchanged stayed missing).
        let visible: std::collections::HashSet<ChunkKey> = view_plan.visible_chunks.iter().map(|plan| plan.chunk_key).collect();
        for (key, entry) in cache.iter_mut() {
            if !visible.contains(key) {
                entry.sent_to_ui = false;
            }
        }
        if !push_worker_result(
            &pending_results,
            XrDepthDebugMeshWorkerResult::VisibleSet(XrDepthDebugMeshVisibleSet {
                request_id: request.request_id,
                generation: request.snapshot.generation,
                update_sequence: request.snapshot.update_sequence,
                snapshot_grid: request.snapshot.grid.clone(),
                visible_chunk_keys: visible.into_iter().collect(),
            }),
        ) {
            return;
        }

        let mut removal_batch = Vec::<ChunkKey>::new();
        let mut upsert_batch = Vec::<DebugDepthMeshChunk>::new();
        let flush_removals = |chunk_keys: &mut Vec<ChunkKey>| -> bool {
            if chunk_keys.is_empty() {
                return true;
            }
            push_worker_result(
                &pending_results,
                XrDepthDebugMeshWorkerResult::ChunkRemovals { request_id: request.request_id, chunk_keys: std::mem::take(chunk_keys) },
            )
        };
        let flush_upserts = |chunks: &mut Vec<DebugDepthMeshChunk>| -> bool {
            if chunks.is_empty() {
                return true;
            }
            push_worker_result(
                &pending_results,
                XrDepthDebugMeshWorkerResult::ChunkUpserts { request_id: request.request_id, chunks: std::mem::take(chunks) },
            )
        };

        let mut aborted = false;
        for plan in &view_plan.visible_chunks {
            if request_obsolete(&mailbox, active_version) {
                aborted = true;
                break;
            }

            let entry = cache.entry(plan.chunk_key).or_default();
            if entry.signature == plan.signature {
                if !entry.sent_to_ui {
                    if let Some(mesh) = entry.mesh.as_ref() {
                        upsert_batch.push(mesh.to_chunk(plan.chunk_key));
                        entry.sent_to_ui = true;
                        if upsert_batch.len() >= XR_DEPTH_DEBUG_MESH_RESULT_BATCH_CHUNKS && !flush_upserts(&mut upsert_batch) {
                            return;
                        }
                    }
                }
                continue;
            }

            let was_sent_to_ui = entry.sent_to_ui;
            entry.signature = plan.signature.clone();
            entry.mesh = triangulator.build_chunk(request.snapshot.as_ref(), view_plan.layout, plan).map(|mut chunk| {
                ages.stamp(&mut chunk, &plan.signature, request.snapshot.grid.voxel_size, request.snapshot.grid.chunk_edge);
                CachedDebugDepthMeshChunkData::from_chunk(chunk)
            });
            if let Some(mesh) = entry.mesh.as_ref() {
                upsert_batch.push(mesh.to_chunk(plan.chunk_key));
                entry.sent_to_ui = true;
                if upsert_batch.len() >= XR_DEPTH_DEBUG_MESH_RESULT_BATCH_CHUNKS && !flush_upserts(&mut upsert_batch) {
                    return;
                }
            } else {
                entry.sent_to_ui = false;
                if was_sent_to_ui {
                    removal_batch.push(plan.chunk_key);
                    if removal_batch.len() >= XR_DEPTH_DEBUG_MESH_RESULT_BATCH_REMOVALS && !flush_removals(&mut removal_batch) {
                        return;
                    }
                }
            }
        }

        if aborted {
            if !flush_removals(&mut removal_batch) || !flush_upserts(&mut upsert_batch) {
                return;
            }
            continue;
        }
        if !flush_removals(&mut removal_batch) || !flush_upserts(&mut upsert_batch) {
            return;
        }
    }
}
