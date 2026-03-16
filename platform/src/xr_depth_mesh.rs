use crate::makepad_math::{vec3, Vec3f};
use parry3d::math::IVector;
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshStats {
    pub frames_seen: u64,
    pub frames_meshed: u64,
    pub frames_dropped: u64,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshChunk {
    pub generation: u64,
    pub chunk_key: IVector,
    pub fingerprint: u64,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub vertices: Vec<Vec3f>,
    pub normals: Vec<Vec3f>,
    pub indices: Vec<u32>,
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
    pub dirty_chunk_keys: Vec<IVector>,
    pub removed_chunk_keys: Vec<IVector>,
    pub mesh_generation: u64,
    pub mesh_vertex_count: usize,
    pub mesh_triangle_count: usize,
}

impl XrDepthMesh {
    pub fn triangle_count(&self) -> usize {
        self.mesh_triangle_count
    }
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshState {
    pub latest_mesh: Option<Arc<XrDepthMesh>>,
    pub stats: XrDepthMeshStats,
    pub last_error: Option<String>,
}

#[derive(Clone, Default)]
pub struct XrDepthMeshStore(Arc<RwLock<XrDepthMeshState>>);

impl XrDepthMeshStore {
    pub fn state(&self) -> Arc<RwLock<XrDepthMeshState>> {
        self.0.clone()
    }

    pub fn latest_mesh(&self) -> Option<Arc<XrDepthMesh>> {
        self.0
            .read()
            .ok()
            .and_then(|state| state.latest_mesh.clone())
    }

    #[allow(dead_code)]
    pub(crate) fn record_seen(&self) {
        if let Ok(mut state) = self.0.write() {
            state.stats.frames_seen += 1;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn record_drop(&self) {
        if let Ok(mut state) = self.0.write() {
            state.stats.frames_dropped += 1;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_error(&self, error: String) {
        if let Ok(mut state) = self.0.write() {
            state.last_error = Some(error);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn publish(&self, mesh: XrDepthMesh) {
        if let Ok(mut state) = self.0.write() {
            state.latest_mesh = Some(Arc::new(mesh));
            state.stats.frames_meshed += 1;
            state.last_error = None;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.0.write() {
            state.latest_mesh = None;
            state.last_error = None;
            state.stats = XrDepthMeshStats::default();
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
