//! A sculpt layer: strokes baked into a sparse grid of distance bricks.
//!
//! The grid is the layer frame's lattice `g * cell`, stored in bricks of
//! 8^3 samples (brick `b` holds samples `8b .. 8b+7` on each axis). Values
//! are clamped to `+-band`. A sample in no brick holds the base field there
//! (the program below the layer, clamped), so the layer's field is defined
//! everywhere and equals the base wherever no stroke reached.
//!
//! The stroke list is the truth; the bricks are a cache of it:
//!
//! - [`Layer::bake`] replays every stroke from the base.
//! - [`Layer::rebake`] re-bakes only what an edit can change (a stroke's
//!   radius or blend edited, a stroke deleted or inserted). The changed
//!   stroke's bricks are propagated forward through later strokes that read
//!   their neighbours (smooth, pinch, grab), then the bricks needed to
//!   recompute those are collected backwards; the result is bit-identical to
//!   a full bake.
//! - [`Layer::live_begin`] / [`Layer::live_update`] / [`Layer::live_end`]
//!   stream a stroke while it is drawn: each update re-bakes only the bricks
//!   the changed tail of the path reaches, from a checkpoint of the bricks
//!   as they were before the stroke. Ending it leaves exactly what a full
//!   bake with the stroke appended gives.
//!
//! This is the CPU reference of the brick engine's bake: the GPU bake must
//! agree with it (same f32 formulas, same order, same Jacobi passes).

use crate::math::*;
use crate::stroke::{Mode, Stroke};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub const BRICK: i32 = 8;
pub const BRICK_SAMPLES: usize = (BRICK * BRICK * BRICK) as usize;

pub type Key = [i32; 3];
/// One brick: 8^3 distances and their materials.
#[derive(Clone)]
pub struct Brick {
    pub d: [f32; BRICK_SAMPLES],
    pub m: [u8; BRICK_SAMPLES],
}
pub type Block = Box<Brick>;
pub type KeySet = BTreeSet<Key>;
/// The field below the layer: distance and material.
pub type Base = Arc<dyn Fn(V3) -> (f32, u8) + Send + Sync>;

#[inline]
fn local_index(l: [i32; 3]) -> usize {
    (l[0] + BRICK * (l[1] + BRICK * l[2])) as usize
}

#[inline]
fn split(g: [i32; 3]) -> (Key, usize) {
    let k = [g[0].div_euclid(BRICK), g[1].div_euclid(BRICK), g[2].div_euclid(BRICK)];
    let l = [g[0].rem_euclid(BRICK), g[1].rem_euclid(BRICK), g[2].rem_euclid(BRICK)];
    (k, local_index(l))
}

/// A brick map with a loader for bricks it does not hold yet.
struct Grid {
    map: HashMap<Key, Block>,
}

impl Grid {
    fn new() -> Grid {
        Grid { map: HashMap::new() }
    }
    fn load(&mut self, keys: &KeySet, loader: &dyn Fn(Key) -> Block) {
        for k in keys {
            if !self.map.contains_key(k) {
                let b = loader(*k);
                self.map.insert(*k, b);
            }
        }
    }
    #[inline]
    fn at_material(&self, g: [i32; 3]) -> (f32, u8) {
        let (k, i) = split(g);
        match self.map.get(&k) {
            Some(b) => (b.d[i], b.m[i]),
            None => panic!("brick {k:?} read but not loaded (the dilation missed it)"),
        }
    }
}

/// The bricks whose samples can fall in `b`.
pub fn keys_in(b: &Aabb, cell: f32) -> KeySet {
    let mut out = KeySet::new();
    if b.is_empty() {
        return out;
    }
    let mut lo = [0i32; 3];
    let mut hi = [0i32; 3];
    for a in 0..3 {
        let glo = (b.lo[a] / cell).ceil() as i32;
        let ghi = (b.hi[a] / cell).floor() as i32;
        if glo > ghi {
            return out;
        }
        lo[a] = glo.div_euclid(BRICK);
        hi[a] = ghi.div_euclid(BRICK);
    }
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                out.insert([x, y, z]);
            }
        }
    }
    out
}

pub fn dilate(s: &KeySet, r: i32) -> KeySet {
    if r <= 0 {
        return s.clone();
    }
    let mut out = KeySet::new();
    for k in s {
        for z in -r..=r {
            for y in -r..=r {
                for x in -r..=r {
                    out.insert([k[0] + x, k[1] + y, k[2] + z]);
                }
            }
        }
    }
    out
}

/// Bricks a stroke can read around a brick it writes (its reach plus the
/// trilinear footprint).
pub fn reach_bricks(s: &Stroke, cell: f32) -> i32 {
    if !s.mode.reads_neighbours() {
        return 0;
    }
    ((s.reach(cell) + cell) / (BRICK as f32 * cell)).ceil() as i32
}

