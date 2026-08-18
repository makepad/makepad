//! MLX steel affine Q4 GEMM for Metal prefill (`M > 8`).
//!
//! GGUF Q4_K/Q5_K/… rows are packed once as MLX affine q4-g64
//! (`scale*q+bias`). `M>8` dispatches vendored MLX v0.31.2
//! `qmm_t_impl<half, 64, 4, aligned_N=true, BM=BK=BN=32>` with F16
//! staging around ggml F32 `[K,M]` / `[N,M]`.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::context::Context;
use crate::quant::{self, f32_to_f16, get_rows_ggml_bytes_cpu};
use crate::tensor::{Tensor, TensorType};

use super::{
    BufferStorageMode, MetalBuffer, MetalBufferBindingRef, MetalPipeline, MetalPipelineDescriptor,
    MetalRuntime, MetalSize,
};

const GROUP: usize = 64;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AffineKey {
    offset: usize,
    nbytes: usize,
    ty: u32,
    n: usize,
    k: usize,
}

struct AffinePacked {
    weights: MetalBuffer,
    scales: MetalBuffer,
    biases: MetalBuffer,
    qparams_per_row: i32,
    weight_bytes_per_row: i32,
}

thread_local! {
    static PACKS: RefCell<HashMap<AffineKey, AffinePacked>> = RefCell::new(HashMap::new());
    static STAGING: RefCell<Option<SteelStaging>> = RefCell::new(None);
    static STEEL_LOGGED: RefCell<bool> = RefCell::new(false);
    static STEEL_DISPATCHES: RefCell<u32> = RefCell::new(0);
    static STEEL_SHAPES: RefCell<HashMap<(i32, i32, i32), u32>> = RefCell::new(HashMap::new());
}

