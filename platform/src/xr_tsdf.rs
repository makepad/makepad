use crate::{
    makepad_math::{vec3f, Vec2f, Vec3f},
    makepad_micro_serde::*,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, OnceLock, RwLock,
    },
    time::Instant,
};

pub const XR_TSDF_DEFAULT_VOXEL_SIZE_METERS: f32 = 0.03;

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
pub struct XrTsdfStats {
    pub frames_seen: u64,
    pub frames_meshed: u64,
    pub frames_dropped: u64,
}

#[derive(Clone, Debug, Default)]
pub struct XrTsdfState {
    pub stats: XrTsdfStats,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SparseTsdReadChunk {
    pub values: Vec<i16>,
    pub valid_bits: Vec<u64>,
    pub confidence: Vec<u8>,
    pub observed_generation: Vec<u16>,
}

impl SparseTsdReadChunk {
    const NORMALIZED_DISTANCE_ENCODE_SCALE: f32 = i16::MAX as f32;
    const GENERATION_WRAP: u64 = u16::MAX as u64 + 1;
    const GENERATION_MASK: u64 = u16::MAX as u64;

    pub fn new(chunk_volume: usize) -> Self {
        Self {
            values: vec![0; chunk_volume],
            valid_bits: vec![0; Self::valid_word_count(chunk_volume)],
            confidence: vec![0; chunk_volume],
            observed_generation: vec![0; chunk_volume],
        }
    }

    pub fn valid_word_count(chunk_volume: usize) -> usize {
        (chunk_volume + u64::BITS as usize - 1) / u64::BITS as usize
    }

    pub fn valid_bit_parts(id: usize) -> (usize, u64) {
        let word_index = id / u64::BITS as usize;
        let bit_mask = 1u64 << (id % u64::BITS as usize);
        (word_index, bit_mask)
    }

    pub fn is_valid_index(valid_bits: &[u64], id: usize) -> bool {
        let (word_index, bit_mask) = Self::valid_bit_parts(id);
        valid_bits
            .get(word_index)
            .copied()
            .is_some_and(|word| (word & bit_mask) != 0)
    }

    pub fn set_valid_index(valid_bits: &mut [u64], id: usize) -> bool {
        let (word_index, bit_mask) = Self::valid_bit_parts(id);
        let Some(word) = valid_bits.get_mut(word_index) else {
            return false;
        };
        let was_valid = (*word & bit_mask) != 0;
        *word |= bit_mask;
        was_valid
    }

    pub fn encode_normalized_distance(value: f32) -> i16 {
        (value.clamp(-1.0, 1.0) * Self::NORMALIZED_DISTANCE_ENCODE_SCALE).round() as i16
    }

    pub fn decode_normalized_distance(value: i16) -> f32 {
        value as f32 / Self::NORMALIZED_DISTANCE_ENCODE_SCALE
    }

    pub fn encode_generation_tag(generation: u64) -> u16 {
        generation as u16
    }

    pub fn decode_generation_tag(tag: u16, current_generation: u64) -> u64 {
        let base = current_generation & !Self::GENERATION_MASK;
        let candidate = base | tag as u64;
        if candidate > current_generation {
            candidate.saturating_sub(Self::GENERATION_WRAP)
        } else {
            candidate
        }
    }

    pub fn value(&self, id: usize) -> Option<f32> {
        if !Self::is_valid_index(&self.valid_bits, id) {
            None
        } else {
            self.values
                .get(id)
                .copied()
                .map(Self::decode_normalized_distance)
        }
    }

    pub fn confidence(&self, id: usize) -> u8 {
        if !Self::is_valid_index(&self.valid_bits, id) {
            0
        } else {
            self.confidence.get(id).copied().unwrap_or(0)
        }
    }

    pub fn observed_generation(&self, id: usize, current_generation: u64) -> u64 {
        let tag = self.observed_generation.get(id).copied().unwrap_or(0);
        Self::decode_generation_tag(tag, current_generation)
    }

    pub fn set_value(&mut self, id: usize, value: f32, confidence: u8, observed_generation: u64) {
        if id >= self.values.len()
            || id >= self.confidence.len()
            || id >= self.observed_generation.len()
        {
            return;
        }
        self.values[id] = Self::encode_normalized_distance(value);
        Self::set_valid_index(&mut self.valid_bits, id);
        self.confidence[id] = confidence;
        self.observed_generation[id] = Self::encode_generation_tag(observed_generation);
    }

