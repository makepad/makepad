use crate::makepad_math::{vec3, Vec3f};
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Clone, Debug, Default)]
pub struct XrDepthMeshStats {
    pub frames_seen: u64,
    pub frames_meshed: u64,
    pub frames_dropped: u64,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthMesh {
    pub generation: u64,
    pub eye_index: usize,
    pub image_width: u32,
    pub image_height: u32,
    pub sample_step: u32,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub vertices: Vec<Vec3f>,
    pub indices: Vec<u32>,
}

impl XrDepthMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn write_position_buffer(&self, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.vertices.len() * 3);
        for vertex in &self.vertices {
            out.push(vertex.x);
            out.push(vertex.y);
            out.push(vertex.z);
        }
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
