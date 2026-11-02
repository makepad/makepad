//! Hand-drawn strokes as a binary log beside a part's script (`vase.strokes`), read by the
//! script as one paint layer (`m.strokes("vase.strokes")`) and indexed on the bake's brick
//! grid, so a point reads only the strokes that can reach its brick and a new stroke dirties
//! only the bricks it passes.
//!
//! Coordinates are the part's model frame (mm), the frame its paint program, its bake and its
//! shape use; the brick grid ([`BRICK`], origin at 0) is the paint program's grid, which the paint
//! bake's bricks nest in (`makepad_paint_field`). Paint strokes colour a part; shape strokes
//! (raise, lower, crease) sculpt its field through [`crate::SdfNode::Sculpt`].
//!
//! # File layout (little-endian, every section 8-byte aligned)
//!
//! ```text
//! Header        80 bytes
//!   magic       [u8; 8]  "FABSTRK1"
//!   version     u32      1
//!   header      u32      80
//!   brick_mm    f32      the index's brick edge
//!   quant_mm    f32      one point unit
//!   bounds      [i32; 6] lo, hi of every point (quant units)
//!   records     u32      strokes in the log
//!   cursor      u32      strokes applied (undo/redo move it; the rest is redo)
//!   palette     u32      swatches
//!   log_bytes   u32
//!   bricks      u32      index bricks
//!   entries     u32      index entries
//!   reserved    [u32; 2]
//! Palette       palette x { rgb: [f32; 3], filament: u32 (0 = none) }
//! Log           records, each 4-aligned:
//!   op u8, slot u8, flags u16, points u32, radius f32, strength f32, start [i32; 3]
//!   (points - 1) x [i16; 3] deltas, padded to 4 bytes
//! Index         bricks x key u64 (sorted), bricks x { first u32, count u32 },
//!               entries x { stroke u32, seg u32, segs u32 }
//! ```
//!
//! A brick's entries are one contiguous slice, by stroke in log order; an entry is a run of a
//! stroke's capsules (`seg .. seg + segs`). A stroke is indexed in every brick its capsules'
//! boxes, grown by its radius and a brick, touch: a brick without an entry has every stroke a
//! brick or more away from all of its points (the same rule as the program's grid).

use crate::shape::P3;
use std::collections::HashSet;
use std::sync::Arc;

/// The index's brick edge (mm): the paint program's grid cell (eight paint samples at 0.4 mm).
pub const BRICK: f64 = 3.2;
/// One stored point unit (mm).
pub const QUANT: f64 = 1.0 / 256.0;
pub const MAGIC: [u8; 8] = *b"FABSTRK1";
pub const VERSION: u32 = 1;
const HEADER: usize = 80;
const RECORD: usize = 28;
const CHUNK: usize = 256;
// A u64 hash has at most thirteen five-bit trie levels.
const TRIE_LEVELS: usize = 13;

#[derive(Clone)]
struct ChunkVec<T: Clone> {
    chunks: Vec<Arc<Vec<T>>>,
    len: usize,
}

impl<T: Clone> Default for ChunkVec<T> {
    fn default() -> Self {
        Self { chunks: Vec::new(), len: 0 }
    }
}

impl<T: Clone> ChunkVec<T> {
    fn push(&mut self, value: T) {
        if self.len % CHUNK == 0 {
            let mut chunk = Vec::with_capacity(CHUNK);
            chunk.push(value);
            self.chunks.push(Arc::new(chunk));
        } else {
            Arc::make_mut(self.chunks.last_mut().unwrap()).push(value);
        }
        self.len += 1;
    }

    fn get(&self, i: usize) -> Option<&T> {
        if i < self.len { Some(&self.chunks[i / CHUNK][i % CHUNK]) } else { None }
    }

    fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i < self.len { Some(&mut Arc::make_mut(&mut self.chunks[i / CHUNK])[i % CHUNK]) } else { None }
    }

    fn last(&self) -> Option<&T> {
        self.len.checked_sub(1).and_then(|i| self.get(i))
    }

    fn len(&self) -> usize {
        self.len
    }

    fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        self.chunks.truncate(len.div_ceil(CHUNK));
        if len % CHUNK != 0 {
            Arc::make_mut(self.chunks.last_mut().unwrap()).truncate(len % CHUNK);
        }
        self.len = len;
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|chunk| chunk.iter())
    }
}

#[derive(Clone, Default)]
struct PMap {
    root: Option<Arc<PNode>>,
    len: usize,
}

#[derive(Clone)]
enum PNode {
    Leaf { key: u64, value: Arc<Vec<Entry>> },
    Branch { bitmap: u32, children: Vec<Arc<PNode>> },
}