    pub fn heap_bytes(&self) -> u64 {
        (self.values.capacity() * std::mem::size_of::<i16>()
            + self.valid_bits.capacity() * std::mem::size_of::<u64>()
            + self.confidence.capacity() * std::mem::size_of::<u8>()
            + self.observed_generation.capacity() * std::mem::size_of::<u16>()) as u64
    }
}

#[derive(Clone, Debug, Default)]
pub struct SparseTsdGridReadSnapshot {
    pub voxel_size: f32,
    pub chunk_edge: i32,
    pub chunk_volume: usize,
    pub active_value_count: usize,
    pub active_bounds: Option<(Vec3f, Vec3f)>,
    pub chunks: HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
}

impl SparseTsdGridReadSnapshot {
    pub fn is_empty(&self) -> bool {
        self.active_value_count == 0
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn world_bounds(&self, padding_voxels: i32) -> Option<(Vec3f, Vec3f)> {
        let (min, max) = self.active_bounds?;
        let padding = padding_voxels as f32 * self.voxel_size;
        Some((
            vec3f(min.x - padding, min.y - padding, min.z - padding),
            vec3f(max.x + padding, max.y + padding, max.z + padding),
        ))
    }

    pub fn world_to_voxel_xyz(&self, point: Vec3f) -> (i32, i32, i32) {
        (
            (point.x / self.voxel_size).floor() as i32,
            (point.y / self.voxel_size).floor() as i32,
            (point.z / self.voxel_size).floor() as i32,
        )
    }

    pub fn voxel_center_world_xyz(&self, x: i32, y: i32, z: i32) -> Vec3f {
        vec3f(
            (x as f32 + 0.5) * self.voxel_size,
            (y as f32 + 0.5) * self.voxel_size,
            (z as f32 + 0.5) * self.voxel_size,
        )
    }

    pub fn chunk_key_and_local_index_xyz(&self, x: i32, y: i32, z: i32) -> (ChunkKey, usize) {
        let chunk_x = x.div_euclid(self.chunk_edge);
        let chunk_y = y.div_euclid(self.chunk_edge);
        let chunk_z = z.div_euclid(self.chunk_edge);
        let lx = x.rem_euclid(self.chunk_edge) as usize;
        let ly = y.rem_euclid(self.chunk_edge) as usize;
        let lz = z.rem_euclid(self.chunk_edge) as usize;
        let edge = self.chunk_edge as usize;
        let local_id = lx + ly * edge + lz * edge * edge;
        (ChunkKey::new(chunk_x, chunk_y, chunk_z), local_id)
    }

    pub fn normalized_distance_xyz(&self, x: i32, y: i32, z: i32) -> Option<f32> {
        let (chunk_key, local_id) = self.chunk_key_and_local_index_xyz(x, y, z);
        let chunk = self.chunks.get(&chunk_key)?;
        chunk.value(local_id)
    }

    pub fn confidence_xyz(&self, x: i32, y: i32, z: i32) -> u8 {
        let (chunk_key, local_id) = self.chunk_key_and_local_index_xyz(x, y, z);
        self.chunks
            .get(&chunk_key)
            .map(|chunk| chunk.confidence(local_id))
            .unwrap_or(0)
    }

    pub fn heap_bytes(&self) -> u64 {
        let chunk_bytes = self
            .chunks
            .values()
            .map(|chunk| chunk.heap_bytes())
            .sum::<u64>();
        let table_bytes = self.chunks.capacity() as u64
            * std::mem::size_of::<(ChunkKey, Arc<SparseTsdReadChunk>)>() as u64;
        chunk_bytes + table_bytes
    }
}

#[derive(Clone, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrDepthAlignHeightMap {
    pub origin_x: f32,
    pub origin_z: f32,
    pub cell_size_meters: f32,
    pub size_x: u16,
    pub size_z: u16,
    pub bottom_y_meters: f32,
    pub top_y_meters: f32,
    pub floor_y_meters: f32,
    pub player_cutout_center: Option<Vec2f>,
    pub player_cutout_radius_meters: f32,
    pub heights_meters: Vec<f32>,
}

impl XrDepthAlignHeightMap {
    pub fn cell_count(&self) -> usize {
        self.size_x as usize * self.size_z as usize
    }

    pub fn size_x_usize(&self) -> usize {
        self.size_x as usize
    }

    pub fn size_z_usize(&self) -> usize {
        self.size_z as usize
    }

    pub fn extent_x_meters(&self) -> f32 {
        self.cell_size_meters * self.size_x.max(1) as f32
    }

    pub fn extent_z_meters(&self) -> f32 {
        self.cell_size_meters * self.size_z.max(1) as f32
    }