/// Applies one stroke to the bricks `set` of `grid`: the kernel's passes
/// ([`crate::kernel::sample`]), Jacobi (every read sees the state before the
/// pass). Bricks the stroke reads are loaded first through `loader`.
fn apply_on(grid: &mut Grid, s: &Stroke, set: &KeySet, cell: f32, band: f32, loader: &dyn Fn(Key) -> Block) {
    if set.is_empty() {
        return;
    }
    let rb = reach_bricks(s, cell);
    grid.load(&dilate(set, rb), loader);
    for _ in 0..crate::kernel::passes(s, cell) {
        let mut out: Vec<(Key, Block)> = Vec::with_capacity(set.len());
        {
            let src = |g: [i32; 3]| grid.at_material(g);
            for k in set {
                let mut nb = grid.map[k].clone();
                for z in 0..BRICK {
                    for y in 0..BRICK {
                        for x in 0..BRICK {
                            let g = [k[0] * BRICK + x, k[1] * BRICK + y, k[2] * BRICK + z];
                            let i = local_index([x, y, z]);
                            let (d, m) = crate::kernel::sample(s, cell, band, g, &src);
                            nb.d[i] = d;
                            nb.m[i] = m;
                        }
                    }
                }
                out.push((*k, nb));
            }
        }
        for (k, b) in out {
            grid.map.insert(k, b);
        }
    }
}

/// A stroke being drawn.
pub struct Live {
    pub stroke: Stroke,
    /// the bricks as they were before the stroke, for those it has written
    checkpoint: HashMap<Key, Block>,
    /// bricks re-baked by the last update
    pub last_touched: usize,
}

pub struct Layer {
    pub cell: f32,
    pub band: f32,
    /// symmetry: every stroke is also applied mirrored across `axis = 0`
    pub mirror: Option<usize>,
    base: Base,
    bricks: HashMap<Key, Block>,
}

impl Layer {
    pub fn new(base: Base, cell: f32, band: f32, mirror: Option<usize>) -> Layer {
        Layer { cell, band, mirror, base, bricks: HashMap::new() }
    }

    pub fn keys(&self) -> KeySet {
        self.bricks.keys().copied().collect()
    }

    pub fn brick_count(&self) -> usize {
        self.bricks.len()
    }

    /// Bricks with a sample inside the band: the near-surface set the
    /// engine keeps in its atlas (the others only need their sign).
    pub fn surface_brick_count(&self) -> usize {
        self.bricks.values().filter(|b| b.d.iter().any(|v| v.abs() < self.band)).count()
    }

    fn base_block(base: &Base, k: Key, cell: f32, band: f32) -> Block {
        let mut b: Block = Box::new(Brick { d: [0.0; BRICK_SAMPLES], m: [0; BRICK_SAMPLES] });
        for z in 0..BRICK {
            for y in 0..BRICK {
                for x in 0..BRICK {
                    let p = [
                        ((k[0] * BRICK + x) as f32) * cell,
                        ((k[1] * BRICK + y) as f32) * cell,
                        ((k[2] * BRICK + z) as f32) * cell,
                    ];
                    let (d, m) = base(p);
                    b.d[local_index([x, y, z])] = d.clamp(-band, band);
                    b.m[local_index([x, y, z])] = m;
                }
            }
        }
        b
    }

    /// The strokes as applied: each followed by its mirror image when the
    /// layer is symmetric.
    pub fn effective(&self, strokes: &[Stroke]) -> Vec<Stroke> {
        let mut out = Vec::with_capacity(strokes.len() * 2);
        for s in strokes {
            out.push(s.clone());
            if let Some(a) = self.mirror {
                out.push(s.mirrored(a));
            }
        }
        out
    }

    fn per_user(&self) -> usize {
        if self.mirror.is_some() {
            2
        } else {
            1
        }
    }

    fn touched(&self, s: &Stroke) -> KeySet {
        keys_in(&s.bounds(self.band), self.cell)
    }

    /// Replays every stroke from the base.
    pub fn bake(&mut self, strokes: &[Stroke]) {
        let mut grid = Grid::new();
        let (base, cell, band) = (self.base.clone(), self.cell, self.band);
        let loader = move |k: Key| Layer::base_block(&base, k, cell, band);
        for s in self.effective(strokes) {
            let t = self.touched(&s);
            apply_on(&mut grid, &s, &t, cell, band, &loader);
        }
        self.bricks = grid.map;
    }

