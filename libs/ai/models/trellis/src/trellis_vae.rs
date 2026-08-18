//! TRELLIS sparse-structure decoder (ss_dec_conv3d from TRELLIS-image-large):
//! a dense conv3d decoder 16^3 x8 latent -> 64^3 occupancy logits. Layout:
//! input conv3d(8->512) -> 2 middle ResBlocks(512) -> [2x ResBlock(512),
//! upsample(512->128) via conv3d + pixel_shuffle_3d, 2x ResBlock(128),
//! upsample(128->32), 2x ResBlock(32)] -> ChannelLayerNorm + silu +
//! conv3d(32->1).
//!
//! Every conv3d runs as 27 neighbor-gather + f32 gemm passes (the trellis.cpp
//! proof: zero new conv ops); pixel shuffle is one column-block gather with
//! the preceding conv's output channels permuted to block-major at load.
//! Weights decode to resident f32 (~300MB) — exactness first; the reference
//! runs these blocks in f16.
//!
//! Grid convention: voxel rows in lexicographic (x, y, z) order, z fastest —
//! matching torch's (B, C, X, Y, Z) spatial flattening. Channel columns per
//! row.

use crate::backend::{
    gpu_add, gpu_download, gpu_gather_rows_colblock, gpu_layer_norm_mul_add,
    gpu_linear_f32_resident, gpu_silu, gpu_upload, gpu_upload_u32, GpuTensor,
};
use crate::trellis::TrellisWeights;
use crate::{DiffusionError, Result};

pub const SS_DEC_LATENT: usize = 8;
pub const SS_DEC_CHANNELS: [usize; 3] = [512, 128, 32];
pub const SS_DEC_NORM_EPS: f32 = 1e-5;

/// One conv3d(3x3x3, pad 1) as 27 resident (Co, Ci) f32 weight parts + bias.
struct Conv3d {
    offsets: Vec<GpuTensor>, // 27 (Co, Ci) matrices, kernel index (kd*3+kh)*3+kw
    bias: GpuTensor,
    out_channels: usize,
}

impl Conv3d {
    /// `permute_shuffle`: reorder output channels from torch's c_*8 + code to
    /// code*C_ + c_ (block-major) so the following pixel shuffle is a single
    /// column-block gather.
    fn load(
        weights: &TrellisWeights,
        name: &str,
        co: usize,
        ci: usize,
        permute_shuffle: bool,
    ) -> Result<Self> {
        let w = weights.tensor_f32(&format!("{name}.weight"))?;
        if w.len() != co * ci * 27 {
            return Err(DiffusionError::model(format!(
                "conv3d '{name}' expected {co}x{ci}x27 weights, got {}",
                w.len()
            )));
        }
        let mut b = weights.tensor_f32(&format!("{name}.bias"))?;
        let out_row = |row: usize| -> usize {
            if permute_shuffle {
                // torch channel = c_*8 + code  ->  block-major col code*C_ + c_
                let c_ = row / 8;
                let code = row % 8;
                code * (co / 8) + c_
            } else {
                row
            }
        };
        if permute_shuffle {
            let orig = b.clone();
            for (row, value) in orig.iter().enumerate() {
                b[out_row(row)] = *value;
            }
        }
        let mut offsets = Vec::with_capacity(27);
        for k in 0..27 {
            // torch weight layout (Co, Ci, kd, kh, kw).
            let mut part = vec![0.0f32; co * ci];
            for row in 0..co {
                let dst = out_row(row);
                for col in 0..ci {
                    part[dst * ci + col] = w[(row * ci + col) * 27 + k];
                }
            }
            offsets.push(gpu_upload(&part, co, ci).map_err(DiffusionError::model)?);
        }
        Ok(Self {
            offsets,
            bias: gpu_upload(&b, 1, co).map_err(DiffusionError::model)?,
            out_channels: co,
        })
    }

    /// x: (V, Ci) on the grid `grid`; returns (V, Co).
    fn forward(&self, x: &GpuTensor, grid: &DenseGrid) -> Result<GpuTensor> {
        let mut acc: Option<GpuTensor> = None;
        for k in 0..27 {
            let gathered = gpu_gather_rows_colblock(x, &grid.neighbors[k], None, x.cols())
                .map_err(DiffusionError::model)?;
            let bias = if k == 13 { Some(&self.bias) } else { None }; // center tap carries the bias
            let y = gpu_linear_f32_resident(&gathered, &self.offsets[k], bias)
                .map_err(DiffusionError::model)?;
            acc = Some(match acc {
                None => y,
                Some(a) => gpu_add(&a, &y).map_err(DiffusionError::model)?,
            });
        }
        let _ = self.out_channels;
        acc.ok_or_else(|| DiffusionError::model("conv3d produced no output"))
    }
}