struct SteelStaging {
    a: MetalBuffer,
    c: MetalBuffer,
    a_bytes: usize,
    c_bytes: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SteelQmmArgs {
    k: i32,
    n: i32,
    m: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SteelQmmF32Args {
    k: i32,
    n: i32,
    m: i32,
    src_m_stride: i32,
    dst_m_stride: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SteelPackAArgs {
    k: i32,
    m: i32,
    src_k_stride: i32,
    src_m_stride: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SteelUnpackCArgs {
    n: i32,
    m: i32,
    dst_n_stride: i32,
    dst_m_stride: i32,
}

pub fn warmup_affine_qmm(runtime: &MetalRuntime, ctx: &Context) -> Result<usize, String> {
    if !affine_qmm_enabled() || !runtime.features().has_simdgroup_mm {
        return Ok(0);
    }
    let mut packed_count = 0usize;
    let mut max_k = 0usize;
    let mut max_n = 0usize;
    for tensor in ctx.tensors() {
        if tensor.desc.layout.rank() < 2 {
            continue;
        }
        let k = usize::try_from(tensor.ne[0]).unwrap_or(0);
        let n = usize::try_from(tensor.ne[1]).unwrap_or(0);
        if k < GROUP || k % GROUP != 0 || n == 0 {
            continue;
        }
        max_k = max_k.max(k);
        max_n = max_n.max(n);
        if !matches!(
            tensor.desc.ty,
            TensorType::Q4K
                | TensorType::Q5K
                | TensorType::Q6K
                | TensorType::Q4_0
                | TensorType::Q4_1
                | TensorType::Q5_0
                | TensorType::Q5_1
                | TensorType::Q8_0
        ) {
            continue;
        }
        let Some(offset) = tensor.data_offset else {
            continue;
        };
        let key = AffineKey {
            offset,
            nbytes: tensor.nbytes(),
            ty: tensor.desc.ty as u32,
            n,
            k,
        };
        let already = PACKS.with(|packs| packs.borrow().contains_key(&key));
        if already {
            continue;
        }
        let Ok(bytes) = ctx.data_at(offset, tensor.nbytes()) else {
            continue;
        };
        let packed = pack_affine_q4(runtime, bytes, tensor.desc.ty as u32, n, k)?;
        PACKS.with(|packs| {
            packs.borrow_mut().insert(key, packed);
        });
        packed_count += 1;
    }
    if packed_count > 0 {
        // Flux tokens are 256; joint can be 512. Size once so dispatch never
        // drops live GPU staging buffers mid-batch (that serializes the GPU).
        let max_m = 512usize;
        let a_bytes = max_m.saturating_mul(max_k).saturating_mul(2);
        let c_bytes = max_m.saturating_mul(max_n).saturating_mul(2);
        steel_staging(runtime, a_bytes, c_bytes)?;
        let _ = steel_qmm_pipeline(runtime)?;
        let _ = steel_qmm_f32io_pipeline(runtime)?;
        let _ = pack_a_pipeline(runtime)?;
        let _ = unpack_c_pipeline(runtime)?;
        eprintln!(
            "metal: affine qmm warmed {packed_count} quantized weight matrices (staging A={} C={} max_k={max_k} max_n={max_n})",
            a_bytes, c_bytes
        );
    }
    Ok(packed_count)
}

pub fn affine_qmm_enabled() -> bool {
    match std::env::var("MAKEPAD_METAL_AFFINE_QMM") {
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("on") => true,
        _ => false,
    }
}

pub fn try_dispatch_affine_qmm(
    runtime: &MetalRuntime,
    _ctx: &Context,
    src0: &Tensor,
    src1: &Tensor,
    dst: &Tensor,
    _compiled_src0: MetalBufferBindingRef<'_>,
    compiled_src1: MetalBufferBindingRef<'_>,
    compiled_dst: MetalBufferBindingRef<'_>,
    ne12: i32,
) -> Result<bool, String> {
    if !affine_qmm_enabled() {
        return Ok(false);
    }
    if !runtime.features().has_simdgroup_mm {
        return Ok(false);
    }
    let k = usize::try_from(src0.ne[0]).map_err(|_| "affine qmm K exceeds usize")?;
    let n = usize::try_from(src0.ne[1]).map_err(|_| "affine qmm N exceeds usize")?;
    let m = usize::try_from(src1.ne[1]).map_err(|_| "affine qmm M exceeds usize")?;
    if m <= 8 || k < GROUP || k % GROUP != 0 || n == 0 || n % 32 != 0 {
        return Ok(false);
    }
    if src1.ne[0] != src0.ne[0] {
        return Ok(false);
    }
    if src1.desc.ty != TensorType::F32 || dst.desc.ty != TensorType::F32 {
        return Ok(false);
    }
    if src1.nb[0] != 4 || src1.nb[1] % 4 != 0 || dst.nb[0] != 4 || dst.nb[1] % 4 != 0 {
        return Ok(false);
    }
    let Some(offset) = src0.data_offset else {
        return Ok(false);
    };
    let ty = src0.desc.ty as u32;
    if !matches!(
        src0.desc.ty,
        TensorType::Q4K
            | TensorType::Q5K
            | TensorType::Q6K
            | TensorType::Q4_0
            | TensorType::Q4_1
            | TensorType::Q5_0
            | TensorType::Q5_1
            | TensorType::Q8_0
    ) {
        return Ok(false);
    }
    let key = AffineKey {
        offset,
        nbytes: src0.nbytes(),
        ty,
        n,
        k,
    };
    let packed = PACKS.with(|packs| packs.borrow().get(&key).map(clone_packed));
    let Some(packed) = packed else {
        STEEL_LOGGED.with(|logged| {
            if !*logged.borrow() {
                eprintln!(
                    "metal: steel qmm pack miss n={n} k={k} (no GPU readback; official mul_mm)"
                );
            }
        });
        return Ok(false);
    };

    let k_i = i32::try_from(k).map_err(|_| "K exceeds i32")?;
    let n_i = i32::try_from(n).map_err(|_| "N exceeds i32")?;
    let m_i = i32::try_from(m).map_err(|_| "M exceeds i32")?;
    let src_k_stride = i32::try_from(src1.nb[0] / 4).map_err(|_| "src k stride exceeds i32")?;
    let src_m_stride = i32::try_from(src1.nb[1] / 4).map_err(|_| "src m stride exceeds i32")?;
    let dst_n_stride = i32::try_from(dst.nb[0] / 4).map_err(|_| "dst n stride exceeds i32")?;
    let dst_m_stride = i32::try_from(dst.nb[1] / 4).map_err(|_| "dst m stride exceeds i32")?;
    let batches = usize::try_from(ne12.max(1)).map_err(|_| "ne12 exceeds usize")?;
    let src_batch_stride = if batches > 1 { src1.nb[2] } else { 0 };
    let dst_batch_stride = if batches > 1 { dst.nb[2] } else { 0 };

    let skip_staging = match std::env::var("MAKEPAD_METAL_STEEL_SKIP_STAGING") {
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("on") => true,
        _ => false,
    };
    STEEL_LOGGED.with(|logged| {
        if !*logged.borrow() {
            eprintln!(
                "metal: steel affine_qmm_t {} n={n} k={k} m={m} batches={batches}",
                if skip_staging { "f16-skip" } else { "f32io" }
            );
            *logged.borrow_mut() = true;
        }
    });
    if !skip_staging {
        let qmm_f32 = steel_qmm_f32io_pipeline(runtime)?;
        let f32_args = SteelQmmF32Args {
            k: k_i,
            n: n_i,
            m: m_i,
            src_m_stride,
            dst_m_stride,
        };
        let qmm_tg = MetalSize {
            width: 32,
            height: 2,
            depth: 2,
        };
        for batch in 0..batches {
            let src_off = compiled_src1
                .offset_bytes
                .saturating_add(batch.saturating_mul(src_batch_stride));
            let dst_off = compiled_dst
                .offset_bytes
                .saturating_add(batch.saturating_mul(dst_batch_stride));
            runtime.dispatch_compute(
                &qmm_f32,
                bytes_of(&f32_args),
                &[
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &packed.weights,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &packed.scales,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &packed.biases,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 4,
                        buffer: compiled_src1.buffer,
                        offset_bytes: src_off,
                    },
                    MetalBufferBindingRef {
                        index: 5,
                        buffer: compiled_dst.buffer,
                        offset_bytes: dst_off,
                    },
                ],
                &[],
                MetalSize {
                    width: ((n + 31) / 32) as u64,
                    height: ((m + 31) / 32) as u64,
                    depth: 1,
                },
                qmm_tg,
            )?;
            STEEL_DISPATCHES.with(|count| {
                let n = *count.borrow() + 1;
                *count.borrow_mut() = n;
                STEEL_SHAPES.with(|shapes| {
                    *shapes.borrow_mut().entry((m_i, k_i, n_i)).or_insert(0) += 1;
                });
                if n == 1 || n % 228 == 0 {
                    eprintln!("metal: steel f32io dispatches={n} last n={n_i} k={k_i} m={m_i}");
                }
            });
        }
        return Ok(true);
    }

    let a_bytes = m * k * 2;
    let c_bytes = m * n * 2;
    let (stage_a, stage_c) = steel_staging(runtime, a_bytes, c_bytes)?;
    let pack = pack_a_pipeline(runtime)?;
    let qmm = steel_qmm_pipeline(runtime)?;
    let unpack = unpack_c_pipeline(runtime)?;

    let pack_args = SteelPackAArgs {
        k: k_i,
        m: m_i,
        src_k_stride,
        src_m_stride,
    };
    let qmm_args = SteelQmmArgs {
        k: k_i,
        n: n_i,
        m: m_i,
    };
    let unpack_args = SteelUnpackCArgs {
        n: n_i,
        m: m_i,
        dst_n_stride,
        dst_m_stride,
    };
    let pack_tg = MetalSize {
        width: 16,
        height: 16,
        depth: 1,
    };
    let unpack_tg = pack_tg;
    let qmm_tg = MetalSize {
        width: 32,
        height: 2,
        depth: 2,
    };

    for batch in 0..batches {
        let src_off = compiled_src1
            .offset_bytes
            .saturating_add(batch.saturating_mul(src_batch_stride));
        let dst_off = compiled_dst
            .offset_bytes
            .saturating_add(batch.saturating_mul(dst_batch_stride));
        if skip_staging {
            runtime.dispatch_compute(
                &qmm,
                bytes_of(&qmm_args),
                &[
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &packed.weights,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &packed.scales,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &packed.biases,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 4,
                        buffer: &stage_a,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 5,
                        buffer: &stage_c,
                        offset_bytes: 0,
                    },
                ],
                &[],
                MetalSize {
                    width: ((n + 31) / 32) as u64,
                    height: ((m + 31) / 32) as u64,
                    depth: 1,
                },
                qmm_tg,
            )?;
            STEEL_DISPATCHES.with(|count| {
                let n = *count.borrow() + 1;
                *count.borrow_mut() = n;
                if n == 1 || n % 228 == 0 {
                    eprintln!("metal: steel qmm dispatches={n} last n={n_i} k={k_i} m={m_i} skip_staging=1");
                }
            });
            continue;
        }
        runtime.dispatch_compute(
            &pack,
            bytes_of(&pack_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: compiled_src1.buffer,
                    offset_bytes: src_off,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &stage_a,
                    offset_bytes: 0,
                },
            ],
            &[],
            MetalSize {
                width: ((m + 15) / 16) as u64,
                height: ((k + 15) / 16) as u64,
                depth: 1,
            },
            pack_tg,
        )?;
        runtime.dispatch_compute(
            &qmm,
            bytes_of(&qmm_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &packed.weights,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &packed.scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &packed.biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &stage_a,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &stage_c,
                    offset_bytes: 0,
                },
            ],
            &[],
            MetalSize {
                width: ((n + 31) / 32) as u64,
                height: ((m + 31) / 32) as u64,
                depth: 1,
            },
            qmm_tg,
        )?;
        runtime.dispatch_compute(
            &unpack,
            bytes_of(&unpack_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &stage_c,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: compiled_dst.buffer,
                    offset_bytes: dst_off,
                },
            ],
            &[],
            MetalSize {
                width: ((m + 15) / 16) as u64,
                height: ((n + 15) / 16) as u64,
                depth: 1,
            },
            unpack_tg,
        )?;
        STEEL_DISPATCHES.with(|count| {
            let n = *count.borrow() + 1;
            *count.borrow_mut() = n;
            STEEL_SHAPES.with(|shapes| {
                *shapes.borrow_mut().entry((m_i, k_i, n_i)).or_insert(0) += 1;
            });
            if n == 1 || n % 228 == 0 {
                eprintln!("metal: steel qmm dispatches={n} last n={n_i} k={k_i} m={m_i}");
                if n % 228 == 0 {
                    let mut rows: Vec<_> = STEEL_SHAPES.with(|s| s.borrow().clone().into_iter().collect());
                    rows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
                    for ((mm, kk, nn), c) in rows.iter().take(12) {
                        eprintln!("metal: steel shape m={mm} k={kk} n={nn} count={c}");
                    }
                }
            }
        });
    }
    Ok(true)
}

