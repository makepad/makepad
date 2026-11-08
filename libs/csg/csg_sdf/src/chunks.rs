//! A field read through a grid of chunks, each its tree reduced to the chunk's box
//! ([`crate::bounds::cut`]): every point's value is the whole tree's, bit for bit, made by only
//! the pieces that can change it there. A chunk is reduced the first time a point reads it, from
//! its block's tree (`BLOCK` chunks a side, the tree reduced to the block's box: a cut of a cut is
//! the tree's); a point outside the grid reads the whole tree. Each knows how steep the field
//! can be over it, outside its solid and inside, at the values within each of a few [`WINDOWS`]
//! of 0 and at any ([`crate::bounds::slopes_within`] of its cut), each the first time it is asked.

use crate::bounds::{cut, meet, slopes_within, Bounds, Cut, Slopes};
use crate::sdf::{Hints, Sdf3};
use crate::shape::{Compiled, SdfNode, P3};
use makepad_csg_math::Vec3d;
use std::sync::{Arc, OnceLock};

/// At most this many chunks to a grid: a bigger box gets bigger chunks.
const MOST: f64 = 16384.0;

/// Chunks a block is a side.
const BLOCK: usize = 4;

/// At most this many chunks a box's slopes read ([`Chunks::slopes_in`]); past them, their blocks'.
const FEW: usize = 64;

/// At most this many chunks a box's emptiness reads ([`Chunks::above`]); past them, their blocks'
/// (a chunk is made to be read: more of them than a few cost more than their blocks' cuts).
const FEW_CUTS: usize = 8;

/// The reaches from 0, in chunk sizes, of the values a chunk keeps its slopes at besides any
/// value's: a ball or a block reads the least that holds its value ([`Chunks::slopes_in`]).
const WINDOWS: [f64; 4] = [1.0 / 64.0, 1.0 / 16.0, 0.25, 1.0];

/// A tree's slopes over a box at the values within each of [`WINDOWS`] (times `size`) of 0, then
/// at any, each made when first asked.
#[derive(Default)]
struct Ladder([OnceLock<Slopes>; WINDOWS.len() + 1]);

impl Ladder {
    /// Rung `i`'s slopes of `n` over `b` (its tree and box).
    fn rung(&self, i: usize, n: &SdfNode, b: &Bounds, size: f64) -> Slopes {
        *self.0[i].get_or_init(|| slopes_within(n, b, WINDOWS.get(i).map_or(f64::INFINITY, |w| w * size)))
    }
}

/// A chunk's tree and what its value can be over the chunk.
pub struct Chunk {
    /// The tree reduced to the chunk: the slots and ids read it ([`crate::distance_slot`]).
    pub node: SdfNode,
    /// The value over the chunk lies in `lo..=hi`.
    pub lo: f64,
    pub hi: f64,
    /// Its box and the grid's chunk size, which its slopes are proven over.
    b: Bounds,
    size: f64,
    /// Its tree compiled, and how steep the field can be over it by the values' reach from 0: each
    /// made on its first read (a chunk its bounds alone answer reads neither), each rung of the
    /// slopes on its own.
    field: OnceLock<Compiled>,
    slopes: Ladder,
}

impl Chunk {
    pub fn distance(&self, p: Vec3d) -> f64 {
        self.field().distance(p)
    }

    fn field(&self) -> &Compiled {
        self.field.get_or_init(|| Compiled::new(&self.node))
    }

    /// Its slopes at rung `rung` of [`WINDOWS`].
    fn slopes(&self, rung: usize) -> Slopes {
        self.slopes.rung(rung, &self.node, &self.b, self.size)
    }
}

/// A block's reduced tree, and its slopes (each rung made on its first read).
struct Block {
    node: SdfNode,
    b: Bounds,
    slopes: Ladder,
}

/// See the module.
pub struct Chunks {
    root: Arc<SdfNode>,
    whole: OnceLock<Compiled>,
    lo: P3,
    size: f64,
    dims: [usize; 3],
    cells: Vec<OnceLock<Box<Chunk>>>,
    blocks: Vec<OnceLock<Block>>,
}