fn index_hash(key: u64) -> u64 {
    key.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn index_bit(hash: u64, shift: u32) -> u32 {
    // High bits spread the packed brick coordinates across the root.
    1u32 << ((hash << shift) >> 59)
}

impl PNode {
    fn leaf(key: u64, update: impl FnOnce(&mut Vec<Entry>)) -> Arc<Self> {
        let mut value = Vec::new();
        update(&mut value);
        Arc::new(Self::Leaf { key, value: Arc::new(value) })
    }

    fn join(a: Arc<Self>, ah: u64, b: Arc<Self>, bh: u64, shift: u32) -> Arc<Self> {
        let abit = index_bit(ah, shift);
        let bbit = index_bit(bh, shift);
        let children = if abit == bbit {
            vec![Self::join(a, ah, b, bh, shift + 5)]
        } else if abit < bbit {
            vec![a, b]
        } else {
            vec![b, a]
        };
        Arc::new(Self::Branch { bitmap: abit | bbit, children })
    }

    fn update(node: &mut Arc<Self>, key: u64, hash: u64, shift: u32, update: impl FnOnce(&mut Vec<Entry>)) -> bool {
        match Arc::make_mut(node) {
            Self::Leaf { key: stored, value } if *stored == key => {
                update(Arc::make_mut(value));
                false
            }
            Self::Leaf { key: stored, .. } => {
                let old_hash = index_hash(*stored);
                *node = Self::join(node.clone(), old_hash, Self::leaf(key, update), hash, shift);
                true
            }
            Self::Branch { bitmap, children } => {
                let bit = index_bit(hash, shift);
                let i = (*bitmap & (bit - 1)).count_ones() as usize;
                if *bitmap & bit == 0 {
                    children.insert(i, Self::leaf(key, update));
                    *bitmap |= bit;
                    true
                } else {
                    Self::update(&mut children[i], key, hash, shift + 5, update)
                }
            }
        }
    }

    /// Retain entries at one key, returning whether this node is empty.
    fn retain(node: &mut Arc<Self>, key: u64, hash: u64, shift: u32, len: &mut usize, keep: &mut impl FnMut(&Entry) -> bool) -> bool {
        match Arc::make_mut(node) {
            Self::Leaf { key: stored, value } => {
                if *stored != key {
                    return false;
                }
                Arc::make_mut(value).retain(keep);
                if value.is_empty() {
                    *len -= 1;
                    true
                } else {
                    false
                }
            }
            Self::Branch { bitmap, children } => {
                let bit = index_bit(hash, shift);
                if *bitmap & bit == 0 {
                    return false;
                }
                let i = (*bitmap & (bit - 1)).count_ones() as usize;
                if Self::retain(&mut children[i], key, hash, shift + 5, len, keep) {
                    children.remove(i);
                    *bitmap &= !bit;
                }
                if children.is_empty() {
                    return true;
                }
                // A leaf can move up; a branch must keep its hash-bit level.
                if children.len() == 1 && matches!(children[0].as_ref(), Self::Leaf { .. }) {
                    *node = children[0].clone();
                }
                false
            }
        }
    }
}

impl PMap {
    fn get(&self, key: u64) -> Option<&Arc<Vec<Entry>>> {
        let mut node = self.root.as_deref()?;
        let mut hash = index_hash(key);
        loop {
            match node {
                PNode::Leaf { key: stored, value } => return (*stored == key).then_some(value),
                PNode::Branch { bitmap, children } => {
                    let bit = 1u32 << (hash >> 59);
                    if bitmap & bit == 0 {
                        return None;
                    }
                    node = children[(bitmap & (bit - 1)).count_ones() as usize].as_ref();
                    hash <<= 5;
                }
            }
        }
    }

    fn update(&mut self, key: u64, update: impl FnOnce(&mut Vec<Entry>)) {
        if let Some(root) = &mut self.root {
            self.len += PNode::update(root, key, index_hash(key), 0, update) as usize;
        } else {
            self.root = Some(PNode::leaf(key, update));
            self.len = 1;
        }
    }

    fn retain(&mut self, key: u64, mut keep: impl FnMut(&Entry) -> bool) {
        if let Some(root) = &mut self.root {
            if PNode::retain(root, key, index_hash(key), 0, &mut self.len, &mut keep) {
                self.root = None;
            }
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn iter(&self) -> PMapIter<'_> {
        PMapIter { next: self.root.as_deref(), stack: std::array::from_fn(|_| [].iter()), depth: 0 }
    }
}

struct PMapIter<'a> {
    next: Option<&'a PNode>,
    stack: [std::slice::Iter<'a, Arc<PNode>>; TRIE_LEVELS],
    depth: usize,
}

impl<'a> Iterator for PMapIter<'a> {
    type Item = (u64, &'a Arc<Vec<Entry>>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(node) = self.next.take() {
                match node {
                    PNode::Leaf { key, value } => return Some((*key, value)),
                    PNode::Branch { children, .. } => {
                        self.stack[self.depth] = children.iter();
                        self.depth += 1;
                    }
                }
            }
            while self.depth > 0 {
                if let Some(child) = self.stack[self.depth - 1].next() {
                    self.next = Some(child.as_ref());
                    break;
                }
                self.depth -= 1;
            }
            if self.next.is_none() {
                return None;
            }
        }
    }
}

