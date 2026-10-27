//! The field as versioned bricks: what a headset sends its peers (other headsets, the desktop
//! that renders for it) so they can run the same hit-surface queries against the room.
//!
//! A brick is one TSDF chunk (`ChunkKey`, `chunk_edge`^3 voxels, 4^3 at 2 cm by default) of
//! *stable near-surface* voxels: valid, observed with at least [`BRICK_MIN_CONFIDENCE`], and
//! within the truncation band. Its payload is the valid-voxel bit set and one signed byte of
//! normalized distance per valid voxel (the i16 the TSDF keeps, rounded to 8 bits). Each brick
//! has a version that grows by one every time it is sent again; a brick that lost its stable
//! voxels is sent once as a removal (empty payload).
//!
//! The producer runs on the depth writer's thread after each published snapshot: chunks whose
//! storage changed are marked dirty, and a dirty chunk is re-sent only when its quantized
//! content moved as a whole (the RMS change of its voxels' distances at least
//! [`BRICK_RESEND_MIN_DELTA`] with [`BRICK_RESEND_MIN_VOXELS`] of them beyond it, or its voxel
//! set changed by [`BRICK_RESEND_MIN_SET_CHANGE`]), and not more often than every
//! [`BRICK_DEBOUNCE`] (the integrator touches the same chunks every frame; depth noise moves
//! single voxels of nearly every surface brick, and resending on any one voxel sent the whole
//! room again every debounce: 300-550 KB/s at rest). The newest version of every brick sits in the
//! store's brick log; a sender asks for what changed since its cursor
//! ([`XrTsdfStore::brick_changes_since`]), so a late joiner gets everything with cursor 0.
//!
//! A receiver installs bricks into its own store with a [`BrickMirror`], which publishes them
//! as a TSDF snapshot: the field queries cannot tell a mirrored field from an integrated one.
use crate::*;
use std::time::{Duration, Instant};

/// Least confidence of a voxel that is sent: the integrator's own "stable" (DEPTH_TSD_STABLE_CONFIDENCE).
pub const BRICK_MIN_CONFIDENCE: u8 = 8;
/// A voxel that went out stays in its brick down to this confidence (hysteresis: a voxel at
/// the stable threshold must not flicker in and out).
pub const BRICK_KEEP_CONFIDENCE: u8 = 3;
/// A brick whose voxel set changed by fewer voxels than this (and whose distances did not
/// move) is not sent again: growth goes out in steps, not one voxel at a time.
pub const BRICK_RESEND_MIN_SET_CHANGE: u32 = 8;
/// Least RMS change of a brick's quantized distances (of 127) that makes it go out again...
pub const BRICK_RESEND_MIN_DELTA: f32 = 6.0;
/// ... with at least this many of its voxels moved by more than that.
pub const BRICK_RESEND_MIN_VOXELS: u32 = 4;
/// A brick sent before is sent again at most this often (a new one goes out at once).
pub const BRICK_DEBOUNCE: Duration = Duration::from_secs(2);

/// One brick as it travels: `payload` empty = the brick is gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Brick {
    pub key: ChunkKey,
    pub version: u32,
    pub payload: Arc<Vec<u8>>,
}

/// The grid a brick set belongs to (sent once per connection, with the first bricks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrickGrid {
    pub voxel_size: f32,
    pub chunk_edge: i32,
}

/// The store's newest version of every brick, and the sequence numbers senders resume from.
#[derive(Default)]
pub struct BrickLog {
    seq: u64,
    grid: Option<BrickGrid>,
    entries: HashMap<ChunkKey, (u64, Brick)>,
    payload_bytes: u64,
}

impl BrickLog {
    fn put(&mut self, brick: Brick) {
        self.seq += 1;
        if let Some((_, old)) = self.entries.get(&brick.key) {
            self.payload_bytes -= old.payload.len() as u64;
        }
        self.payload_bytes += brick.payload.len() as u64;
        self.entries.insert(brick.key, (self.seq, brick));
    }

    fn clear(&mut self) {
        self.seq += 1;
        self.entries.clear();
        self.payload_bytes = 0;
    }

    /// Every brick newer than `cursor` and the cursor to resume from.
    pub fn changes_since(&self, cursor: u64) -> (Vec<Brick>, u64) {
        let mut out: Vec<(u64, Brick)> = self.entries.values().filter(|(seq, _)| *seq > cursor).cloned().collect();
        out.sort_by_key(|(seq, _)| *seq);
        (out.into_iter().map(|(_, b)| b).collect(), self.seq)
    }

    pub fn grid(&self) -> Option<BrickGrid> {
        self.grid
    }

    /// (bricks with content, total payload bytes)
    pub fn stats(&self) -> (usize, u64) {
        (self.entries.values().filter(|(_, b)| !b.payload.is_empty()).count(), self.payload_bytes)
    }
}

/// The quantized content of one brick: the valid voxels and their distances (127 = +1).
#[derive(Clone, PartialEq)]
pub(crate) struct Quantized {
    pub(crate) valid: u64,
    pub(crate) distance: [i8; 64],
}