impl Chunks {
    /// The tree `root` over `region`, in chunks of about `size` (more when the box would hold
    /// too many).
    pub fn new(root: Arc<SdfNode>, region: Bounds, size: f64) -> Chunks {
        let ext: P3 = std::array::from_fn(|k| (region.1[k] - region.0[k]).max(0.0));
        let fits = ext.iter().all(|e| e.is_finite()) && size > 0.0;
        let size = if fits { size.max((ext[0] * ext[1] * ext[2] / MOST).cbrt()).max(ext.iter().fold(0.0f64, |a, &e| a.max(e)) / 256.0) } else { 1.0 };
        let dims: [usize; 3] = if fits && size > 0.0 { std::array::from_fn(|k| ((ext[k] / size).ceil() as usize).max(1)) } else { [0; 3] };
        let cells = (0..dims[0] * dims[1] * dims[2]).map(|_| OnceLock::new()).collect();
        let blocks = (0..dims.iter().map(|d| d.div_ceil(BLOCK)).product::<usize>()).map(|_| OnceLock::new()).collect();
        Chunks { root, whole: OnceLock::new(), lo: region.0, size, dims, cells, blocks }
    }

    /// A shape's tree over its box, grown by a margin for the reads just outside it.
    pub fn of_shape(shape: &crate::shape::SdfShape) -> Chunks {
        let ext = (0..3).map(|k| shape.max[k] - shape.min[k]).fold(0.0f64, f64::max);
        let margin = 0.125 * ext;
        let region = (shape.min.map(|x| x - margin), shape.max.map(|x| x + margin));
        let volume = (0..3).map(|k| region.1[k] - region.0[k]).product::<f64>();
        Chunks::new(Arc::new(shape.root.clone()), region, (volume / MOST).cbrt())
    }

    pub fn root(&self) -> &SdfNode {
        &self.root
    }

    /// The chunk holding `p`, None outside the grid.
    pub fn index(&self, p: P3) -> Option<usize> {
        let at = self.cell_of(p)?;
        Some((at[2] * self.dims[1] + at[1]) * self.dims[0] + at[0])
    }

    fn cell_of(&self, p: P3) -> Option<[usize; 3]> {
        let mut at = [0usize; 3];
        for k in 0..3 {
            let f = (p[k] - self.lo[k]) / self.size;
            if !(f >= 0.0 && f < self.dims[k] as f64) {
                return None;
            }
            at[k] = f as usize;
        }
        Some(at)
    }

    /// The cells (or blocks of `per` cells a side) box `b` meets, None where it leaves the grid.
    fn span_of(&self, b: &Bounds, per: usize) -> Option<[std::ops::RangeInclusive<usize>; 3]> {
        let (a, z) = (self.cell_of(b.0)?, self.cell_of(b.1)?);
        Some(std::array::from_fn(|k| a[k] / per..=z[k] / per))
    }

    fn cell_box(&self, at: [usize; 3], per: usize) -> Bounds {
        let lo: P3 = std::array::from_fn(|k| self.lo[k] + (at[k] * per) as f64 * self.size);
        (lo, std::array::from_fn(|k| lo[k] + per as f64 * self.size))
    }

    fn block(&self, at: [usize; 3]) -> &Block {
        let bd: [usize; 3] = std::array::from_fn(|k| self.dims[k].div_ceil(BLOCK));
        self.blocks[(at[2] * bd[1] + at[1]) * bd[0] + at[0]].get_or_init(|| {
            let b = self.cell_box(at, BLOCK);
            Block { node: cut(&self.root, &b).node, b, slopes: Ladder::default() }
        })
    }

    fn block_slopes(&self, at: [usize; 3], rung: usize) -> Slopes {
        let block = self.block(at);
        block.slopes.rung(rung, &block.node, &block.b, self.size)
    }

