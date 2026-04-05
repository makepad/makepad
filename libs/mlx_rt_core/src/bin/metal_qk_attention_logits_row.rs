use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBufferBindingRef, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_mlx_rt_core::{
    fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxSafetensorsHeader,
};
use std::env;
use std::error::Error;
use std::mem::size_of;
use std::path::PathBuf;
use std::slice;
use std::time::Instant;

const INPUT_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.input_layernorm.weight";
const Q_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_norm.weight";
const K_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.k_norm.weight";
const NORM_LEN: usize = 2_816;
const EPS: f32 = 1e-6;
const ROPE_BASE: f32 = 10_000.0;
const ROPE_SCALE: f32 = 1.0;
const ROPE_OFFSET: i32 = 17;

#[derive(Clone, Copy)]
struct ProjectionPathOracle {
    prefix: &'static str,
    weight_name: &'static str,
    scales_name: &'static str,
    biases_name: &'static str,
    norm_weight_name: &'static str,
    expected_rope_hash: u64,
    expected_rope_first16_bits: [u32; 16],
}

const Q_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    prefix: "q_proj",
    weight_name: "language_model.model.layers.0.self_attn.q_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.q_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.q_proj.biases",
    norm_weight_name: Q_NORM_WEIGHT_NAME,
    expected_rope_hash: 0xCE41_D175_51C1_C0FA,
    expected_rope_first16_bits: [
        0xC027_0000,
        0xC0DE_0000,
        0xBF6C_0000,
        0xBEF8_0000,
        0x3EF2_0000,
        0xBFC8_0000,
        0xBF7C_0000,
        0x3E4E_0000,
        0xBF1E_0000,
        0x3E1D_0000,
        0xBF73_0000,
        0xBFE0_0000,
        0xBFA2_0000,
        0x3EAD_0000,
        0xBD06_0000,
        0xC047_0000,
    ],
};

const K_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    prefix: "k_proj",
    weight_name: "language_model.model.layers.0.self_attn.k_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.k_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.k_proj.biases",
    norm_weight_name: K_NORM_WEIGHT_NAME,
    expected_rope_hash: 0x9731_B5D8_139C_BB3D,
    expected_rope_first16_bits: [
        0xBE4E_0000,
        0x3DD3_0000,
        0xBD8F_0000,
        0xBBC6_0000,
        0xBDA6_0000,
        0x3E0F_0000,
        0x3DCB_0000,
        0x3E35_0000,
        0xBD66_0000,
        0xBE66_0000,
        0xBD12_0000,
        0x3C18_0000,
        0xBDEC_0000,
        0x3D99_0000,
        0x3CAC_0000,
        0x3DD2_0000,
    ],
};

const EXPECTED_LOGITS_HASH: u64 = 0x2BF6_1C33_DE46_DDBE;
const EXPECTED_LOGITS_BITS: [u32; 16] = [
    0x4032_0000,
    0xC107_0000,
    0xC079_0000,
    0x3F94_0000,
    0x412B_0000,
    0x4146_0000,
    0x3EAA_0000,
    0xBFE9_0000,
    0xC078_0000,
    0xBFE9_0000,
    0xC0F4_0000,
    0x4196_0000,
    0x411F_0000,
    0xC089_0000,
    0x411A_0000,
    0xBFA4_0000,
];

#[repr(C)]
struct MlxRmsNormRowArgs {
    n: u32,
    eps: f32,
}

#[repr(C)]
struct MlxRmsNormRowsArgs {
    n: u32,
    row_stride: u32,
    row_count: u32,
    eps: f32,
}

#[repr(C)]
struct MlxAffineQprojRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

#[repr(C)]
struct MlxRopeSingleArgs {
    half_dims: u32,
    row_stride: u32,
    row_count: u32,
    offset: i32,
    scale: f32,
    base_log2: f32,
}