/// What a stroke does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StrokeOp {
    /// Paint palette entry `slot` (0-based into the file's palette).
    Paint { slot: u8 },
    /// Back to the part's own colour.
    Erase,
    /// Shape: push the surface out / in along its normal.
    Raise,
    Lower,
    /// Shape: even out the surface.
    Smooth,
    /// Shape: press the surface onto the plane at the stroke.
    Flatten,
    /// Shape: a sharp groove.
    Crease,
}

impl StrokeOp {
    fn code(&self) -> (u8, u8) {
        match self {
            StrokeOp::Paint { slot } => (1, *slot),
            StrokeOp::Erase => (2, 0),
            StrokeOp::Raise => (16, 0),
            StrokeOp::Lower => (17, 0),
            StrokeOp::Smooth => (18, 0),
            StrokeOp::Flatten => (19, 0),
            StrokeOp::Crease => (20, 0),
        }
    }
    fn from_code(op: u8, slot: u8) -> Option<StrokeOp> {
        Some(match op {
            1 => StrokeOp::Paint { slot },
            2 => StrokeOp::Erase,
            16 => StrokeOp::Raise,
            17 => StrokeOp::Lower,
            18 => StrokeOp::Smooth,
            19 => StrokeOp::Flatten,
            20 => StrokeOp::Crease,
            _ => return None,
        })
    }
    /// A paint stroke (colour), not a shape one.
    pub fn is_paint(&self) -> bool {
        matches!(self, StrokeOp::Paint { .. } | StrokeOp::Erase)
    }
}

/// What a shape stroke does to the field: its tube (the sharp union of its capsules at
/// `radius`) smooth-unioned (`raise`) or smooth-subtracted over `blend`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tube {
    pub radius: f64,
    pub blend: f64,
    pub raise: bool,
}

impl Stroke {
    /// A shape stroke's tube: a brush of radius `R` and strength `s` raises or lowers the
    /// surface it is drawn on by about `s R / 2`, blended over `R`; a crease cuts a narrow groove.
    pub fn tube(&self) -> Option<Tube> {
        let (r, s) = (self.radius, self.strength.clamp(0.0, 2.0));
        match self.op {
            StrokeOp::Raise => Some(Tube { radius: 0.5 * s * r, blend: r, raise: true }),
            StrokeOp::Lower => Some(Tube { radius: 0.5 * s * r, blend: r, raise: false }),
            StrokeOp::Crease => Some(Tube { radius: 0.3 * s * r, blend: 0.1 * r, raise: false }),
            // smoothing and flattening read the surface around them: not in the field yet
            _ => None,
        }
    }

    /// How far from its centreline a stroke can change anything.
    pub fn reach(&self) -> f64 {
        match self.tube() {
            Some(t) => t.radius + t.blend,
            None => self.radius,
        }
    }
}

/// One palette entry: a linear colour and the filament it asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatch {
    pub rgb: [f32; 3],
    pub filament: Option<u32>,
}

/// One stroke of the log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub op: StrokeOp,
    pub radius: f64,
    pub strength: f64,
    /// The number of points in [`Strokes::stroke_points`].
    pub points: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub stroke: u32,
    pub seg: u32,
    pub segs: u32,
}

/// The log, its points decoded, and its brick index.
#[derive(Clone, Default)]
pub struct Strokes {
    pub palette: Vec<Swatch>,
    strokes: ChunkVec<Stroke>,
    /// Every stroke's points (quantised, in mm).
    points: ChunkVec<Arc<Vec<P3>>>,
    cursor: u32,
    index: PMap,
    /// Per stroke: the hash of the log up to and including it.
    hashes: ChunkVec<u64>,
}

impl std::fmt::Debug for Strokes {
    /// What it paints, not its points: a paint key formats it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Strokes({} of {}, {:016x}, {:?})", self.cursor, self.strokes.len(), self.key(), self.palette)
    }
}