/// Precomputed neighbor index buffers for one cubic resolution.
pub struct DenseGrid {
    pub res: usize,
    /// 27 buffers of V u32: linear neighbor index or u32::MAX outside.
    neighbors: Vec<GpuTensor>,
    /// Pixel-shuffle maps to the 2x grid: parent row + child column block.
    shuffle_rows: GpuTensor,
    shuffle_blocks: GpuTensor,
}

impl DenseGrid {
    pub fn new(res: usize) -> Result<Self> {
        let v = res * res * res;
        let r = res as i64;
        let mut neighbors = Vec::with_capacity(27);
        for kd in -1i64..=1 {
            for kh in -1i64..=1 {
                for kw in -1i64..=1 {
                    let mut idx = vec![u32::MAX; v];
                    for x in 0..r {
                        for y in 0..r {
                            for z in 0..r {
                                let (nx, ny, nz) = (x + kd, y + kh, z + kw);
                                if nx >= 0 && nx < r && ny >= 0 && ny < r && nz >= 0 && nz < r {
                                    idx[((x * r + y) * r + z) as usize] =
                                        ((nx * r + ny) * r + nz) as u32;
                                }
                            }
                        }
                    }
                    neighbors.push(gpu_upload_u32(&idx).map_err(DiffusionError::model)?);
                }
            }
        }
        // 2x pixel shuffle: out voxel (x, y, z) at 2*res <- parent (x/2, y/2,
        // z/2), block code (x%2)*4 + (y%2)*2 + (z%2) (z-minor, torch reshape).
        let r2 = 2 * res;
        let mut rows = vec![0u32; r2 * r2 * r2];
        let mut blocks = vec![0u32; r2 * r2 * r2];
        for x in 0..r2 {
            for y in 0..r2 {
                for z in 0..r2 {
                    let out = (x * r2 + y) * r2 + z;
                    rows[out] = (((x / 2) * res + y / 2) * res + z / 2) as u32;
                    blocks[out] = ((x % 2) * 4 + (y % 2) * 2 + (z % 2)) as u32;
                }
            }
        }
        Ok(Self {
            res,
            neighbors,
            shuffle_rows: gpu_upload_u32(&rows).map_err(DiffusionError::model)?,
            shuffle_blocks: gpu_upload_u32(&blocks).map_err(DiffusionError::model)?,
        })
    }
}

struct ResBlock {
    norm1_w: Vec<f32>,
    norm1_b: Vec<f32>,
    conv1: Conv3d,
    norm2_w: Vec<f32>,
    norm2_b: Vec<f32>,
    conv2: Conv3d,
}

impl ResBlock {
    fn load(weights: &TrellisWeights, prefix: &str, channels: usize) -> Result<Self> {
        Ok(Self {
            norm1_w: weights.tensor_f32(&format!("{prefix}.norm1.weight"))?,
            norm1_b: weights.tensor_f32(&format!("{prefix}.norm1.bias"))?,
            conv1: Conv3d::load(weights, &format!("{prefix}.conv1"), channels, channels, false)?,
            norm2_w: weights.tensor_f32(&format!("{prefix}.norm2.weight"))?,
            norm2_b: weights.tensor_f32(&format!("{prefix}.norm2.bias"))?,
            conv2: Conv3d::load(weights, &format!("{prefix}.conv2"), channels, channels, false)?,
        })
    }

    fn forward(&self, x: &GpuTensor, grid: &DenseGrid) -> Result<GpuTensor> {
        let h = gpu_layer_norm_mul_add(x, &self.norm1_w, &self.norm1_b, SS_DEC_NORM_EPS)
            .map_err(DiffusionError::model)?;
        let h = gpu_silu(&h).map_err(DiffusionError::model)?;
        let h = self.conv1.forward(&h, grid)?;
        let h = gpu_layer_norm_mul_add(&h, &self.norm2_w, &self.norm2_b, SS_DEC_NORM_EPS)
            .map_err(DiffusionError::model)?;
        let h = gpu_silu(&h).map_err(DiffusionError::model)?;
        let h = self.conv2.forward(&h, grid)?;
        gpu_add(&h, x).map_err(DiffusionError::model)
    }
}

