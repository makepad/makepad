use crate::depth_debug_mesh::{
    debug_depth_mesh_view_plan, DebugDepthMeshChunk, DebugDepthMeshChunkSignature,
    DebugDepthMeshTriangulator,
};
use crate::*;
use makepad_widgets::makepad_platform::{
    ChunkKey, SparseTsdGridReadSnapshot, TsdfPublishedSnapshot,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

const XR_DEPTH_DEBUG_MESH_RESULT_BATCH_CHUNKS: usize = 4;
const XR_DEPTH_DEBUG_MESH_RESULT_BATCH_REMOVALS: usize = 16;

struct DepthDebugMeshWorkerRequest {
    request_id: u64,
    snapshot: Arc<TsdfPublishedSnapshot>,
    head_pose: Pose,
}

#[derive(Default)]
struct DepthDebugMeshWorkerMailbox {
    version: u64,
    shutdown: bool,
    pending_request: Option<DepthDebugMeshWorkerRequest>,
}

pub(crate) struct XrDepthDebugMeshVisibleSet {
    pub(crate) request_id: u64,
    pub(crate) generation: u64,
    pub(crate) update_sequence: u64,
    pub(crate) snapshot_grid: Arc<SparseTsdGridReadSnapshot>,
    pub(crate) visible_chunk_keys: Vec<ChunkKey>,
}

pub(crate) enum XrDepthDebugMeshWorkerResult {
    VisibleSet(XrDepthDebugMeshVisibleSet),
    ChunkUpserts {
        request_id: u64,
        chunks: Vec<DebugDepthMeshChunk>,
    },
    ChunkRemovals {
        request_id: u64,
        chunk_keys: Vec<ChunkKey>,
    },
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
        Self {
            fingerprint: chunk.fingerprint,
            indices: chunk.indices,
            vertices: chunk.vertices,
        }
    }

    fn to_chunk(&self, chunk_key: ChunkKey) -> DebugDepthMeshChunk {
        DebugDepthMeshChunk {
            chunk_key,
            fingerprint: self.fingerprint,
            indices: self.indices.clone(),
            vertices: self.vertices.clone(),
        }
    }
}

pub(crate) struct XrDepthDebugMeshWorker {
    mailbox: Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>,
    pending_results: Arc<Mutex<VecDeque<XrDepthDebugMeshWorkerResult>>>,
    join_handle: Option<JoinHandle<()>>,
    next_request_id: u64,
}

impl XrDepthDebugMeshWorker {
    pub(crate) fn new() -> Self {
        let mailbox = Arc::new((
            Mutex::new(DepthDebugMeshWorkerMailbox::default()),
            Condvar::new(),
        ));
        let pending_results = Arc::new(Mutex::new(VecDeque::new()));
        let mailbox_thread = mailbox.clone();
        let results_thread = pending_results.clone();
        let join_handle = thread::Builder::new()
            .name("makepad-xr-depth-debug-mesh".to_string())
            .spawn(move || depth_debug_mesh_worker_loop(mailbox_thread, results_thread))
            .ok();

        Self {
            mailbox,
            pending_results,
            join_handle,
            next_request_id: 0,
        }
    }

