//! The brick volume of one object: which bricks of its lattice hold surface,
//! the program each one bakes from, and where each one lives in the atlas.
//!
//! Lattice (csg_sculpt's): sample `g` sits at `g * cell` in the object frame;
//! brick `b` holds samples `8b .. 8b + 7` per axis (no shared borders).
//! Values are clamped to `±band`. A brick is allocated when the distance at
//! the centre of its region (its samples and the gap to the next brick's
//! first) is within the region's half diagonal plus the band (the field is
//! 1-Lipschitz, so no sample of an unallocated brick lies inside the band).
//!
//! Work is per block of 4^3 bricks: one culled program decides a block that
//! the surface does not come near (all its bricks empty, one signed skip
//! distance), otherwise each brick is culled, lowered and tested. A brick's
//! program is its node culled to its box grown by the band; equal programs
//! are stored once. Blocks are classified in parallel, then applied in
//! lattice order so program ids and atlas slots stay stable. Re-baking after
//! an edit redoes the blocks the edit's changed region touches, the same
//! computation as a full build, so a dirty re-bake equals a full bake.

use crate::program::{lower, LowerError, Program};
use crate::twin;
use makepad_csg_math::thread_pool::parallel_map;
use makepad_csg_sdf::{changed, cull, node_bounds, Bounds, SdfNode};
use std::collections::HashMap;
use std::sync::Arc;

/// Samples per brick edge.
pub const BS: i32 = 8;
/// Bricks per block edge.
pub const BLOCK: i32 = 4;
/// Truth-atlas slots per row: a slot is 64 x 8 texels (8 z slices of 8 x 8).
pub const ATLAS_SLOTS_X: usize = 64;
pub const ATLAS_W: usize = ATLAS_SLOTS_X * 64;

/// What one brick of the volume is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Brick {
    /// Holds surface: its atlas slot and its program.
    Baked { slot: u32, program: u32 },
    /// No sample within the band: a signed lower bound of the distance over it
    /// (negative inside the solid).
    Empty { skip: f32 },
}

/// One object's brick volume.
pub struct Volume {
    pub cell: f32,
    pub band: f32,
    /// The bricks the volume covers: `min .. min + dim` per axis.
    pub min: [i32; 3],
    pub dim: [i32; 3],
    pub bricks: Vec<Brick>,
    /// Distinct programs; `Brick::Baked::program` indexes them.
    pub programs: Vec<Program>,
    program_ids: HashMap<Vec<u32>, u32>,
    free_slots: Vec<u32>,
    pub slot_count: u32,
    /// Bricks whose texels must be (re)baked.
    pub dirty: Vec<usize>,
    /// Bricks a CPU bake must fill (a program the GPU cannot run).
    pub cpu_only: Vec<usize>,
    /// Bricks a build classified again (baked or not): their neighbours'
    /// render-copy aprons may have changed.
    pub touched: Vec<usize>,
}

/// Build statistics.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildStats {
    pub blocks: usize,
    pub blocks_skipped: usize,
    pub bricks_tested: usize,
    pub bricks_baked: usize,
    pub programs: usize,
    pub ms: f64,
}

enum BrickResult {
    Empty { skip: f32 },
    Bake { program: Program, gpu: bool },
}

struct BlockResult {
    block_skip: Option<f64>,
    // Keep brick errors in order so release and touched still precede them.
    bricks: Vec<(usize, Result<BrickResult, LowerError>)>,
}

fn grow(b: Bounds, r: f64) -> Bounds {
    (b.0.map(|x| x - r), b.1.map(|x| x + r))
}