impl PartialEq for Strokes {
    /// The same applied log, the same redo tail's length and the same palette.
    fn eq(&self, other: &Strokes) -> bool {
        self.key() == other.key() && self.cursor == other.cursor && self.strokes.len() == other.strokes.len() && self.palette == other.palette
    }
}

pub fn brick_of(p: P3) -> [i32; 3] {
    std::array::from_fn(|k| (p[k] / BRICK).floor() as i32)
}

fn key(b: [i32; 3]) -> u64 {
    let f = |v: i32| (v as i64 + (1 << 20)) as u64 & ((1 << 21) - 1);
    (f(b[0]) << 42) | (f(b[1]) << 21) | f(b[2])
}

fn unkey(k: u64) -> [i32; 3] {
    let f = |v: u64| ((v & ((1 << 21) - 1)) as i64 - (1 << 20)) as i32;
    [f(k >> 42), f(k >> 21), f(k)]
}

fn box_bricks(lo: P3, hi: P3) -> impl Iterator<Item = u64> {
    let (a, b) = (brick_of(lo.map(|v| v - BRICK)), brick_of(hi.map(|v| v + BRICK)));
    (a[2]..=b[2]).flat_map(move |z| (a[1]..=b[1]).flat_map(move |y| (a[0]..=b[0]).map(move |x| key([x, y, z]))))
}

fn quant(v: f64) -> i32 {
    (v / QUANT).round() as i32
}

pub fn capsule(p: P3, a: P3, b: P3) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let l = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if l > 1e-18 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / l).clamp(0.0, 1.0) } else { 0.0 };
    let q = [ap[0] - ab[0] * t, ap[1] - ab[1] * t, ap[2] - ab[2] * t];
    (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt()
}

/// The lower bound of the distance from `p` to anything a brick's list leaves out: a brick past
/// the brick's faces (the same rule bounds the program's grid).
pub fn floor_of(p: P3) -> f64 {
    let b = brick_of(p);
    let face = (0..3).map(|k| (p[k] - b[k] as f64 * BRICK).min((b[k] + 1) as f64 * BRICK - p[k])).fold(f64::MAX, f64::min);
    BRICK + face.max(0.0)
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3))
}

/// What a paint stroke decides at a point: its palette slot (`None`: erased) and the distance
/// to its edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub paint: Option<Option<u8>>,
    pub dist: f64,
}

impl Strokes {
    pub fn new() -> Strokes {
        Strokes::default()
    }

    /// Strokes applied (the log's cursor).
    pub fn len(&self) -> usize {
        self.cursor as usize
    }
    pub fn is_empty(&self) -> bool {
        self.cursor == 0
    }
    /// Strokes in the log, undone ones included.
    pub fn records(&self) -> usize {
        self.strokes.len()
    }
    pub fn stroke(&self, i: usize) -> &Stroke {
        self.strokes.get(i).unwrap()
    }
    pub fn stroke_points(&self, i: usize) -> &[P3] {
        self.points.get(i).unwrap().as_slice()
    }
    /// The applied strokes.
    pub fn applied(&self) -> impl Iterator<Item = (usize, &Stroke)> {
        self.strokes.iter().take(self.cursor as usize).enumerate()
    }

    /// What the applied log is: equal logs, equal keys.
    pub fn key(&self) -> u64 {
        if self.cursor == 0 { 0x5ee0 } else { *self.hashes.get(self.cursor as usize - 1).unwrap() }
    }

    /// The palette slot for a colour (added when new).
    pub fn slot_for(&mut self, swatch: Swatch) -> u8 {
        match self.palette.iter().position(|s| *s == swatch) {
            Some(i) => i as u8,
            None => {
                self.palette.push(swatch);
                (self.palette.len() - 1) as u8
            }
        }
    }

    /// The finest applied paint stroke's radius.
    pub fn finest_paint(&self) -> Option<f64> {
        self.applied().filter(|(_, s)| s.op.is_paint()).map(|(_, s)| s.radius).fold(None, |a, r| Some(a.map_or(r, |a: f64| a.min(r))))
    }

    /// Quantise a polyline: points on the unit lattice, a segment longer than an i16 delta
    /// split along itself.
    fn quantise(points: &[P3]) -> Vec<[i32; 3]> {
        let mut out: Vec<[i32; 3]> = Vec::with_capacity(points.len());
        for p in points {
            let q = p.map(quant);
            if let Some(last) = out.last().copied() {
                let span = (0..3).map(|k| (q[k] - last[k]).abs()).max().unwrap_or(0);
                let n = ((span + i16::MAX as i32 - 1) / i16::MAX as i32).max(1);
                for s in 1..n {
                    out.push(std::array::from_fn(|k| last[k] + ((q[k] - last[k]) as i64 * s as i64 / n as i64) as i32));
                }
            }
            out.push(q);
        }
        out
    }

