//! A read-only 3D grid of f32 samples that a Splash field kernel reads (a stress field that
//! grades a lattice, a grown field): [`crate::SdfSplashExpr::with_grid`] binds it as the
//! kernel's `sdf_grid` input.
//!
//! The kernel sees one flat f32 buffer: a header of [`HEADER`] words, then the samples, x
//! fastest (`i + nx (j + ny k)`). The header holds the whole grid's frame (the kernel-frame
//! position of sample (0, 0, 0), one over the spacing, its sample counts) and the piece of it
//! the buffer holds (its first sample's index and its counts): a brick's program holds only
//! the samples near the brick ([`SdfGrid::crop`]), and a kernel computes the same grid
//! coordinates from the same header words whichever piece it reads, so a cropped grid gives the
//! same bits as the whole one wherever the crop reaches.
//!
//! Header words: origin x y z, 1 / spacing, sample counts nx ny nz, the piece's first sample
//! (x y z) and its counts (x y z), the buffer's word count, where its table starts (0: none), then
//! a 0.
//!
//! A grid may carry a table ([`SdfGrid::with_table`]): words after the samples that a kernel reads by
//! index, not by place. Such a grid stays whole: a piece would copy the table to save a few samples.

use crate::bounds::Bounds;
use std::collections::BinaryHeap;
use std::sync::Arc;

/// Header words before the samples.
pub const HEADER: usize = 16;

/// How far a grid's read reaches along a direction ([`SdfGrid::solid_side`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidSide {
    /// Nothing solid of the read lies past this (under the ceiling asked).
    pub top: f64,
    /// A solid point the caller took, the highest it was offered.
    pub witness: Option<[f64; 3]>,
}

/// A piece of a cell, `lo..hi` in the cell's own 0..1, by the most its points reach.
#[derive(Clone, Copy)]
struct Piece {
    top: f64,
    cell: [usize; 3],
    lo: [f64; 3],
    hi: [f64; 3],
}

impl PartialEq for Piece {
    fn eq(&self, o: &Self) -> bool {
        self.top == o.top
    }
}
impl Eq for Piece {}
impl PartialOrd for Piece {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Piece {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.top.total_cmp(&o.top)
    }
}

#[derive(Clone)]
pub struct SdfGrid {
    /// The kernel-frame position of sample (0, 0, 0).
    pub origin: [f32; 3],
    /// The sample spacing (kernel units).
    pub h: f32,
    /// The whole grid's sample counts.
    pub dims: [u32; 3],
    /// How far from the point it evaluates a kernel reads the grid (kernel units): a crop keeps
    /// this much round its region, and the samples round that.
    pub reach: f32,
    /// The piece held: its first sample's index in the whole grid and its counts.
    pub first: [u32; 3],
    pub count: [u32; 3],
    /// The piece's samples, x fastest.
    pub data: Arc<[f32]>,
    /// Words after the samples.
    pub table: Arc<[f32]>,
    hash: u64,
}

impl SdfGrid {
    /// The whole grid: `data` holds `dims` samples, x fastest.
    pub fn new(origin: [f32; 3], h: f32, dims: [u32; 3], reach: f32, data: Arc<[f32]>) -> SdfGrid {
        assert!(dims.iter().all(|n| *n >= 2), "a grid of at least 2 samples per axis: {dims:?}");
        assert_eq!(data.len(), dims.iter().map(|n| *n as usize).product::<usize>(), "the grid's samples");
        assert!(h > 0.0 && reach >= 0.0);
        let mut g = SdfGrid { origin, h, dims, reach, first: [0; 3], count: dims, data, table: Arc::from([]), hash: 0 };
        g.hash = g.content_hash();
        g
    }

    /// This grid with `table` after its samples.
    pub fn with_table(self, table: Arc<[f32]>) -> SdfGrid {
        let mut g = SdfGrid { table, ..self };
        g.hash = g.content_hash();
        g
    }

    /// The flat buffer a kernel reads: the header, the samples, the table.
    pub fn words(&self) -> Vec<f32> {
        let mut w = Vec::with_capacity(HEADER + self.data.len() + self.table.len());
        w.extend(self.origin);
        w.push(1.0 / self.h);
        w.extend(self.dims.map(|n| n as f32));
        w.extend(self.first.map(|n| n as f32));
        w.extend(self.count.map(|n| n as f32));
        w.push((HEADER + self.data.len() + self.table.len()) as f32);
        w.push(if self.table.is_empty() { 0.0 } else { (HEADER + self.data.len()) as f32 });
        w.resize(HEADER, 0.0);
        w.extend_from_slice(&self.data);
        w.extend_from_slice(&self.table);
        w
    }

