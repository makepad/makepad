//! The synthetic room: a 5.2 x 2.55 x 5.9 m box room with three pieces of furniture, the scene
//! the TSDF bench integrates and a stand-in field for testing a receiver without a headset.
//!
//! [`spawn_synthetic_room_feed`] plays the part of a headset's brick stream into this process's
//! store (through a [`BrickMirror`], as a desktop that renders for a headset receives them): the
//! room appears in a scan sweep around its centre over the first seconds, then the table slides
//! back and forth, so its bricks are re-sent (new versions, removals) every half second.
use crate::bricks::{encode, Quantized};
use crate::*;

#[derive(Clone, Copy)]
pub(crate) struct SyntheticBox {
    pub(crate) min: Vec3f,
    pub(crate) max: Vec3f,
}

impl SyntheticBox {
    pub(crate) const fn new(min: Vec3f, max: Vec3f) -> Self {
        Self { min, max }
    }

    /// Signed distance from `p` to the box (negative inside).
    fn distance(self, p: Vec3f) -> f32 {
        let c = (self.min + self.max).scale(0.5);
        let h = (self.max - self.min).scale(0.5);
        let q = vec3f((p.x - c.x).abs() - h.x, (p.y - c.y).abs() - h.y, (p.z - c.z).abs() - h.z);
        let outside = vec3f(q.x.max(0.0), q.y.max(0.0), q.z.max(0.0)).length();
        outside + q.x.max(q.y.max(q.z)).min(0.0)
    }

    fn offset(self, d: Vec3f) -> Self {
        Self::new(self.min + d, self.max + d)
    }
}

/// Floor, ceiling, four walls, a table, a cabinet and a chest.
pub(crate) fn synthetic_scene_boxes() -> Vec<SyntheticBox> {
    vec![
        SyntheticBox::new(vec3f(-2.60, -0.10, -5.20), vec3f(2.60, 0.0, 0.70)),
        SyntheticBox::new(vec3f(-2.60, 2.45, -5.20), vec3f(2.60, 2.55, 0.70)),
        SyntheticBox::new(vec3f(-2.60, 0.0, -5.20), vec3f(-2.50, 2.55, 0.70)),
        SyntheticBox::new(vec3f(2.50, 0.0, -5.20), vec3f(2.60, 2.55, 0.70)),
        SyntheticBox::new(vec3f(-2.60, 0.0, -5.20), vec3f(2.60, 2.55, -5.10)),
        SyntheticBox::new(vec3f(-0.45, 0.0, -2.65), vec3f(0.45, 1.15, -1.85)),
        SyntheticBox::new(vec3f(1.15, 0.0, -3.55), vec3f(1.85, 1.65, -2.95)),
        SyntheticBox::new(vec3f(-1.85, 0.0, -4.20), vec3f(-1.25, 0.95, -3.55)),
    ]
}

/// The index of the table in [`synthetic_scene_boxes`] (the box that moves).
const TABLE: usize = 5;

/// The room's field as bricks, for a receiver test (see the module doc).
pub struct SyntheticRoom {
    grid: BrickGrid,
    boxes: Vec<SyntheticBox>,
    /// the content last sent per brick (its version, its voxels)
    sent: HashMap<ChunkKey, (u32, Option<Quantized>)>,
    /// every brick of the static room, the order the sweep reveals them in (azimuth)
    sweep: Vec<(f32, ChunkKey)>,
    revealed: usize,
    table_at: f32,
    /// the table slides (else the room stays still once swept)
    moving: bool,
}

impl SyntheticRoom {
    pub fn new(voxel_size: f32, moving: bool) -> Self {
        let grid = BrickGrid { voxel_size, chunk_edge: 4 };
        let mut room = Self { grid, boxes: synthetic_scene_boxes(), sent: HashMap::new(), sweep: Vec::new(), revealed: 0, table_at: 0.0, moving };
        let chunk_m = voxel_size * grid.chunk_edge as f32;
        let lo = |v: f32| (v / chunk_m).floor() as i32 - 1;
        let hi = |v: f32| (v / chunk_m).ceil() as i32 + 1;
        let centre = vec3f(0.0, 1.2, -2.25);
        for z in lo(-5.20)..hi(0.70) {
            for y in lo(-0.10)..hi(2.55) {
                for x in lo(-2.60)..hi(2.60) {
                    let key = ChunkKey::new(x, y, z);
                    if room.quantize(key, &room.boxes).is_some() {
                        let c = room.chunk_centre(key) - centre;
                        room.sweep.push((c.z.atan2(c.x), key));
                    }
                }
            }
        }
        room.sweep.sort_by(|a, b| a.0.total_cmp(&b.0));
        room
    }

    pub fn grid(&self) -> BrickGrid {
        self.grid
    }

    /// How many bricks the whole room has.
    pub fn brick_count(&self) -> usize {
        self.sweep.len()
    }