/// `sent`: the brick as it last went out (its voxels are kept down to BRICK_KEEP_CONFIDENCE).
fn quantize(chunk: &SparseTsdReadChunk, volume: usize, sent: Option<&Quantized>) -> Option<Quantized> {
    if volume > 64 {
        return None;
    }
    let mut q = Quantized { valid: 0, distance: [0; 64] };
    for id in 0..volume {
        let Some(value) = chunk.value(id) else { continue };
        let was_sent = sent.is_some_and(|s| (s.valid >> id) & 1 != 0);
        let least = if was_sent { BRICK_KEEP_CONFIDENCE } else { BRICK_MIN_CONFIDENCE };
        if chunk.confidence(id) < least || !(-1.0..=1.0).contains(&value) {
            continue;
        }
        q.valid |= 1 << id;
        q.distance[id] = (value * 127.0).round().clamp(-127.0, 127.0) as i8;
    }
    (q.valid != 0).then_some(q)
}

fn moved(a: &Quantized, b: &Quantized) -> bool {
    if (a.valid ^ b.valid).count_ones() >= BRICK_RESEND_MIN_SET_CHANGE {
        return true;
    }
    let shared = a.valid & b.valid;
    let (mut sum, mut n, mut far) = (0.0f32, 0u32, 0u32);
    for i in (0..64).filter(|i| (shared >> i) & 1 != 0) {
        let d = (a.distance[i] as f32 - b.distance[i] as f32).abs();
        sum += d * d;
        n += 1;
        far += (d > BRICK_RESEND_MIN_DELTA) as u32;
    }
    n > 0 && (sum / n as f32).sqrt() >= BRICK_RESEND_MIN_DELTA && far >= BRICK_RESEND_MIN_VOXELS
}

/// payload: valid bits (u64 LE), then one i8 per valid voxel in index order.
pub(crate) fn encode(q: &Quantized) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + q.valid.count_ones() as usize);
    out.extend_from_slice(&q.valid.to_le_bytes());
    for i in 0..64 {
        if (q.valid >> i) & 1 != 0 {
            out.push(q.distance[i] as u8);
        }
    }
    out
}

/// A brick payload back into a read chunk of `volume` voxels (confidence full); None when the
/// payload is damaged.
pub fn decode_brick(payload: &[u8], volume: usize) -> Option<SparseTsdReadChunk> {
    let valid = u64::from_le_bytes(payload.get(0..8)?.try_into().ok()?);
    if volume > 64 || (volume < 64 && valid >> volume != 0) || payload.len() != 8 + valid.count_ones() as usize {
        return None;
    }
    let mut chunk = SparseTsdReadChunk::new(volume);
    let mut at = 8;
    for id in 0..volume {
        if (valid >> id) & 1 != 0 {
            chunk.set_value(id, payload[at] as i8 as f32 / 127.0, 63, 0);
            at += 1;
        }
    }
    Some(chunk)
}

struct SentBrick {
    version: u32,
    content: Option<Quantized>,
    sent_at: Instant,
}

/// The sending side (the depth writer's thread).
#[derive(Default)]
pub struct BrickProducer {
    seen: HashMap<ChunkKey, usize>,
    sent: HashMap<ChunkKey, SentBrick>,
    dirty: Vec<ChunkKey>,
    grid: Option<Arc<SparseTsdGridReadSnapshot>>,
}

impl BrickProducer {
    /// After a reset of the volume: everything goes again.
    pub fn reset(&mut self, store: &XrTsdfStore) {
        *self = Self::default();
        lock_bricks(store).clear();
    }

    /// A new snapshot: the chunks whose storage changed (or went) become dirty.
    pub fn note_snapshot(&mut self, snapshot: &TsdfPublishedSnapshot) {
        let grid = &snapshot.grid;
        for (key, chunk) in &grid.chunks {
            let ptr = Arc::as_ptr(chunk) as usize;
            if self.seen.insert(*key, ptr) != Some(ptr) {
                self.dirty.push(*key);
            }
        }
        if self.seen.len() > grid.chunks.len() {
            let gone: Vec<ChunkKey> = self.seen.keys().filter(|k| !grid.chunks.contains_key(k)).copied().collect();
            for key in gone {
                self.seen.remove(&key);
                self.dirty.push(key);
            }
        }
        self.grid = Some(grid.clone());
    }

