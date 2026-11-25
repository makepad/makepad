//! The Ignition Field: the boot window while the program compiles. A face-on spiral galaxy forms
//! as the program does: it opens on a field of scattered source dust that, over its first second
//! and a half, warps in place into the spiral (each grain eases along a short curve to a place on
//! an arm while the disk starts to turn), the core lighting up as the parse completes; then every
//! function installed is a star of its own, born when the JIT installs its code and arriving with
//! a brief brighter twinkle, filling the disk from the core outward along two logarithmic arms
//! (and two fainter ones) over an exponential falloff, so the galaxy's reach is the work done. A
//! star's size and light are its function's weight (its code's size, on a log scale): a heavy
//! function is a big, bright star with a halo, a small one a faint point; as the disk fills, the
//! faint stars dim (an exposure against the count) so it never turns into a white blob, and the
//! heavy ones stand out. A soft core, arm glow with dust lanes and the stars all turn slowly as
//! one, so the spiral never winds up. A function installed again (a tier-up, a live patch) glints
//! its own star; its O3 code turns its star gold with a gold flare, in the boot window and over
//! the running app (the window layer: the warm-up after the first present, the JIT loader of
//! later sustained JIT waiting).
//! Nothing else sparkles: every flash is a function installed. While functions install fast,
//! arrivals are softer (`glitter`, a smoothed install rate mapped into 0..1), so a burst of
//! thousands never flashes the disk.
//!
//! The HUD shows one status line, without numbers. The picture is retained instance buffers the
//! GPU places from the uniforms: one for the backdrop, the disk and the dust (boot.splash
//! `ignition`), and the stars' (`ignition_stars`, a small shader of their own: all that is the
//! same over a star is worked out per vertex). The frame appends the stars born since the last
//! one (one `buffer_update` of the new tail) and writes again the records of the stars it
//! glinted. The stars' buffers are chunks of doubling size (FIRST_CHUNK, twice that, ...), each
//! made when its first star is born: a star never moves, so a program of any size gets all its
//! stars and growing never writes the ones already there. Per star the field keeps its log entry
//! (birth, glint and O3 moments, weight) to write its record again, and its slot's place in the
//! slot-to-star map.
//!
//! The bake: the disk turns as one, so a star at rest is a fixed dot in the disk's own frame.
//! Stars older than BAKE_AGE (their arrival over) join two float layers in disk space
//! (`ignition_bake`), which the disk's quad shows turned (`ignition`), each star twinkling at its
//! own phase; birth order is settling order, so the bake is one frontier `baked` in the log: the
//! stars before it are in the layers, the ones after it are drawn one by one. It moves when a
//! quarter second of stars waits, so most frames bake nothing. What a baked star shows beyond its
//! dot at rest is drawn live from the sparks' buffer: a big star's rays and halo, a glint's added
//! light for its half second, and gold stars whole (a star turning gold is taken out of the bake
//! with weight -1, as one whose weight changes is baked again). A new size, device or document
//! draws every star one by one and bakes them again over a few frames; a halted view draws them
//! all (its colours change with the fault).
use crate::{document::Line, gpu::Pipeline, text::Text, Card, Layout, State, TextLine};

/// How long the dust takes to warp into the spiral (the shader's clock, boot.splash `ignition`).
pub(crate) const WARP: std::time::Duration = std::time::Duration::from_millis(1600);

use makepad_hal::{buffer_kind, format, usage, Attr, CmdWriter, Id};
use makepad_script::{id, LiveId};
use relax_base::{boot_stats as stats, run_state::Phase};
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

/// the buffer of the fixed instances and the dust
const INSTANCES: u32 = 2;
/// the stars' buffers: chunk k (this id + k) holds FIRST_CHUNK << k stars
const STAR_BUFFERS: u32 = 3;
const FIRST_CHUNK: usize = 16 * 1024;
/// chunks enough for every star an install count (u32) can number
const CHUNKS: usize = 19;
const DUST: usize = 1200;
/// the backdrop (four bands around the disk's square), the disk (core and arm glow), swap ring,
/// corner brackets, warm-up arc
const FIXED: usize = 8;
/// glinted stars at most this far apart are written again in one update (with the ones between)
const GLINT_GAP: usize = 32;
/// the sparks' buffer: the records of baked stars drawn live as well
const SPARK_BUFFER: u32 = 22;
/// the bake's two layers (textures, this index and the next)
const BAKE_LAYERS: u32 = 25;
/// a star joins the bake this long after its birth: its arrival is over (0.1% of it left)
const BAKE_AGE: f32 = 3.0;
/// the frontier moves once a quarter second of stars waits past BAKE_AGE
const BAKE_WAIT: f32 = 0.25;
/// stars baked in a frame at most: after a new size, device or document the layers fill again
/// over frames, the stars not in them yet drawn one by one
const BAKE_BATCH: usize = 16 * 1024;
/// the layers' side in disk radii: they cover the disk's quad (1.4 radii each way)
const BAKE_SPAN: f32 = 2.8;
/// a new size of the disk keeps its stars drawn one by one until it has held this long (s)
const BAKE_SETTLE: f32 = 0.25;
/// a glint's length (boot.splash `ignition_stars`: `g`)
const GLINT: f32 = 0.5;
/// the warp's ease into the turn: the disk's spin speeds up from rest over about this (s)
const SPIN_EASE: f32 = 0.9;
/// functions installed per second at which the glitter is at 63% of its cap
const GLITTER_RATE: f32 = 2000.0;

// the spiral, shared with boot.splash (`arm_phase`): an arm at base angle b runs along
// a = b - WIND * ln(r / R0), a logarithmic spiral of pitch atan(1 / WIND) (about 23 degrees)
const WIND: f32 = 2.3;
const R0: f32 = 0.08;
/// the disk's reach after the first star and its growth: the reach after `g` stars is
/// REACH0 + (1 - REACH0) * (1 - exp(-g / REACH_STARS))
const REACH0: f32 = 0.18;
const REACH_STARS: f32 = 5600.0;