fn clone_packed(packed: &AffinePacked) -> AffinePacked {
    AffinePacked {
        weights: packed.weights.clone(),
        scales: packed.scales.clone(),
        biases: packed.biases.clone(),
        qparams_per_row: packed.qparams_per_row,
        weight_bytes_per_row: packed.weight_bytes_per_row,
    }
}

fn steel_qmm_pipeline(runtime: &MetalRuntime) -> Result<MetalPipeline, String> {
    runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_steel_qmm_f16".to_string(),
        base_name: "kernel_mlx_steel_qmm_f16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 32,
        nr1: 32,
        nsg: 4,
    })
}

fn steel_qmm_f32io_pipeline(runtime: &MetalRuntime) -> Result<MetalPipeline, String> {
    runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_steel_qmm_f32io".to_string(),
        base_name: "kernel_mlx_steel_qmm_f32io".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 32,
        nr1: 32,
        nsg: 4,
    })
}

fn pack_a_pipeline(runtime: &MetalRuntime) -> Result<MetalPipeline, String> {
    runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_steel_qmm_pack_a".to_string(),
        base_name: "kernel_mlx_steel_qmm_pack_a".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 16,
        nr1: 16,
        nsg: 1,
    })
}

fn unpack_c_pipeline(runtime: &MetalRuntime) -> Result<MetalPipeline, String> {
    runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_steel_qmm_unpack_c".to_string(),
        base_name: "kernel_mlx_steel_qmm_unpack_c".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 16,
        nr1: 16,
        nsg: 1,
    })
}

