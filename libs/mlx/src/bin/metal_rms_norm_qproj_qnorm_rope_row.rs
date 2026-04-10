use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBufferBindingRef, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_mlx::{
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
struct ProjectionOracle {
    prefix: &'static str,
    weight_name: &'static str,
    scales_name: &'static str,
    biases_name: &'static str,
    norm_weight_name: &'static str,
    expected_proj_hash: u64,
    expected_proj_first16_bits: [u32; 16],
    expected_norm_hash: u64,
    expected_norm_first16_bits: [u32; 16],
    expected_rope_hash: u64,
    expected_rope_first16_bits: [u32; 16],
}

const Q_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "q_proj",
    weight_name: "language_model.model.layers.0.self_attn.q_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.q_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.q_proj.biases",
    norm_weight_name: Q_NORM_WEIGHT_NAME,
    expected_proj_hash: 0xA2FC_CDC3_E3B9_9A86,
    expected_proj_first16_bits: [
        0x3F20_0000,
        0x425B_0000,
        0x40E3_0000,
        0xBFFF_0000,
        0x4088_0000,
        0xC07A_0000,
        0xC0E5_0000,
        0x417F_0000,
        0x408C_0000,
        0x3DE2_0000,
        0x40A4_0000,
        0x40BE_0000,
        0xBFC0_0000,
        0x405F_0000,
        0xBE52_0000,
        0xC1C6_0000,
    ],
    expected_norm_hash: 0xFDFC_27D5_C13C_170B,
    expected_norm_first16_bits: [
        0x3DA1_0000,
        0x40DC_0000,
        0x3F63_0000,
        0xBE80_0000,
        0x3F08_0000,
        0xBEFB_0000,
        0xBF65_0000,
        0x4000_0000,
        0x3F0C_0000,
        0x3C62_0000,
        0x3F25_0000,
        0x3F3E_0000,
        0xBE40_0000,
        0x3EDF_0000,
        0xBCD3_0000,
        0xC047_0000,
    ],
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

const K_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "k_proj",
    weight_name: "language_model.model.layers.0.self_attn.k_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.k_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.k_proj.biases",
    norm_weight_name: K_NORM_WEIGHT_NAME,
    expected_proj_hash: 0x199D_94F3_2CC8_9820,
    expected_proj_first16_bits: [
        0x41EA_0000,
        0xC0C8_0000,
        0xBF44_0000,
        0xBFC8_0000,
        0xC0CD_0000,
        0x4153_0000,
        0xC104_0000,
        0xC159_0000,
        0x4088_0000,
        0x4121_0000,
        0x40CB_0000,
        0xC0C5_0000,
        0xC0BE_0000,
        0x40A1_0000,
        0x3FE8_0000,
        0x4105_0000,
    ],
    expected_norm_hash: 0x77A8_4686_A3CE_50CB,
    expected_norm_first16_bits: [
        0x3EC9_0000,
        0xBDAC_0000,
        0xBC29_0000,
        0xBCAC_0000,
        0xBDB1_0000,
        0x3E36_0000,
        0xBDE4_0000,
        0xBE3B_0000,
        0x3D6A_0000,
        0x3E0B_0000,
        0x3DAF_0000,
        0xBDAA_0000,
        0xBDA3_0000,
        0x3D8B_0000,
        0x3CC7_0000,
        0x3DE5_0000,
    ],
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

fn projection_oracle(prefix: &str) -> Result<ProjectionOracle, Box<dyn Error>> {
    match prefix {
        "q_proj" => Ok(Q_PROJ_ORACLE),
        "k_proj" => Ok(K_PROJ_ORACLE),
        _ => Err("--tensor-prefix must be q_proj or k_proj".into()),
    }
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
    let mut tensor_prefix = "q_proj".to_string();

    let mut args_iter = env::args().skip(1);
    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--tensor-prefix" => {
                tensor_prefix = args_iter.next().ok_or("--tensor-prefix expects a value")?;
            }
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
                    "Usage: metal_rms_norm_qproj_qnorm_rope_row [model.safetensors] [--tensor-prefix q_proj|k_proj] [--warmup N] [--iters N]"
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
    let oracle = projection_oracle(&tensor_prefix)?;

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
    if !runtime.features().has_bfloat {
        return Err("Metal device does not report BF16 support".into());
    }

    let x_words = gemma4_qproj_case_input_bf16_words(NORM_LEN);
    let x_bytes = bytes_from_bf16_words(&x_words);
    let input_norm_weight_bytes = header.read_tensor_bytes(INPUT_NORM_WEIGHT_NAME)?;
    let proj_weight_bytes = header.read_tensor_bytes(oracle.weight_name)?;
    let proj_scales_bytes = header.read_tensor_bytes(oracle.scales_name)?;
    let proj_biases_bytes = header.read_tensor_bytes(oracle.biases_name)?;
    let norm_weight_bytes = header.read_tensor_bytes(oracle.norm_weight_name)?;

    let proj_weight_entry = header
        .tensor(oracle.weight_name)
        .ok_or("missing projection weight entry")?;
    let proj_scales_entry = header
        .tensor(oracle.scales_name)
        .ok_or("missing projection scales entry")?;
    let norm_weight_entry = header
        .tensor(oracle.norm_weight_name)
        .ok_or("missing norm weight entry")?;

    let proj_out_len = usize::try_from(proj_weight_entry.shape[0])?;
    let proj_head_dim = usize::try_from(norm_weight_entry.shape[0])?;
    if proj_head_dim == 0 || proj_out_len % proj_head_dim != 0 {
        return Err(format!(
            "invalid head norm layout: out_len={} head_dim={}",
            proj_out_len, proj_head_dim
        )
        .into());
    }
    let proj_head_count = proj_out_len / proj_head_dim;

    let x_buf = runtime.create_buffer_with_bytes(&x_bytes, BufferStorageMode::Private)?;
    let input_norm_weight_buf =
        runtime.create_buffer_with_bytes(&input_norm_weight_bytes, BufferStorageMode::Private)?;
    let h_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Private)?;
    let proj_weight_buf =
        runtime.create_buffer_with_bytes(&proj_weight_bytes, BufferStorageMode::Private)?;
    let proj_scales_buf =
        runtime.create_buffer_with_bytes(&proj_scales_bytes, BufferStorageMode::Private)?;
    let proj_biases_buf =
        runtime.create_buffer_with_bytes(&proj_biases_bytes, BufferStorageMode::Private)?;
    let norm_weight_buf =
        runtime.create_buffer_with_bytes(&norm_weight_bytes, BufferStorageMode::Private)?;
    let proj_buf = runtime.create_buffer(proj_out_len * 2, BufferStorageMode::Private)?;
    let norm_buf = runtime.create_buffer(proj_out_len * 2, BufferStorageMode::Private)?;
    let rope_buf = runtime.create_buffer(proj_out_len * 2, BufferStorageMode::Private)?;

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

    let head_norm_threadgroup_needed = proj_head_dim.div_ceil(n_reads);
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
    let proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: proj_weight_entry.shape[1] as u32,
        qparams_per_row: proj_scales_entry.shape[1] as u32,
        out_rows: proj_out_len as u32,
    };
    let head_norm_args = MlxRmsNormRowsArgs {
        n: proj_head_dim as u32,
        row_stride: proj_head_dim as u32,
        row_count: proj_head_count as u32,
        eps: EPS,
    };
    let rope_args = MlxRopeSingleArgs {
        half_dims: (proj_head_dim / 2) as u32,
        row_stride: proj_head_dim as u32,
        row_count: proj_head_count as u32,
        offset: ROPE_OFFSET,
        scale: ROPE_SCALE,
        base_log2: ROPE_BASE.log2(),
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
    let proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &proj_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &proj_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &proj_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &proj_buf,
            offset_bytes: 0,
        },
    ];
    let head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &norm_buf,
            offset_bytes: 0,
        },
    ];
    let rope_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &norm_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &rope_buf,
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
    let proj_threadgroups = MetalSize {
        width: 1,
        height: (proj_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let head_norm_threadgroups = MetalSize {
        width: proj_head_count as u64,
        height: 1,
        depth: 1,
    };
    let head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let rope_threadgroups = MetalSize {
        width: ((proj_head_dim / 2) as u64).div_ceil(32),
        height: proj_head_count as u64,
        depth: 1,
    };
    let rope_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 1,
        depth: 1,
    };

    let rms_args_bytes = bytes_of(&rms_args);
    let proj_args_bytes = bytes_of(&proj_args);
    let head_norm_args_bytes = bytes_of(&head_norm_args);
    let rope_args_bytes = bytes_of(&rope_args);

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
            proj_args_bytes,
            &proj_bindings,
            &[],
            proj_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &head_norm_pipeline,
            head_norm_args_bytes,
            &head_norm_bindings,
            &[],
            head_norm_threadgroups,
            head_norm_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rope_pipeline,
            rope_args_bytes,
            &rope_bindings,
            &[],
            rope_threadgroups,
            rope_threads_per_threadgroup,
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

    let proj_bytes = runtime.read_buffer(&proj_buf, proj_out_len * 2)?;
    let norm_bytes = runtime.read_buffer(&norm_buf, proj_out_len * 2)?;
    let rope_bytes = runtime.read_buffer(&rope_buf, proj_out_len * 2)?;

    let decode_bits = |bytes: Vec<u8>| {
        bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .map(bf16_word_to_f32)
            .map(f32::to_bits)
            .collect::<Vec<_>>()
    };

    let proj_bits = decode_bits(proj_bytes);
    let norm_bits = decode_bits(norm_bytes);
    let rope_bits = decode_bits(rope_bytes);

    let proj_hash = fnv1a64_u32_words(&proj_bits);
    let norm_hash = fnv1a64_u32_words(&norm_bits);
    let rope_hash = fnv1a64_u32_words(&rope_bits);

    if proj_bits[..16] != oracle.expected_proj_first16_bits {
        return Err(format!(
            "{} first16 mismatch: got {:08X?} expected {:08X?}",
            oracle.prefix,
            &proj_bits[..16],
            oracle.expected_proj_first16_bits
        )
        .into());
    }
    if proj_hash != oracle.expected_proj_hash {
        return Err(format!(
            "{} hash mismatch: got 0x{proj_hash:016X} expected 0x{:016X}",
            oracle.prefix, oracle.expected_proj_hash
        )
        .into());
    }
    if norm_bits[..16] != oracle.expected_norm_first16_bits {
        return Err(format!(
            "{} head_norm first16 mismatch: got {:08X?} expected {:08X?}",
            oracle.prefix,
            &norm_bits[..16],
            oracle.expected_norm_first16_bits
        )
        .into());
    }
    if norm_hash != oracle.expected_norm_hash {
        return Err(format!(
            "{} head_norm hash mismatch: got 0x{norm_hash:016X} expected 0x{:016X}",
            oracle.prefix, oracle.expected_norm_hash
        )
        .into());
    }
    if rope_bits[..16] != oracle.expected_rope_first16_bits {
        return Err(format!(
            "{} rope first16 mismatch: got {:08X?} expected {:08X?}",
            oracle.prefix,
            &rope_bits[..16],
            oracle.expected_rope_first16_bits
        )
        .into());
    }
    if rope_hash != oracle.expected_rope_hash {
        return Err(format!(
            "{} rope hash mismatch: got 0x{rope_hash:016X} expected 0x{:016X}",
            oracle.prefix, oracle.expected_rope_hash
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("input_norm_weight_name={INPUT_NORM_WEIGHT_NAME}");
    println!("tensor_prefix={}", oracle.prefix);
    println!("proj_weight_name={}", oracle.weight_name);
    println!("norm_weight_name={}", oracle.norm_weight_name);
    println!("eps={EPS}");
    println!("rope_base={ROPE_BASE}");
    println!("rope_scale={ROPE_SCALE}");
    println!("rope_offset={ROPE_OFFSET}");
    println!("warmup_iters={warmup_iters}");
    println!("bench_iters={bench_iters}");
    println!("rms_threads_per_threadgroup={rms_threadgroup_size}");
    println!("head_norm_threads_per_threadgroup={head_norm_threadgroup_size}");
    println!("proj_head_count={proj_head_count}");
    println!("proj_head_dim={proj_head_dim}");
    println!("total_ns={}", elapsed.as_nanos());
    println!(
        "avg_ns={:.0}",
        elapsed.as_secs_f64() * 1e9 / bench_iters as f64
    );
    println!(
        "avg_us={:.3}",
        elapsed.as_secs_f64() * 1e6 / bench_iters as f64
    );
    println!("proj_full_row_fnv1a64=0x{proj_hash:016X}");
    println!("head_norm_full_row_fnv1a64=0x{norm_hash:016X}");
    println!("rope_full_row_fnv1a64=0x{rope_hash:016X}");
    print!("rope_first16_f32_bits=");
    for (index, bits) in rope_bits.iter().take(16).enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();
    println!("status=ok");

    Ok(())
}