/// The weights (`boot_stats::weight_of`: 8 log2 of the code's bytes) of the lightest star and of
/// the heaviest, 32 bytes and 8 KB: lighter code is as light, heavier as heavy. (A cold run's
/// functions: median 116-144 bytes, p90 650-770, p99 4-7 KB, p99.9 16-44 KB, the largest 70-230 KB.)
const WEIGHT_LIGHT: f32 = 8.0 * 5.0;
const WEIGHT_HEAVY: f32 = 8.0 * 13.0;
/// A star's size (boot.splash `star_px`, about its glow's sigma in 0.42 points at scale 1) for
/// the lightest and the heaviest code; the shader reads the weight back from it.
const SIZE_LIGHT: f32 = 0.45;
const SIZE_HEAVY: f32 = 2.85;
/// Stars up to about this many keep their full light; past it the light ones dim as
/// sqrt(STARS_LIT / stars), the heaviest not at all (boot.splash `star_px`).
const STARS_LIT: f32 = 6000.0;

const KIND_STAR: f32 = 0.0;
/// a spark's two records (boot.splash `ignition_stars`): its rays across, its rays down at rest;
/// the star whole and nothing while it arrives, glints or is gold
const SPARK_RECORDS: [f32; 2] = [1.0, 2.0];
const KIND_DUST: f32 = 1.0;
const KIND_DISK: f32 = 3.0;
const KIND_RING: f32 = 4.0;
const KIND_BRACKETS: f32 = 5.0;
const KIND_BACKDROP: f32 = 6.0;
const KIND_ARC: f32 = 7.0;

// the concept's ink
const INK: [f32; 4] = [0.925, 0.902, 1.0, 1.0];

/// A star: one function installed. Its place in the log is its birth order, which places it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Born {
    /// when it was born (window time)
    at: f32,
    /// the last time its function was installed again (window time; 0: never)
    twinkle: f32,
    /// when its function's O3 code was installed (window time; 0: not O3)
    gold: f32,
    /// its code's weight (`boot_stats::weight_of`), its last install's
    weight: u8,
    /// the weight its dot was baked at (stars before the frontier; None: not in the bake, gold)
    baked: Option<u8>,
}

/// The bake's layers: made for one disk radius (points) and dpi, which place and size the stars
/// in them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layers {
    /// texels a side
    side: u32,
    radius: f32,
    dpi: f32,
}

/// A frame's bake pass into the layers (`ignition_bake`), before the frame's own.
pub struct BakePass {
    pub layers: [Id; 2],
    /// texels a side
    pub side: u32,
    /// the layers hold nothing yet (or lost it): cleared first
    pub clear: bool,
    /// (texels a disk radius, texels a point)
    pub scale: [f32; 2],
    /// stars put in: (their chunk, first, count)
    pub runs: Vec<(Id, u32, u32)>,
    /// stars put in again (a new weight) and taken out (weight -1: gold, or the old weight), as
    /// records
    pub put_in: Vec<f32>,
    pub take_out: Vec<f32>,
}

pub struct Field {
    /// the pipelines (the field's, the stars') the buffers were written for (a reload makes new
    /// ones; None: write them all again)
    pipes: Option<(Id, Id)>,
    /// their records' floats
    stride: usize,
    star_stride: usize,
    /// the buffers' generation (a new record layout makes them again)
    gen: u32,
    /// the buffer of the fixed instances and the dust
    instances: Id,
    /// instances in it
    count: usize,
    /// the stars' buffers made so far, chunk k holding FIRST_CHUNK << k stars (`chunk_of`)
    chunks: Vec<Id>,
    /// the stars, by birth order (`chunk_of` its entry is its record's place)
    log: Vec<Born>,
    /// each slot's star (its entry + 1; 0: none yet), of the program's units and of the
    /// proc-macro world's (its own slot numbers)
    star_of: [Vec<u32>; 2],
    /// the install log's entries read so far
    seen: u32,
    floats: Vec<f32>,
    bytes: Vec<u8>,
    /// `ms_since_process_start` when this run of the field began
    began_ms: f64,
    time: f32,
    /// entries of stars glinted this frame
    touched: Vec<u32>,
    p_smooth: f32,
    /// the disk's reach, eased towards that of the stars born
    reach: f32,
    /// the galaxy's turn at the last frame that moved
    spin: f32,
    /// functions installed per second, smoothed
    rate: f32,
    /// this field's CPU time per frame since `take_cpu`: (sum ns, max ns, frames)
    cpu: (u64, u64, u32),
    status: String,
    status_at: f32,
    /// the bake's frontier: stars [0, baked) are in the layers
    baked: usize,
    layers: Option<Layers>,
    layer_gen: u32,
    /// the layers lost what they held (new, a new run, device or document): the next bake
    /// clears them first
    layers_clear: bool,
    /// the layers a new size asks for, since (window time)
    want: Option<(Layers, f32)>,
    /// baked stars drawn live as well, by their slot in the sparks' buffer (`spark_slot`: an
    /// entry's slot)
    sparks: Vec<u32>,
    spark_slot: HashMap<u32, u32>,
    /// sparks that stay only while they glint: looked at each frame
    glinting: Vec<u32>,
    spark_buffer: Id,
    spark_cap: usize,
    spark_gen: u32,
    /// the sparks' slots to write again (from, to)
    spark_dirty: (usize, usize),
    /// this frame's records to bake again and to take out
    put_in: Vec<f32>,
    take_out: Vec<f32>,
    /// the bake pass no frame has drawn yet (`take_bake`; a frame that ends early leaves it to
    /// the next, which adds its own)
    pending: Option<BakePass>,
}

/// What the frame needs to draw it: the uniforms and the instance counts.
pub struct Frame {
    /// the fixed instances and the dust (`ignition`)
    pub instances: Id,
    pub count: u32,
    /// the stars drawn one by one (`ignition_stars`, a draw each): their chunk, first, count
    pub live: [(Id, u32, u32); CHUNKS],
    /// baked stars drawn live as well (`ignition_stars`, field_stars.y 1): buffer, records (two a
    /// spark)
    pub sparks: (Id, u32),
    /// the layers the disk shows the baked stars from (none: the bake holds none)
    pub shown: Option<[Id; 2]>,
    pub view: [f32; 4],
    pub params: [f32; 4],
    pub fall: [f32; 4],
    /// (the faint stars' exposure, 1 when the disk shows the bake, 0, 0)
    pub stars: [f32; 4],
}