    pub(crate) fn request_snapshot(
        &mut self,
        snapshot: Arc<TsdfPublishedSnapshot>,
        head_pose: Pose,
    ) {
        self.next_request_id = self.next_request_id.wrapping_add(1);
        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }

        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_request = Some(DepthDebugMeshWorkerRequest {
                request_id: self.next_request_id,
                snapshot,
                head_pose,
            });
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }

    pub(crate) fn take_next_result(&mut self) -> Option<XrDepthDebugMeshWorkerResult> {
        self.pending_results
            .lock()
            .ok()
            .and_then(|mut results| results.pop_front())
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

fn push_worker_result(
    pending_results: &Mutex<VecDeque<XrDepthDebugMeshWorkerResult>>,
    result: XrDepthDebugMeshWorkerResult,
) -> bool {
    if let Ok(mut pending_results) = pending_results.lock() {
        pending_results.push_back(result);
        true
    } else {
        false
    }
}

fn request_obsolete(
    mailbox: &Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>,
    active_version: u64,
) -> bool {
    let (lock, _) = &**mailbox;
    lock.lock()
        .map(|guard| guard.shutdown || guard.version != active_version)
        .unwrap_or(true)
}

fn depth_debug_mesh_worker_loop(
    mailbox: Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>,
    pending_results: Arc<Mutex<VecDeque<XrDepthDebugMeshWorkerResult>>>,
) {
    let mut seen_version = 0u64;
    let mut triangulator = DebugDepthMeshTriangulator::default();
    let mut cache = HashMap::<ChunkKey, CachedDebugDepthMeshChunk>::new();

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
        let view_plan = debug_depth_mesh_view_plan(request.snapshot.as_ref(), request.head_pose);
        if request_obsolete(&mailbox, active_version) {
            continue;
        }

        if let Ok(mut results) = pending_results.lock() {
            results.clear();
        } else {
            return;
        }
        if !push_worker_result(
            &pending_results,
            XrDepthDebugMeshWorkerResult::VisibleSet(XrDepthDebugMeshVisibleSet {
                request_id: request.request_id,
                generation: request.snapshot.generation,
                update_sequence: request.snapshot.update_sequence,
                snapshot_grid: request.snapshot.grid.clone(),
                visible_chunk_keys: view_plan
                    .visible_chunks
                    .iter()
                    .map(|plan| plan.chunk_key)
                    .collect(),
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
                XrDepthDebugMeshWorkerResult::ChunkRemovals {
                    request_id: request.request_id,
                    chunk_keys: std::mem::take(chunk_keys),
                },
            )
        };
        let flush_upserts = |chunks: &mut Vec<DebugDepthMeshChunk>| -> bool {
            if chunks.is_empty() {
                return true;
            }
            push_worker_result(
                &pending_results,
                XrDepthDebugMeshWorkerResult::ChunkUpserts {
                    request_id: request.request_id,
                    chunks: std::mem::take(chunks),
                },
            )
        };

        for plan in &view_plan.visible_chunks {
            if request_obsolete(&mailbox, active_version) {
                break;
            }

            let entry = cache.entry(plan.chunk_key).or_default();
            if entry.signature == plan.signature {
                if !entry.sent_to_ui {
                    if let Some(mesh) = entry.mesh.as_ref() {
                        upsert_batch.push(mesh.to_chunk(plan.chunk_key));
                        entry.sent_to_ui = true;
                        if upsert_batch.len() >= XR_DEPTH_DEBUG_MESH_RESULT_BATCH_CHUNKS
                            && !flush_upserts(&mut upsert_batch)
                        {
                            return;
                        }
                    }
                }
                continue;
            }

            if entry.sent_to_ui {
                removal_batch.push(plan.chunk_key);
                entry.sent_to_ui = false;
                if removal_batch.len() >= XR_DEPTH_DEBUG_MESH_RESULT_BATCH_REMOVALS
                    && !flush_removals(&mut removal_batch)
                {
                    return;
                }
            }

            entry.signature = plan.signature.clone();
            entry.mesh = triangulator
                .build_chunk(request.snapshot.as_ref(), view_plan.layout, plan)
                .map(CachedDebugDepthMeshChunkData::from_chunk);
            if let Some(mesh) = entry.mesh.as_ref() {
                upsert_batch.push(mesh.to_chunk(plan.chunk_key));
                entry.sent_to_ui = true;
                if upsert_batch.len() >= XR_DEPTH_DEBUG_MESH_RESULT_BATCH_CHUNKS
                    && !flush_upserts(&mut upsert_batch)
                {
                    return;
                }
            }
        }

        if request_obsolete(&mailbox, active_version) {
            continue;
        }
        if !flush_removals(&mut removal_batch) || !flush_upserts(&mut upsert_batch) {
            return;
        }
    }
}