    /// The piece's identity (its frame and samples): equal hashes read equal values.
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// The first and last sample a kernel at points in `region` (kernel frame) reads, per
    /// axis: the region grown by the reach, the cells it touches, clamped to the whole grid.
    fn span(&self, region: &Bounds) -> ([u32; 3], [u32; 3]) {
        let inv = 1.0 / self.h as f64;
        let mut lo = [0u32; 3];
        let mut hi = [0u32; 3];
        for a in 0..3 {
            // a cell's lower sample, and the one above it (a sample of margin for rounding)
            let last = self.dims[a] as f64 - 1.0;
            let u0 = ((region.0[a] - self.reach as f64 - self.origin[a] as f64) * inv).floor() - 1.0;
            let u1 = ((region.1[a] + self.reach as f64 - self.origin[a] as f64) * inv).floor() + 2.0;
            lo[a] = u0.clamp(0.0, last - 1.0) as u32;
            hi[a] = u1.clamp(lo[a] as f64 + 1.0, last) as u32;
        }
        (lo, hi)
    }

    /// The piece that serves points in `region` (kernel frame; from this whole grid or a piece
    /// that covers it).
    pub fn crop(&self, region: &Bounds) -> SdfGrid {
        if !self.table.is_empty() {
            return self.clone();
        }
        let (lo, hi) = self.span(region);
        let lo: [u32; 3] = std::array::from_fn(|a| lo[a].max(self.first[a]));
        let hi: [u32; 3] = std::array::from_fn(|a| hi[a].min(self.first[a] + self.count[a] - 1).max(lo[a]));
        let count: [u32; 3] = std::array::from_fn(|a| hi[a] - lo[a] + 1);
        if lo == self.first && count == self.count {
            return self.clone();
        }
        let (cx, cy) = (self.count[0] as usize, self.count[1] as usize);
        let mut data = Vec::with_capacity(count.iter().map(|n| *n as usize).product());
        for k in lo[2]..=hi[2] {
            for j in lo[1]..=hi[1] {
                let row = ((k - self.first[2]) as usize * cy + (j - self.first[1]) as usize) * cx;
                let x0 = row + (lo[0] - self.first[0]) as usize;
                data.extend_from_slice(&self.data[x0..x0 + count[0] as usize]);
            }
        }
        let mut g = SdfGrid { first: lo, count, data: data.into(), hash: 0, ..self.clone() };
        g.hash = g.content_hash();
        g
    }

    /// Whether points in regions `a` and `b` read the same piece.
    pub fn crops_alike(&self, a: &Bounds, b: &Bounds) -> bool {
        !self.table.is_empty() || self.span(a) == self.span(b)
    }

    /// The box of the solid of this piece's read (where the trilinear value, clamped to the grid's
    /// edge, is at most 0), exact; a side the read carries off the grid's edge (a solid sample on
    /// that edge) is infinite. None: no sample is solid, so the read is over 0 everywhere.
    pub fn solid_box(&self) -> Option<Bounds> {
        let side = |axis: usize, sign: f64| {
            let mut dn = [0.0; 3];
            dn[axis] = sign;
            self.solid_side(dn, f64::INFINITY, self.h as f64 / 1024.0, usize::MAX, &|_, _| false, &mut |_| true).map(|s| s.top)
        };
        let mut b: Bounds = ([0.0; 3], [0.0; 3]);
        for axis in 0..3 {
            b.0[axis] = -side(axis, -1.0)?;
            b.1[axis] = side(axis, 1.0)?;
        }
        Some(b)
    }

