//! TRELLIS sparse structured-latent VAE decoders (shape FlexiDualGrid + tex
//! SparseUnet): ConvNeXt stacks over submanifold 3x3x3 sparse convolutions
//! with Channel2Spatial upsampling, channels [1024, 512, 256, 128, 64],
//! blocks [4, 16, 8, 4, 0].
//!
//! Sparse conv = 27 taps of (neighbor gather with zero row for absent
//! neighbors -> cached f16 gemm -> accumulate), the trellis.cpp recipe.
//! Checkpoint conv weights are ALREADY permuted [Co, kd, kh, kw, Ci]
//! (flex_gemm layout) — tap slices are Ci-contiguous. Tap k = (kd*3+kh)*3+kw
//! pairs with input voxel (x+kd-1, y+kh-1, z+kw-1) (cross-correlation).
//!
//! Channel2Spatial: child s of a parent (s = dx + 2*dy + 4*dz, x fastest)
//! takes column block s of the (N, 8*C) tensor — one gather_rows_colblock.
//! Child rows are parent-major then s-ascending, matching torch's
//! repeat_interleave + nonzero order. The skip path expands the parent's
//! (C/8)-block by repeat_interleave(out/(C/8)) — a resident 0/1 matrix gemm.
//!
//! Norm semantics (LayerNorm32, f32 math): ConvNeXt norm AFFINE eps 1e-6;
//! up-block norm1 AFFINE 1e-6, norm2 NO affine 1e-6; final head layer norm
//! weightless eps 1e-5. Blocks run f16 weights (native f16 gemms, f32
//! accumulate); from_latent / to_subdiv / output_layer are f32.

use crate::backend::{
    gpu_add, gpu_download, gpu_gather_rows_colblock, gpu_layer_norm_mul_add,
    gpu_linear_f32_resident, gpu_linear_nt_cached, gpu_silu, gpu_sparse_conv27, gpu_upload,
    gpu_upload_u32, gpu_weight_cache_ensure, GpuLinearPart, GpuTensor,
};
use crate::trellis::TrellisWeights;
use crate::{DiffusionError, Result};
use makepad_ggml::quant::{GGML_TYPE_BF16, GGML_TYPE_F16};
use makepad_ai_loader::MlxDType;
use std::collections::HashMap;

pub const T2_DEC_CHANNELS: [usize; 5] = [1024, 512, 256, 128, 64];
pub const T2_DEC_BLOCKS: [usize; 5] = [4, 16, 8, 4, 0];
pub const T2_DEC_LATENT: usize = 32;
pub const T2_DEC_NORM_EPS: f32 = 1e-6;
pub const T2_DEC_FINAL_EPS: f32 = 1e-5;

fn host_f32(weights: &TrellisWeights, name: &str) -> Result<Vec<f32>> {
    weights.tensor_f32(name)
}

/// The im2col single-gemm sparse conv (default). T2_CONV_IM2COL=0 restores
/// the legacy 27 x (gather + gemm + add) path for A/B.
fn t2_conv_im2col_enabled() -> bool {
    match std::env::var("T2_CONV_IM2COL") {
        Ok(value) => value != "0",
        Err(_) => true,
    }
}

/// Neighbor index buffers (27 taps) for one sparse coordinate set.
pub struct SparseStageCtx {
    pub coords: Vec<[i32; 3]>,
    /// Tap-major (27*N) neighbor rows, u32::MAX = absent — one upload,
    /// consumed by the im2col conv.
    neighbors27: GpuTensor,
    /// Per-tap index tensors, built only for the legacy conv path.
    tap_neighbors: Vec<GpuTensor>,
}

fn t2_trace_enabled() -> bool {
    match std::env::var("T2_TRACE") {
        Ok(value) => value != "0",
        Err(_) => false,
    }
}

/// Open-addressing coord -> row map (multiplicative hash over the packed
/// coord, linear probe). The std HashMap's SipHash over [i32; 3] dominated
/// the host neighbor build at the 5M-voxel stage; coords pack into 11 bits
/// per axis (+1 bias so the -1 neighbor probes stay in range). Shared with
/// the dual-grid mesh extraction (same shape of lookup at the same scale).
pub struct CoordMap {
    keys: Vec<u64>,
    values: Vec<u32>,
    mask: usize,
}