fn hash(x: u32, salt: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Star `entry`'s buffer and its record's place in it: chunk k holds the FIRST_CHUNK << k stars
/// from FIRST_CHUNK * (2^k - 1) on, so a star's place never changes as more are born.
fn chunk_of(entry: usize) -> (usize, usize) {
    let q = entry / FIRST_CHUNK + 1;
    let k = (usize::BITS - 1 - q.leading_zeros()) as usize;
    (k, entry - FIRST_CHUNK * ((1 << k) - 1))
}

fn reach_of(stars: f32) -> f32 {
    REACH0 + (1.0 - REACH0) * (1.0 - (-stars / REACH_STARS).exp())
}

/// A star's size for its code's weight: from SIZE_LIGHT to SIZE_HEAVY by the square of the
/// weight's place between WEIGHT_LIGHT and WEIGHT_HEAVY, so most functions are small points (the
/// median about 0.6, a 1 KB function 1.4) and the heaviest few percent grow a halo and rays
/// (boot.splash: from size 2.0, about 2.8 KB, in full from 2.8, about 7.5 KB).
fn size_of(weight: u8) -> f32 {
    let t = ((weight as f32 - WEIGHT_LIGHT) / (WEIGHT_HEAVY - WEIGHT_LIGHT)).clamp(0.0, 1.0);
    SIZE_LIGHT + (SIZE_HEAVY - SIZE_LIGHT) * t * t
}

/// The angle and radius (in disk radii, before the turn) and seed of the `g`th star born: a tenth
/// in the bulge, most along the arms (two main, two faint), the rest between them; the radius
/// falls off exponentially inside the reach the disk had at its birth.
fn star_at(g: u32) -> (f32, f32, f32) {
    let h = |salt| hash(g, salt);
    let reach = reach_of(g as f32);
    let kind = h(1);
    let (a, r) = if kind < 0.1 {
        let r = (0.12 * (-(h(2).max(1e-4)).ln()).sqrt()).min(0.4).min(reach);
        (h(3) * TAU, r)
    } else {
        // a truncated exponential on [0, 1] of scale 0.5
        let s: f32 = 0.5;
        let x = -s * (1.0 - h(2) * (1.0 - (-1.0 / s).exp())).ln();
        let r = 0.03 + x * reach;
        let (base, spread) = if kind < 0.86 {
            let arm = h(4);
            let base = if arm < 0.4 {
                0.0
            } else if arm < 0.8 {
                PI
            } else if arm < 0.9 {
                0.5 * PI
            } else {
                1.5 * PI
            };
            // about Gaussian across the arm, wider outward
            (base, (h(5) + h(6) - 1.0) * (0.26 + 0.2 * r))
        } else {
            (h(5) * TAU, 0.0)
        };
        (base + spread - WIND * (r / R0).ln(), r)
    };
    (a, r, h(8))
}

impl Field {
    pub fn new() -> Self {
        let mut field = Self {
            pipes: None,
            stride: 0,
            star_stride: 0,
            gen: 1,
            instances: Id::new(INSTANCES, 1),
            count: 0,
            chunks: Vec::new(),
            log: Vec::new(),
            star_of: Default::default(),
            seen: 0,
            floats: Vec::new(),
            bytes: Vec::new(),
            began_ms: 0.0,
            time: 0.0,
            touched: Vec::new(),
            p_smooth: 0.0,
            reach: REACH0,
            spin: 0.0,
            rate: 0.0,
            cpu: (0, 0, 0),
            status: String::new(),
            status_at: 0.0,
            baked: 0,
            layers: None,
            layer_gen: 0,
            layers_clear: true,
            want: None,
            sparks: Vec::new(),
            spark_slot: HashMap::new(),
            glinting: Vec::new(),
            spark_buffer: Id::NONE,
            spark_cap: 0,
            spark_gen: 0,
            spark_dirty: (usize::MAX, 0),
            put_in: Vec::new(),
            take_out: Vec::new(),
            pending: None,
        };
        field.reset();
        field
    }

    /// A new run begins (a retried build): the installs count from here, the buffer starts again.
    pub fn reset(&mut self) {
        self.began_ms = relax_base::clock::ms_since_process_start();
        self.time = 0.0;
        self.seen = stats::installs();
        for map in &mut self.star_of {
            map.clear();
        }
        self.p_smooth = 0.0;
        self.reach = REACH0;
        self.rate = 0.0;
        self.status.clear();
        self.status_at = 0.0;
        self.log.clear();
        self.touched.clear();
        // written again by the next frame
        self.pipes = None;
        self.unbake();
    }

    /// The bake holds nothing (a new run, size, device or document): every star is drawn one by
    /// one until it is baked again, into layers cleared first.
    fn unbake(&mut self) {
        self.baked = 0;
        self.layers_clear = true;
        self.sparks.clear();
        self.spark_slot.clear();
        self.glinting.clear();
        self.spark_dirty = (usize::MAX, 0);
        self.put_in.clear();
        self.take_out.clear();
        self.pending = None;
    }

    /// The bake pass to draw before this frame's own (its layers then hold what `Frame::shown`
    /// shows).
    pub fn take_bake(&mut self) -> Option<BakePass> {
        self.pending.take()
    }

    /// The field is put away (the app has the window): its layers go (made again, and baked
    /// again, by its next frame).
    pub fn release(&mut self, writer: &mut CmdWriter) {
        if self.layers.take().is_some() {
            writer.texture_destroy(Id::new(BAKE_LAYERS, self.layer_gen));
            writer.texture_destroy(Id::new(BAKE_LAYERS + 1, self.layer_gen));
        }
        self.want = None;
        self.unbake();
    }

    /// A new GPU device kept the buffer's id, not what it held: the next frame writes it again
    /// from nothing (the installs go on; `reset` is a new run).
    pub fn device_reset(&mut self) {
        self.pipes = None;
    }

    /// The buffers for these record layouts: the ones made stay (a new run, a new device only
    /// writes them again); another layout makes them again, a new generation, as the HAL refuses
    /// a destroyed id's generation. The stars' chunks are made as stars are born (`chunk`).
    fn buffers_for(&mut self, writer: &mut CmdWriter, stride: usize, star_stride: usize) {
        if (self.stride, self.star_stride) == (stride, star_stride) {
            return;
        }
        if self.stride != 0 {
            writer.buffer_destroy(self.instances);
            for id in self.chunks.drain(..) {
                writer.buffer_destroy(id);
            }
            self.gen += 1;
        }
        if self.spark_cap > 0 {
            writer.buffer_destroy(self.spark_buffer);
            self.spark_cap = 0;
        }
        (self.stride, self.star_stride) = (stride, star_stride);
        self.instances = Id::new(INSTANCES, self.gen);
        writer.buffer_create(self.instances, buffer_kind::INSTANCE, ((FIXED + DUST) * stride * 4) as u64, Attr::default());
    }

    /// Makes the stars' chunks up to chunk `k`: room, nothing to copy.
    fn chunk(&mut self, writer: &mut CmdWriter, k: usize) {
        while self.chunks.len() <= k {
            let n = self.chunks.len();
            let id = Id::new(STAR_BUFFERS + n as u32, self.gen);
            writer.buffer_create(id, buffer_kind::INSTANCE, ((FIRST_CHUNK << n) * self.star_stride * 4) as u64, Attr::default());
            self.chunks.push(id);
        }
    }

    fn push(&mut self, pipe: &Pipeline, rect: [f32; 4], info: [f32; 4]) {
        pipe.instances
            .write(&[(id!(rect), &rect), (id!(info), &info)], &mut self.floats);
    }

    /// Star `entry`'s record: rect = (angle, radius, size, born), info = (kind, O3 moment + 1,
    /// glint moment + 1, seed), a moment 0 when there is none.
    fn write_star(&mut self, pipe: &Pipeline, entry: usize) {
        self.write_record(pipe, entry, KIND_STAR);
    }

    /// Star `entry`'s record of `kind` (a live star's, or one of a spark's two).
    fn write_record(&mut self, pipe: &Pipeline, entry: usize, kind: f32) {
        let b = self.log[entry];
        let (a, r, seed) = star_at(entry as u32);
        let mark = |t: f32| if t > 0.0 { t + 1.0 } else { 0.0 };
        self.push(pipe, [a, r, size_of(b.weight), b.at], [kind, mark(b.gold), mark(b.twinkle), seed]);
    }

    /// Writes the records of stars `from..to` to their chunks, one update per chunk they are in.
    fn write_stars(&mut self, writer: &mut CmdWriter, stars: &Pipeline, from: usize, to: usize) {
        let mut entry = from;
        while entry < to {
            let (k, at) = chunk_of(entry);
            let end = to.min(entry + (FIRST_CHUNK << k) - at);
            self.chunk(writer, k);
            for e in entry..end {
                self.write_star(stars, e);
            }
            self.upload(writer, self.chunks[k], at, self.star_stride);
            entry = end;
        }
    }

    /// Writes the fixed instances and the dust from nothing.
    fn rewrite(&mut self, writer: &mut CmdWriter, pipe: &Pipeline) {
        self.floats.clear();
        self.floats.reserve((FIXED + DUST) * self.stride);
        for band in 0..4 {
            self.push(pipe, [0.0; 4], [KIND_BACKDROP, band as f32, 0.0, 0.0]);
        }
        for kind in [KIND_DISK, KIND_RING, KIND_BRACKETS, KIND_ARC] {
            self.push(pipe, [0.0; 4], [kind, 0.0, 0.0, 0.0]);
        }
        // the source dust: scattered over the disk, each grain with the place it warps to (its
        // radius in rect.w, the turn to it in info.z: the nearest ridge of an arm at about its own
        // radius, so every path is short) and when it starts (info.y)
        for i in 0..DUST {
            let i = i as u32;
            let h = |salt| hash(i, salt);
            let a = h(11) * TAU;
            let r = 0.08 + 1.02 * h(12).powf(0.65);
            let to = (r * (0.9 + 0.1 * h(13))).min(1.0);
            let kind = h(16);
            let delta = if kind < 0.86 {
                // two main arms, a few grains on the faint pair between them
                let base = if kind < 0.7 { 0.0 } else { 0.5 * PI };
                let spread = (h(17) + h(18) - 1.0) * (0.26 + 0.2 * to);
                let ridge = base + spread - WIND * (to / R0).ln();
                (ridge - a + 0.5 * PI).rem_euclid(PI) - 0.5 * PI
            } else {
                // between the arms: a small drift
                (h(17) - 0.5) * 0.4
            };
            let size = 0.8 + 1.0 * h(14).powi(3);
            self.push(pipe, [a, r, size, to], [KIND_DUST, h(19), delta, h(15)]);
        }
        self.count = self.floats.len() / self.stride;
        self.upload(writer, self.instances, 0, self.stride);
    }

    /// Sends the records in `floats` (of `stride` floats, the first one record number `first`)
    /// to buffer `id`.
    fn upload(&mut self, writer: &mut CmdWriter, id: Id, first: usize, stride: usize) {
        self.bytes.clear();
        for v in &self.floats {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        if !self.bytes.is_empty() {
            writer.buffer_update(id, (first * stride * 4) as u64, &self.bytes);
        }
        self.floats.clear();
    }

    /// Reads the installs since the last frame (`previous`, window time): a function's first
    /// install is a star born (the births spread over the frame interval, not stacked at its end),
    /// a later one glints its star (`touched`), its O3 code turns it gold. Answers the functions
    /// installed (O3 code apart: a tier-up, not new work).
    fn ingest(&mut self, previous: f32, time: f32) -> u32 {
        let end = stats::installs();
        if end < self.seen {
            // the log began again (a run's end in place before this field's `reset`)
            self.seen = 0;
        }
        let first = self.log.len();
        let mut installed = 0;
        let now = time.max(1e-3);
        while self.seen < end {
            // an install whose writer has not stored it yet: the next frame goes on from it
            let Some(x) = stats::install(self.seen) else { break };
            self.seen += 1;
            installed += !x.o3 as u32;
            let map = &mut self.star_of[x.aux as usize];
            let slot = x.slot as usize;
            if map.len() <= slot {
                map.resize(slot + 1, 0);
            }
            match map[slot] {
                0 => {
                    map[slot] = self.log.len() as u32 + 1;
                    // (`at` and an O3 star's `gold` are set below)
                    self.log.push(Born { at: 0.0, twinkle: 0.0, gold: x.o3 as u32 as f32, weight: x.weight, baked: None });
                }
                n => {
                    let b = &mut self.log[n as usize - 1];
                    if x.o3 {
                        b.gold = now;
                    } else {
                        b.twinkle = now;
                        b.weight = x.weight;
                    }
                    self.touched.push(n - 1);
                }
            }
        }
        let born = self.log.len() - first;
        let dt = (time - previous).max(0.0);
        for (i, b) in self.log[first..].iter_mut().enumerate() {
            b.at = previous + dt * (i + 1) as f32 / born as f32;
            if b.gold > 0.0 {
                b.gold = b.at.max(1e-3);
            }
        }
        installed
    }

    /// One frame: reads the loader's installs, appends the stars born since the last frame,
    /// writes again the ones glinted, and answers the uniforms. `pipe` draws the backdrop, the
    /// disk and the dust, `stars` the stars. `time` is the window's clock in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        writer: &mut CmdWriter,
        pipe: &Pipeline,
        stars_pipe: &Pipeline,
        state: &State,
        time: f32,
        region: [f32; 4],
        reduce_motion: bool,
        halted: bool,
        resolve: f32,
        dpi: f32,
    ) -> Frame {
        use std::sync::atomic::Ordering::Relaxed;
        let started = std::time::Instant::now();
        let previous = self.time;
        let dt = (time - previous).max(0.0);
        self.time = time;
        let now_ms = relax_base::clock::ms_since_process_start();

        // ---- stars: one per function installed, placed by birth order
        let old = self.log.len();
        let installed_now = self.ingest(previous, time);
        if self.pipes != Some((pipe.id, stars_pipe.id)) {
            // a new run, a new device or a replaced document (its own record layouts): every
            // record written again
            self.buffers_for(writer, pipe.instances.defaults.len(), stars_pipe.instances.defaults.len());
            self.rewrite(writer, pipe);
            self.write_stars(writer, stars_pipe, 0, self.log.len());
            self.touched.clear();
            self.pipes = Some((pipe.id, stars_pipe.id));
            self.unbake();
        } else {
            self.write_stars(writer, stars_pipe, old, self.log.len());
            self.touch_baked(stars_pipe);
            self.glint(writer, stars_pipe, old);
        }

        // ---- the geometry of the picture, in `region` (the window, or the halted view's part):
        // round at any size, the radius one length in points for both axes
        let [rx, ry, w, h] = region;
        let narrow = w < 600.0;
        let tiny = w < 300.0 || h < 220.0;
        let (radius, cy) = if halted {
            ((w * 0.40).min(h * 0.40).max(12.0), h * 0.5)
        } else {
            let radius = (w * if narrow { 0.36 } else { 0.30 })
                .min(h * if tiny { 0.30 } else { 0.33 })
                .max(16.0);
            (radius, h * if tiny { 0.46 } else { 0.48 })
        };
        // the reach follows the stars born, eased over about half a second
        let stars = self.log.len() as f32;
        self.reach += (reach_of(stars) - self.reach) * (1.0 - (-dt * 4.0).exp());
        let view = [rx + w * 0.5, ry + cy, radius, self.reach];
        let scale = (radius / 220.0).clamp(0.75, 2.0);
        // a halted field stands still where it stopped; the turn eases in from rest with the warp
        if !halted {
            self.spin = if reduce_motion {
                0.0
            } else {
                0.035 * (time - SPIN_EASE * (1.0 - (-time / SPIN_EASE).exp()))
            };
        }
        // the glitter: the install rate, smoothed over about 0.4 s, eased into 0..1
        if dt > 0.0 {
            let rate = installed_now as f32 / dt.max(1.0 / 240.0);
            self.rate += (rate - self.rate) * (1.0 - (-dt * 2.5).exp());
        }
        let glitter = 1.0 - (-self.rate / GLITTER_RATE).exp();
        let spin = self.spin;

        // ---- events on the loader's clock, as ages in the window's
        let age = |ms: u64, since: f64| -> f32 {
            if ms == 0 || (ms as f64 - 1.0) < since {
                -1.0
            } else {
                ((now_ms - (ms as f64 - 1.0)) / 1000.0) as f32
            }
        };
        let main_age = age(stats::MAIN_AT_MS.load(Relaxed), self.began_ms);
        let swap_age = age(stats::SETTLE_END_MS.load(Relaxed), self.began_ms);

        // ---- the core lights up as the crates are parsed
        let target = match state.phase {
            Phase::Resolve => 0.0,
            Phase::Parse => match (state.done, state.total) {
                (Some(done), Some(total)) if total > 0 => done as f32 / total as f32,
                _ => 0.0,
            },
            _ => 1.0,
        };
        self.p_smooth += (target - self.p_smooth).max(0.0) * (1.0 - (-dt * 3.0).exp());
        if target - self.p_smooth < 0.002 {
            // lands: the shader knows the parse is over (the ring is gone)
            self.p_smooth = self.p_smooth.max(target);
        }
        let dust_fade = if swap_age >= 0.0 {
            let x = (swap_age / 0.5).clamp(0.0, 1.0);
            1.0 - x * x * (3.0 - 2.0 * x)
        } else {
            1.0
        } * if halted { 0.35 } else { 1.0 };
        // the faint stars' exposure: full up to STARS_LIT stars, then sqrt(STARS_LIT / stars)
        let exposure = (STARS_LIT / stars.max(1.0)).sqrt().min(1.0);

        // ---- the bake: a halted view draws every star (its colours change with the fault)
        let shown = if halted { None } else { self.bake(writer, stars_pipe, time, radius, dpi) };
        let live_from = if halted { 0 } else { self.baked };
        let mut live = [(Id::NONE, 0, 0); CHUNKS];
        for (k, id) in self.chunks.iter().enumerate() {
            let first = FIRST_CHUNK * ((1 << k) - 1);
            let end = self.log.len().saturating_sub(first).min(FIRST_CHUNK << k);
            let start = live_from.saturating_sub(first).min(end);
            live[k] = (*id, start as u32, (end - start) as u32);
        }
        let sparks = if halted { (Id::NONE, 0) } else { (self.spark_buffer, (self.sparks.len() * SPARK_RECORDS.len()) as u32) };
        let ns = started.elapsed().as_nanos() as u64;
        self.cpu = (self.cpu.0 + ns, self.cpu.1.max(ns), self.cpu.2 + 1);
        Frame {
            instances: self.instances,
            count: self.count as u32,
            live,
            sparks,
            shown,
            view,
            params: [spin, scale, main_age, swap_age.max(-1.0)],
            fall: [self.p_smooth, dust_fade, glitter, resolve],
            stars: [exposure, shown.is_some() as u8 as f32, 0.0, 0.0],
        }
    }

    /// The bake's frame: the layers for this size (a new size draws the stars one by one until
    /// it holds BAKE_SETTLE), the frontier moved over the stars past BAKE_AGE (when a quarter
    /// second of them waits) into the pending bake pass, the sparks kept and written. Answers the
    /// layers the disk shows.
    fn bake(&mut self, writer: &mut CmdWriter, stars: &Pipeline, time: f32, radius: f32, dpi: f32) -> Option<[Id; 2]> {
        // texels a point: 2 (a pixel at dpi 2), more on a denser screen
        let texels = dpi.max(2.0);
        let want = Layers { side: (BAKE_SPAN * radius * texels).ceil() as u32, radius, dpi };
        if self.layers != Some(want) {
            if self.baked > 0 || !self.sparks.is_empty() {
                self.unbake();
            }
            let since = match self.want {
                Some((w, at)) if w == want => at,
                _ => {
                    self.want = Some((want, time));
                    time
                }
            };
            if self.layers.is_none() || time - since >= BAKE_SETTLE {
                if self.layers.is_some() {
                    writer.texture_destroy(Id::new(BAKE_LAYERS, self.layer_gen));
                    writer.texture_destroy(Id::new(BAKE_LAYERS + 1, self.layer_gen));
                }
                self.layer_gen = self.layer_gen.wrapping_add(1).max(1);
                for index in [BAKE_LAYERS, BAKE_LAYERS + 1] {
                    writer.texture_create(
                        Id::new(index, self.layer_gen),
                        usage::RENDER_TARGET | usage::SAMPLED,
                        format::RGBA16F,
                        want.side,
                        want.side,
                        [1, 1],
                        Attr::default(),
                    );
                }
                self.layers = Some(want);
                self.want = None;
                self.layers_clear = true;
            }
        }
        if self.layers != Some(want) {
            return None;
        }
        // the frontier: over the stars past BAKE_AGE once the oldest of them is BAKE_WAIT past it
        let mut runs = Vec::new();
        let ready = self.log.partition_point(|b| b.at <= time - BAKE_AGE);
        if self.baked < ready && self.log[self.baked].at <= time - BAKE_AGE - BAKE_WAIT {
            let (from, to) = (self.baked, ready.min(self.baked + BAKE_BATCH));
            for e in from..to {
                let b = &mut self.log[e];
                b.baked = (b.gold == 0.0).then_some(b.weight);
                if Self::sparks_for_good(b) || b.twinkle > 0.0 && time - b.twinkle < GLINT {
                    self.spark(e);
                }
            }
            let mut e = from;
            while e < to {
                let (k, at) = chunk_of(e);
                let end = to.min(e + (FIRST_CHUNK << k) - at);
                runs.push((self.chunks[k], at as u32, (end - e) as u32));
                e = end;
            }
            self.baked = to;
        }
        // a spark there for its glint alone leaves as it ends
        self.glinting.sort_unstable();
        self.glinting.dedup();
        let mut i = 0;
        while i < self.glinting.len() {
            let e = self.glinting[i];
            let b = self.log[e as usize];
            match self.spark_slot.get(&e) {
                Some(_) if !Self::sparks_for_good(&b) && b.twinkle > 0.0 && time - b.twinkle < GLINT => i += 1,
                Some(&slot) if !Self::sparks_for_good(&b) => {
                    self.unspark(slot as usize);
                    self.glinting.swap_remove(i);
                }
                _ => {
                    self.glinting.swap_remove(i);
                }
            }
        }
        self.write_sparks(writer, stars);
        let layers = [Id::new(BAKE_LAYERS, self.layer_gen), Id::new(BAKE_LAYERS + 1, self.layer_gen)];
        if !runs.is_empty() || !self.put_in.is_empty() || !self.take_out.is_empty() {
            let pass = self.pending.get_or_insert_with(|| BakePass {
                layers,
                side: want.side,
                clear: false,
                scale: [radius * texels, texels],
                runs: Vec::new(),
                put_in: Vec::new(),
                take_out: Vec::new(),
            });
            pass.clear |= std::mem::replace(&mut self.layers_clear, false);
            pass.runs.append(&mut runs);
            pass.put_in.append(&mut self.put_in);
            pass.take_out.append(&mut self.take_out);
        }
        (self.baked > 0 && !self.layers_clear).then_some(layers)
    }

    /// Whether a baked star is drawn live for good: a big one's rays and halo, a gold one whole.
    fn sparks_for_good(b: &Born) -> bool {
        b.gold > 0.0 || size_of(b.weight) > 2.0
    }

    /// The stars glinted, turned gold or given a new weight this frame that the bake holds: what
    /// it holds of them follows (a gold star is taken out, a new weight baked again), and each is
    /// drawn live as well, at least while it glints.
    fn touch_baked(&mut self, stars: &Pipeline) {
        for i in 0..self.touched.len() {
            let e = self.touched[i] as usize;
            if e >= self.baked {
                continue;
            }
            let b = self.log[e];
            let now = (b.gold == 0.0).then_some(b.weight);
            if b.baked != now {
                if let Some(weight) = b.baked {
                    Self::bake_record(stars, &mut self.take_out, e, weight, b.at);
                }
                if let Some(weight) = now {
                    Self::bake_record(stars, &mut self.put_in, e, weight, b.at);
                }
                self.log[e].baked = now;
            }
            self.spark(e);
        }
    }

    /// Star `entry`'s record as the bake holds it at `weight` (never gold), into `out`.
    fn bake_record(stars: &Pipeline, out: &mut Vec<f32>, entry: usize, weight: u8, at: f32) {
        let (a, r, seed) = star_at(entry as u32);
        stars.instances.write(&[(id!(rect), &[a, r, size_of(weight), at]), (id!(info), &[KIND_STAR, 0.0, 0.0, seed])], out);
    }

    /// Baked star `entry` is drawn live as well (its record written again if it already is),
    /// looked at each frame while it may leave.
    fn spark(&mut self, entry: usize) {
        let key = entry as u32;
        let slot = match self.spark_slot.get(&key) {
            Some(&slot) => slot as usize,
            None => {
                self.sparks.push(key);
                self.spark_slot.insert(key, self.sparks.len() as u32 - 1);
                self.sparks.len() - 1
            }
        };
        self.glinting.push(key);
        self.spark_dirty = (self.spark_dirty.0.min(slot), self.spark_dirty.1.max(slot + 1));
    }

    /// The spark in `slot` leaves; the last one takes its slot.
    fn unspark(&mut self, slot: usize) {
        let entry = self.sparks.swap_remove(slot);
        self.spark_slot.remove(&entry);
        if slot < self.sparks.len() {
            self.spark_slot.insert(self.sparks[slot], slot as u32);
            self.spark_dirty = (self.spark_dirty.0.min(slot), self.spark_dirty.1.max(slot + 1));
        }
    }

    /// Writes the sparks' changed records (all of them into a bigger buffer when they outgrow
    /// it).
    fn write_sparks(&mut self, writer: &mut CmdWriter, stars: &Pipeline) {
        if self.sparks.len() > self.spark_cap {
            if self.spark_cap > 0 {
                writer.buffer_destroy(self.spark_buffer);
            }
            self.spark_cap = self.sparks.len().next_power_of_two().max(1024);
            self.spark_gen = self.spark_gen.wrapping_add(1).max(1);
            self.spark_buffer = Id::new(SPARK_BUFFER, self.spark_gen);
            let bytes = self.spark_cap * SPARK_RECORDS.len() * self.star_stride * 4;
            writer.buffer_create(self.spark_buffer, buffer_kind::INSTANCE, bytes as u64, Attr::default());
            self.spark_dirty = (0, self.sparks.len());
        }
        let (from, to) = (self.spark_dirty.0, self.spark_dirty.1.min(self.sparks.len()));
        if from < to {
            for slot in from..to {
                for kind in SPARK_RECORDS {
                    self.write_record(stars, self.sparks[slot] as usize, kind);
                }
            }
            self.upload(writer, self.spark_buffer, from * SPARK_RECORDS.len(), self.star_stride);
        }
        self.spark_dirty = (usize::MAX, 0);
    }

    /// The stars born so far and this field's CPU time per frame since the last call (mean and
    /// max, us): `RELAX_BOOT_STATS`.
    pub fn take_cpu(&mut self) -> (usize, f64, f64) {
        let (sum, max, frames) = std::mem::replace(&mut self.cpu, (0, 0, 0));
        (self.log.len(), sum as f64 / frames.max(1) as f64 / 1e3, max as f64 / 1e3)
    }

    /// Writes again the records of the stars glinted this frame (`touched`) that were born before
    /// entry `fresh` (the ones after it were just written whole): one buffer update per run of
    /// them (per chunk it is in), with the unchanged records between two at most GLINT_GAP apart.
    fn glint(&mut self, writer: &mut CmdWriter, stars: &Pipeline, fresh: usize) {
        let mut touched = std::mem::take(&mut self.touched);
        touched.retain(|&entry| (entry as usize) < fresh);
        touched.sort_unstable();
        touched.dedup();
        let mut i = 0;
        while i < touched.len() {
            let start = touched[i] as usize;
            let mut end = start + 1;
            i += 1;
            while i < touched.len() && touched[i] as usize <= end + GLINT_GAP {
                end = touched[i] as usize + 1;
                i += 1;
            }
            self.write_stars(writer, stars, start, end);
        }
        touched.clear();
        self.touched = touched;
    }

    /// One status line, without numbers, changing at most every 0.7 s so it reads calmly; the
    /// caption's title and its window buttons' cards.
    #[allow(clippy::too_many_arguments)]
    pub fn hud(
        &mut self,
        text: &mut Text,
        state: &State,
        hint: &str,
        status: Option<&str>,
        viewport: [f32; 2],
        time: f32,
        caption: &crate::caption::Caption,
        layout: &mut Layout,
    ) {
        let [w, h] = viewport;
        layout.fixed_text.clear();
        layout.text.clear();
        layout.cards.clear();
        layout.icon = None;
        layout.panel = [0.0, 0.0, w, h];
        layout.clip = [0.0, 0.0, w, h];
        layout.content_height = 0.0;
        layout.text_ends.clear();
        let tiny = w < 300.0 || h < 220.0;
        let narrow = w < 600.0;
        let fixed = &mut layout.fixed_text;
        if caption.height > 0.0 {
            let style = Line {
                role: crate::document::Role::App,
                size: caption.size,
                height: (caption.size * 1.3).round(),
                gap: 0.0,
                color: caption.ink,
                align: 0.0,
            };
            let name = caption.title.clone().unwrap_or_else(|| state.app.clone());
            fixed.extend(crate::caption::title(text, caption, w, &[(name, style)]));
        }
        if !tiny {
            let wanted = match status {
                Some(s) => s.to_string(),
                None => match state.phase {
                    Phase::Resolve => hint.to_string(),
                    Phase::Parse => {
                        let current = state.current.trim();
                        if current.is_empty() {
                            hint.to_string()
                        } else {
                            format!("{hint} {}", cut(current, 40))
                        }
                    }
                    Phase::Typecheck | Phase::Codegen | Phase::Link | Phase::FirstFrame => {
                        let app = caption.title.as_deref().unwrap_or(&state.app);
                        format!("{hint} {app}")
                    }
                },
            };
            if wanted != self.status && (self.status.is_empty() || time - self.status_at >= 0.7) {
                self.status = wanted;
                self.status_at = time;
            }
            let size = if narrow { 11.0 } else { 12.0 };
            let style = Line {
                role: crate::document::Role::Status,
                size,
                height: size * 1.3,
                gap: 0.0,
                color: [INK[0], INK[1], INK[2], 0.72],
                align: 0.5,
            };
            let run = text.shape(&self.status, size, f32::INFINITY, style.height);
            let base = h - if narrow { 28.0 } else { 34.0 };
            fixed.push(TextLine {
                position: [w * 0.5 - run.width * 0.5, base - text.ascender(size)],
                run,
                style,
                fixed: true,
            });
        }
        for (action, rect) in &caption.buttons {
            layout.cards.push(Card {
                rect: *rect,
                action: *action,
                accent: crate::CAPTION_BUTTON,
                members: 0..0,
                fixed: true,
            });
        }
    }
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heavier_function_is_never_a_smaller_star() {
        let sizes: Vec<f32> = (0..=255u8).map(size_of).collect();
        assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "sizes fall somewhere: {sizes:?}");
        assert_eq!((sizes[0], sizes[255]), (SIZE_LIGHT, SIZE_HEAVY));
        // a typical function (100 bytes) is a small point, a 4 KB one a big star (a halo from
        // 2.0), an 8 KB one the heaviest
        assert!(size_of(stats::weight_of(100)) < 0.7, "{}", size_of(stats::weight_of(100)));
        assert!(size_of(stats::weight_of(4 << 10)) > 2.0, "{}", size_of(stats::weight_of(4 << 10)));
        assert_eq!(size_of(stats::weight_of(8 << 10)), SIZE_HEAVY);
    }

    // one test: the install log is a process-wide static
    #[test]
    fn each_function_is_its_own_star() {
        let mut field = Field::new();
        // first installs: a star each, in install order, with its code's weight; the proc-macro
        // world's slot 7 is another function than the program's
        for slot in 0..3000u32 {
            stats::fn_installed(slot, false, 16 + slot as usize);
        }
        stats::fn_installed(7, true, 100);
        assert_eq!(field.ingest(0.0, 1.0), 3001);
        assert_eq!(field.log.len(), 3001);
        for slot in 0..3000 {
            assert_eq!(field.star_of[0][slot], slot as u32 + 1, "slot {slot}'s star");
            assert_eq!(field.log[slot].weight, stats::weight_of(16 + slot), "slot {slot}'s weight");
        }
        assert_eq!(field.star_of[1][7], 3001);
        assert!(field.touched.is_empty(), "a first install glints nothing but its birth");
        // the births are spread over the frame, the last at its end
        assert!(field.log[0].at > 0.0 && field.log[1].at > field.log[0].at && field.log[3000].at == 1.0);

        // installed again, function 1234 glints its own star (and takes its new weight); O3 code
        // turns function 42's star gold (its weight stays: O3 code holds its inlined callees);
        // a function whose first code is O3 is born gold
        stats::fn_installed(1234, false, 4096);
        stats::o3_installed(42, false, 9999);
        stats::o3_installed(5000, false, 64);
        assert_eq!(field.ingest(1.0, 2.0), 1);
        assert_eq!(field.log.len(), 3002);
        assert_eq!(field.touched, [1234, 42]);
        assert_eq!((field.log[1234].twinkle, field.log[1234].weight), (2.0, stats::weight_of(4096)));
        assert_eq!((field.log[42].gold, field.log[42].weight), (2.0, stats::weight_of(16 + 42)));
        assert_eq!(field.log[3001].gold, field.log[3001].at);
        assert_eq!(field.star_of[0][5000], 3002);

        // a new run counts from its own start
        field.reset();
        stats::fn_installed(1234, false, 64);
        assert_eq!(field.ingest(0.0, 0.5), 1);
        assert_eq!((field.log.len(), field.star_of[0][1234]), (1, 1));

        // each star's record has one place in the stars' buffers, kept as more are born: the
        // chunks lie end to end, chunk k holding FIRST_CHUNK << k stars, and they number every
        // star an install count can
        let mut next = (0, 0);
        for entry in 0..FIRST_CHUNK * 15 + 3 {
            let place = chunk_of(entry);
            assert_eq!(place, next, "star {entry}'s place");
            next = if place.1 + 1 == FIRST_CHUNK << place.0 { (place.0 + 1, 0) } else { (place.0, place.1 + 1) };
        }
        assert_eq!(next, (4, 3));
        assert!(chunk_of(u32::MAX as usize).0 < CHUNKS);

        // the bake: stars past BAKE_AGE join it once a quarter second of them waits, at most
        // BAKE_BATCH a frame; the stars after the frontier draw live; the baked stars drawn live as
        // well (big, gold, glinting) are the sparks
        let program = crate::document::Program::compile(&crate::Document::bundled(), makepad_hal::dialect::MSL, 0, 1)
            .unwrap_or_else(|e| panic!("{:?}", e.iter().map(|d| d.message.clone()).collect::<Vec<_>>()));
        let (pipe, stars) = (&program.pipes[crate::FIELD], &program.pipes[crate::STARS]);
        let mut writer = CmdWriter::new(makepad_hal::session::HOST);
        let state = State::default();
        let mut frame = |field: &mut Field, time: f32, w: f32| {
            field.frame(&mut writer, pipe, stars, &state, time, [0.0, 0.0, w, 800.0], false, false, -1.0, 1.0)
        };
        field.reset();
        // 20000 stars, every 100th big (8 KB of code)
        for slot in 0..20_000u32 {
            stats::fn_installed(slot, false, if slot % 100 == 0 { 8192 } else { 64 });
        }
        let live = |f: &Frame| f.live.iter().map(|l| l.2).sum::<u32>();
        let f = frame(&mut field, 1.0, 1280.0);
        assert_eq!((field.baked, f.shown.is_some(), live(&f)), (0, false, 20_000), "none past BAKE_AGE yet");
        let f = frame(&mut field, 4.0, 1280.0);
        assert_eq!((field.baked, f.shown.is_some(), live(&f)), (BAKE_BATCH, true, 20_000 - BAKE_BATCH as u32), "a batch at most a frame");
        let f = frame(&mut field, 4.1, 1280.0);
        assert_eq!((field.baked, live(&f), f.sparks.1), (20_000, 0, 2 * 200), "all baked, the big ones sparks (two records each)");
        let pass = field.take_bake().expect("the frames' bake pass");
        assert!(pass.clear && pass.put_in.is_empty() && pass.take_out.is_empty());
        assert_eq!(pass.runs.iter().map(|r| r.2 as usize).sum::<usize>(), 20_000, "every star put in once");
        // a glint, gold and a new weight on baked stars: gold is taken out of the bake, a new
        // weight baked again, all three drawn live as well; the glint's spark leaves as it ends
        stats::fn_installed(1, false, 64);
        stats::o3_installed(2, false, 64);
        stats::fn_installed(3, false, 8192);
        let f = frame(&mut field, 4.2, 1280.0);
        let pass = field.take_bake().expect("the corrections' pass");
        let record = stars.instances.defaults.len();
        assert!(!pass.clear && pass.runs.is_empty());
        assert_eq!((pass.put_in.len() / record, pass.take_out.len() / record), (1, 2), "star 3 again; stars 2 and 3 out");
        assert_eq!((f.sparks.1, field.log[2].baked, field.log[3].baked), (2 * 203, None, Some(stats::weight_of(8192))));
        let f = frame(&mut field, 4.8, 1280.0);
        assert_eq!(f.sparks.1, 2 * 202, "the glint's spark left, gold and big stay");
        assert!(field.take_bake().is_none(), "no bake work");
        // a new size (a narrower window: a smaller disk) draws every star live until it holds
        // BAKE_SETTLE, then bakes them again
        let f = frame(&mut field, 5.0, 800.0);
        assert_eq!((field.baked, f.shown.is_some(), live(&f), f.sparks.1), (0, false, 20_000, 0));
        let f = frame(&mut field, 5.1, 800.0);
        assert_eq!((field.baked, live(&f)), (0, 20_000), "the size has not held yet");
        let f = frame(&mut field, 5.3, 800.0);
        assert_eq!((field.baked, f.shown.is_some()), (BAKE_BATCH, true));
        assert!(field.take_bake().is_some_and(|p| p.clear), "new layers start clear");
    }
}