    /// How far along unit `dn` the solid of this piece's read reaches, under `ceiling` (what else
    /// limits the solid along `dn`). Nothing solid of the read lies past `top`, an upper bound by the
    /// grid's own numbers: a read is a trilinear value in each cell, so a piece of a cell whose eight
    /// corners are over 0 is over 0 throughout. And along an axis the solid of a piece of a cell
    /// ends on one of the piece's four edges along that axis: at any height the value is bilinear
    /// across the axis, so where it is at most 0 takes in a corner. So a piece shrinks to its edges'
    /// ends in one step, which is exact along an axis of the grid, and any other `dn` splits only
    /// where it still reaches past the best point found. The walk spends at most `budget` pieces; past
    /// them `top` is what it had proven.
    ///
    /// `clear` says a ball holds nothing of what surrounds the read's solid (another operand's cut):
    /// a piece in such a ball is empty for the caller. `witness` is the highest point `accept` took
    /// among those it was offered (the caller's whole solid); `top` is at most `near` past it once the
    /// walk closed, and `NEG_INFINITY` when nothing of the read's solid is under the ceiling.
    /// None: no sample is solid.
    pub fn solid_side(&self, dn: [f64; 3], ceiling: f64, near: f64, budget: usize, clear: &dyn Fn([f64; 3], f64) -> bool, accept: &mut dyn FnMut([f64; 3]) -> bool) -> Option<SolidSide> {
        let n = self.count.map(|c| c as usize);
        let h = self.h as f64;
        let base: [f64; 3] = std::array::from_fn(|a| self.origin[a] as f64 + self.first[a] as f64 * h);
        let dot = |p: [f64; 3]| dn[0] * p[0] + dn[1] * p[1] + dn[2] * p[2];
        // the highest solid sample under the ceiling, and whether any solid sample is on an edge
        // the read is carried past along `dn` (it is clamped: past an edge it holds the edge's value)
        let mut under = f64::NEG_INFINITY;
        let (mut any, mut off_edge) = (false, false);
        let mut sample = 0;
        for k in 0..n[2] {
            for j in 0..n[1] {
                let row = dn[0] * base[0] + dn[1] * (base[1] + j as f64 * h) + dn[2] * (base[2] + k as f64 * h);
                for (i, v) in self.data[sample..sample + n[0]].iter().enumerate() {
                    if *v <= 0.0 {
                        any = true;
                        let z = row + dn[0] * i as f64 * h;
                        if z <= ceiling {
                            under = under.max(z);
                        }
                        let ijk = [i, j, k];
                        off_edge |= (0..3).any(|a| (ijk[a] == 0 && dn[a] < 0.0) || (ijk[a] == n[a] - 1 && dn[a] > 0.0));
                    }
                }
                sample += n[0];
            }
        }
        if !any {
            return None;
        }
        let at = |cell: [usize; 3], t: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| base[a] + h * (cell[a] as f64 + t[a])) };
        let reach = |cell: [usize; 3], lo: [f64; 3], hi: [f64; 3]| -> (f64, f64) {
            let top: [f64; 3] = std::array::from_fn(|a| if dn[a] > 0.0 { hi[a] } else { lo[a] });
            let bottom: [f64; 3] = std::array::from_fn(|a| if dn[a] > 0.0 { lo[a] } else { hi[a] });
            (dot(at(cell, bottom)), dot(at(cell, top)))
        };
        // The cells a piece of the top can be in: those rising past the highest solid sample, and a
        // cell's rise more for a top that sample's neighbours cut away; none wholly over the ceiling.
        let rise = h * (dn[0].abs() + dn[1].abs() + dn[2].abs());
        let start = if under > f64::NEG_INFINITY { under } else { ceiling } - rise;
        let mut heap = BinaryHeap::new();
        for k in 0..n[2].saturating_sub(1) {
            for j in 0..n[1].saturating_sub(1) {
                for i in 0..n[0].saturating_sub(1) {
                    let cell = [i, j, k];
                    let (bottom, top) = reach(cell, [0.0; 3], [1.0; 3]);
                    if top > start && bottom <= ceiling && self.corners(cell, n).iter().any(|v| *v <= 0.0) {
                        heap.push(Piece { top: top.min(ceiling), cell, lo: [0.0; 3], hi: [1.0; 3] });
                    }
                }
            }
        }
        let mut best: Option<(f64, [f64; 3])> = None;
        let goal = |best: &Option<(f64, [f64; 3])>| best.map_or(f64::NEG_INFINITY, |(s, _)| s + near);
        // the highest reach of the pieces the walk let go without showing them empty
        let mut open = f64::NEG_INFINITY;
        let mut spent = 0usize;
        while let Some(mut piece) = heap.pop() {
            if piece.top <= goal(&best) || spent >= budget {
                // the highest piece is down, or the walk is out of pieces: the heap holds no higher one
                open = open.max(piece.top);
                break;
            }
            spent += 1;
            let c = self.corners(piece.cell, n).map(|v| v as f64);
            let read = |t: [f64; 3]| -> f64 {
                let lerp = |a: f64, b: f64, x: f64| a + (b - a) * x;
                let y0 = lerp(lerp(c[0], c[1], t[0]), lerp(c[2], c[3], t[0]), t[1]);
                let y1 = lerp(lerp(c[4], c[5], t[0]), lerp(c[6], c[7], t[0]), t[1]);
                lerp(y0, y1, t[2])
            };
            let corner: [[f64; 3]; 8] = std::array::from_fn(|q| std::array::from_fn(|a| if q >> a & 1 == 1 { piece.hi[a] } else { piece.lo[a] }));
            let values = corner.map(|t| read(t));
            if values.iter().all(|v| *v > 0.0) {
                continue;
            }
            let centre: [f64; 3] = std::array::from_fn(|a| 0.5 * (piece.lo[a] + piece.hi[a]));
            let radius = 0.5 * h * (0..3).map(|a| (piece.hi[a] - piece.lo[a]).powi(2)).sum::<f64>().sqrt();
            if clear(at(piece.cell, centre), radius) {
                continue;
            }
            // Per axis, how far along `dn` the solid of the piece's edges on that axis gets (the
            // piece shrinks to it), and the best point of those edges under the ceiling, just
            // inside (the read is 0 at a root), to offer.
            let mut offer: Option<(f64, [f64; 3])> = None;
            let mut ends = [None; 3];
            for a in 0..3 {
                let up = dn[a] >= 0.0;
                let step = near * 0.125 / (h * dn[a].abs().max(1e-3));
                for q in (0..8).filter(|q| q >> a & 1 == 0) {
                    let (v_lo, v_hi) = (values[q], values[q | 1 << a]);
                    let (top, bottom, t_top, t_bottom) = if up { (v_hi, v_lo, piece.hi[a], piece.lo[a]) } else { (v_lo, v_hi, piece.lo[a], piece.hi[a]) };
                    if top > 0.0 && bottom > 0.0 {
                        continue;
                    }
                    let root = || t_bottom + (t_top - t_bottom) * (bottom / (bottom - top));
                    let (end, other) = if top <= 0.0 { (t_top, if bottom <= 0.0 { t_bottom } else { root() }) } else { (root(), t_bottom) };
                    ends[a] = Some(ends[a].map_or(end, |e: f64| if up { e.max(end) } else { e.min(end) }));
                    let mut t = corner[q];
                    t[a] = end;
                    let z = dot(at(piece.cell, t));
                    // the highest of the edge's solid part the ceiling leaves
                    let under_ceiling = if z <= ceiling {
                        Some(end)
                    } else if dn[a] == 0.0 {
                        None
                    } else {
                        let back = end - (z - ceiling) / (dn[a] * h);
                        (if up { back >= other } else { back <= other }).then_some(back)
                    };
                    if let Some(e) = under_ceiling {
                        t[a] = if up { (e - step).max(other) } else { (e + step).min(other) };
                        let p = at(piece.cell, t);
                        if offer.map_or(true, |(s, _)| dot(p) > s) {
                            offer = Some((dot(p), p));
                        }
                    }
                }
            }
            if let Some((z, p)) = offer {
                if best.map_or(true, |(s, _)| z > s) && accept(p) {
                    best = Some((z, p));
                }
            }
            for a in 0..3 {
                match ends[a] {
                    Some(e) if dn[a] > 0.0 => piece.hi[a] = e,
                    Some(e) if dn[a] < 0.0 => piece.lo[a] = e,
                    _ => {}
                }
            }
            let top = reach(piece.cell, piece.lo, piece.hi).1.min(ceiling);
            // Split where `dn` gains the most; a piece too small to split stays open.
            let gain: [f64; 3] = std::array::from_fn(|a| dn[a].abs() * (piece.hi[a] - piece.lo[a]) * h);
            if top <= goal(&best) || gain[0] + gain[1] + gain[2] <= 0.25 * near {
                open = open.max(top);
                continue;
            }
            let a = (0..3).max_by(|&x, &y| gain[x].total_cmp(&gain[y])).unwrap_or(0);
            let mid = 0.5 * (piece.lo[a] + piece.hi[a]);
            for side in 0..2 {
                let mut p = piece;
                if side == 0 {
                    p.hi[a] = mid;
                } else {
                    p.lo[a] = mid;
                }
                let (bottom, top) = reach(p.cell, p.lo, p.hi);
                if bottom <= ceiling {
                    p.top = top.min(ceiling);
                    heap.push(p);
                }
            }
        }
        let left = heap.peek().map_or(f64::NEG_INFINITY, |p| p.top);
        let found = best.map_or(f64::NEG_INFINITY, |(s, _)| s);
        let top = if off_edge { ceiling } else { open.max(left).max(found).min(ceiling) };
        Some(SolidSide { top, witness: best.map(|b| b.1) })
    }

    /// A cell's eight samples, x fastest.
    fn corners(&self, c: [usize; 3], n: [usize; 3]) -> [f32; 8] {
        let at = |i: usize, j: usize, k: usize| self.data[c[0] + i + n[0] * (c[1] + j + n[1] * (c[2] + k))];
        [at(0, 0, 0), at(1, 0, 0), at(0, 1, 0), at(1, 1, 0), at(0, 0, 1), at(1, 0, 1), at(0, 1, 1), at(1, 1, 1)]
    }

    fn content_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for w in self.words() {
            h = (h ^ w.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
        }
        h
    }
}

impl std::fmt::Debug for SdfGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SdfGrid({:?} of {:?} at {:?}, {:016x})", self.count, self.dims, self.first, self.hash)
    }
}