    /// Sends the dirty bricks that moved and whose debounce passed (the rest stay dirty).
    /// Returns how many went out.
    pub fn produce(&mut self, store: &XrTsdfStore, now: Instant) -> usize {
        let Some(grid) = self.grid.clone() else { return 0 };
        if self.dirty.is_empty() {
            return 0;
        }
        self.dirty.sort_unstable_by_key(|k| (k.x, k.y, k.z));
        self.dirty.dedup();
        let mut keep = Vec::new();
        let mut out = Vec::new();
        for key in std::mem::take(&mut self.dirty) {
            if self.sent.get(&key).is_some_and(|s| now.duration_since(s.sent_at) < BRICK_DEBOUNCE) {
                keep.push(key);
                continue;
            }
            let sent = self.sent.get(&key).and_then(|s| s.content.as_ref());
            let content = grid.chunks.get(&key).and_then(|c| quantize(c, grid.chunk_volume, sent));
            let changed = match (self.sent.get(&key), &content) {
                (None, None) => false,
                (None, Some(_)) => true,
                (Some(s), c) => match (&s.content, c) {
                    (Some(a), Some(b)) => moved(a, b),
                    (None, None) => false,
                    _ => true,
                },
            };
            if !changed {
                continue;
            }
            let version = self.sent.get(&key).map_or(1, |s| s.version + 1);
            let payload = Arc::new(content.as_ref().map(encode).unwrap_or_default());
            self.sent.insert(key, SentBrick { version, content, sent_at: now });
            out.push(Brick { key, version, payload });
        }
        self.dirty = keep;
        let n = out.len();
        if n != 0 {
            let mut log = lock_bricks(store);
            log.grid = Some(BrickGrid { voxel_size: grid.voxel_size, chunk_edge: grid.chunk_edge });
            for brick in out {
                log.put(brick);
            }
        }
        n
    }
}

fn lock_bricks(store: &XrTsdfStore) -> std::sync::MutexGuard<'_, BrickLog> {
    store.bricks.lock().unwrap_or_else(|e| e.into_inner())
}

impl XrTsdfStore {
    /// The bricks that changed since `cursor` (0: all of them), the grid they are of, and the
    /// cursor to ask with next time.
    pub fn brick_changes_since(&self, cursor: u64) -> (Option<BrickGrid>, Vec<Brick>, u64) {
        let log = lock_bricks(self);
        let (bricks, next) = log.changes_since(cursor);
        (log.grid(), bricks, next)
    }

    /// (bricks with content, their payload bytes)
    pub fn brick_stats(&self) -> (usize, u64) {
        lock_bricks(self).stats()
    }
}

/// The receiving side: bricks from a peer installed in this process's store, published as a
/// TSDF snapshot (the hit-surface queries read it like an integrated field).
#[derive(Default)]
pub struct BrickMirror {
    grid: Option<BrickGrid>,
    versions: HashMap<ChunkKey, u32>,
    chunks: HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
    sequence: u64,
}

impl BrickMirror {
    /// Takes `bricks` (older versions than held are ignored) and publishes the field when any
    /// changed. A different grid starts over.
    pub fn apply(&mut self, store: &XrTsdfStore, grid: BrickGrid, bricks: &[Brick]) -> usize {
        if self.grid != Some(grid) {
            *self = Self { grid: Some(grid), ..Self::default() };
        }
        let edge = grid.chunk_edge.max(1);
        let volume = (edge * edge * edge) as usize;
        let mut changed = 0;
        for brick in bricks {
            if self.versions.get(&brick.key).is_some_and(|v| *v >= brick.version) {
                continue;
            }
            self.versions.insert(brick.key, brick.version);
            if brick.payload.is_empty() {
                self.chunks.remove(&brick.key);
            } else if let Some(chunk) = decode_brick(&brick.payload, volume) {
                self.chunks.insert(brick.key, Arc::new(chunk));
            } else {
                continue;
            }
            changed += 1;
        }
        if changed != 0 {
            self.publish(store, grid, volume);
        }
        changed
    }

    fn publish(&mut self, store: &XrTsdfStore, grid: BrickGrid, volume: usize) {
        self.sequence += 1;
        let edge = grid.chunk_edge;
        let shift = (edge > 0 && (edge as u32).is_power_of_two()).then(|| (edge as u32).trailing_zeros() as u8);
        let active_value_count = self.chunks.values().map(|c| c.valid_bits.iter().map(|w| w.count_ones() as usize).sum::<usize>()).sum();
        let chunk_m = grid.voxel_size * edge as f32;
        let active_bounds = self.chunks.keys().fold(None::<(Vec3f, Vec3f)>, |acc, k| {
            let lo = vec3f(k.x as f32 * chunk_m, k.y as f32 * chunk_m, k.z as f32 * chunk_m);
            let hi = vec3f(lo.x + chunk_m, lo.y + chunk_m, lo.z + chunk_m);
            Some(match acc {
                None => (lo, hi),
                Some((a, b)) => (vec3f(a.x.min(lo.x), a.y.min(lo.y), a.z.min(lo.z)), vec3f(b.x.max(hi.x), b.y.max(hi.y), b.z.max(hi.z))),
            })
        });
        let snapshot = TsdfPublishedSnapshot {
            generation: 0,
            latest_topology_generation: self.sequence,
            update_sequence: self.sequence,
            grid: Arc::new(SparseTsdGridReadSnapshot {
                voxel_size: grid.voxel_size,
                chunk_edge: edge,
                chunk_edge_shift: shift,
                chunk_edge_mask: if shift.is_some() { edge - 1 } else { 0 },
                chunk_volume: volume,
                active_value_count,
                active_bounds,
                chunks: self.chunks.clone(),
            }),
            height_map: None,
        };
        store.publish_tsdf_snapshot(snapshot);
    }
}