    /// Start a stroke (redo past the cursor is dropped). Returns its id.
    pub fn begin(&mut self, op: StrokeOp, radius: f64, strength: f64) -> usize {
        self.truncate();
        // as the file keeps them (f32): a reloaded log has the same key
        let (radius, strength) = (radius as f32 as f64, strength as f32 as f64);
        self.strokes.push(Stroke { op, radius, strength, points: 0 });
        self.points.push(Arc::new(Vec::new()));
        self.hashes.push(self.hashes.last().copied().unwrap_or(0xcbf2_9ce4_8422_2325));
        self.cursor = self.strokes.len() as u32;
        self.strokes.len() - 1
    }

    /// Points added to the last stroke (model mm). Returns the boxes (lo, hi) of the capsules
    /// it adds, grown by the radius: what they can change.
    pub fn extend(&mut self, points: &[P3]) -> Vec<(P3, P3)> {
        let Some(si) = self.strokes.len().checked_sub(1) else { return Vec::new() };
        let stroke = *self.stroke(si);
        let prev = self.stroke_points(si).last().copied();
        let mut all: Vec<P3> = prev.into_iter().collect();
        all.extend_from_slice(points);
        let q = Self::quantise(&all);
        let skip = prev.is_some() as usize;
        let new: Vec<P3> = q[skip..].iter().map(|v| v.map(|x| x as f64 * QUANT)).collect();
        let mut h = *self.hashes.get(si).unwrap();
        if stroke.points == 0 {
            let (op, slot) = stroke.op.code();
            h = fnv(h, &[op, slot]);
            h = fnv(h, &stroke.radius.to_bits().to_le_bytes());
            h = fnv(h, &stroke.strength.to_bits().to_le_bytes());
        }
        for v in &q[skip..] {
            for c in v {
                h = fnv(h, &c.to_le_bytes());
            }
        }
        *self.hashes.get_mut(si).unwrap() = h;
        let start = stroke.points as usize;
        Arc::make_mut(self.points.get_mut(si).unwrap()).extend(new);
        self.strokes.get_mut(si).unwrap().points += (q.len() - skip) as u32;
        let n = self.stroke(si).points as usize;
        // the capsules the new points close (a lone first point: a sphere)
        let from = start.saturating_sub(1);
        let segs: Vec<u32> = if n == 1 { vec![0] } else { (from..n - 1).map(|s| s as u32).collect() };
        let mut boxes = Vec::with_capacity(segs.len());
        let reach = stroke.reach();
        for s in segs {
            let (lo, hi) = self.seg_box(si, s as usize, reach);
            boxes.push((lo, hi));
            for k in box_bricks(lo, hi) {
                self.index.update(k, |list| match list.last_mut() {
                    Some(e) if e.stroke == si as u32 && e.seg + e.segs >= s => e.segs = e.segs.max(s + 1 - e.seg),
                    _ => list.push(Entry { stroke: si as u32, seg: s, segs: 1 }),
                });
            }
        }
        boxes
    }

    /// Capsule `s` of stroke `si`, grown by `r`.
    fn seg_box(&self, si: usize, s: usize, r: f64) -> (P3, P3) {
        let pts = self.stroke_points(si);
        let a = pts[s.min(pts.len() - 1)];
        let b = pts[(s + 1).min(pts.len() - 1)];
        (std::array::from_fn(|k| a[k].min(b[k]) - r), std::array::from_fn(|k| a[k].max(b[k]) + r))
    }

    /// A whole stroke appended. Returns its capsules' boxes.
    pub fn push(&mut self, op: StrokeOp, radius: f64, strength: f64, points: &[P3]) -> Vec<(P3, P3)> {
        self.begin(op, radius, strength);
        self.extend(points)
    }

    /// The capsule boxes (grown by the radius) of strokes `range`: what moving the cursor
    /// across them changes.
    pub fn boxes(&self, range: std::ops::Range<usize>) -> Vec<(P3, P3)> {
        let mut out = Vec::new();
        for si in range {
            let n = self.stroke(si).points as usize;
            for s in 0..n.saturating_sub(1).max(1) {
                out.push(self.seg_box(si, s, self.stroke(si).reach()));
            }
        }
        out
    }