fn steel_staging(
    runtime: &MetalRuntime,
    a_bytes: usize,
    c_bytes: usize,
) -> Result<(MetalBuffer, MetalBuffer), String> {
    STAGING.with(|cell| {
        let mut slot = cell.borrow_mut();
        let reuse = slot
            .as_ref()
            .is_some_and(|s| s.a_bytes >= a_bytes && s.c_bytes >= c_bytes);
        if !reuse {
            if slot.is_some() {
                eprintln!(
                    "metal: steel staging realloc A={a_bytes} C={c_bytes} (avoid mid-graph)"
                );
            }
            *slot = Some(SteelStaging {
                a: runtime.create_buffer(a_bytes.max(1), BufferStorageMode::Private)?,
                c: runtime.create_buffer(c_bytes.max(1), BufferStorageMode::Private)?,
                a_bytes: a_bytes.max(1),
                c_bytes: c_bytes.max(1),
            });
        }
        let staging = slot
            .as_ref()
            .ok_or_else(|| "steel qmm staging disappeared".to_string())?;
        Ok((staging.a.clone(), staging.c.clone()))
    })
}

fn pack_affine_q4(
    runtime: &MetalRuntime,
    src: &[u8],
    ty: u32,
    n: usize,
    k: usize,
) -> Result<AffinePacked, String> {
    let indices: Vec<i32> = (0..i32::try_from(n).map_err(|_| "N exceeds i32")?).collect();
    let values = get_rows_ggml_bytes_cpu(src, ty, k, n, &indices).ok_or_else(|| {
        format!("affine q4 pack: cannot dequantize type {ty} n={n} k={k}")
    })?;
    if values.len() != n * k {
        return Err(format!(
            "affine q4 pack: dequant len {} != {}",
            values.len(),
            n * k
        ));
    }
    let groups = k / GROUP;
    let mut packed = vec![0u8; n * (k / 2)];
    let mut scales = vec![0u8; n * groups * 2];
    let mut biases = vec![0u8; n * groups * 2];
    for row in 0..n {
        let row_vals = &values[row * k..(row + 1) * k];
        for g in 0..groups {
            let group = &row_vals[g * GROUP..(g + 1) * GROUP];
            let mut min_v = f32::INFINITY;
            let mut max_v = f32::NEG_INFINITY;
            for &v in group {
                min_v = min_v.min(v);
                max_v = max_v.max(v);
            }
            let bias = min_v;
            let scale = ((max_v - min_v) / 15.0).max(1e-8);
            let inv = 1.0 / scale;
            let scale_bits = f32_to_f16(scale).to_le_bytes();
            let bias_bits = f32_to_f16(bias).to_le_bytes();
            let so = (row * groups + g) * 2;
            scales[so] = scale_bits[0];
            scales[so + 1] = scale_bits[1];
            biases[so] = bias_bits[0];
            biases[so + 1] = bias_bits[1];
            let po = row * (k / 2) + g * (GROUP / 2);
            for i in 0..(GROUP / 2) {
                let q0 = ((group[2 * i] - bias) * inv).round().clamp(0.0, 15.0) as u8;
                let q1 = ((group[2 * i + 1] - bias) * inv).round().clamp(0.0, 15.0) as u8;
                packed[po + i] = q0 | (q1 << 4);
            }
        }
    }
    Ok(AffinePacked {
        weights: runtime.create_buffer_with_bytes(&packed, BufferStorageMode::Private)?,
        scales: runtime.create_buffer_with_bytes(&scales, BufferStorageMode::Private)?,
        biases: runtime.create_buffer_with_bytes(&biases, BufferStorageMode::Private)?,
        qparams_per_row: i32::try_from(groups).map_err(|_| "qparams exceeds i32")?,
        weight_bytes_per_row: i32::try_from(k / 2).map_err(|_| "weight row exceeds i32")?,
    })
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>()) }
}