    fn chunk_centre(&self, key: ChunkKey) -> Vec3f {
        let chunk_m = self.grid.voxel_size * self.grid.chunk_edge as f32;
        vec3f((key.x as f32 + 0.5) * chunk_m, (key.y as f32 + 0.5) * chunk_m, (key.z as f32 + 0.5) * chunk_m)
    }

    /// The brick's quantized voxels in a scene of `boxes` (None: no voxel within the band).
    fn quantize(&self, key: ChunkKey, boxes: &[SyntheticBox]) -> Option<Quantized> {
        let v = self.grid.voxel_size;
        let edge = self.grid.chunk_edge;
        // the integrator's truncation band: two voxels
        let band = v * 2.0;
        let field = |p: Vec3f| boxes.iter().map(|b| b.distance(p)).fold(f32::INFINITY, f32::min);
        let reach = v * edge as f32 * 0.87 + band;
        if field(self.chunk_centre(key)).abs() > reach {
            return None;
        }
        let mut q = Quantized { valid: 0, distance: [0; 64] };
        for lz in 0..edge {
            for ly in 0..edge {
                for lx in 0..edge {
                    let id = (lx + ly * edge + lz * edge * edge) as usize;
                    let p = vec3f(
                        ((key.x * edge + lx) as f32 + 0.5) * v,
                        ((key.y * edge + ly) as f32 + 0.5) * v,
                        ((key.z * edge + lz) as f32 + 0.5) * v,
                    );
                    let d = field(p) / band;
                    if (-1.0..=1.0).contains(&d) {
                        q.valid |= 1 << id;
                        q.distance[id] = (d * 127.0).round() as i8;
                    }
                }
            }
        }
        (q.valid != 0).then_some(q)
    }

    fn send(&mut self, key: ChunkKey, content: Option<Quantized>, out: &mut Vec<Brick>) {
        let (version, old) = self.sent.get(&key).map_or((0, None), |(v, q)| (*v, q.clone()));
        if version != 0 && old == content {
            return;
        }
        if version == 0 && content.is_none() {
            return;
        }
        let payload = Arc::new(content.as_ref().map(encode).unwrap_or_default());
        self.sent.insert(key, (version + 1, content));
        out.push(Brick { key, version: version + 1, payload });
    }

    /// The bricks that changed by `t` seconds since the feed started.
    pub fn bricks_at(&mut self, t: f32) -> Vec<Brick> {
        const SWEEP_SECONDS: f32 = 6.0;
        let mut out = Vec::new();
        let reveal = ((t / SWEEP_SECONDS).min(1.0) * self.sweep.len() as f32) as usize;
        while self.revealed < reveal {
            let key = self.sweep[self.revealed].1;
            self.revealed += 1;
            let content = self.quantize(key, &self.boxes);
            self.send(key, content, &mut out);
        }
        if self.moving && t > SWEEP_SECONDS {
            // the table slides along x (0.6 m each way, a 12 s period), in 5 cm steps
            let at = ((t - SWEEP_SECONDS) * std::f32::consts::TAU / 12.0).sin() * 0.6;
            let at = (at / 0.05).round() * 0.05;
            if at != self.table_at {
                let base = synthetic_scene_boxes()[TABLE];
                let before = base.offset(vec3f(self.table_at, 0.0, 0.0));
                let after = base.offset(vec3f(at, 0.0, 0.0));
                self.table_at = at;
                self.boxes[TABLE] = after;
                let chunk_m = self.grid.voxel_size * self.grid.chunk_edge as f32;
                let (min, max) = (vec3f(before.min.x.min(after.min.x), before.min.y, before.min.z), vec3f(before.max.x.max(after.max.x), before.max.y, before.max.z));
                let lo = |v: f32| (v / chunk_m).floor() as i32 - 1;
                let hi = |v: f32| (v / chunk_m).ceil() as i32 + 1;
                for z in lo(min.z)..hi(max.z) {
                    for y in lo(min.y)..hi(max.y) {
                        for x in lo(min.x)..hi(max.x) {
                            let key = ChunkKey::new(x, y, z);
                            let content = self.quantize(key, &self.boxes);
                            self.send(key, content, &mut out);
                        }
                    }
                }
            }
        }
        out
    }
}

/// Feeds the synthetic room into `store` as a mirrored field, on its own thread, every 100 ms
/// (see the module doc). For testing a field receiver without a headset. Starts once per
/// process; later calls do nothing. `moving`: the table slides after the sweep.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_synthetic_room_feed(store: XrTsdfStore, moving: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new().name("tsdf-synthetic-room".into()).spawn(move || {
        crate::depth_writer::background_priority();
        let mut room = SyntheticRoom::new(store.voxel_size_meters(), moving);
        let mut mirror = BrickMirror::default();
        let start = Instant::now();
        loop {
            let bricks = room.bricks_at(start.elapsed().as_secs_f32());
            if !bricks.is_empty() {
                mirror.apply(&store, room.grid(), &bricks);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}