const COORD_EMPTY: u64 = u64::MAX;

#[inline]
fn coord_pack(c: [i32; 3]) -> u64 {
    (((c[0] + 1) as u64) << 22) | (((c[1] + 1) as u64) << 11) | ((c[2] + 1) as u64)
}

impl CoordMap {
    pub fn build(coords: &[[i32; 3]]) -> Self {
        let capacity = (coords.len() * 2).next_power_of_two().max(64);
        let mut map = Self {
            keys: vec![COORD_EMPTY; capacity],
            values: vec![0; capacity],
            mask: capacity - 1,
        };
        for (i, c) in coords.iter().enumerate() {
            let key = coord_pack(*c);
            let mut slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & map.mask;
            while map.keys[slot] != COORD_EMPTY {
                slot = (slot + 1) & map.mask;
            }
            map.keys[slot] = key;
            map.values[slot] = i as u32;
        }
        map
    }

    #[inline]
    pub fn get(&self, c: [i32; 3]) -> u32 {
        if c[0] < -1 || c[1] < -1 || c[2] < -1 || c[0] >= 2047 || c[1] >= 2047 || c[2] >= 2047 {
            return u32::MAX;
        }
        let key = coord_pack(c);
        let mut slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & self.mask;
        loop {
            let stored = self.keys[slot];
            if stored == key {
                return self.values[slot];
            }
            if stored == COORD_EMPTY {
                return u32::MAX;
            }
            slot = (slot + 1) & self.mask;
        }
    }
}

