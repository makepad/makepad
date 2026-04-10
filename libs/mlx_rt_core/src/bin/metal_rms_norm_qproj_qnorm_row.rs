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

#[derive(Clone, Copy)]
struct ProjectionOracle {
    prefix: &'static str,
    weight_name: &'static str,
    scales_name: &'static str,
    biases_name: &'static str,
    head_dim_ref_name: &'static str,
    norm_weight_name: Option<&'static str>,
    head_norm_kind: &'static str,
    expected_proj_hash: u64,
    expected_proj_first16_bits: [u32; 16],
    expected_norm_hash: u64,
    expected_norm_first16_bits: [u32; 16],
}

const Q_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "q_proj",
    weight_name: "language_model.model.layers.0.self_attn.q_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.q_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.q_proj.biases",
    head_dim_ref_name: Q_NORM_WEIGHT_NAME,
    norm_weight_name: Some(Q_NORM_WEIGHT_NAME),
    head_norm_kind: "weighted",
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
};

const K_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "k_proj",
    weight_name: "language_model.model.layers.0.self_attn.k_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.k_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.k_proj.biases",
    head_dim_ref_name: K_NORM_WEIGHT_NAME,
    norm_weight_name: Some(K_NORM_WEIGHT_NAME),
    head_norm_kind: "weighted",
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
};