#[derive(Clone, Copy, Debug)]
pub struct SteelBenchResult {
    pub gpu_wall_ms: f64,
    pub gpu_ts_ms: f64,
    pub wall_min: f64,
    pub wall_max: f64,
}

/// Isolated steel `qmm_t` microbench. `include_staging` adds the F32↔F16
/// transpose kernels used by the Flux hook. `chain` issues that many GEMMs
/// in one command batch (1 = MLX isolated, 228 = MLX sequential).
pub fn bench_steel_isolated(
    m: usize,
    k: usize,
    n: usize,
    warmup: u32,
    iters: u32,
    chain: u32,
    include_staging: bool,
) -> Result<SteelBenchResult, String> {
    if m == 0 || n == 0 || k < GROUP || k % GROUP != 0 || n % 32 != 0 {
        return Err(format!("steel bench shape m={m} k={k} n={n} is invalid"));
    }
    let runtime = MetalRuntime::new()?;
    if !runtime.features().has_simdgroup_mm {
        return Err("device has no simdgroup_mm".to_string());
    }
    let mut values = vec![0.0f32; n * k];
    for (i, v) in values.iter_mut().enumerate() {
        *v = ((i % 17) as f32) * 0.07 - 0.5;
    }
    let packed = pack_affine_from_f32(&runtime, &values, n, k)?;
    let mut a_f16 = vec![0u8; m * k * 2];
    for i in 0..(m * k) {
        let bits = f32_to_f16(((i % 13) as f32) * 0.03 - 0.2).to_le_bytes();
        a_f16[i * 2] = bits[0];
        a_f16[i * 2 + 1] = bits[1];
    }
    let a_f32: Vec<u8> = (0..(m * k))
        .flat_map(|i| (((i % 13) as f32) * 0.03 - 0.2).to_le_bytes())
        .collect();
    let stage_a = runtime.create_buffer_with_bytes(&a_f16, BufferStorageMode::Private)?;
    let stage_c = runtime.create_buffer(m * n * 2, BufferStorageMode::Private)?;
    let src_f32 = runtime.create_buffer_with_bytes(&a_f32, BufferStorageMode::Private)?;
    let dst_f32 = runtime.create_buffer(m * n * 4, BufferStorageMode::Private)?;
    let qmm = steel_qmm_pipeline(&runtime)?;
    let pack = pack_a_pipeline(&runtime)?;
    let unpack = unpack_c_pipeline(&runtime)?;
    let k_i = i32::try_from(k).map_err(|_| "K exceeds i32")?;
    let n_i = i32::try_from(n).map_err(|_| "N exceeds i32")?;
    let m_i = i32::try_from(m).map_err(|_| "M exceeds i32")?;
    let qmm_args = SteelQmmArgs {
        k: k_i,
        n: n_i,
        m: m_i,
    };
    let pack_args = SteelPackAArgs {
        k: k_i,
        m: m_i,
        src_k_stride: 1,
        src_m_stride: k_i,
    };
    let unpack_args = SteelUnpackCArgs {
        n: n_i,
        m: m_i,
        dst_n_stride: 1,
        dst_m_stride: n_i,
    };
    let chain = chain.max(1);
    let issue = |runtime: &MetalRuntime| -> Result<(), String> {
        if include_staging {
            runtime.dispatch_compute(
                &pack,
                bytes_of(&pack_args),
                &[
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &src_f32,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &stage_a,
                        offset_bytes: 0,
                    },
                ],
                &[],
                MetalSize {
                    width: ((m + 15) / 16) as u64,
                    height: ((k + 15) / 16) as u64,
                    depth: 1,
                },
                MetalSize {
                    width: 16,
                    height: 16,
                    depth: 1,
                },
            )?;
        }
        runtime.dispatch_compute(
            &qmm,
            bytes_of(&qmm_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &packed.weights,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &packed.scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &packed.biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &stage_a,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &stage_c,
                    offset_bytes: 0,
                },
            ],
            &[],
            MetalSize {
                width: ((n + 31) / 32) as u64,
                height: ((m + 31) / 32) as u64,
                depth: 1,
            },
            MetalSize {
                width: 32,
                height: 2,
                depth: 2,
            },
        )?;
        if include_staging {
            runtime.dispatch_compute(
                &unpack,
                bytes_of(&unpack_args),
                &[
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &stage_c,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &dst_f32,
                        offset_bytes: 0,
                    },
                ],
                &[],
                MetalSize {
                    width: ((m + 15) / 16) as u64,
                    height: ((n + 15) / 16) as u64,
                    depth: 1,
                },
                MetalSize {
                    width: 16,
                    height: 16,
                    depth: 1,
                },
            )?;
        }
        Ok(())
    };

    for _ in 0..warmup {
        runtime.begin_command_batch()?;
        for _ in 0..chain {
            issue(&runtime)?;
        }
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
    }

    let mut walls = Vec::with_capacity(iters as usize);
    let mut tss = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        runtime.reset_counters();
        let t0 = std::time::Instant::now();
        runtime.begin_command_batch()?;
        for _ in 0..chain {
            issue(&runtime)?;
        }
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        walls.push(t0.elapsed().as_secs_f64() * 1000.0);
        tss.push(runtime.counters().gpu_elapsed_ns as f64 / 1e6);
    }
    walls.sort_by(|a, b| a.partial_cmp(b).unwrap());
    tss.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(SteelBenchResult {
        gpu_wall_ms: walls[walls.len() / 2],
        gpu_ts_ms: tss[tss.len() / 2],
        wall_min: walls[0],
        wall_max: walls[walls.len() - 1],
    })
}