#[repr(C)]
struct MlxGqaAttentionLogitsArgs {
    head_dim: u32,
    q_head_stride: u32,
    k_head_stride: u32,
    q_head_count: u32,
    q_heads_per_kv: u32,
}

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn bytes_from_bf16_words(words: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 2);
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut warmup_iters = 0usize;
    let mut bench_iters = 1usize;

    let mut args_iter = env::args().skip(1);
    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--warmup" => {
                let value = args_iter.next().ok_or("--warmup expects a value")?;
                warmup_iters = value.parse::<usize>()?;
            }
            "--iters" => {
                let value = args_iter.next().ok_or("--iters expects a value")?;
                bench_iters = value.parse::<usize>()?;
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metal_qk_attention_logits_row [model.safetensors] [--warmup N] [--iters N]"
                );
                return Ok(());
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {arg}").into());
            }
            _ => {
                model_path = PathBuf::from(arg);
            }
        }
    }
    if bench_iters == 0 {
        return Err("--iters must be greater than zero".into());
    }

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
    if !runtime.features().has_bfloat {
        return Err("Metal device does not report BF16 support".into());
    }

    let x_words = gemma4_qproj_case_input_bf16_words(NORM_LEN);
    let x_bytes = bytes_from_bf16_words(&x_words);
    let input_norm_weight_bytes = header.read_tensor_bytes(INPUT_NORM_WEIGHT_NAME)?;
    let q_weight_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.weight_name)?;
    let q_scales_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.scales_name)?;
    let q_biases_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.biases_name)?;
    let q_norm_weight_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.norm_weight_name)?;
    let k_weight_bytes = header.read_tensor_bytes(K_PATH_ORACLE.weight_name)?;
    let k_scales_bytes = header.read_tensor_bytes(K_PATH_ORACLE.scales_name)?;
    let k_biases_bytes = header.read_tensor_bytes(K_PATH_ORACLE.biases_name)?;
    let k_norm_weight_bytes = header.read_tensor_bytes(K_PATH_ORACLE.norm_weight_name)?;

    let q_weight_entry = header
        .tensor(Q_PATH_ORACLE.weight_name)
        .ok_or("missing q projection weight entry")?;
    let q_scales_entry = header
        .tensor(Q_PATH_ORACLE.scales_name)
        .ok_or("missing q projection scales entry")?;
    let q_norm_weight_entry = header
        .tensor(Q_PATH_ORACLE.norm_weight_name)
        .ok_or("missing q norm weight entry")?;
    let k_weight_entry = header
        .tensor(K_PATH_ORACLE.weight_name)
        .ok_or("missing k projection weight entry")?;
    let k_scales_entry = header
        .tensor(K_PATH_ORACLE.scales_name)
        .ok_or("missing k projection scales entry")?;
    let k_norm_weight_entry = header
        .tensor(K_PATH_ORACLE.norm_weight_name)
        .ok_or("missing k norm weight entry")?;

    let q_out_len = usize::try_from(q_weight_entry.shape[0])?;
    let k_out_len = usize::try_from(k_weight_entry.shape[0])?;
    let head_dim = usize::try_from(q_norm_weight_entry.shape[0])?;
    let k_head_dim = usize::try_from(k_norm_weight_entry.shape[0])?;
    if head_dim == 0 || head_dim != k_head_dim {
        return Err(format!("invalid q/k head_dim: q={head_dim} k={k_head_dim}").into());
    }
    if q_out_len % head_dim != 0 || k_out_len % head_dim != 0 {
        return Err(format!(
            "invalid q/k head layout: q_out_len={} k_out_len={} head_dim={}",
            q_out_len, k_out_len, head_dim
        )
        .into());
    }
    let q_head_count = q_out_len / head_dim;
    let k_head_count = k_out_len / head_dim;
    if k_head_count == 0 || q_head_count % k_head_count != 0 {
        return Err(format!(
            "invalid grouped-query head layout: q_head_count={} k_head_count={}",
            q_head_count, k_head_count
        )
        .into());
    }
    let q_heads_per_kv = q_head_count / k_head_count;

    let x_buf = runtime.create_buffer_with_bytes(&x_bytes, BufferStorageMode::Private)?;
    let input_norm_weight_buf =
        runtime.create_buffer_with_bytes(&input_norm_weight_bytes, BufferStorageMode::Private)?;
    let h_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Private)?;

    let q_weight_buf = runtime.create_buffer_with_bytes(&q_weight_bytes, BufferStorageMode::Private)?;
    let q_scales_buf =
        runtime.create_buffer_with_bytes(&q_scales_bytes, BufferStorageMode::Private)?;
    let q_biases_buf =
        runtime.create_buffer_with_bytes(&q_biases_bytes, BufferStorageMode::Private)?;
    let q_norm_weight_buf =
        runtime.create_buffer_with_bytes(&q_norm_weight_bytes, BufferStorageMode::Private)?;
    let q_proj_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_norm_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_rope_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;

    let k_weight_buf = runtime.create_buffer_with_bytes(&k_weight_bytes, BufferStorageMode::Private)?;
    let k_scales_buf =
        runtime.create_buffer_with_bytes(&k_scales_bytes, BufferStorageMode::Private)?;
    let k_biases_buf =
        runtime.create_buffer_with_bytes(&k_biases_bytes, BufferStorageMode::Private)?;
    let k_norm_weight_buf =
        runtime.create_buffer_with_bytes(&k_norm_weight_bytes, BufferStorageMode::Private)?;
    let k_proj_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_norm_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_rope_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;

    let logits_buf = runtime.create_buffer(q_head_count * 2, BufferStorageMode::Private)?;

    let rms_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let proj_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_row_bf16".to_string(),
        base_name: "kernel_mlx_affine_qmv_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let head_norm_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_rows_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_rows_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let rope_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rope_single_bf16".to_string(),
        base_name: "kernel_mlx_rope_single_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let logits_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_gqa_attention_logits_bf16".to_string(),
        base_name: "kernel_mlx_gqa_attention_logits_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;

    let n_reads = 4usize;
    let simd_size = 32usize;

    let rms_threadgroup_needed = NORM_LEN.div_ceil(n_reads);
    let rms_simds_needed = rms_threadgroup_needed.div_ceil(simd_size);
    let rms_threadgroup_size = simd_size * rms_simds_needed;
    if rms_threadgroup_size as u64 > rms_pipeline.max_threads_per_threadgroup {
        return Err(format!(
            "rms threadgroup_size {} exceeds pipeline max {}",
            rms_threadgroup_size, rms_pipeline.max_threads_per_threadgroup
        )
        .into());
    }

    let head_norm_threadgroup_needed = head_dim.div_ceil(n_reads);
    let head_norm_simds_needed = head_norm_threadgroup_needed.div_ceil(simd_size);
    let head_norm_threadgroup_size = simd_size * head_norm_simds_needed;
    if head_norm_threadgroup_size as u64 > head_norm_pipeline.max_threads_per_threadgroup {
        return Err(format!(
            "head_norm threadgroup_size {} exceeds pipeline max {}",
            head_norm_threadgroup_size, head_norm_pipeline.max_threads_per_threadgroup
        )
        .into());
    }

    let rms_args = MlxRmsNormRowArgs {
        n: NORM_LEN as u32,
        eps: EPS,
    };
    let q_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: q_weight_entry.shape[1] as u32,
        qparams_per_row: q_scales_entry.shape[1] as u32,
        out_rows: q_out_len as u32,
    };
    let k_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: k_weight_entry.shape[1] as u32,
        qparams_per_row: k_scales_entry.shape[1] as u32,
        out_rows: k_out_len as u32,
    };
    let q_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: q_head_count as u32,
        eps: EPS,
    };
    let k_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: k_head_count as u32,
        eps: EPS,
    };
    let q_rope_args = MlxRopeSingleArgs {
        half_dims: (head_dim / 2) as u32,
        row_stride: head_dim as u32,
        row_count: q_head_count as u32,
        offset: ROPE_OFFSET,
        scale: ROPE_SCALE,
        base_log2: ROPE_BASE.log2(),
    };
    let k_rope_args = MlxRopeSingleArgs {
        half_dims: (head_dim / 2) as u32,
        row_stride: head_dim as u32,
        row_count: k_head_count as u32,
        offset: ROPE_OFFSET,
        scale: ROPE_SCALE,
        base_log2: ROPE_BASE.log2(),
    };
    let logits_args = MlxGqaAttentionLogitsArgs {
        head_dim: head_dim as u32,
        q_head_stride: head_dim as u32,
        k_head_stride: head_dim as u32,
        q_head_count: q_head_count as u32,
        q_heads_per_kv: q_heads_per_kv as u32,
    };

    let rms_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &x_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &input_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &h_buf,
            offset_bytes: 0,
        },
    ];
    let q_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &q_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &q_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &q_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &q_proj_buf,
            offset_bytes: 0,
        },
    ];
    let q_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &q_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &q_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &q_norm_buf,
            offset_bytes: 0,
        },
    ];
    let q_rope_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &q_norm_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &q_rope_buf,
            offset_bytes: 0,
        },
    ];
    let k_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &k_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &k_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &k_proj_buf,
            offset_bytes: 0,
        },
    ];
    let k_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &k_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &k_norm_buf,
            offset_bytes: 0,
        },
    ];
    let k_rope_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &k_norm_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_rope_buf,
            offset_bytes: 0,
        },
    ];
    let logits_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &q_rope_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_rope_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &logits_buf,
            offset_bytes: 0,
        },
    ];

    let rms_threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let rms_threads_per_threadgroup = MetalSize {
        width: rms_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let q_proj_threadgroups = MetalSize {
        width: 1,
        height: (q_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let q_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let q_head_norm_threadgroups = MetalSize {
        width: q_head_count as u64,
        height: 1,
        depth: 1,
    };
    let q_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let q_rope_threadgroups = MetalSize {
        width: ((head_dim / 2) as u64).div_ceil(32),
        height: q_head_count as u64,
        depth: 1,
    };
    let q_rope_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 1,
        depth: 1,
    };
    let k_proj_threadgroups = MetalSize {
        width: 1,
        height: (k_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let k_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let k_head_norm_threadgroups = MetalSize {
        width: k_head_count as u64,
        height: 1,
        depth: 1,
    };
    let k_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let k_rope_threadgroups = MetalSize {
        width: ((head_dim / 2) as u64).div_ceil(32),
        height: k_head_count as u64,
        depth: 1,
    };
    let k_rope_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 1,
        depth: 1,
    };
    let logits_threadgroups = MetalSize {
        width: q_head_count as u64,
        height: 1,
        depth: 1,
    };
    let logits_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 1,
        depth: 1,
    };

    let rms_args_bytes = bytes_of(&rms_args);
    let q_proj_args_bytes = bytes_of(&q_proj_args);
    let k_proj_args_bytes = bytes_of(&k_proj_args);
    let q_head_norm_args_bytes = bytes_of(&q_head_norm_args);
    let k_head_norm_args_bytes = bytes_of(&k_head_norm_args);
    let q_rope_args_bytes = bytes_of(&q_rope_args);
    let k_rope_args_bytes = bytes_of(&k_rope_args);
    let logits_args_bytes = bytes_of(&logits_args);

    let run_once = || -> Result<(), Box<dyn Error>> {
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            rms_args_bytes,
            &rms_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            q_proj_args_bytes,
            &q_proj_bindings,
            &[],
            q_proj_threadgroups,
            q_proj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &head_norm_pipeline,
            q_head_norm_args_bytes,
            &q_head_norm_bindings,
            &[],
            q_head_norm_threadgroups,
            q_head_norm_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rope_pipeline,
            q_rope_args_bytes,
            &q_rope_bindings,
            &[],
            q_rope_threadgroups,
            q_rope_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            k_proj_args_bytes,
            &k_proj_bindings,
            &[],
            k_proj_threadgroups,
            k_proj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &head_norm_pipeline,
            k_head_norm_args_bytes,
            &k_head_norm_bindings,
            &[],
            k_head_norm_threadgroups,
            k_head_norm_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rope_pipeline,
            k_rope_args_bytes,
            &k_rope_bindings,
            &[],
            k_rope_threadgroups,
            k_rope_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &logits_pipeline,
            logits_args_bytes,
            &logits_bindings,
            &[],
            logits_threadgroups,
            logits_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Ok(())
    };

    for _ in 0..warmup_iters {
        run_once()?;
    }

    let start = Instant::now();
    for _ in 0..bench_iters {
        run_once()?;
    }
    let elapsed = start.elapsed();

    run_once()?;

    let decode_bits = |bytes: Vec<u8>| {
        bytes.chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .map(bf16_word_to_f32)
            .map(f32::to_bits)
            .collect::<Vec<_>>()
    };

    let q_rope_bits = decode_bits(runtime.read_buffer(&q_rope_buf, q_out_len * 2)?);
    let k_rope_bits = decode_bits(runtime.read_buffer(&k_rope_buf, k_out_len * 2)?);
    let logits_bits = decode_bits(runtime.read_buffer(&logits_buf, q_head_count * 2)?);

    let q_rope_hash = fnv1a64_u32_words(&q_rope_bits);
    let k_rope_hash = fnv1a64_u32_words(&k_rope_bits);
    let logits_hash = fnv1a64_u32_words(&logits_bits);

    if q_rope_bits[..16] != Q_PATH_ORACLE.expected_rope_first16_bits {
        return Err(format!(
            "{} rope first16 mismatch: got {:08X?} expected {:08X?}",
            Q_PATH_ORACLE.prefix,
            &q_rope_bits[..16],
            Q_PATH_ORACLE.expected_rope_first16_bits
        )
        .into());
    }
    if q_rope_hash != Q_PATH_ORACLE.expected_rope_hash {
        return Err(format!(
            "{} rope hash mismatch: got 0x{q_rope_hash:016X} expected 0x{:016X}",
            Q_PATH_ORACLE.prefix, Q_PATH_ORACLE.expected_rope_hash
        )
        .into());
    }
    if k_rope_bits[..16] != K_PATH_ORACLE.expected_rope_first16_bits {
        return Err(format!(
            "{} rope first16 mismatch: got {:08X?} expected {:08X?}",
            K_PATH_ORACLE.prefix,
            &k_rope_bits[..16],
            K_PATH_ORACLE.expected_rope_first16_bits
        )
        .into());
    }
    if k_rope_hash != K_PATH_ORACLE.expected_rope_hash {
        return Err(format!(
            "{} rope hash mismatch: got 0x{k_rope_hash:016X} expected 0x{:016X}",
            K_PATH_ORACLE.prefix, K_PATH_ORACLE.expected_rope_hash
        )
        .into());
    }
    if logits_bits != EXPECTED_LOGITS_BITS {
        return Err(format!(
            "attention logits mismatch: got {:08X?} expected {:08X?}",
            &logits_bits, EXPECTED_LOGITS_BITS
        )
        .into());
    }
    if logits_hash != EXPECTED_LOGITS_HASH {
        return Err(format!(
            "attention logits hash mismatch: got 0x{logits_hash:016X} expected 0x{:016X}",
            EXPECTED_LOGITS_HASH
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("input_norm_weight_name={INPUT_NORM_WEIGHT_NAME}");
    println!("q_weight_name={}", Q_PATH_ORACLE.weight_name);
    println!("k_weight_name={}", K_PATH_ORACLE.weight_name);
    println!("q_norm_weight_name={}", Q_PATH_ORACLE.norm_weight_name);
    println!("k_norm_weight_name={}", K_PATH_ORACLE.norm_weight_name);
    println!("eps={EPS}");
    println!("rope_base={ROPE_BASE}");
    println!("rope_scale={ROPE_SCALE}");
    println!("rope_offset={ROPE_OFFSET}");
    println!("warmup_iters={warmup_iters}");
    println!("bench_iters={bench_iters}");
    println!("rms_threads_per_threadgroup={rms_threadgroup_size}");
    println!("head_norm_threads_per_threadgroup={head_norm_threadgroup_size}");
    println!("q_head_count={q_head_count}");
    println!("k_head_count={k_head_count}");
    println!("q_heads_per_kv={q_heads_per_kv}");
    println!("head_dim={head_dim}");
    println!("total_ns={}", elapsed.as_nanos());
    println!(
        "avg_ns={:.0}",
        elapsed.as_secs_f64() * 1e9 / bench_iters as f64
    );
    println!(
        "avg_us={:.3}",
        elapsed.as_secs_f64() * 1e6 / bench_iters as f64
    );
    println!("q_rope_full_row_fnv1a64=0x{q_rope_hash:016X}");
    println!("k_rope_full_row_fnv1a64=0x{k_rope_hash:016X}");
    println!("attention_logits_fnv1a64=0x{logits_hash:016X}");
    print!("attention_logits_f32_bits=");
    for (index, bits) in logits_bits.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();
    println!("status=ok");

    Ok(())
}