    pub fn cell_index(&self, x: usize, z: usize) -> usize {
        x + z * self.size_x_usize()
    }

    pub fn is_empty(&self) -> bool {
        self.size_x == 0
            || self.size_z == 0
            || self.heights_meters.len() != self.cell_count()
            || self.heights_meters.iter().all(|value| !value.is_finite())
    }
}

#[derive(Clone, Debug, Default)]
pub struct TsdfPublishedSnapshot {
    pub generation: u64,
    pub latest_topology_generation: u64,
    pub update_sequence: u64,
    pub grid: Arc<SparseTsdGridReadSnapshot>,
    pub height_map: Option<XrDepthAlignHeightMap>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XrTsdfCooperativeStepResult {
    pub did_work: bool,
    pub has_more_work: bool,
    pub completed_cycle: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XrTsdfCooperativeStepStats {
    pub callback_registered: bool,
    pub has_more_work: bool,
    pub last_step_micros: u64,
    pub average_step_micros: u64,
    pub last_cycle_compute_micros: u64,
    pub average_cycle_compute_micros: u64,
    pub last_cycle_micros: u64,
    pub average_cycle_micros: u64,
    pub current_cycle_steps: u32,
    pub completed_cycles: u64,
}

type XrTsdfCooperativeStepCallback =
    Box<dyn FnMut() -> XrTsdfCooperativeStepResult + Send + 'static>;

#[derive(Default)]
struct XrTsdfCooperativeStepSlot {
    callback: Option<XrTsdfCooperativeStepCallback>,
    stats: XrTsdfCooperativeStepStats,
    current_cycle_compute_micros: u64,
    current_cycle_started_at: Option<Instant>,
}

impl XrTsdfCooperativeStepSlot {
    fn reset_cycle(&mut self) {
        self.current_cycle_compute_micros = 0;
        self.current_cycle_started_at = None;
        self.stats.current_cycle_steps = 0;
        self.stats.has_more_work = false;
    }
}

#[derive(Clone)]
pub struct XrTsdfStore {
    state: Arc<RwLock<XrTsdfState>>,
    published_snapshot: Arc<RwLock<Option<Arc<TsdfPublishedSnapshot>>>>,
    reset_generation: Arc<AtomicU64>,
    surface_analysis_enabled: Arc<AtomicBool>,
    voxel_size_meters_bits: Arc<AtomicU32>,
    cooperative_step: Arc<Mutex<XrTsdfCooperativeStepSlot>>,
}

impl Default for XrTsdfStore {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(XrTsdfState::default())),
            published_snapshot: Arc::new(RwLock::new(None)),
            reset_generation: Arc::new(AtomicU64::new(0)),
            surface_analysis_enabled: Arc::new(AtomicBool::new(false)),
            voxel_size_meters_bits: Arc::new(AtomicU32::new(
                XR_TSDF_DEFAULT_VOXEL_SIZE_METERS.to_bits(),
            )),
            cooperative_step: Arc::new(Mutex::new(XrTsdfCooperativeStepSlot::default())),
        }
    }
}

#[allow(dead_code)]
impl XrTsdfStore {
    pub fn set_cooperative_step_callback(
        &self,
        callback: Option<XrTsdfCooperativeStepCallback>,
    ) {
        if let Ok(mut slot) = self.cooperative_step.lock() {
            slot.callback = callback;
            slot.stats.callback_registered = slot.callback.is_some();
            if slot.callback.is_none() {
                slot.reset_cycle();
                slot.stats.last_step_micros = 0;
                slot.stats.average_step_micros = 0;
                slot.stats.last_cycle_compute_micros = 0;
                slot.stats.average_cycle_compute_micros = 0;
                slot.stats.last_cycle_micros = 0;
                slot.stats.average_cycle_micros = 0;
                slot.stats.completed_cycles = 0;
            }
        }
    }

    pub fn cooperative_step_stats(&self) -> XrTsdfCooperativeStepStats {
        self.cooperative_step
            .lock()
            .map(|slot| slot.stats)
            .unwrap_or_default()
    }

    pub fn set_voxel_size_meters(&self, voxel_size_meters: f32) -> f32 {
        let voxel_size_meters = voxel_size_meters.clamp(0.03, 0.10);
        let previous = self
            .voxel_size_meters_bits
            .swap(voxel_size_meters.to_bits(), Ordering::AcqRel);
        if previous != voxel_size_meters.to_bits() {
            self.clear();
        }
        voxel_size_meters
    }

    pub fn voxel_size_meters(&self) -> f32 {
        f32::from_bits(self.voxel_size_meters_bits.load(Ordering::Acquire))
    }