    /// Chunk `i`, reduced on its first read.
    pub fn chunk(&self, i: usize) -> &Chunk {
        self.cells[i].get_or_init(|| {
            let (x, rest) = (i % self.dims[0], i / self.dims[0]);
            let (y, z) = (rest % self.dims[1], rest / self.dims[1]);
            let block = self.block([x / BLOCK, y / BLOCK, z / BLOCK]);
            let b = self.cell_box([x, y, z], 1);
            let c = cut(&block.node, &b);
            Box::new(Chunk { node: c.node, lo: c.lo, hi: c.hi, b, size: self.size, field: OnceLock::new(), slopes: Ladder::default() })
        })
    }

    fn whole(&self) -> &Compiled {
        self.whole.get_or_init(|| Compiled::new(&self.root))
    }

    /// The node that makes the value at `p`: its chunk's, or the whole tree outside the grid.
    pub fn node_at(&self, p: P3) -> &SdfNode {
        match self.index(p) {
            Some(i) => &self.chunk(i).node,
            None => &self.root,
        }
    }

    /// The value at `p` and the material slot there ([`crate::distance_slot`]).
    pub fn distance_slot(&self, p: Vec3d) -> (f64, u8) {
        crate::shape::distance_slot(self.node_at([p.x, p.y, p.z]), p)
    }

    /// The tree's [`cut`] to box `b`, from the reduced tree of the chunk or block that holds it
    /// (the same cut: each is exact over its box).
    pub fn cut(&self, b: &Bounds) -> Cut {
        match (self.span_of(b, 1), self.span_of(b, BLOCK)) {
            (Some([x, y, z]), _) if x.start() == x.end() && y.start() == y.end() && z.start() == z.end() => {
                cut(&self.chunk((*z.start() * self.dims[1] + *y.start()) * self.dims[0] + *x.start()).node, b)
            }
            (_, Some([x, y, z])) if x.start() == x.end() && y.start() == y.end() && z.start() == z.end() => cut(&self.block([*x.start(), *y.start(), *z.start()]).node, b),
            _ => cut(&self.root, b),
        }
    }

    /// How steep the field can be in box `b`, outside its solid at the values `0..=reach` and
    /// inside at `-reach..=0` (a value `v` that falls to 0 along a line passes only those within
    /// `|v|`): the most of the chunks it meets, or of their blocks where it meets more than [`FEW`]
    /// chunks (a block's hold over all of it), at the least of their [`WINDOWS`] holding `reach`;
    /// infinite where it leaves the grid.
    pub fn slopes_in(&self, b: &Bounds, reach: f64) -> Slopes {
        let (Some(cells), Some(blocks)) = (self.span_of(b, 1), self.span_of(b, BLOCK)) else { return Slopes { outside: f64::INFINITY, inside: f64::INFINITY } };
        let count = |r: &[std::ops::RangeInclusive<usize>; 3]| r.iter().map(|r| r.end() + 1 - r.start()).product::<usize>();
        let rung = WINDOWS.iter().position(|w| reach <= w * self.size).unwrap_or(WINDOWS.len());
        let mut most = Slopes { outside: 0.0, inside: 0.0 };
        let mut take = |s: Slopes| most = Slopes { outside: most.outside.max(s.outside), inside: most.inside.max(s.inside) };
        if count(&cells) <= FEW {
            for z in cells[2].clone() {
                for y in cells[1].clone() {
                    for x in cells[0].clone() {
                        take(self.chunk((z * self.dims[1] + y) * self.dims[0] + x).slopes(rung));
                    }
                }
            }
        } else {
            for z in blocks[2].clone() {
                for y in blocks[1].clone() {
                    for x in blocks[0].clone() {
                        take(self.block_slopes([x, y, z], rung));
                    }
                }
            }
        }
        most
    }