    /// The capsule boxes that differ between two logs of one file: the strokes past their
    /// common applied prefix in either (an appended stroke, an undo, a redo).
    /// Only the paint strokes (`paint`) or only the shape strokes: a sculpt leaves the paint
    /// as it is and a paint stroke the shape.
    pub fn changed_boxes(&self, other: &Strokes, paint: bool) -> Vec<(P3, P3)> {
        let n = self.cursor.min(other.cursor) as usize;
        let common = (0..n).take_while(|&k| self.hashes.get(k) == other.hashes.get(k)).count();
        let mut out = Vec::new();
        for (log, end) in [(self, self.cursor as usize), (other, other.cursor as usize)] {
            for si in common..end {
                if log.stroke(si).op.is_paint() == paint {
                    out.extend(log.boxes(si..si + 1));
                }
            }
        }
        out
    }

    /// A key of the applied shape strokes alone (what the sculpted field depends on): painting
    /// a sculpted part keeps its field's key.
    pub fn shape_key(&self) -> u64 {
        let mut h = 0x5ee0_5ca1_u64;
        for (si, s) in self.applied() {
            if s.tube().is_none() {
                continue;
            }
            let (op, slot) = s.op.code();
            h = fnv(h, &[op, slot]);
            h = fnv(h, &s.radius.to_bits().to_le_bytes());
            h = fnv(h, &s.strength.to_bits().to_le_bytes());
            for p in self.stroke_points(si) {
                for c in p {
                    h = fnv(h, &c.to_bits().to_le_bytes());
                }
            }
        }
        h
    }