impl Volume {
    /// An empty volume over `bounds` (the object's solid) at `cell`.
    pub fn new(bounds: Bounds, cell: f32, band: f32) -> Volume {
        let reach = band as f64 + cell as f64;
        let lo = bounds.0.map(|x| ((x - reach) / cell as f64 / BS as f64).floor() as i32);
        let hi = bounds.1.map(|x| ((x + reach) / cell as f64 / BS as f64).floor() as i32);
        // whole blocks
        let dim: [i32; 3] = std::array::from_fn(|i| ((hi[i] - lo[i] + 1 + BLOCK - 1) / BLOCK) * BLOCK);
        let n = (dim[0] * dim[1] * dim[2]) as usize;
        Volume {
            cell,
            band,
            min: lo,
            dim,
            bricks: vec![Brick::Empty { skip: f32::MAX }; n],
            programs: Vec::new(),
            program_ids: HashMap::new(),
            free_slots: Vec::new(),
            slot_count: 0,
            dirty: Vec::new(),
            cpu_only: Vec::new(),
            touched: Vec::new(),
        }
    }

    pub fn index(&self, b: [i32; 3]) -> usize {
        let l = [b[0] - self.min[0], b[1] - self.min[1], b[2] - self.min[2]];
        ((l[2] * self.dim[1] + l[1]) * self.dim[0] + l[0]) as usize
    }

    pub fn brick_of(&self, i: usize) -> [i32; 3] {
        let i = i as i32;
        let (x, y, z) = (i % self.dim[0], (i / self.dim[0]) % self.dim[1], i / (self.dim[0] * self.dim[1]));
        [x + self.min[0], y + self.min[1], z + self.min[2]]
    }

    /// The region brick `b` answers for: its samples and the gap to the next
    /// brick's first sample, `[8b, 8b + 8]` cells (object frame).
    pub fn brick_region(&self, b: [i32; 3]) -> Bounds {
        let c = self.cell as f64;
        (b.map(|v| (v * BS) as f64 * c), b.map(|v| (v * BS + BS) as f64 * c))
    }

    /// The box of brick `b`'s samples (object frame).
    pub fn brick_box(&self, b: [i32; 3]) -> Bounds {
        let c = self.cell as f64;
        (b.map(|v| (v * BS) as f64 * c), b.map(|v| (v * BS + BS - 1) as f64 * c))
    }

    fn program_id(&mut self, p: Program) -> u32 {
        let key: Vec<u32> = p.texels.iter().flat_map(|t| t.map(f32::to_bits)).collect();
        if let Some(id) = self.program_ids.get(&key) {
            return *id;
        }
        let id = self.programs.len() as u32;
        self.programs.push(p);
        self.program_ids.insert(key, id);
        id
    }

    fn alloc_slot(&mut self) -> u32 {
        self.free_slots.pop().unwrap_or_else(|| {
            self.slot_count += 1;
            self.slot_count - 1
        })
    }

    fn release(&mut self, i: usize) {
        if let Brick::Baked { slot, .. } = self.bricks[i] {
            self.free_slots.push(slot);
        }
    }