    /// Re-bakes after an edit of the stroke list. `strokes` is the new list;
    /// strokes before `first` are unchanged; `changed` are the strokes whose
    /// influence changed: the old and the new version of an edited stroke,
    /// a deleted one, an inserted one. Returns the number of bricks written.
    pub fn rebake(&mut self, strokes: &[Stroke], first: usize, changed: &[Stroke]) -> usize {
        let eff = self.effective(strokes);
        let k0 = (first * self.per_user()).min(eff.len());
        let mut dirty = KeySet::new();
        for s in changed {
            for e in self.effective(std::slice::from_ref(s)) {
                dirty.extend(self.touched(&e));
            }
        }
        let touched: Vec<KeySet> = eff.iter().map(|s| self.touched(s)).collect();
        // forward: what later neighbour-reading strokes spread the change to
        for j in k0..eff.len() {
            let rb = reach_bricks(&eff[j], self.cell);
            if rb > 0 {
                let grown = dilate(&dirty, rb);
                for k in grown.intersection(&touched[j]) {
                    dirty.insert(*k);
                }
            }
        }
        // backward: the bricks needed after each stroke to recompute `dirty`
        let mut need_after: Vec<KeySet> = vec![KeySet::new(); eff.len()];
        let mut need = dirty.clone();
        for j in (0..eff.len()).rev() {
            need_after[j] = need.clone();
            let rb = reach_bricks(&eff[j], self.cell);
            if rb > 0 {
                let w: KeySet = need.intersection(&touched[j]).copied().collect();
                need.extend(dilate(&w, rb));
            }
        }
        // replay on a scratch grid holding exactly `need` (from the base)
        let (base, cell, band) = (self.base.clone(), self.cell, self.band);
        let mut grid = Grid::new();
        grid.load(&need, &|k| Layer::base_block(&base, k, cell, band));
        let strict = |k: Key| -> Block { panic!("re-bake read brick {k:?} outside its need set") };
        for j in 0..eff.len() {
            let set: KeySet = need_after[j].intersection(&touched[j]).copied().collect();
            apply_on(&mut grid, &eff[j], &set, cell, band, &strict);
        }
        for k in &dirty {
            let b = grid.map.remove(k).unwrap();
            self.bricks.insert(*k, b);
        }
        dirty.len()
    }

    /// The layer's field at a grid sample.
    pub fn at(&self, g: [i32; 3]) -> f32 {
        self.at_material(g).0
    }

    /// Distance and material at a grid sample.
    pub fn at_material(&self, g: [i32; 3]) -> (f32, u8) {
        let (k, i) = split(g);
        match self.bricks.get(&k) {
            Some(b) => (b.d[i], b.m[i]),
            None => {
                let p = [g[0] as f32 * self.cell, g[1] as f32 * self.cell, g[2] as f32 * self.cell];
                let (d, m) = (self.base)(p);
                (d.clamp(-self.band, self.band), m)
            }
        }
    }

    /// The shaded surface values at any point: the corner samples'
    /// materials mixed by the trilinear weights (what the renderer reads).
    pub fn surface(&self, x: V3, table: &crate::material::MaterialTable) -> crate::material::Surface {
        let u = scale(x, 1.0 / self.cell);
        let f = [u[0].floor(), u[1].floor(), u[2].floor()];
        let g = [f[0] as i32, f[1] as i32, f[2] as i32];
        let t = [u[0] - f[0], u[1] - f[1], u[2] - f[2]];
        let mut c = [0u8; 8];
        for (i, slot) in c.iter_mut().enumerate() {
            *slot = self.at_material([g[0] + (i & 1) as i32, g[1] + ((i >> 1) & 1) as i32, g[2] + ((i >> 2) & 1) as i32]).1;
        }
        crate::material::blend(table, c, t)
    }

    /// The material at any point: the nearest sample's (what the shader and
    /// the multi-material slicer read).
    pub fn material(&self, x: V3) -> u8 {
        self.at_material([nearest(x[0] / self.cell), nearest(x[1] / self.cell), nearest(x[2] / self.cell)]).1
    }

    /// The layer's field at any point (trilinear over the grid): what the
    /// renderer and the slicer read.
    pub fn sample(&self, x: V3) -> f32 {
        let u = scale(x, 1.0 / self.cell);
        let f = [u[0].floor(), u[1].floor(), u[2].floor()];
        let g = [f[0] as i32, f[1] as i32, f[2] as i32];
        let t = [u[0] - f[0], u[1] - f[1], u[2] - f[2]];
        let s = |dx: i32, dy: i32, dz: i32| self.at([g[0] + dx, g[1] + dy, g[2] + dz]);
        let x00 = mix(s(0, 0, 0), s(1, 0, 0), t[0]);
        let x10 = mix(s(0, 1, 0), s(1, 1, 0), t[0]);
        let x01 = mix(s(0, 0, 1), s(1, 0, 1), t[0]);
        let x11 = mix(s(0, 1, 1), s(1, 1, 1), t[0]);
        mix(mix(x00, x10, t[1]), mix(x01, x11, t[1]), t[2])
    }

    // ---- live strokes -------------------------------------------------------