    /// Move the cursor (undo: back, redo: forward), within the log.
    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.strokes.len()) as u32;
    }

    /// Drop the undone strokes (a new stroke after an undo).
    fn truncate(&mut self) {
        let c = self.cursor as usize;
        if c == self.strokes.len() {
            return;
        }
        let mut bricks = HashSet::new();
        for si in c..self.strokes.len() {
            let stroke = self.stroke(si);
            if stroke.points == 0 {
                continue;
            }
            for s in 0..(stroke.points as usize).saturating_sub(1).max(1) {
                let (lo, hi) = self.seg_box(si, s, stroke.reach());
                bricks.extend(box_bricks(lo, hi));
            }
        }
        for k in bricks {
            self.index.retain(k, |e| (e.stroke as usize) < c);
        }
        self.strokes.truncate(c);
        self.hashes.truncate(c);
        self.points.truncate(c);
    }

    /// The paint strokes at `p` (model mm), the last applied one holding it deciding: what it
    /// paints and the distance to the nearest edge at or above it; with no stroke holding it,
    /// the distance to the nearest edge (at least the brick floor).
    pub fn sample(&self, p: P3) -> Hit {
        let mut dist = floor_of(p);
        let Some(list) = self.index.get(key(brick_of(p))) else { return Hit { paint: None, dist } };
        let mut i = list.len();
        while i > 0 {
            let stroke = list[i - 1].stroke;
            let mut from = i - 1;
            while from > 0 && list[from - 1].stroke == stroke {
                from -= 1;
            }
            let runs = &list[from..i];
            i = from;
            if stroke >= self.cursor {
                continue;
            }
            let s = self.stroke(stroke as usize);
            if !s.op.is_paint() {
                continue;
            }
            let pts = self.stroke_points(stroke as usize);
            let mut d = f64::MAX;
            for e in runs {
                for seg in e.seg..e.seg + e.segs {
                    let seg = seg as usize;
                    d = d.min(capsule(p, pts[seg.min(pts.len() - 1)], pts[(seg + 1).min(pts.len() - 1)]));
                }
            }
            let d = d - s.radius;
            dist = dist.min(d.abs());
            if d <= 0.0 {
                let paint = match s.op {
                    StrokeOp::Paint { slot } => Some(Some(slot)),
                    _ => Some(None),
                };
                return Hit { paint, dist };
            }
        }
        Hit { paint: None, dist }
    }

    /// Whether any applied stroke changes the shape.
    pub fn has_shape(&self) -> bool {
        self.applied().any(|(_, s)| s.tube().is_some())
    }

    /// The tubes' boxes (grown by their reach) of the applied shape strokes.
    pub fn shape_bounds(&self) -> Option<(P3, P3)> {
        let mut out: Option<(P3, P3)> = None;
        for (si, s) in self.applied() {
            if s.tube().is_none() {
                continue;
            }
            for (lo, hi) in self.boxes(si..si + 1) {
                out = Some(match out {
                    None => (lo, hi),
                    Some((a, b)) => (std::array::from_fn(|k| a[k].min(lo[k])), std::array::from_fn(|k| b[k].max(hi[k]))),
                });
            }
            let _ = s;
        }
        out
    }

    /// The field `d` (of the shape under the strokes, at `p`) with the applied shape strokes
    /// indexed to `p`'s brick applied in order (`smin` is the field's smooth minimum).
    /// Strokes a brick or more away bound it by the brick floor.
    pub fn sculpt(&self, p: P3, mut d: f64, smin: fn(f64, f64, f64) -> f64) -> f64 {
        let list = self.index.get(key(brick_of(p))).map_or(&[][..], |v| v.as_slice());
        let mut i = 0;
        while i < list.len() {
            let stroke = list[i].stroke;
            let mut to = i + 1;
            while to < list.len() && list[to].stroke == stroke {
                to += 1;
            }
            let runs = &list[i..to];
            i = to;
            if stroke >= self.cursor {
                continue;
            }
            let s = self.stroke(stroke as usize);
            let Some(t) = s.tube() else { continue };
            let pts = self.stroke_points(stroke as usize);
            let mut c = f64::MAX;
            for e in runs {
                for seg in e.seg..e.seg + e.segs {
                    let seg = seg as usize;
                    c = c.min(capsule(p, pts[seg.min(pts.len() - 1)], pts[(seg + 1).min(pts.len() - 1)]));
                }
            }
            let tube = c - t.radius;
            d = if t.raise { smin(d, tube, t.blend) } else { -smin(-d, tube, t.blend) };
        }
        if self.applied().any(|(_, s)| s.tube().is_some_and(|t| t.raise)) {
            d = d.min(floor_of(p));
        }
        d
    }

    /// The applied shape strokes that reach the box `(lo, hi)`, in log order, each with its
    /// capsules there (point pairs): what a field culled to the box keeps.
    #[allow(clippy::type_complexity)]
    pub fn shape_near(&self, lo: P3, hi: P3) -> Vec<(Tube, Vec<(P3, P3)>)> {
        let (a, b) = (brick_of(lo), brick_of(hi));
        let mut segs: std::collections::BTreeMap<u32, std::collections::BTreeSet<u32>> = Default::default();
        // in f64: an unbounded box's brick span overflows any integer
        let cells = (0..3).map(|k| (b[k] as f64 - a[k] as f64 + 1.0).max(0.0)).product::<f64>();
        if cells > self.index.len() as f64 {
            // a box past the strokes: every brick of the index
            for (k, list) in self.index.iter() {
                let c = unkey(k);
                if (0..3).all(|j| c[j] >= a[j] && c[j] <= b[j]) {
                    for e in list.iter() {
                        segs.entry(e.stroke).or_default().extend(e.seg..e.seg + e.segs);
                    }
                }
            }
        } else {
            for z in a[2]..=b[2] {
                for y in a[1]..=b[1] {
                    for x in a[0]..=b[0] {
                        for e in self.index.get(key([x, y, z])).map_or(&[][..], |v| v.as_slice()) {
                            segs.entry(e.stroke).or_default().extend(e.seg..e.seg + e.segs);
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (stroke, set) in segs {
            if stroke >= self.cursor {
                continue;
            }
            let s = self.stroke(stroke as usize);
            let Some(t) = s.tube() else { continue };
            let pts = self.stroke_points(stroke as usize);
            let caps = set.into_iter().map(|seg| (pts[(seg as usize).min(pts.len() - 1)], pts[(seg as usize + 1).min(pts.len() - 1)])).collect();
            out.push((t, caps));
        }
        out
    }

    /// The strokes indexed in the brick holding `p` (applied ones, in log order, each once).
    pub fn near(&self, p: P3) -> impl Iterator<Item = usize> + '_ {
        let list = self.index.get(key(brick_of(p))).map_or(&[][..], |v| v.as_slice());
        let cursor = self.cursor;
        list.iter().enumerate().filter(move |(k, e)| e.stroke < cursor && (*k == 0 || list[k - 1].stroke != e.stroke)).map(|(_, e)| e.stroke as usize)
    }

    /// The file's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut log: Vec<u8> = Vec::new();
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        for (si, s) in self.strokes.iter().enumerate() {
            let pts: Vec<[i32; 3]> = self.stroke_points(si).iter().map(|p| p.map(quant)).collect();
            for q in &pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(q[k]);
                    hi[k] = hi[k].max(q[k]);
                }
            }
            let (op, slot) = s.op.code();
            log.extend_from_slice(&[op, slot, 0, 0]);
            log.extend_from_slice(&s.points.to_le_bytes());
            log.extend_from_slice(&(s.radius as f32).to_le_bytes());
            log.extend_from_slice(&(s.strength as f32).to_le_bytes());
            let start = pts.first().copied().unwrap_or([0; 3]);
            for c in start {
                log.extend_from_slice(&c.to_le_bytes());
            }
            for w in pts.windows(2) {
                for k in 0..3 {
                    log.extend_from_slice(&((w[1][k] - w[0][k]) as i16).to_le_bytes());
                }
            }
            while log.len() % 4 != 0 {
                log.push(0);
            }
        }
        while log.len() % 8 != 0 {
            log.push(0);
        }
        if self.strokes.len() == 0 {
            (lo, hi) = ([0; 3], [0; 3]);
        }
        let mut bricks: Vec<_> = self.index.iter().collect();
        bricks.sort_unstable_by_key(|(k, _)| *k);
        let entries: usize = bricks.iter().map(|(_, list)| list.len()).sum();
        let mut out = Vec::with_capacity(HEADER + self.palette.len() * 16 + log.len() + bricks.len() * 16 + entries * 12);
        out.extend_from_slice(&MAGIC);
        for v in [VERSION, HEADER as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(BRICK as f32).to_le_bytes());
        out.extend_from_slice(&(QUANT as f32).to_le_bytes());
        for c in lo.iter().chain(&hi) {
            out.extend_from_slice(&c.to_le_bytes());
        }
        for v in [self.strokes.len() as u32, self.cursor, self.palette.len() as u32, log.len() as u32, bricks.len() as u32, entries as u32, 0, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        debug_assert_eq!(out.len(), HEADER);
        for s in &self.palette {
            for c in s.rgb {
                out.extend_from_slice(&c.to_le_bytes());
            }
            out.extend_from_slice(&s.filament.unwrap_or(0).to_le_bytes());
        }
        out.extend_from_slice(&log);
        for (k, _) in &bricks {
            out.extend_from_slice(&k.to_le_bytes());
        }
        let mut first = 0u32;
        for (_, list) in &bricks {
            let n = list.len() as u32;
            out.extend_from_slice(&first.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
            first += n;
        }
        for (_, list) in &bricks {
            for e in list.iter() {
                for v in [e.stroke, e.seg, e.segs] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        out
    }

    /// A log from its file's bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Strokes, String> {
        let u32_at = |o: usize| -> Result<u32, String> { bytes.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| "the strokes file is cut short".to_string()) };
        let f32_at = |o: usize| u32_at(o).map(f32::from_bits);
        if bytes.len() < HEADER || bytes[..8] != MAGIC {
            return Err("not a strokes file".into());
        }
        if u32_at(8)? != VERSION {
            return Err(format!("strokes file version {} (this Fab reads {VERSION})", u32_at(8)?));
        }
        let header = u32_at(12)? as usize;
        let quant_mm = f32_at(20)? as f64;
        let records = u32_at(48)? as usize;
        let cursor = u32_at(52)?;
        let palette_n = u32_at(56)? as usize;
        let log_bytes = u32_at(60)? as usize;
        let mut s = Strokes::new();
        let mut o = header;
        for _ in 0..palette_n {
            let filament = u32_at(o + 12)?;
            s.palette.push(Swatch { rgb: [f32_at(o)?, f32_at(o + 4)?, f32_at(o + 8)?], filament: (filament != 0).then_some(filament) });
            o += 16;
        }
        let end = o + log_bytes;
        if end > bytes.len() {
            return Err("the strokes file is cut short".into());
        }
        for _ in 0..records {
            if o + RECORD > end {
                return Err("the strokes file's log is cut short".into());
            }
            let op = StrokeOp::from_code(bytes[o], bytes[o + 1]).ok_or_else(|| format!("unknown stroke op {}", bytes[o]))?;
            let n = u32_at(o + 4)? as usize;
            let radius = f32_at(o + 8)? as f64;
            let strength = f32_at(o + 12)? as f64;
            let mut q = [u32_at(o + 16)? as i32, u32_at(o + 20)? as i32, u32_at(o + 24)? as i32];
            o += RECORD;
            if n == 0 || o + (n - 1) * 6 > end {
                return Err("a stroke's points are cut short".into());
            }
            let mut pts = Vec::with_capacity(n);
            pts.push(q.map(|v| v as f64 * quant_mm));
            for k in 0..n - 1 {
                let at = o + k * 6;
                for c in 0..3 {
                    q[c] += i16::from_le_bytes([bytes[at + c * 2], bytes[at + c * 2 + 1]]) as i32;
                }
                pts.push(q.map(|v| v as f64 * quant_mm));
            }
            o += ((n - 1) * 6).div_ceil(4) * 4;
            s.push(op, radius, strength, &pts);
        }
        s.cursor = cursor.min(records as u32);
        Ok(s)
    }

    /// Every index brick (for tools that walk them).
    pub fn bricks(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.index.iter().map(|(k, _)| unkey(k))
    }
}