const V_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "v_proj",
    weight_name: "language_model.model.layers.0.self_attn.v_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.v_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.v_proj.biases",
    head_dim_ref_name: K_NORM_WEIGHT_NAME,
    norm_weight_name: None,
    head_norm_kind: "no_scale",
    expected_proj_hash: 0x870F_85C9_945E_F8DB,
    expected_proj_first16_bits: [
        0xBFE4_0000,
        0xC04A_0000,
        0xC10C_0000,
        0xC0DD_0000,
        0x3E06_0000,
        0xC0A5_0000,
        0x4030_0000,
        0x4127_0000,
        0x4024_0000,
        0x3F94_0000,
        0x3F86_0000,
        0x3FB1_0000,
        0xC084_0000,
        0x40D9_0000,
        0x40FD_0000,
        0x3FE8_0000,
    ],
    expected_norm_hash: 0xDAC6_F97C_1CD9_387D,
    expected_norm_first16_bits: [
        0xBE60_0000,
        0xBEC6_0000,
        0xBF89_0000,
        0xBF59_0000,
        0x3C84_0000,
        0xBF22_0000,
        0x3EAD_0000,
        0x3FA4_0000,
        0x3EA1_0000,
        0x3E11_0000,
        0x3E04_0000,
        0x3E2E_0000,
        0xBF02_0000,
        0x3F55_0000,
        0x3F78_0000,
        0x3E64_0000,
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

fn projection_oracle(prefix: &str) -> Result<ProjectionOracle, Box<dyn Error>> {
    match prefix {
        "q_proj" => Ok(Q_PROJ_ORACLE),
        "k_proj" => Ok(K_PROJ_ORACLE),
        "v_proj" => Ok(V_PROJ_ORACLE),
        _ => Err("--tensor-prefix must be q_proj, k_proj, or v_proj".into()),
    }
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
                    "Usage: metal_rms_norm_qproj_qnorm_row [model.safetensors] [--tensor-prefix q_proj|k_proj|v_proj] [--warmup N] [--iters N]"
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

    let proj_weight_entry = header
        .tensor(oracle.weight_name)
        .ok_or("missing projection weight entry")?;
    let proj_scales_entry = header
        .tensor(oracle.scales_name)
        .ok_or("missing projection scales entry")?;
    let head_dim_entry = header
        .tensor(oracle.head_dim_ref_name)
        .ok_or("missing head-dim reference entry")?;

    let proj_out_len = usize::try_from(proj_weight_entry.shape[0])?;
    let proj_head_dim = usize::try_from(head_dim_entry.shape[0])?;
    if proj_head_dim == 0 || proj_out_len % proj_head_dim != 0 {
        return Err(format!(
            "invalid head norm layout: out_len={} head_dim={}",
            proj_out_len, proj_head_dim
        )
        .into());
    }
    let proj_head_count = proj_out_len / proj_head_dim;

    let norm_weight_bytes = if let Some(norm_weight_name) = oracle.norm_weight_name {
        header.read_tensor_bytes(norm_weight_name)?
    } else {
        bytes_from_bf16_words(&vec![0x3F80u16; proj_head_dim])
    };

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
    let out_buf = runtime.create_buffer(proj_out_len * 2, BufferStorageMode::Private)?;

    let rms_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let qproj_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
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
    let qproj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: proj_weight_entry.shape[1] as u32,
        qparams_per_row: proj_scales_entry.shape[1] as u32,
        out_rows: proj_out_len as u32,
    };
    let qnorm_args = MlxRmsNormRowsArgs {
        n: proj_head_dim as u32,
        row_stride: proj_head_dim as u32,
        row_count: proj_head_count as u32,
        eps: EPS,
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
    let qproj_bindings = [
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
    let qnorm_bindings = [
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
            buffer: &out_buf,
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
    let qproj_threadgroups = MetalSize {
        width: 1,
        height: (proj_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let qproj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let qnorm_threadgroups = MetalSize {
        width: proj_head_count as u64,
        height: 1,
        depth: 1,
    };
    let qnorm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };

    let rms_args_bytes = bytes_of(&rms_args);
    let qproj_args_bytes = bytes_of(&qproj_args);
    let qnorm_args_bytes = bytes_of(&qnorm_args);

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
            &qproj_pipeline,
            qproj_args_bytes,
            &qproj_bindings,
            &[],
            qproj_threadgroups,
            qproj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &head_norm_pipeline,
            qnorm_args_bytes,
            &qnorm_bindings,
            &[],
            qnorm_threadgroups,
            qnorm_threads_per_threadgroup,
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

    let q_bytes = runtime.read_buffer(&proj_buf, proj_out_len * 2)?;
    let out_bytes = runtime.read_buffer(&out_buf, proj_out_len * 2)?;

    let q_words = q_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    let out_words = out_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();

    let q_bits = q_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let out_bits = out_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect::<Vec<_>>();

    let proj_hash = fnv1a64_u32_words(&q_bits);
    let out_hash = fnv1a64_u32_words(&out_bits);

    if q_bits[..16] != oracle.expected_proj_first16_bits {
        return Err(format!(
            "{} first16 mismatch: got {:08X?} expected {:08X?}",
            oracle.prefix,
            &q_bits[..16],
            oracle.expected_proj_first16_bits
        )
        .into());
    }
    if proj_hash != oracle.expected_proj_hash {
        return Err(format!(
            "{} hash mismatch: got 0x{proj_hash:016X} expected 0x{:016X}, first16={:08X?}",
            oracle.prefix,
            oracle.expected_proj_hash,
            &q_bits[..16]
        )
        .into());
    }
    if out_bits[..16] != oracle.expected_norm_first16_bits {
        return Err(format!(
            "{}_head_norm first16 mismatch: got {:08X?} expected {:08X?}",
            oracle.prefix,
            &out_bits[..16],
            oracle.expected_norm_first16_bits
        )
        .into());
    }
    if out_hash != oracle.expected_norm_hash {
        return Err(format!(
            "{}_head_norm hash mismatch: got 0x{out_hash:016X} expected 0x{:016X}, first16={:08X?}",
            oracle.prefix,
            oracle.expected_norm_hash,
            &out_bits[..16]
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("input_norm_weight_name={INPUT_NORM_WEIGHT_NAME}");
    println!("tensor_prefix={}", oracle.prefix);
    println!("proj_weight_name={}", oracle.weight_name);
    println!("head_norm_kind={}", oracle.head_norm_kind);
    println!(
        "head_norm_weight_name={}",
        oracle.norm_weight_name.unwrap_or("none")
    );
    println!("eps={EPS}");
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
    print!("proj_first16_f32_bits=");
    for (index, bits) in q_bits.iter().take(16).enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();
    println!("head_norm_full_row_fnv1a64=0x{out_hash:016X}");
    print!("head_norm_first16_f32_bits=");
    for (index, bits) in out_bits.iter().take(16).enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();
    println!("status=ok");

    Ok(())
}