fn pack_affine_from_f32(
    runtime: &MetalRuntime,
    values: &[f32],
    n: usize,
    k: usize,
) -> Result<AffinePacked, String> {
    if values.len() != n * k {
        return Err(format!(
            "steel bench pack: len {} != {}",
            values.len(),
            n * k
        ));
    }
    let groups = k / GROUP;
    let mut packed = vec![0u8; n * (k / 2)];
    let mut scales = vec![0u8; n * groups * 2];
    let mut biases = vec![0u8; n * groups * 2];
    for row in 0..n {
        let row_vals = &values[row * k..(row + 1) * k];
        for g in 0..groups {
            let group = &row_vals[g * GROUP..(g + 1) * GROUP];
            let mut min_v = f32::INFINITY;
            let mut max_v = f32::NEG_INFINITY;
            for &v in group {
                min_v = min_v.min(v);
                max_v = max_v.max(v);
            }
            let bias = min_v;
            let scale = ((max_v - min_v) / 15.0).max(1e-8);
            let inv = 1.0 / scale;
            let scale_bits = f32_to_f16(scale).to_le_bytes();
            let bias_bits = f32_to_f16(bias).to_le_bytes();
            let so = (row * groups + g) * 2;
            scales[so] = scale_bits[0];
            scales[so + 1] = scale_bits[1];
            biases[so] = bias_bits[0];
            biases[so + 1] = bias_bits[1];
            let po = row * (k / 2) + g * (GROUP / 2);
            for i in 0..(GROUP / 2) {
                let q0 = ((group[2 * i] - bias) * inv).round().clamp(0.0, 15.0) as u8;
                let q1 = ((group[2 * i + 1] - bias) * inv).round().clamp(0.0, 15.0) as u8;
                packed[po + i] = q0 | (q1 << 4);
            }
        }
    }
    Ok(AffinePacked {
        weights: runtime.create_buffer_with_bytes(&packed, BufferStorageMode::Private)?,
        scales: runtime.create_buffer_with_bytes(&scales, BufferStorageMode::Private)?,
        biases: runtime.create_buffer_with_bytes(&biases, BufferStorageMode::Private)?,
        qparams_per_row: i32::try_from(groups).map_err(|_| "qparams exceeds i32")?,
        weight_bytes_per_row: i32::try_from(k / 2).map_err(|_| "weight row exceeds i32")?,
    })
}

#[allow(dead_code)]
fn _keep_quant_link() {
    let _ = quant::block_size;
}