    pub fn live_begin(&mut self, stroke: Stroke) -> Live {
        let mut live = Live { stroke: Stroke { path: Vec::new(), ..stroke.clone() }, checkpoint: HashMap::new(), last_touched: 0 };
        self.live_update(&mut live, stroke.path);
        live
    }

    /// The stroke's path is now `path` (the recorder appends points and
    /// moves its provisional tip). Re-bakes the bricks the changed tail can
    /// reach, newest path wins.
    pub fn live_update(&mut self, live: &mut Live, path: Vec<[f32; 4]>) {
        let old = std::mem::replace(&mut live.stroke.path, path);
        let new = &live.stroke.path;
        let common = old.iter().zip(new.iter()).take_while(|(a, b)| a == b).count();
        let from = common.saturating_sub(1);
        let mut changed_bounds = Aabb::EMPTY;
        for (p, _) in [(&old, 0), (new, 1)] {
            if p.len() > from {
                let tail = Stroke { path: p[from..].to_vec(), ..live.stroke.clone() };
                changed_bounds = changed_bounds.union(&tail.bounds(self.band));
            }
        }
        let eff = self.effective(std::slice::from_ref(&live.stroke));
        let mut q = keys_in(&changed_bounds, self.cell);
        let rb = reach_bricks(&eff[0], self.cell);
        let margin = if let Some(a) = self.mirror {
            // the mirror twin reads the stroke's result: what changed can
            // reach the twin's output within its reach, on both sides
            let mut b = changed_bounds;
            b.lo[a] = -changed_bounds.hi[a];
            b.hi[a] = -changed_bounds.lo[a];
            q.extend(keys_in(&b, self.cell));
            q = dilate(&q, rb);
            2 * rb
        } else if eff[0].mode == Mode::Smooth {
            rb
        } else {
            0
        };
        // Jacobi passes are wrong at the working set's edge (they read the
        // pre-stroke state there): recompute a margin around `q`, keep `q`
        let work = dilate(&q, margin);
        let (base, cell, band) = (self.base.clone(), self.cell, self.band);
        let mut grid = Grid::new();
        {
            let checkpoint = &live.checkpoint;
            let bricks = &self.bricks;
            let pre = |k: Key| -> Block {
                if let Some(b) = checkpoint.get(&k) {
                    return b.clone();
                }
                if let Some(b) = bricks.get(&k) {
                    return b.clone();
                }
                Layer::base_block(&base, k, cell, band)
            };
            grid.load(&work, &pre);
            for e in &eff {
                let t: KeySet = keys_in(&e.bounds(band), cell).intersection(&work).copied().collect();
                apply_on(&mut grid, e, &t, cell, band, &pre);
            }
        }
        for k in &q {
            if !live.checkpoint.contains_key(k) {
                let pre = match self.bricks.get(k) {
                    Some(b) => b.clone(),
                    None => Layer::base_block(&self.base, *k, cell, band),
                };
                live.checkpoint.insert(*k, pre);
            }
            self.bricks.insert(*k, grid.map.remove(k).unwrap());
        }
        live.last_touched = q.len();
    }

    /// Ends the stroke: the bricks already hold it. Returns the stroke to
    /// append to the program (one span edit, one undo step).
    pub fn live_end(&mut self, live: Live) -> Stroke {
        live.stroke
    }

    /// Drops the stroke: the checkpoint goes back.
    pub fn live_cancel(&mut self, live: Live) {
        for (k, b) in live.checkpoint {
            self.bricks.insert(k, b);
        }
    }
}

/// What a sculpt layer is, as a value: the language's `Sculpt{..}` lowered.
/// Its [`SculptSpec::key`] is the content key the node's canonical encoding
/// carries (the base field's key is the source node's, kept by the caller).
#[derive(Clone, Debug, PartialEq)]
pub struct SculptSpec {
    pub cell: f32,
    pub band: f32,
    pub mirror: Option<usize>,
    pub strokes: Vec<Stroke>,
}

impl SculptSpec {
    /// FNV-1a 64 over the exact bits of every field and every parameter
    /// block word (`kernel::pack`): equal keys = bit-identical bakes over
    /// the same base.
    pub fn key(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |x: u32| {
            for b in x.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(self.cell.to_bits());
        eat(self.band.to_bits());
        eat(self.mirror.map(|a| a as u32 + 1).unwrap_or(0));
        eat(self.strokes.len() as u32);
        for s in &self.strokes {
            let w = crate::kernel::pack(s, self.cell, self.band);
            eat(w.len() as u32);
            for x in w {
                eat(x.to_bits());
            }
        }
        h
    }

    /// A layer over `base`, baked.
    pub fn bake(&self, base: Base) -> Layer {
        let mut l = Layer::new(base, self.cell, self.band, self.mirror);
        l.bake(&self.strokes);
        l
    }
}