    /// Classifies and programs every brick of the blocks overlapping `region`
    /// (everything when `None`), queueing the baked ones for (re)baking.
    pub fn build(&mut self, root: &SdfNode, region: Option<Bounds>) -> Result<BuildStats, LowerError> {
        let t0 = std::time::Instant::now();
        let mut st = BuildStats::default();
        let band = self.band as f64;
        let c = self.cell as f64;
        let min = self.min;
        let dim = self.dim;
        let brick_region = move |b: [i32; 3]| {
            (b.map(|v| (v * BS) as f64 * c), b.map(|v| (v * BS + BS) as f64 * c))
        };
        // A brick's region is what the march looks it up for: [8b, 8b + 8) cells
        // (its samples and the gap to the next brick's first); the tests cover it whole.
        let half_diag = move |n: f64| 0.5 * n * BS as f64 * c * 3f64.sqrt();
        let nb: [i32; 3] = std::array::from_fn(|i| dim[i] / BLOCK);
        let mut blocks: Vec<[i32; 3]> = Vec::new();
        for bz in 0..nb[2] {
            for by in 0..nb[1] {
                for bx in 0..nb[0] {
                    let b0 = [min[0] + bx * BLOCK, min[1] + by * BLOCK, min[2] + bz * BLOCK];
                    let bb = (brick_region(b0).0, brick_region([b0[0] + BLOCK - 1, b0[1] + BLOCK - 1, b0[2] + BLOCK - 1]).1);
                    if let Some(r) = &region {
                        let g = grow(*r, band + c);
                        if (0..3).any(|i| bb.1[i] < g.0[i] || bb.0[i] > g.1[i]) {
                            continue;
                        }
                    }
                    blocks.push(b0);
                }
            }
        }
        let root = Arc::new(root.clone());
        let results = parallel_map(&blocks, move |chunk| {
            chunk
                .iter()
                .map(|&b0| -> Result<BlockResult, LowerError> {
                    let bb = (brick_region(b0).0, brick_region([b0[0] + BLOCK - 1, b0[1] + BLOCK - 1, b0[2] + BLOCK - 1]).1);
                    // the block as a whole: one test at its centre
                    let rb = half_diag(BLOCK as f64);
                    let centre: [f64; 3] = std::array::from_fn(|i| 0.5 * (bb.0[i] + bb.1[i]));
                    // A culled program is exact only up to the cull's margin (band + c)
                    // around the region: what it dropped can be that near, so a skip
                    // distance never goes past the margin.
                    let margin = band + c;
                    let skip_of = |d: f64, r: f64| if d < 0.0 { -(-d - r).min(margin) } else { (d - r).min(margin) };
                    let block_skip = match cull(&root, &grow(bb, margin)) {
                        None => Some(margin),
                        Some(n) => {
                            let p = lower(&n)?;
                            let (d, _) = twin::eval(&p.texels, centre.map(|v| v as f32));
                            let d = d as f64;
                            (d.abs() > rb + band).then(|| skip_of(d, rb))
                        }
                    };
                    let mut bricks = Vec::with_capacity((BLOCK * BLOCK * BLOCK) as usize);
                    for z in 0..BLOCK {
                        for y in 0..BLOCK {
                            for x in 0..BLOCK {
                                let b = [b0[0] + x, b0[1] + y, b0[2] + z];
                                let l = [b[0] - min[0], b[1] - min[1], b[2] - min[2]];
                                let i = ((l[2] * dim[1] + l[1]) * dim[0] + l[0]) as usize;
                                if let Some(skip) = block_skip {
                                    bricks.push((i, Ok(BrickResult::Empty { skip: skip as f32 })));
                                    continue;
                                }
                                let bx = brick_region(b);
                                let r = half_diag(1.0);
                                let ctr: [f64; 3] = std::array::from_fn(|k| 0.5 * (bx.0[k] + bx.1[k]));
                                let brick = match cull(&root, &grow(bx, band + c)) {
                                    None => Ok(BrickResult::Empty { skip: margin as f32 }),
                                    Some(n) => lower(&n).map(|p| {
                                        let (d, _) = twin::eval(&p.texels, ctr.map(|v| v as f32));
                                        let d = d as f64;
                                        if d.abs() > r + band {
                                            BrickResult::Empty { skip: skip_of(d, r) as f32 }
                                        } else {
                                            let gpu = p.fits_gpu();
                                            BrickResult::Bake { program: p, gpu }
                                        }
                                    }),
                                };
                                bricks.push((i, brick));
                            }
                        }
                    }
                    Ok(BlockResult { block_skip, bricks })
                })
                .collect()
        });
        // Apply in lattice order: slot reuse and program ids depend on it.
        for result in results {
            st.blocks += 1;
            let block = result?;
            for (i, brick) in block.bricks {
                self.release(i);
                self.touched.push(i);
                if block.block_skip.is_none() {
                    st.bricks_tested += 1;
                }
                self.bricks[i] = match brick? {
                    BrickResult::Empty { skip } => Brick::Empty { skip },
                    BrickResult::Bake { program: p, gpu } => {
                        let program = self.program_id(p);
                        let slot = self.alloc_slot();
                        st.bricks_baked += 1;
                        if gpu {
                            self.dirty.push(i);
                        } else {
                            self.cpu_only.push(i);
                        }
                        Brick::Baked { slot, program }
                    }
                };
            }
            if block.block_skip.is_some() {
                st.blocks_skipped += 1;
            }
        }
        st.programs = self.programs.len();
        st.ms = t0.elapsed().as_secs_f64() * 1e3;
        Ok(st)
    }

