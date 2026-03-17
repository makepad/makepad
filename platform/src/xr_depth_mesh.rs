use crate::makepad_math::{vec3, Vec3f};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock, RwLock},
};

const XR_DEPTH_QUERY_MAX_PENDING: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChunkKey {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkKey {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshStats {
    pub frames_seen: u64,
    pub frames_meshed: u64,
    pub frames_dropped: u64,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshChunk {
    pub generation: u64,
    pub chunk_key: ChunkKey,
    pub fingerprint: u64,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub vertices: Vec<Vec3f>,
    pub normals: Vec<Vec3f>,
    pub indices: Vec<u32>,
    pub planar_patches: Vec<XrDepthPlanePatch>,
}

impl XrDepthMeshChunk {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMesh {
    pub generation: u64,
    pub latest_topology_generation: u64,
    pub update_sequence: u64,
    pub eye_index: usize,
    pub image_width: u32,
    pub image_height: u32,
    pub sample_step: u32,
    pub voxel_size_meters: f32,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub mesh_chunks: Vec<XrDepthMeshChunk>,
    pub dirty_chunk_keys: Vec<ChunkKey>,
    pub removed_chunk_keys: Vec<ChunkKey>,
    pub mesh_generation: u64,
    pub mesh_vertex_count: usize,
    pub mesh_triangle_count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshState {
    pub latest_mesh: Option<Arc<XrDepthMesh>>,
    pub stats: XrDepthMeshStats,
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XrDepthPlaneKind {
    Floor,
    Table,
    Ceiling,
    Wall,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthPlanePatch {
    pub generation: u64,
    pub kind: XrDepthPlaneKind,
    pub center: Vec3f,
    pub normal: Vec3f,
    pub tangent: Vec3f,
    pub bitangent: Vec3f,
    pub half_extent_tangent: f32,
    pub half_extent_bitangent: f32,
    pub area: f32,
    pub support_triangles: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct XrDepthMeshQuery {
    pub key: u64,
    pub center: Vec3f,
    pub predicted_center: Vec3f,
    pub velocity: Vec3f,
    pub radius: f32,
    pub max_distance: f32,
    pub include_planar_patches: bool,
}

#[derive(Clone, Debug)]
pub struct XrDepthMeshQuerySurfaceHit {
    pub distance: f32,
    pub point: Vec3f,
    pub normal: Vec3f,
    pub from_planar_patch: bool,
    pub triangle: [Vec3f; 3],
    pub patch: [Vec3f; 4],
    pub chunk_key: ChunkKey,
}

#[derive(Clone, Debug)]
pub struct XrDepthMeshQueryHit {
    pub key: u64,
    pub version: u64,
    pub mesh_generation: u64,
    pub distance: f32,
    pub point: Vec3f,
    pub normal: Vec3f,
    pub from_planar_patch: bool,
    pub triangle: [Vec3f; 3],
    pub patch: [Vec3f; 4],
    pub chunk_key: ChunkKey,
    pub additional_hits: Vec<XrDepthMeshQuerySurfaceHit>,
}

#[derive(Clone, Debug)]
pub enum XrDepthMeshQueryResult {
    Hit(XrDepthMeshQueryHit),
    Miss {
        key: u64,
        version: u64,
        mesh_generation: u64,
    },
}

impl XrDepthMeshQueryResult {
    pub fn key(&self) -> u64 {
        match self {
            Self::Hit(hit) => hit.key,
            Self::Miss { key, .. } => *key,
        }
    }

    pub fn version(&self) -> u64 {
        match self {
            Self::Hit(hit) => hit.version,
            Self::Miss { version, .. } => *version,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct XrDepthMeshPendingQuery {
    pub query: XrDepthMeshQuery,
    pub version: u64,
}

#[derive(Default)]
struct XrDepthMeshQueryState {
    next_versions: HashMap<u64, u64>,
    pending: HashMap<u64, XrDepthMeshPendingQuery>,
    pending_order: VecDeque<u64>,
    results: HashMap<u64, XrDepthMeshQueryResult>,
}

#[derive(Clone)]
pub struct XrDepthMeshStore {
    state: Arc<RwLock<XrDepthMeshState>>,
    queries: Arc<Mutex<XrDepthMeshQueryState>>,
}

impl Default for XrDepthMeshStore {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(XrDepthMeshState::default())),
            queries: Arc::new(Mutex::new(XrDepthMeshQueryState::default())),
        }
    }
}

impl XrDepthMeshStore {
    pub fn state(&self) -> Arc<RwLock<XrDepthMeshState>> {
        self.state.clone()
    }

    pub fn latest_mesh(&self) -> Option<Arc<XrDepthMesh>> {
        self.state
            .read()
            .ok()
            .and_then(|state| state.latest_mesh.clone())
    }

    pub fn submit_query(&self, query: XrDepthMeshQuery) -> Option<u64> {
        let Ok(mut state) = self.queries.lock() else {
            return None;
        };
        let version = state
            .next_versions
            .entry(query.key)
            .and_modify(|version| *version = version.saturating_add(1))
            .or_insert(1);
        let version = *version;

        if let Some(pending) = state.pending.get_mut(&query.key) {
            pending.query = query;
            pending.version = version;
            return Some(version);
        }

        if state.pending.len() >= XR_DEPTH_QUERY_MAX_PENDING {
            return None;
        }

        state
            .pending
            .insert(query.key, XrDepthMeshPendingQuery { query, version });
        state.pending_order.push_back(query.key);
        Some(version)
    }

    pub fn latest_query_result(&self, key: u64) -> Option<XrDepthMeshQueryResult> {
        self.queries
            .lock()
            .ok()
            .and_then(|state| state.results.get(&key).cloned())
    }

    pub fn clear_query(&self, key: u64) {
        if let Ok(mut state) = self.queries.lock() {
            state.pending.remove(&key);
            state
                .pending_order
                .retain(|pending_key| *pending_key != key);
            state.results.remove(&key);
            state.next_versions.remove(&key);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn record_seen(&self) {
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_seen += 1;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn record_drop(&self) {
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_dropped += 1;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_error(&self, error: String) {
        if let Ok(mut state) = self.state.write() {
            state.last_error = Some(error);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn publish(&self, mesh: XrDepthMesh) {
        if let Ok(mut state) = self.state.write() {
            state.latest_mesh = Some(Arc::new(mesh));
            state.stats.frames_meshed += 1;
            state.last_error = None;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn drain_pending_queries(&self, max_queries: usize) -> Vec<XrDepthMeshPendingQuery> {
        let Ok(mut state) = self.queries.lock() else {
            return Vec::new();
        };
        let mut drained = Vec::with_capacity(max_queries.min(state.pending.len()));
        for _ in 0..max_queries {
            let Some(key) = state.pending_order.pop_front() else {
                break;
            };
            let Some(query) = state.pending.remove(&key) else {
                continue;
            };
            drained.push(query);
        }
        drained
    }

    #[allow(dead_code)]
    pub(crate) fn publish_query_results(&self, results: Vec<XrDepthMeshQueryResult>) {
        if results.is_empty() {
            return;
        }
        if let Ok(mut state) = self.queries.lock() {
            for result in results {
                state.results.insert(result.key(), result);
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.state.write() {
            state.latest_mesh = None;
            state.last_error = None;
            state.stats = XrDepthMeshStats::default();
        }
        if let Ok(mut queries) = self.queries.lock() {
            *queries = XrDepthMeshQueryState::default();
        }
    }
}

pub fn xr_depth_mesh_store() -> XrDepthMeshStore {
    static STORE: OnceLock<XrDepthMeshStore> = OnceLock::new();
    STORE.get_or_init(XrDepthMeshStore::default).clone()
}

#[allow(dead_code)]
pub(crate) fn empty_bounds() -> (Vec3f, Vec3f) {
    (vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, 0.0))
}