pub struct T2SsDec {
    input_conv: Conv3d,
    middle: Vec<ResBlock>,
    stage512: Vec<ResBlock>,
    up512_128: Conv3d, // output channels block-major for the shuffle
    stage128: Vec<ResBlock>,
    up128_32: Conv3d,
    stage32: Vec<ResBlock>,
    out_norm_w: Vec<f32>,
    out_norm_b: Vec<f32>,
    out_conv: Conv3d,
}

impl T2SsDec {
    pub fn prepare(weights: &TrellisWeights) -> Result<Self> {
        let ch = SS_DEC_CHANNELS;
        Ok(Self {
            input_conv: Conv3d::load(weights, "input_layer", ch[0], SS_DEC_LATENT, false)?,
            middle: (0..2)
                .map(|i| ResBlock::load(weights, &format!("middle_block.{i}"), ch[0]))
                .collect::<Result<Vec<_>>>()?,
            stage512: (0..2)
                .map(|i| ResBlock::load(weights, &format!("blocks.{i}"), ch[0]))
                .collect::<Result<Vec<_>>>()?,
            up512_128: Conv3d::load(weights, "blocks.2.conv", ch[1] * 8, ch[0], true)?,
            stage128: (3..5)
                .map(|i| ResBlock::load(weights, &format!("blocks.{i}"), ch[1]))
                .collect::<Result<Vec<_>>>()?,
            up128_32: Conv3d::load(weights, "blocks.5.conv", ch[2] * 8, ch[1], true)?,
            stage32: (6..8)
                .map(|i| ResBlock::load(weights, &format!("blocks.{i}"), ch[2]))
                .collect::<Result<Vec<_>>>()?,
            out_norm_w: weights.tensor_f32("out_layer.0.weight")?,
            out_norm_b: weights.tensor_f32("out_layer.0.bias")?,
            out_conv: Conv3d::load(weights, "out_layer.2", 1, ch[2], false)?,
        })
    }

    /// z_s: (4096, 8) token-major latent (the SS flow output transposed to
    /// rows). Returns 64^3 occupancy logits, (x, y, z) lex order.
    pub fn decode(&self, z_s: &[f32]) -> Result<Vec<f32>> {
        let g16 = DenseGrid::new(16)?;
        let g32 = DenseGrid::new(32)?;
        let g64 = DenseGrid::new(64)?;
        if z_s.len() != 4096 * SS_DEC_LATENT {
            return Err(DiffusionError::workflow("ss_dec latent shape mismatch"));
        }
        let x = gpu_upload(z_s, 4096, SS_DEC_LATENT).map_err(DiffusionError::model)?;
        let mut h = self.input_conv.forward(&x, &g16)?;
        drop(x);
        for block in &self.middle {
            h = block.forward(&h, &g16)?;
        }
        for block in &self.stage512 {
            h = block.forward(&h, &g16)?;
        }
        // upsample 16 -> 32
        let shuffled = self.up512_128.forward(&h, &g16)?;
        h = gpu_gather_rows_colblock(
            &shuffled,
            &g16.shuffle_rows,
            Some(&g16.shuffle_blocks),
            SS_DEC_CHANNELS[1],
        )
        .map_err(DiffusionError::model)?;
        drop(shuffled);
        for block in &self.stage128 {
            h = block.forward(&h, &g32)?;
        }
        // upsample 32 -> 64
        let shuffled = self.up128_32.forward(&h, &g32)?;
        h = gpu_gather_rows_colblock(
            &shuffled,
            &g32.shuffle_rows,
            Some(&g32.shuffle_blocks),
            SS_DEC_CHANNELS[2],
        )
        .map_err(DiffusionError::model)?;
        drop(shuffled);
        for block in &self.stage32 {
            h = block.forward(&h, &g64)?;
        }
        let out = gpu_layer_norm_mul_add(&h, &self.out_norm_w, &self.out_norm_b, SS_DEC_NORM_EPS)
            .map_err(DiffusionError::model)?;
        drop(h);
        let out = gpu_silu(&out).map_err(DiffusionError::model)?;
        let out = self.out_conv.forward(&out, &g64)?;
        gpu_download(&out).map_err(DiffusionError::model)
    }
}
