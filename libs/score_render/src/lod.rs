use crate::{PageId, PaletteId, Point};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Raster resolution in 1/256 physical pixel per staff space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RasterLevel(pub u16);

impl RasterLevel {
    pub const HALF: Self = Self(128);
    pub const ONE: Self = Self(256);
    pub const TWO: Self = Self(512);
    pub const FOUR: Self = Self(1024);

    pub fn px_per_sp(self) -> f64 {
        self.0 as f64 / 256.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LodMode {
    Raster {
        level: RasterLevel,
    },
    CrossFade {
        level: RasterLevel,
        raster_alpha: f32,
        vector_alpha: f32,
    },
    Vector,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodPolicy {
    /// At and below this scale, use cached tiles once resident.
    pub raster_max_px_per_sp: f64,
    /// At and above this scale, use live vectors exclusively.
    pub vector_min_px_per_sp: f64,
    pub tile_size_px: u16,
    pub tile_gutter_px: u8,
}

impl LodPolicy {
    pub fn choose(&self, px_per_sp: f64) -> LodMode {
        let level = raster_level_for(px_per_sp);
        if px_per_sp <= self.raster_max_px_per_sp {
            return LodMode::Raster { level };
        }
        if px_per_sp >= self.vector_min_px_per_sp {
            return LodMode::Vector;
        }
        let t = ((px_per_sp - self.raster_max_px_per_sp)
            / (self.vector_min_px_per_sp - self.raster_max_px_per_sp))
            .clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        LodMode::CrossFade {
            level,
            raster_alpha: (1.0 - smooth) as f32,
            vector_alpha: smooth as f32,
        }
    }

    pub fn tiles_for_page(
        &self,
        page: PageId,
        revision: u64,
        palette: PaletteId,
        level: RasterLevel,
        page_size_sp: Point,
    ) -> Vec<TileKey> {
        let content = (self.tile_size_px as u32)
            .saturating_sub(self.tile_gutter_px as u32 * 2)
            .max(1);
        let width = (page_size_sp.x * level.px_per_sp()).ceil().max(1.0) as u32;
        let height = (page_size_sp.y * level.px_per_sp()).ceil().max(1.0) as u32;
        let columns = width.div_ceil(content);
        let rows = height.div_ceil(content);
        let mut keys = Vec::with_capacity((columns * rows) as usize);
        for y in 0..rows {
            for x in 0..columns {
                keys.push(TileKey {
                    page,
                    revision,
                    palette,
                    level,
                    x: x as u16,
                    y: y as u16,
                });
            }
        }
        keys
    }
}

impl Default for LodPolicy {
    fn default() -> Self {
        Self {
            raster_max_px_per_sp: 2.0,
            vector_min_px_per_sp: 3.0,
            tile_size_px: 512,
            tile_gutter_px: 2,
        }
    }
}

fn raster_level_for(px_per_sp: f64) -> RasterLevel {
    if px_per_sp <= 0.5 {
        RasterLevel::HALF
    } else if px_per_sp <= 1.0 {
        RasterLevel::ONE
    } else if px_per_sp <= 2.0 {
        RasterLevel::TWO
    } else {
        RasterLevel::FOUR
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageLodPlan {
    pub page: PageId,
    pub level: Option<RasterLevel>,
    pub raster_alpha: f32,
    pub vector_alpha: f32,
    pub tiles: Vec<TileKey>,
    pub missing_tiles: Vec<TileKey>,
}

impl PageLodPlan {
    pub fn uses_live_vectors(&self) -> bool {
        self.vector_alpha > 0.0
    }

    pub fn uses_raster_tiles(&self) -> bool {
        self.raster_alpha > 0.0 && self.missing_tiles.is_empty()
    }
}

/// Resolves the policy against residency. A missing tile always promotes the
/// exact vector page to alpha 1 for that frame; it can cost more, but can never
/// make the score incomplete or stale.
#[allow(clippy::too_many_arguments)]
pub fn plan_page_lod<T>(
    policy: LodPolicy,
    cache: &TileCache<T>,
    page: PageId,
    revision: u64,
    palette: PaletteId,
    page_size_sp: Point,
    px_per_sp: f64,
) -> PageLodPlan {
    match policy.choose(px_per_sp) {
        LodMode::Vector => PageLodPlan {
            page,
            level: None,
            raster_alpha: 0.0,
            vector_alpha: 1.0,
            tiles: Vec::new(),
            missing_tiles: Vec::new(),
        },
        LodMode::Raster { level } => {
            resident_lod_plan(policy, cache, page, revision, palette, page_size_sp, level, 1.0, 0.0)
        }
        LodMode::CrossFade {
            level,
            raster_alpha,
            vector_alpha,
        } => resident_lod_plan(
            policy,
            cache,
            page,
            revision,
            palette,
            page_size_sp,
            level,
            raster_alpha,
            vector_alpha,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn resident_lod_plan<T>(
    policy: LodPolicy,
    cache: &TileCache<T>,
    page: PageId,
    revision: u64,
    palette: PaletteId,
    page_size_sp: Point,
    level: RasterLevel,
    raster_alpha: f32,
    vector_alpha: f32,
) -> PageLodPlan {
    let tiles = policy.tiles_for_page(page, revision, palette, level, page_size_sp);
    let missing_tiles: Vec<_> = tiles
        .iter()
        .filter(|key| !cache.contains(**key))
        .copied()
        .collect();
    if missing_tiles.is_empty() {
        PageLodPlan {
            page,
            level: Some(level),
            raster_alpha,
            vector_alpha,
            tiles,
            missing_tiles,
        }
    } else {
        PageLodPlan {
            page,
            level: Some(level),
            raster_alpha: 0.0,
            vector_alpha: 1.0,
            tiles,
            missing_tiles,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileKey {
    pub page: PageId,
    pub revision: u64,
    pub palette: PaletteId,
    pub level: RasterLevel,
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Debug)]
struct TileEntry<T> {
    payload: T,
    bytes: usize,
    last_used_frame: u64,
}

/// Deterministic, byte-bounded LRU for raster-tile resources.
///
/// `T` is a backend handle (GPU texture-array slot or headless bitmap). Evicted
/// handles are returned to the caller for explicit backend destruction.
#[derive(Clone, Debug)]
pub struct TileCache<T> {
    entries: BTreeMap<TileKey, TileEntry<T>>,
    max_bytes: usize,
    resident_bytes: usize,
}

impl<T> TileCache<T> {
    pub const DEFAULT_MAX_BYTES: usize = 128 * 1024 * 1024;

    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_bytes,
            resident_bytes: 0,
        }
    }

    pub fn get(&mut self, key: TileKey, frame: u64) -> Option<&T> {
        let entry = self.entries.get_mut(&key)?;
        entry.last_used_frame = frame;
        Some(&entry.payload)
    }

    pub fn contains(&self, key: TileKey) -> bool {
        self.entries.contains_key(&key)
    }

    pub fn insert(
        &mut self,
        key: TileKey,
        payload: T,
        bytes: usize,
        frame: u64,
    ) -> Vec<(TileKey, T)> {
        let mut evicted = Vec::new();
        if bytes > self.max_bytes {
            evicted.push((key, payload));
            return evicted;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.resident_bytes = self.resident_bytes.saturating_sub(old.bytes);
            evicted.push((key, old.payload));
        }
        self.resident_bytes += bytes;
        self.entries.insert(
            key,
            TileEntry {
                payload,
                bytes,
                last_used_frame: frame,
            },
        );
        while self.resident_bytes > self.max_bytes {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(key, entry)| (entry.last_used_frame, **key))
                .map(|(key, _)| *key)
                .expect("a tile was just inserted");
            let entry = self.entries.remove(&victim).expect("victim exists");
            self.resident_bytes -= entry.bytes;
            evicted.push((victim, entry.payload));
        }
        evicted
    }

    pub fn invalidate_page(&mut self, page: PageId) -> Vec<(TileKey, T)> {
        let keys: Vec<_> = self
            .entries
            .keys()
            .filter(|key| key.page == page)
            .copied()
            .collect();
        let mut removed = Vec::with_capacity(keys.len());
        for key in keys {
            let entry = self.entries.remove(&key).expect("key was collected");
            self.resident_bytes -= entry.bytes;
            removed.push((key, entry.payload));
        }
        removed
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

impl<T> Default for TileCache<T> {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_BYTES)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkKind {
    UploadInstances { bytes: u32 },
    RasterTile { pixels: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkKey {
    Upload { page: PageId, revision: u64, chunk: u32 },
    Tile(TileKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeferredWork {
    pub key: WorkKey,
    pub kind: WorkKind,
    /// 0 visible, 1 adjacent, 2 background.
    pub priority: u8,
    pub distance: u16,
    pub estimated_cpu_us: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameBudget {
    pub upload_bytes: u32,
    pub raster_pixels: u32,
    pub cpu_us: u32,
}

impl FrameBudget {
    /// Leaves over 6 ms of an 8.33 ms frame to drawing/input on a 120 Hz display.
    pub const TARGET_120HZ: Self = Self {
        upload_bytes: 2 * 1024 * 1024,
        raster_pixels: 512 * 512,
        cpu_us: 1_500,
    };
}

#[derive(Clone, Debug, Default)]
pub struct WorkQueue {
    pending: VecDeque<DeferredWork>,
    keys: BTreeSet<WorkKey>,
}

impl WorkQueue {
    pub const MAX_UPLOAD_CHUNK_BYTES: u32 = 512 * 1024;

    pub fn enqueue(&mut self, work: DeferredWork) {
        if !self.keys.insert(work.key) {
            return;
        }
        self.pending.push_back(work);
        let slice = self.pending.make_contiguous();
        slice.sort_by_key(|work| (work.priority, work.distance, work.key));
    }

    /// Splits a page-instance upload into bounded chunks before queueing it.
    pub fn enqueue_upload(
        &mut self,
        page: PageId,
        revision: u64,
        total_bytes: u32,
        priority: u8,
        distance: u16,
    ) {
        let mut offset = 0u32;
        let mut chunk = 0u32;
        while offset < total_bytes {
            let bytes = (total_bytes - offset).min(Self::MAX_UPLOAD_CHUNK_BYTES);
            self.enqueue(DeferredWork {
                key: WorkKey::Upload {
                    page,
                    revision,
                    chunk,
                },
                kind: WorkKind::UploadInstances { bytes },
                priority,
                distance,
                estimated_cpu_us: 80 + bytes / 8_192,
            });
            offset += bytes;
            chunk += 1;
        }
    }

    /// Queues only absent page tiles; each job is independently frame-budgeted.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_missing_tiles<T>(
        &mut self,
        policy: LodPolicy,
        cache: &TileCache<T>,
        page: PageId,
        revision: u64,
        palette: PaletteId,
        level: RasterLevel,
        page_size_sp: Point,
        priority: u8,
        distance: u16,
    ) -> usize {
        let mut queued = 0;
        let pixels = policy.tile_size_px as u32 * policy.tile_size_px as u32;
        for key in policy.tiles_for_page(page, revision, palette, level, page_size_sp) {
            if cache.contains(key) {
                continue;
            }
            let before = self.len();
            self.enqueue(DeferredWork {
                key: WorkKey::Tile(key),
                kind: WorkKind::RasterTile { pixels },
                priority,
                distance,
                estimated_cpu_us: 900,
            });
            queued += usize::from(self.len() > before);
        }
        queued
    }

    pub fn take_frame(&mut self, budget: FrameBudget) -> Vec<DeferredWork> {
        let mut upload = 0u32;
        let mut pixels = 0u32;
        let mut cpu = 0u32;
        let mut selected = Vec::new();
        let mut deferred = VecDeque::new();
        while let Some(work) = self.pending.pop_front() {
            let (next_upload, next_pixels) = match work.kind {
                WorkKind::UploadInstances { bytes } => (upload.saturating_add(bytes), pixels),
                WorkKind::RasterTile { pixels: count } => (upload, pixels.saturating_add(count)),
            };
            let next_cpu = cpu.saturating_add(work.estimated_cpu_us);
            if next_upload <= budget.upload_bytes
                && next_pixels <= budget.raster_pixels
                && next_cpu <= budget.cpu_us
            {
                upload = next_upload;
                pixels = next_pixels;
                cpu = next_cpu;
                self.keys.remove(&work.key);
                selected.push(work);
            } else {
                deferred.push_back(work);
            }
        }
        self.pending = deferred;
        selected
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Retains the previous tile level during a 90 ms resolution change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileLevelTransition {
    pub from: RasterLevel,
    pub to: RasterLevel,
    pub started_at_s: f64,
}

impl TileLevelTransition {
    pub const DURATION_S: f64 = 0.090;

    pub fn weights(self, now_s: f64) -> (f32, f32) {
        let t = ((now_s - self.started_at_s) / Self::DURATION_S).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        ((1.0 - smooth) as f32, smooth as f32)
    }

    pub fn finished(self, now_s: f64) -> bool {
        now_s - self.started_at_s >= Self::DURATION_S
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(page: u32, x: u16) -> TileKey {
        TileKey {
            page: PageId(page),
            revision: 1,
            palette: PaletteId::Light,
            level: RasterLevel::ONE,
            x,
            y: 0,
        }
    }

    #[test]
    fn lod_thresholds_crossfade_without_a_jump() {
        let policy = LodPolicy::default();
        assert_eq!(policy.choose(2.0), LodMode::Raster { level: RasterLevel::TWO });
        assert_eq!(policy.choose(3.0), LodMode::Vector);
        let LodMode::CrossFade {
            raster_alpha,
            vector_alpha,
            ..
        } = policy.choose(2.5)
        else {
            panic!("expected crossfade");
        };
        assert!((raster_alpha - 0.5).abs() < 1e-6);
        assert!((vector_alpha - 0.5).abs() < 1e-6);
        assert!((raster_alpha + vector_alpha - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tile_cache_never_exceeds_its_byte_budget() {
        let mut cache = TileCache::new(1024);
        for i in 0..10 {
            cache.insert(key(0, i), i, 300, i as u64);
            assert!(cache.resident_bytes() <= cache.max_bytes());
        }
        assert_eq!(cache.len(), 3);
        assert!(!cache.contains(key(0, 0)));
        assert!(cache.contains(key(0, 9)));
    }

    #[test]
    fn scheduler_enforces_every_frame_budget() {
        let mut queue = WorkQueue::default();
        for i in 0..8 {
            queue.enqueue(DeferredWork {
                key: WorkKey::Upload {
                    page: PageId(0),
                    revision: 1,
                    chunk: i,
                },
                kind: WorkKind::UploadInstances { bytes: 400 },
                priority: 0,
                distance: i as u16,
                estimated_cpu_us: 200,
            });
        }
        let jobs = queue.take_frame(FrameBudget {
            upload_bytes: 1_000,
            raster_pixels: 0,
            cpu_us: 500,
        });
        assert_eq!(jobs.len(), 2);
        assert_eq!(queue.len(), 6);
    }

    #[test]
    fn uploads_are_split_and_only_missing_tiles_are_scheduled() {
        let mut queue = WorkQueue::default();
        queue.enqueue_upload(PageId(3), 8, 1_300_000, 0, 0);
        assert_eq!(queue.len(), 3);

        let policy = LodPolicy::default();
        let mut cache = TileCache::new(1_000_000);
        let keys = policy.tiles_for_page(
            PageId(1),
            2,
            PaletteId::Light,
            RasterLevel::ONE,
            Point::new(180.0, 260.0),
        );
        cache.insert(keys[0], (), 100, 0);
        let queued = queue.enqueue_missing_tiles(
            policy,
            &cache,
            PageId(1),
            2,
            PaletteId::Light,
            RasterLevel::ONE,
            Point::new(180.0, 260.0),
            0,
            0,
        );
        assert_eq!(queued, keys.len() - 1);
    }

    #[test]
    fn absent_lod_tiles_fall_back_to_complete_vectors() {
        let policy = LodPolicy::default();
        let mut cache = TileCache::new(1_000_000);
        let missing = plan_page_lod(
            policy,
            &cache,
            PageId(2),
            9,
            PaletteId::Dark,
            Point::new(180.0, 260.0),
            1.0,
        );
        assert!(missing.uses_live_vectors());
        assert!(!missing.uses_raster_tiles());
        assert!(!missing.missing_tiles.is_empty());
        for (index, key) in missing.tiles.iter().copied().enumerate() {
            cache.insert(key, index, 100, 0);
        }
        let resident = plan_page_lod(
            policy,
            &cache,
            PageId(2),
            9,
            PaletteId::Dark,
            Point::new(180.0, 260.0),
            1.0,
        );
        assert!(!resident.uses_live_vectors());
        assert!(resident.uses_raster_tiles());
    }
}