    pub fn reset_generation(&self) -> u64 {
        self.reset_generation.load(Ordering::Acquire)
    }

    pub fn request_reset(&self) -> u64 {
        let generation = self.reset_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.clear();
        generation
    }

    pub fn set_surface_analysis_enabled(&self, enabled: bool) {
        self.surface_analysis_enabled
            .store(enabled, Ordering::Release);
    }

    pub fn surface_analysis_enabled(&self) -> bool {
        self.surface_analysis_enabled.load(Ordering::Acquire)
    }

    pub fn state(&self) -> Arc<RwLock<XrTsdfState>> {
        self.state.clone()
    }

    pub fn latest_tsdf_snapshot(&self) -> Option<Arc<TsdfPublishedSnapshot>> {
        self.published_snapshot
            .read()
            .ok()
            .and_then(|snapshot| snapshot.clone())
    }

    pub(crate) fn record_seen(&self) {
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_seen += 1;
        }
    }

    pub(crate) fn record_drop(&self) {
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_dropped += 1;
        }
    }

    pub(crate) fn set_error(&self, error: String) {
        if let Ok(mut state) = self.state.write() {
            state.last_error = Some(error);
        }
    }

    pub(crate) fn publish_tsdf_snapshot(&self, snapshot: TsdfPublishedSnapshot) {
        if let Ok(mut published_snapshot) = self.published_snapshot.write() {
            *published_snapshot = Some(Arc::new(snapshot));
        }
        if let Ok(mut state) = self.state.write() {
            state.stats.frames_meshed += 1;
            state.last_error = None;
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut state) = self.state.write() {
            state.last_error = None;
            state.stats = XrTsdfStats::default();
        }
        if let Ok(mut published_snapshot) = self.published_snapshot.write() {
            *published_snapshot = None;
        }
        if let Ok(mut slot) = self.cooperative_step.lock() {
            slot.reset_cycle();
        }
    }

    pub(crate) fn run_cooperative_step(&self) -> Option<XrTsdfCooperativeStepResult> {
        let mut slot = self.cooperative_step.lock().ok()?;
        let callback = slot.callback.as_mut()?;
        let started = Instant::now();
        let result = callback();
        let elapsed_micros = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        slot.stats.callback_registered = true;
        slot.stats.last_step_micros = elapsed_micros;
        slot.stats.average_step_micros =
            ema_u64(slot.stats.average_step_micros, elapsed_micros, 1, 4);
        if result.did_work {
            slot.current_cycle_started_at.get_or_insert(started);
            slot.current_cycle_compute_micros =
                slot.current_cycle_compute_micros.saturating_add(elapsed_micros);
            slot.stats.current_cycle_steps = slot.stats.current_cycle_steps.saturating_add(1);
        } else if !result.has_more_work {
            slot.reset_cycle();
        }
        slot.stats.has_more_work = result.has_more_work;
        if result.completed_cycle {
            let cycle_wall_micros = slot
                .current_cycle_started_at
                .map(|cycle_started| {
                    cycle_started
                        .elapsed()
                        .as_micros()
                        .min(u64::MAX as u128) as u64
                })
                .unwrap_or(slot.current_cycle_compute_micros);
            slot.stats.last_cycle_compute_micros = slot.current_cycle_compute_micros;
            slot.stats.average_cycle_compute_micros = ema_u64(
                slot.stats.average_cycle_compute_micros,
                slot.current_cycle_compute_micros,
                1,
                4,
            );
            slot.stats.last_cycle_micros = cycle_wall_micros;
            slot.stats.average_cycle_micros =
                ema_u64(slot.stats.average_cycle_micros, cycle_wall_micros, 1, 4);
            slot.stats.completed_cycles = slot.stats.completed_cycles.saturating_add(1);
            slot.reset_cycle();
        }
        Some(result)
    }
}

fn ema_u64(current: u64, sample: u64, numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return sample;
    }
    if current == 0 {
        sample
    } else {
        current
            .saturating_mul(denominator.saturating_sub(numerator))
            .saturating_add(sample.saturating_mul(numerator))
            / denominator
    }
}

pub fn xr_tsdf_store() -> XrTsdfStore {
    static STORE: OnceLock<XrTsdfStore> = OnceLock::new();
    STORE.get_or_init(XrTsdfStore::default).clone()
}

#[allow(dead_code)]
pub(crate) fn empty_bounds() -> (Vec3f, Vec3f) {
    (vec3f(0.0, 0.0, 0.0), vec3f(0.0, 0.0, 0.0))
}
