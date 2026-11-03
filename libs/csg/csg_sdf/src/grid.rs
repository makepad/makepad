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
//! (x y z) and its counts (x y z), the buffer's word count, then 0s.

use crate::bounds::Bounds;
use std::sync::Arc;

/// Header words before the samples.
pub const HEADER: usize = 16;

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
    hash: u64,
}

impl SdfGrid {
    /// The whole grid: `data` holds `dims` samples, x fastest.
    pub fn new(origin: [f32; 3], h: f32, dims: [u32; 3], reach: f32, data: Arc<[f32]>) -> SdfGrid {
        assert!(dims.iter().all(|n| *n >= 2), "a grid of at least 2 samples per axis: {dims:?}");
        assert_eq!(data.len(), dims.iter().map(|n| *n as usize).product::<usize>(), "the grid's samples");
        assert!(h > 0.0 && reach >= 0.0);
        let mut g = SdfGrid { origin, h, dims, reach, first: [0; 3], count: dims, data, hash: 0 };
        g.hash = g.content_hash();
        g
    }

    /// The flat buffer a kernel reads: the header, then the samples.
    pub fn words(&self) -> Vec<f32> {
        let mut w = Vec::with_capacity(HEADER + self.data.len());
        w.extend(self.origin);
        w.push(1.0 / self.h);
        w.extend(self.dims.map(|n| n as f32));
        w.extend(self.first.map(|n| n as f32));
        w.extend(self.count.map(|n| n as f32));
        w.push((HEADER + self.data.len()) as f32);
        w.resize(HEADER, 0.0);
        w.extend_from_slice(&self.data);
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
        self.span(a) == self.span(b)
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
