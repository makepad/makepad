use crate::makepad_math::{vec3, Pose, Vec3f};
use makepad_sparse_voxels::SparseChunkStorage;
use parry3d::{
    math::{IVector, Vector},
    shape::{SharedShape, Voxels, VoxelsChunk},
};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
    time::Instant,
};

#[derive(Clone, Debug, Default)]
pub struct XrDepthVoxelsStats {
    pub frames_seen: u64,
    pub frames_integrated: u64,
    pub frames_dropped: u64,
    pub latest_generation: u64,
    pub active_voxels: usize,
    pub active_chunks: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct XrDepthPhysicsBox {
    pub pose: Pose,
    pub half_extents: Vec3f,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct XrDepthPhysicsChunkKey {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Clone, Debug)]
pub struct XrDepthPhysicsChunk {
    pub key: XrDepthPhysicsChunkKey,
    pub boxes: Vec<XrDepthPhysicsBox>,
    pub shape: SharedShape,
}

#[derive(Clone, Debug)]
pub struct XrDepthEvidenceChunk {
    pub evidence: [i16; VoxelsChunk::VOXELS_PER_CHUNK],
}

impl Default for XrDepthEvidenceChunk {
    fn default() -> Self {
        Self {
            evidence: [0; VoxelsChunk::VOXELS_PER_CHUNK],
        }
    }
}

pub type XrDepthEvidenceStorage = SparseChunkStorage<IVector, XrDepthEvidenceChunk>;

#[derive(Clone, Debug)]
pub struct XrDepthSurfaceChunk {
    pub point_sum: [Vec3f; VoxelsChunk::VOXELS_PER_CHUNK],
    pub normal_sum: [Vec3f; VoxelsChunk::VOXELS_PER_CHUNK],
    pub sample_count: [u16; VoxelsChunk::VOXELS_PER_CHUNK],
}

impl Default for XrDepthSurfaceChunk {
    fn default() -> Self {
        Self {
            point_sum: [Vec3f::default(); VoxelsChunk::VOXELS_PER_CHUNK],
            normal_sum: [Vec3f::default(); VoxelsChunk::VOXELS_PER_CHUNK],
            sample_count: [0; VoxelsChunk::VOXELS_PER_CHUNK],
        }
    }
}

pub type XrDepthSurfaceStorage = SparseChunkStorage<IVector, XrDepthSurfaceChunk>;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct XrDepthPlaneKey {
    pub family: u8,
    pub orientation_bin: i16,
    pub distance_bin: i16,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthPlaneCell {
    pub support: u16,
    pub last_seen_generation: u64,
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthPlaneAccumulator {
    pub total_support: u32,
    pub last_seen_generation: u64,
    pub cells: HashMap<(i16, i16), XrDepthPlaneCell>,
}

#[derive(Debug)]
pub struct XrDepthVoxels {
    pub generation: u64,
    pub physics_generation: u64,
    pub latest_topology_generation: u64,
    pub eye_index: usize,
    pub image_width: u32,
    pub image_height: u32,
    pub sample_step: u32,
    pub voxel_size_meters: f32,
    pub activation_threshold: i16,
    pub removal_threshold: i16,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub active_voxel_count: usize,
    pub physics_box_count: usize,
    pub physics_chunk_count: usize,
    pub physics_chunks: Vec<XrDepthPhysicsChunk>,
    pub voxels: Voxels,
    pub evidence: XrDepthEvidenceStorage,
    pub surfaces: XrDepthSurfaceStorage,
    pub planes: HashMap<XrDepthPlaneKey, XrDepthPlaneAccumulator>,
    pub pending_physics_changes: usize,
    pub last_physics_rebuild_at: Instant,
}

impl XrDepthVoxels {
    pub fn new(sample_step: u32, voxel_size_meters: f32) -> Self {
        let empty_voxels = Voxels::new(Vector::splat(voxel_size_meters), &[]);
        Self {
            generation: 0,
            physics_generation: 0,
            latest_topology_generation: 0,
            eye_index: 0,
            image_width: 0,
            image_height: 0,
            sample_step,
            voxel_size_meters,
            activation_threshold: 4,
            removal_threshold: -2,
            bounds_min: vec3(0.0, 0.0, 0.0),
            bounds_max: vec3(0.0, 0.0, 0.0),
            active_voxel_count: 0,
            physics_box_count: 0,
            physics_chunk_count: 0,
            physics_chunks: Vec::new(),
            voxels: empty_voxels,
            evidence: XrDepthEvidenceStorage::new(VoxelsChunk::INVALID_CHUNK_KEY),
            surfaces: XrDepthSurfaceStorage::new(VoxelsChunk::INVALID_CHUNK_KEY),
            planes: HashMap::new(),
            pending_physics_changes: 0,
            last_physics_rebuild_at: Instant::now(),
        }
    }

    pub fn active_chunk_count(&self) -> usize {
        self.voxels.storage().chunk_headers.len()
    }

    pub fn update_bounds(&mut self) {
        if self.active_voxel_count == 0 {
            self.bounds_min = vec3(0.0, 0.0, 0.0);
            self.bounds_max = vec3(0.0, 0.0, 0.0);
            return;
        }
        let aabb = self.voxels.local_aabb();
        self.bounds_min = vec3(aabb.mins.x, aabb.mins.y, aabb.mins.z);
        self.bounds_max = vec3(aabb.maxs.x, aabb.maxs.y, aabb.maxs.z);
    }
}

#[derive(Clone, Debug, Default)]
pub struct XrDepthVoxelsState {
    pub latest_voxels: Option<Arc<RwLock<XrDepthVoxels>>>,
    pub stats: XrDepthVoxelsStats,
    pub last_error: Option<String>,
}

#[derive(Clone, Default)]
pub struct XrDepthVoxelsStore(Arc<RwLock<XrDepthVoxelsState>>);

impl XrDepthVoxelsStore {
    pub fn state(&self) -> Arc<RwLock<XrDepthVoxelsState>> {
        self.0.clone()
    }

    pub fn latest_voxels(&self) -> Option<Arc<RwLock<XrDepthVoxels>>> {
        self.0
            .read()
            .ok()
            .and_then(|state| state.latest_voxels.clone())
    }

    #[allow(dead_code)]
    pub(crate) fn ensure_volume(
        &self,
        make_volume: impl FnOnce() -> XrDepthVoxels,
    ) -> Arc<RwLock<XrDepthVoxels>> {
        if let Ok(mut state) = self.0.write() {
            if let Some(volume) = &state.latest_voxels {
                return volume.clone();
            }
            let volume = Arc::new(RwLock::new(make_volume()));
            state.latest_voxels = Some(volume.clone());
            return volume;
        }
        Arc::new(RwLock::new(make_volume()))
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
    pub(crate) fn record_integrated(&self, volume: &XrDepthVoxels) {
        if let Ok(mut state) = self.0.write() {
            state.stats.frames_integrated += 1;
            state.stats.latest_generation = volume.generation;
            state.stats.active_voxels = volume.active_voxel_count;
            state.stats.active_chunks = volume.active_chunk_count();
            state.last_error = None;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_error(&self, error: String) {
        if let Ok(mut state) = self.0.write() {
            state.last_error = Some(error);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.0.write() {
            state.latest_voxels = None;
            state.last_error = None;
            state.stats = XrDepthVoxelsStats::default();
        }
    }
}

pub fn xr_depth_voxels_store() -> XrDepthVoxelsStore {
    static STORE: OnceLock<XrDepthVoxelsStore> = OnceLock::new();
    STORE.get_or_init(XrDepthVoxelsStore::default).clone()
}