    /// Whether the tree is above 0 all over box `b`, by the value range of each chunk it meets
    /// or else the tree's [`cut`] over the part of `b` in it (of each block where it meets more
    /// than [`FEW_CUTS`] chunks, of the whole tree where it leaves the grid): each cut of a
    /// chunk's or block's tree is exact over its box, and over a part of a box at least as tight.
    pub fn above(&self, b: &Bounds) -> bool {
        let (Some(cells), Some(blocks)) = (self.span_of(b, 1), self.span_of(b, BLOCK)) else { return cut(&self.root, b).lo > 0.0 };
        let count = |r: &[std::ops::RangeInclusive<usize>; 3]| r.iter().map(|r| r.end() + 1 - r.start()).product::<usize>();
        let over = |n: &SdfNode, at: [usize; 3], per: usize| meet(b, &self.cell_box(at, per)).map_or(true, |m| cut(n, &m).lo > 0.0);
        if count(&cells) <= FEW_CUTS {
            cells[2].clone().all(|z| cells[1].clone().all(|y| cells[0].clone().all(|x| {
                let c = self.chunk((z * self.dims[1] + y) * self.dims[0] + x);
                c.lo > 0.0 || over(&c.node, [x, y, z], 1)
            })))
        } else {
            blocks[2].clone().all(|z| blocks[1].clone().all(|y| blocks[0].clone().all(|x| over(&self.block([x, y, z]).node, [x, y, z], BLOCK))))
        }
    }
}

impl Chunks {
    /// The points of `ps` by chunk (`usize::MAX`: outside the grid), each chunk's in order, and
    /// `read` with each chunk's points, their values written back in place.
    fn by_chunk(&self, ps: &[Vec3d], out: &mut [f64], mut read: impl FnMut(usize, &[Vec3d], &mut [f64])) {
        let at: Vec<usize> = ps.iter().map(|p| self.index([p.x, p.y, p.z]).unwrap_or(usize::MAX)).collect();
        if at.iter().all(|&i| i == at[0]) {
            if let Some(&i) = at.first() {
                read(i, ps, out);
            }
            return;
        }
        let mut order: Vec<u32> = (0..ps.len() as u32).collect();
        order.sort_unstable_by_key(|&k| at[k as usize]);
        let (mut q, mut d) = (Vec::with_capacity(ps.len()), Vec::with_capacity(ps.len()));
        let mut s = 0;
        while s < order.len() {
            let i = at[order[s] as usize];
            let e = s + order[s..].iter().take_while(|&&k| at[k as usize] == i).count();
            q.clear();
            q.extend(order[s..e].iter().map(|&k| ps[k as usize]));
            d.resize(q.len(), 0.0);
            read(i, &q, &mut d);
            for (&k, v) in order[s..e].iter().zip(&d) {
                out[k as usize] = *v;
            }
            s = e;
        }
    }

    /// Chunk `i`'s value clamped to `lower..=upper` where its bounds alone decide it.
    fn decided(&self, i: usize, lower: f64, upper: f64) -> Option<f64> {
        let c = self.chunk(i);
        if c.lo >= upper {
            Some(upper)
        } else if c.hi <= lower {
            Some(lower)
        } else {
            None
        }
    }
}

impl Sdf3 for Chunks {
    fn distance(&self, p: Vec3d) -> f64 {
        match self.index([p.x, p.y, p.z]) {
            Some(i) => self.chunk(i).distance(p),
            None => self.whole().distance(p),
        }
    }