impl SparseStageCtx {
    pub fn new(coords: Vec<[i32; 3]>) -> Result<Self> {
        let trace_start = std::time::Instant::now();
        let map = CoordMap::build(&coords);
        let map_built = trace_start.elapsed();
        // 27 taps of N read-only hashmap lookups: thread across taps (the
        // host neighbor build dominated cold decode otherwise).
        let offsets: Vec<[i32; 3]> = (-1i32..=1)
            .flat_map(|kd| {
                (-1i32..=1).flat_map(move |kh| (-1i32..=1).map(move |kw| [kd, kh, kw]))
            })
            .collect();
        let tap_indices: Vec<Vec<u32>> = std::thread::scope(|scope| {
            let handles: Vec<_> = offsets
                .iter()
                .map(|offset| {
                    let map = &map;
                    let coords = &coords;
                    let offset = *offset;
                    scope.spawn(move || {
                        coords
                            .iter()
                            .map(|c| {
                                map.get([c[0] + offset[0], c[1] + offset[1], c[2] + offset[2]])
                            })
                            .collect::<Vec<u32>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("neighbor tap thread"))
                .collect()
        });
        let taps_built = trace_start.elapsed();
        let mut combined = Vec::with_capacity(27 * coords.len());
        for idx in &tap_indices {
            combined.extend_from_slice(idx);
        }
        let neighbors27 = gpu_upload_u32(&combined).map_err(DiffusionError::model)?;
        if t2_trace_enabled() {
            println!(
                "TRACE stage_ctx n={} map={:.3}s taps={:.3}s upload+total={:.3}s",
                coords.len(),
                map_built.as_secs_f64(),
                (taps_built - map_built).as_secs_f64(),
                trace_start.elapsed().as_secs_f64(),
            );
        }
        let mut tap_neighbors = Vec::new();
        if !t2_conv_im2col_enabled() {
            tap_neighbors.reserve(27);
            for idx in &tap_indices {
                tap_neighbors.push(gpu_upload_u32(idx).map_err(DiffusionError::model)?);
            }
        }
        Ok(Self {
            coords,
            neighbors27,
            tap_neighbors,
        })
    }

    pub fn len(&self) -> usize {
        self.coords.len()
    }

    pub fn is_empty(&self) -> bool {
        self.coords.is_empty()
    }
}

/// Child expansion for Channel2Spatial: parents in order, active children by
/// ascending s (s = dx + 2*dy + 4*dz). Returns (child coords, parent row per
/// child, block index per child).
pub fn t2_expand_children(
    coords: &[[i32; 3]],
    masks: &[[bool; 8]],
) -> (Vec<[i32; 3]>, Vec<u32>, Vec<u32>) {
    let mut child_coords = Vec::new();
    let mut rows = Vec::new();
    let mut blocks = Vec::new();
    for (parent, (coord, mask)) in coords.iter().zip(masks.iter()).enumerate() {
        for s in 0..8u32 {
            if mask[s as usize] {
                let dx = (s & 1) as i32;
                let dy = ((s >> 1) & 1) as i32;
                let dz = ((s >> 2) & 1) as i32;
                child_coords.push([coord[0] * 2 + dx, coord[1] * 2 + dy, coord[2] * 2 + dz]);
                rows.push(parent as u32);
                blocks.push(s);
            }
        }
    }
    (child_coords, rows, blocks)
}

/// The decoder: shared by shape (pred_subdiv, 7ch out) and tex (guide_subs,
/// 6ch out).
pub struct T2SparseDec {
    weights: TrellisWeights,
    namespace: &'static str,
    pub out_channels: usize,
    pub pred_subdiv: bool,
    from_latent_w: GpuTensor,
    from_latent_b: GpuTensor,
    output_w: GpuTensor,
    output_b: GpuTensor,
    // Host norm vectors / biases per stage.
    convnext_norm: Vec<Vec<(Vec<f32>, Vec<f32>)>>, // [stage][block] (w, b)
    conv_bias: HashMap<String, Vec<f32>>,
    mlp_bias: HashMap<String, Vec<f32>>,
    up_norm1: Vec<(Vec<f32>, Vec<f32>)>,
    up_subdiv: Vec<Option<(GpuTensor, GpuTensor)>>, // to_subdiv w (8, C) + b
    up_expand: Vec<GpuTensor>,                      // 0/1 (C_out, C_in/8)
}

impl T2SparseDec {
    pub fn prepare(
        weights: TrellisWeights,
        namespace: &'static str,
        out_channels: usize,
        pred_subdiv: bool,
    ) -> Result<Self> {
        let mut convnext_norm = Vec::new();
        let mut conv_bias = HashMap::new();
        let mut mlp_bias = HashMap::new();
        let mut up_norm1 = Vec::new();
        let mut up_subdiv = Vec::new();
        let mut up_expand = Vec::new();
        for stage in 0..5 {
            let mut stage_norms = Vec::new();
            for j in 0..T2_DEC_BLOCKS[stage] {
                let p = format!("blocks.{stage}.{j}");
                stage_norms.push((
                    host_f32(&weights, &format!("{p}.norm.weight"))?,
                    host_f32(&weights, &format!("{p}.norm.bias"))?,
                ));
                conv_bias.insert(
                    format!("{p}.conv"),
                    host_f32(&weights, &format!("{p}.conv.bias"))?,
                );
                mlp_bias.insert(
                    format!("{p}.mlp.0"),
                    host_f32(&weights, &format!("{p}.mlp.0.bias"))?,
                );
                mlp_bias.insert(
                    format!("{p}.mlp.2"),
                    host_f32(&weights, &format!("{p}.mlp.2.bias"))?,
                );
            }
            convnext_norm.push(stage_norms);
            if stage < 4 {
                let p = format!("blocks.{stage}.{}", T2_DEC_BLOCKS[stage]);
                up_norm1.push((
                    host_f32(&weights, &format!("{p}.norm1.weight"))?,
                    host_f32(&weights, &format!("{p}.norm1.bias"))?,
                ));
                conv_bias.insert(
                    format!("{p}.conv1"),
                    host_f32(&weights, &format!("{p}.conv1.bias"))?,
                );
                conv_bias.insert(
                    format!("{p}.conv2"),
                    host_f32(&weights, &format!("{p}.conv2.bias"))?,
                );
                if pred_subdiv {
                    let ch = T2_DEC_CHANNELS[stage];
                    let w = host_f32(&weights, &format!("{p}.to_subdiv.weight"))?;
                    let b = host_f32(&weights, &format!("{p}.to_subdiv.bias"))?;
                    up_subdiv.push(Some((
                        gpu_upload(&w, 8, ch).map_err(DiffusionError::model)?,
                        gpu_upload(&b, 1, 8).map_err(DiffusionError::model)?,
                    )));
                } else {
                    up_subdiv.push(None);
                }
                // repeat_interleave expansion: out_ch columns from C/8 inputs.
                let cin8 = T2_DEC_CHANNELS[stage] / 8;
                let cout = T2_DEC_CHANNELS[stage + 1];
                let factor = cout / cin8;
                let mut expand = vec![0.0f32; cout * cin8];
                for j in 0..cout {
                    expand[j * cin8 + j / factor] = 1.0;
                }
                up_expand.push(gpu_upload(&expand, cout, cin8).map_err(DiffusionError::model)?);
            }
        }
        let from_latent_w = host_f32(&weights, "from_latent.weight")?;
        let from_latent_b = host_f32(&weights, "from_latent.bias")?;
        let output_w = host_f32(&weights, "output_layer.weight")?;
        let output_b = host_f32(&weights, "output_layer.bias")?;
        Ok(Self {
            from_latent_w: gpu_upload(&from_latent_w, T2_DEC_CHANNELS[0], T2_DEC_LATENT)
                .map_err(DiffusionError::model)?,
            from_latent_b: gpu_upload(&from_latent_b, 1, T2_DEC_CHANNELS[0])
                .map_err(DiffusionError::model)?,
            output_w: gpu_upload(&output_w, out_channels, T2_DEC_CHANNELS[4])
                .map_err(DiffusionError::model)?,
            output_b: gpu_upload(&output_b, 1, out_channels).map_err(DiffusionError::model)?,
            convnext_norm,
            conv_bias,
            mlp_bias,
            up_norm1,
            up_subdiv,
            up_expand,
            weights,
            namespace,
            out_channels,
            pred_subdiv,
        })
    }

    /// Ensure one tap slice of a sparse conv weight [Co, 3, 3, 3, Ci] in the
    /// device cache; returns the part for the cached gemm.
    fn conv_tap_part(
        &self,
        name: String,
        co: usize,
        ci: usize,
        tap: usize,
    ) -> Result<(String, usize)> {
        let (dtype, _) = self.weights.tensor_dtype_shape(&name)?;
        let ggml_type = match dtype {
            MlxDType::F16 => GGML_TYPE_F16,
            MlxDType::BF16 => GGML_TYPE_BF16,
            other => {
                return Err(DiffusionError::model(format!(
                    "sparse conv '{name}' unsupported dtype {other:?}"
                )))
            }
        };
        let key = format!("{name}::t{tap}");
        let want_a16 = crate::backend::gpu_gemm_f16acc_enabled();
        gpu_weight_cache_ensure(self.namespace, &key, ggml_type, co, ci, want_a16, || {
            let bytes = self.weights.tensor_bytes(&name).map_err(|e| e.to_string())?;
            let row_len = 27 * ci * 2;
            let mut out = vec![0u8; co * ci * 2];
            for row in 0..co {
                let src = row * row_len + tap * ci * 2;
                out[row * ci * 2..(row + 1) * ci * 2]
                    .copy_from_slice(&bytes[src..src + ci * 2]);
            }
            Ok(out)
        })
        .map_err(DiffusionError::model)?;
        Ok((key, ggml_type as usize))
    }

    /// Ensure the FULL [Co, 27*Ci] flex_gemm conv weight (checkpoint layout,
    /// f16, no permute needed) in the device cache for the im2col conv.
    fn ensure_conv_full(&self, weight_name: &str, co: usize, ci: usize) -> Result<String> {
        let (dtype, _) = self.weights.tensor_dtype_shape(weight_name)?;
        if dtype != MlxDType::F16 {
            return Err(DiffusionError::model(format!(
                "sparse conv '{weight_name}' im2col path needs f16, got {dtype:?}"
            )));
        }
        let key = format!("{weight_name}::full");
        gpu_weight_cache_ensure(self.namespace, &key, GGML_TYPE_F16, co, 27 * ci, false, || {
            self.weights
                .tensor_bytes(weight_name)
                .map_err(|e| e.to_string())
        })
        .map_err(DiffusionError::model)?;
        Ok(key)
    }

    /// Submanifold conv3d. Default: chunked im2col slab + one tensor-core
    /// gemm per chunk (gpu_sparse_conv27). Legacy (T2_CONV_IM2COL=0):
    /// 27 x (gather -> cached gemm -> add), bias on the center tap.
    fn sparse_conv(
        &self,
        x: &GpuTensor,
        ctx: &SparseStageCtx,
        name: &str,
        co: usize,
    ) -> Result<GpuTensor> {
        let ci = x.cols();
        let bias = self
            .conv_bias
            .get(name)
            .ok_or_else(|| DiffusionError::model(format!("missing conv bias {name}")))?;
        let weight_name = format!("{name}.weight");
        if t2_conv_im2col_enabled() {
            let key = self.ensure_conv_full(&weight_name, co, ci)?;
            return gpu_sparse_conv27(x, &ctx.neighbors27, self.namespace, &key, co, bias)
                .map_err(DiffusionError::model);
        }
        let mut acc: Option<GpuTensor> = None;
        for tap in 0..27 {
            let gathered = gpu_gather_rows_colblock(x, &ctx.tap_neighbors[tap], None, ci)
                .map_err(DiffusionError::model)?;
            let (key, ggml_type) = self.conv_tap_part(weight_name.clone(), co, ci, tap)?;
            let part = GpuLinearPart {
                bt_ggml_type: ggml_type as u32,
                n: co,
                cache_key: &key,
                bytes: &[],
            };
            let parts = [part];
            let tap_bias: &[f32] = if tap == 13 { bias } else { &[] };
            let y = gpu_linear_nt_cached(&gathered, self.namespace, &parts, tap_bias)
                .map_err(DiffusionError::model)?;
            drop(gathered);
            acc = Some(match acc {
                None => y,
                Some(a) => gpu_add(&a, &y).map_err(DiffusionError::model)?,
            });
        }
        acc.ok_or_else(|| DiffusionError::model("sparse conv produced no output"))
    }

    fn linear_cached(&self, x: &GpuTensor, name: &str, n: usize, bias: &[f32]) -> Result<GpuTensor> {
        let weight_name = format!("{name}.weight");
        let (dtype, _) = self.weights.tensor_dtype_shape(&weight_name)?;
        let ggml_type = match dtype {
            MlxDType::F16 => GGML_TYPE_F16,
            MlxDType::BF16 => GGML_TYPE_BF16,
            other => {
                return Err(DiffusionError::model(format!(
                    "linear '{weight_name}' unsupported dtype {other:?}"
                )))
            }
        };
        let want_a16 = crate::backend::gpu_gemm_f16acc_enabled();
        gpu_weight_cache_ensure(self.namespace, &weight_name, ggml_type, n, x.cols(), want_a16, || {
            self.weights
                .tensor_bytes(&weight_name)
                .map_err(|e| e.to_string())
        })
        .map_err(DiffusionError::model)?;
        let part = GpuLinearPart {
            bt_ggml_type: ggml_type,
            n,
            cache_key: &weight_name,
            bytes: &[],
        };
        let parts = [part];
        gpu_linear_nt_cached(x, self.namespace, &parts, bias).map_err(DiffusionError::model)
    }

    fn convnext(
        &self,
        x: GpuTensor,
        ctx: &SparseStageCtx,
        stage: usize,
        block: usize,
    ) -> Result<GpuTensor> {
        let p = format!("blocks.{stage}.{block}");
        let c = T2_DEC_CHANNELS[stage];
        let h = self.sparse_conv(&x, ctx, &format!("{p}.conv"), c)?;
        let (norm_w, norm_b) = &self.convnext_norm[stage][block];
        let h = gpu_layer_norm_mul_add(&h, norm_w, norm_b, T2_DEC_NORM_EPS)
            .map_err(DiffusionError::model)?;
        let mid = self.linear_cached(
            &h,
            &format!("{p}.mlp.0"),
            4 * c,
            &self.mlp_bias[&format!("{p}.mlp.0")],
        )?;
        drop(h);
        let mid = gpu_silu(&mid).map_err(DiffusionError::model)?;
        let h = self.linear_cached(
            &mid,
            &format!("{p}.mlp.2"),
            c,
            &self.mlp_bias[&format!("{p}.mlp.2")],
        )?;
        drop(mid);
        gpu_add(&h, &x).map_err(DiffusionError::model)
    }

    /// One up stage: returns (child hidden, child ctx, subdivision masks).
    /// `guide`: provided subdivision masks (tex decoder) — otherwise
    /// to_subdiv predicts them.
    fn up_stage(
        &self,
        x: GpuTensor,
        ctx: &SparseStageCtx,
        stage: usize,
        guide: Option<&[[bool; 8]]>,
    ) -> Result<(GpuTensor, SparseStageCtx, Vec<[bool; 8]>)> {
        let p = format!("blocks.{stage}.{}", T2_DEC_BLOCKS[stage]);
        let cin = T2_DEC_CHANNELS[stage];
        let cout = T2_DEC_CHANNELS[stage + 1];

        let masks: Vec<[bool; 8]> = match guide {
            Some(masks) => masks.to_vec(),
            None => {
                let (sw, sb) = self.up_subdiv[stage].as_ref().ok_or_else(|| {
                    DiffusionError::model("decoder lacks to_subdiv but no guide given")
                })?;
                let logits = gpu_linear_f32_resident(&x, sw, Some(sb))
                    .map_err(DiffusionError::model)?;
                let host = gpu_download(&logits).map_err(DiffusionError::model)?;
                host.chunks_exact(8)
                    .map(|row| {
                        let mut m = [false; 8];
                        for (dst, v) in m.iter_mut().zip(row) {
                            *dst = *v > 0.0;
                        }
                        m
                    })
                    .collect()
            }
        };

        let (child_coords, rows, blocks) = t2_expand_children(&ctx.coords, &masks);
        let rows_gpu = gpu_upload_u32(&rows).map_err(DiffusionError::model)?;
        let blocks_gpu = gpu_upload_u32(&blocks).map_err(DiffusionError::model)?;

        let (norm1_w, norm1_b) = &self.up_norm1[stage];
        let h = gpu_layer_norm_mul_add(&x, norm1_w, norm1_b, T2_DEC_NORM_EPS)
            .map_err(DiffusionError::model)?;
        let h = gpu_silu(&h).map_err(DiffusionError::model)?;
        let h = self.sparse_conv(&h, ctx, &format!("{p}.conv1"), cout * 8)?;
        // Channel2Spatial on the conv output (block s = cols s*cout..)
        let h_child = gpu_gather_rows_colblock(&h, &rows_gpu, Some(&blocks_gpu), cout)
            .map_err(DiffusionError::model)?;
        drop(h);
        // Skip path: C2S on raw x (blocks of cin/8) then repeat_interleave.
        let x_child = gpu_gather_rows_colblock(&x, &rows_gpu, Some(&blocks_gpu), cin / 8)
            .map_err(DiffusionError::model)?;
        drop(x);
        let skip = gpu_linear_f32_resident(&x_child, &self.up_expand[stage], None)
            .map_err(DiffusionError::model)?;
        drop(x_child);

        let child_ctx = SparseStageCtx::new(child_coords)?;
        // norm2 (no affine) + silu + conv2 at the child grid.
        let ones = vec![1.0f32; cout];
        let zeros = vec![0.0f32; cout];
        let h2 = gpu_layer_norm_mul_add(&h_child, &ones, &zeros, T2_DEC_NORM_EPS)
            .map_err(DiffusionError::model)?;
        drop(h_child);
        let h2 = gpu_silu(&h2).map_err(DiffusionError::model)?;
        let h2 = self.sparse_conv(&h2, &child_ctx, &format!("{p}.conv2"), cout)?;
        let out = gpu_add(&h2, &skip).map_err(DiffusionError::model)?;
        Ok((out, child_ctx, masks))
    }

    /// Cascade upsample: from_latent + the first `times` stages (blocks +
    /// predicted subdivision), returning the coords AFTER the last up.
    pub fn upsample(&self, latent: &[f32], coords: Vec<[i32; 3]>, times: usize) -> Result<Vec<[i32; 3]>> {
        let mut ctx = SparseStageCtx::new(coords)?;
        let x = gpu_upload(latent, ctx.len(), T2_DEC_LATENT).map_err(DiffusionError::model)?;
        let mut h = gpu_linear_f32_resident(&x, &self.from_latent_w, Some(&self.from_latent_b))
            .map_err(DiffusionError::model)?;
        drop(x);
        for stage in 0..times {
            for block in 0..T2_DEC_BLOCKS[stage] {
                h = self.convnext(h, &ctx, stage, block)?;
            }
            let (next, next_ctx, _masks) = self.up_stage(h, &ctx, stage, None)?;
            h = next;
            ctx = next_ctx;
        }
        Ok(ctx.coords)
    }

    /// Full decode. Returns (features (N, out_channels), coords, per-stage
    /// subdivision masks).
    pub fn decode(
        &self,
        latent: &[f32],
        coords: Vec<[i32; 3]>,
        guide_subs: Option<&[Vec<[bool; 8]>]>,
    ) -> Result<(Vec<f32>, Vec<[i32; 3]>, Vec<Vec<[bool; 8]>>)> {
        self.decode_ctl(latent, coords, guide_subs, &|| false, &mut |_, _| {})
    }

    /// [`Self::decode`] with service-job control: `cancel` is checked between
    /// every ConvNeXt block and up-stage (returning `true` unwinds with
    /// [`DiffusionError::Cancelled`]); `on_stage(done, total)` reports each of
    /// the 5 resolution stages as it starts.
    pub fn decode_ctl(
        &self,
        latent: &[f32],
        coords: Vec<[i32; 3]>,
        guide_subs: Option<&[Vec<[bool; 8]>]>,
        cancel: &(dyn Fn() -> bool),
        on_stage: &mut dyn FnMut(usize, usize),
    ) -> Result<(Vec<f32>, Vec<[i32; 3]>, Vec<Vec<[bool; 8]>>)> {
        let mut ctx = SparseStageCtx::new(coords)?;
        let x = gpu_upload(latent, ctx.len(), T2_DEC_LATENT).map_err(DiffusionError::model)?;
        let mut h = gpu_linear_f32_resident(&x, &self.from_latent_w, Some(&self.from_latent_b))
            .map_err(DiffusionError::model)?;
        drop(x);
        let mut subs = Vec::new();
        for stage in 0..5 {
            on_stage(stage, 5);
            for block in 0..T2_DEC_BLOCKS[stage] {
                if cancel() {
                    return Err(DiffusionError::Cancelled);
                }
                h = self.convnext(h, &ctx, stage, block)?;
            }
            if stage < 4 {
                if cancel() {
                    return Err(DiffusionError::Cancelled);
                }
                let guide = guide_subs.map(|g| g[stage].as_slice());
                let (next, next_ctx, masks) = self.up_stage(h, &ctx, stage, guide)?;
                h = next;
                ctx = next_ctx;
                subs.push(masks);
            }
        }
        let ones = vec![1.0f32; T2_DEC_CHANNELS[4]];
        let zeros = vec![0.0f32; T2_DEC_CHANNELS[4]];
        let h = gpu_layer_norm_mul_add(&h, &ones, &zeros, T2_DEC_FINAL_EPS)
            .map_err(DiffusionError::model)?;
        let out = gpu_linear_f32_resident(&h, &self.output_w, Some(&self.output_b))
            .map_err(DiffusionError::model)?;
        drop(h);
        let feats = gpu_download(&out).map_err(DiffusionError::model)?;
        Ok((feats, ctx.coords, subs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_expansion_order() {
        let coords = vec![[3, 0, 5], [0, 1, 0]];
        let masks = vec![
            {
                let mut m = [false; 8];
                m[0] = true; // (0,0,0)
                m[5] = true; // s=5: dx=1, dy=0, dz=1
                m
            },
            {
                let mut m = [false; 8];
                m[2] = true; // s=2: dx=0, dy=1, dz=0
                m
            },
        ];
        let (children, rows, blocks) = t2_expand_children(&coords, &masks);
        assert_eq!(children, vec![[6, 0, 10], [7, 0, 11], [0, 3, 0]]);
        assert_eq!(rows, vec![0, 0, 1]);
        assert_eq!(blocks, vec![0, 5, 2]);
    }

    #[test]
    fn neighbor_tap_center() {
        // Tap 13 = (kd, kh, kw) = (1, 1, 1) = self.
        let mut tap = 0;
        let mut center = None;
        for kd in -1i32..=1 {
            for kh in -1i32..=1 {
                for kw in -1i32..=1 {
                    if kd == 0 && kh == 0 && kw == 0 {
                        center = Some(tap);
                    }
                    tap += 1;
                }
            }
        }
        assert_eq!(center, Some(13));
    }
}