    /// Whether the volume's bricks cover `bounds` grown by the band.
    pub fn covers(&self, bounds: &Bounds) -> bool {
        let lo = self.brick_box(self.min).0;
        let hi = self.brick_box([self.min[0] + self.dim[0] - 1, self.min[1] + self.dim[1] - 1, self.min[2] + self.dim[2] - 1]).1;
        let g = grow(*bounds, self.band as f64 + self.cell as f64);
        (0..3).all(|i| g.0[i] >= lo[i] && g.1[i] <= hi[i])
    }

    /// Re-builds after `old` became `new`: the blocks the change can reach.
    /// `Ok(None)` when the new solid leaves the volume (the caller makes a new
    /// one).
    pub fn edit(&mut self, old: &SdfNode, new: &SdfNode) -> Result<Option<BuildStats>, LowerError> {
        if !node_bounds(new).is_some_and(|b| self.covers(&b)) {
            return Ok(None);
        }
        match changed(old, new) {
            None => Ok(Some(BuildStats::default())),
            Some(r) => self.build(new, Some(r)).map(Some),
        }
    }

    /// The index the GPU reads per brick (x + dim.x (y + dim.y z)): its slot
    /// (-1 when empty) and, for an empty brick, its signed skip distance.
    pub fn index_texels(&self) -> Vec<[f32; 4]> {
        self.bricks
            .iter()
            .map(|b| match *b {
                Brick::Baked { slot, .. } => [slot as f32, 0.0, 0.0, 0.0],
                Brick::Empty { skip } => [-1.0, skip, 0.0, 0.0],
            })
            .collect()
    }

    /// The 26 neighbours of brick `i` inside the volume, and itself.
    pub fn around(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let b = self.brick_of(i);
        (0..27).filter_map(move |k| {
            let n = [b[0] + k % 3 - 1, b[1] + (k / 3) % 3 - 1, b[2] + k / 9 - 1];
            let inside = (0..3).all(|a| n[a] >= self.min[a] && n[a] < self.min[a] + self.dim[a]);
            inside.then(|| self.index(n))
        })
    }

    /// The baked value of every sample of brick `i` on the CPU twin (64 x 8
    /// slot texels, z slices along x), and its material slots.
    pub fn bake_brick_cpu(&self, i: usize) -> Option<(Vec<f32>, Vec<u8>)> {
        let Brick::Baked { program, .. } = self.bricks[i] else { return None };
        let b = self.brick_of(i);
        let p = &self.programs[program as usize].texels;
        let mut d = vec![0.0f32; 512];
        let mut s = vec![0u8; 512];
        for y in 0..8 {
            for tx in 0..64 {
                let (z, x) = (tx / 8, tx % 8);
                let g = [b[0] * BS + x, b[1] * BS + y, b[2] * BS + z];
                let q = g.map(|v| v as f32 * self.cell);
                let (v, slot) = twin::eval(p, q);
                d[(y * 64 + tx) as usize] = v.max(-self.band).min(self.band) + 0.0;
                s[(y * 64 + tx) as usize] = slot;
            }
        }
        Some((d, s))
    }

    /// The node's bounds (the volume a new object needs).
    pub fn bounds_of(root: &SdfNode) -> Option<Bounds> {
        node_bounds(root)
    }
}