    /// A chunk whose bounds are past `lower..=upper` is the bound, unread; else its tree is
    /// read only as far as the bounds need, its hints its own (part `i + 1`; the whole tree's 0).
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        match self.index([p.x, p.y, p.z]) {
            Some(i) => match self.decided(i, lower, upper) {
                Some(v) => v,
                None => self.chunk(i).field().distance_bounded(p, lower, upper, hints.part(i + 1)),
            },
            None => self.whole().distance_bounded(p, lower, upper, hints.part(0)),
        }
    }

    /// As [`Chunks::distance_bounded`], each chunk's points in one call with hints of their own
    /// for the call (a batch brings a chunk's points together, and keeping hints for every chunk
    /// a batch meets would cost more than the batch).
    fn distance_many_bounded(&self, ps: &[Vec3d], lower: f64, upper: f64, out: &mut [f64], hints: &mut Hints) {
        self.by_chunk(ps, out, |i, q, d| match i {
            usize::MAX => self.whole().distance_many_bounded(q, lower, upper, d, hints.part(0)),
            i => match self.decided(i, lower, upper) {
                Some(v) => d.iter_mut().for_each(|d| *d = v),
                None => self.chunk(i).field().distance_many_bounded(q, lower, upper, d, &mut Hints::default()),
            },
        });
    }

    /// The points by chunk, each chunk's in one call.
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        self.by_chunk(ps, out, |i, q, d| match i {
            usize::MAX => self.whole().distance_many(q, d),
            i => self.chunk(i).field().distance_many(q, d),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::tests::{crowd, unit};
    use crate::shape::{SdfKernel, SdfOp};

    /// Twice a ball's distance: a kernel twice as steep as a distance.
    struct Steep;
    impl Sdf3 for Steep {
        fn distance(&self, p: Vec3d) -> f64 {
            2.0 * (p.length() - 4.0)
        }
    }

    /// Chunks read the whole tree's values bit for bit, in the grid and off it, a point at a time
    /// and in batches, and its material slots; a box's slopes are unbounded where a kernel makes the
    /// values and off the grid, more than a distance's over a flat ellipsoid (less so at the values
    /// near 0), a distance's by a ball.
    #[test]
    fn chunks_read_the_trees_values() {
        let kernel = SdfNode::Kernel(SdfKernel { key: 5, field: Arc::new(Steep), splash: None, bounds: Some(([-5.0; 3], [5.0; 3])), solid: None });
        let kernel = SdfNode::Transform { at: [0.0, 30.0, 30.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(kernel) };
        let ball = SdfNode::Transform { at: [33.0, 0.0, 33.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: 4.0 }) };
        let root = SdfNode::Combine { op: SdfOp::Union, blend: 1.0, of: vec![crowd(2, [1.0, 2.0, 3.0]), kernel, ball] };
        let chunks = Chunks::new(Arc::new(root.clone()), ([-40.0, -15.0, -40.0], [40.0, 85.0, 40.0]), 2.0);
        let whole = Compiled::new(&root);
        let mut s = 7u64;
        let ps: Vec<Vec3d> = (0..4000).map(|_| Vec3d::new(-50.0 + 100.0 * unit(&mut s), -25.0 + 120.0 * unit(&mut s), -50.0 + 100.0 * unit(&mut s))).collect();
        let mut many = vec![0.0; ps.len()];
        chunks.distance_many(&ps, &mut many);
        for (p, m) in ps.iter().zip(&many) {
            let want = whole.distance(*p);
            assert_eq!(want.to_bits(), chunks.distance(*p).to_bits(), "{p:?}");
            assert_eq!(want.to_bits(), m.to_bits(), "{p:?} in a batch");
            assert_eq!(crate::shape::distance_slot(&root, *p).1, chunks.distance_slot(*p).1, "{p:?}");
        }
        let slopes = |b: Bounds| chunks.slopes_in(&b, f64::INFINITY);
        assert_eq!(slopes(([-1.0, 29.0, 29.0], [1.0, 31.0, 31.0])).outside, f64::INFINITY, "at the kernel");
        let flat = ([-1.0, 64.5, -1.0], [1.0, 65.5, 1.0]);
        assert!(slopes(flat).outside > 1.0, "over a flat ellipsoid");
        assert!(chunks.slopes_in(&flat, 0.01).outside < slopes(flat).outside, "a flat ellipsoid's estimate less steep near its surface");
        assert_eq!(slopes(([31.0, -1.0, 37.5], [33.0, 1.0, 38.5])), Slopes { outside: 1.0, inside: 1.0 }, "by the ball alone");
        assert_eq!(slopes(([-50.0, 0.0, 0.0], [-45.0, 1.0, 1.0])).outside, f64::INFINITY, "off the grid");
        assert_eq!(slopes(([-40.0, -15.0, -40.0], [39.0, 84.0, 39.0])).outside, f64::INFINITY, "the whole grid, by blocks");
    }
}
