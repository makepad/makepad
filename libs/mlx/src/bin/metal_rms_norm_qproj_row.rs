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

const RMS_WEIGHT_NAME: &str = "language_model.model.layers.0.input_layernorm.weight";
const NORM_LEN: usize = 2_816;
const EPS: f32 = 1e-6;
const GPU_RMS_NORM_HASH: u64 = 0xBF5E_A05B_53DF_E923;
const GPU_RMS_NORM_FIRST16_BITS: [u32; 16] = [
    0xC0A2_0000,
    0xC080_0000,
    0xC033_0000,
    0xBFBC_0000,
    0x0000_0000,
    0x3FB6_0000,
    0x402F_0000,
    0x40E3_0000,
    0x4126_0000,
    0x4041_0000,
    0x0000_0000,
    0xC048_0000,
    0xC11C_0000,
    0x3F6C_0000,
    0x4081_0000,
    0x40A4_0000,
];

#[derive(Clone, Copy)]
struct ProjectionOracle {
    prefix: &'static str,
    expected_hash: u64,
    expected_first16_bits: [u32; 16],
}

const Q_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "q_proj",
    expected_hash: 0xA2FC_CDC3_E3B9_9A86,
    expected_first16_bits: [
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
};

const K_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "k_proj",
    expected_hash: 0x199D_94F3_2CC8_9820,
    expected_first16_bits: [
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
};

const V_PROJ_ORACLE: ProjectionOracle = ProjectionOracle {
    prefix: "v_proj",
    expected_hash: 0x870F_85C9_945E_F8DB,
    expected_first16_bits: [
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
};

#[repr(C)]
struct MlxRmsNormRowArgs {
    n: u32,
    eps: f32,
}

#[repr(C)]
struct MlxAffineQprojRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

fn projection_oracle(prefix: &str) -> Result<ProjectionOracle, Box<dyn Error>> {
    match prefix {
        "q_proj" => Ok(Q_PROJ_ORACLE),
        "k_proj" => Ok(K_PROJ_ORACLE),
        "v_proj" => Ok(V_PROJ_ORACLE),
        _ => Err("--tensor-prefix must be q_proj, k_proj, or v_proj".into()),
    }
}

fn projection_tensor_names(prefix: &str) -> (String, String, String) {
    let base = format!("language_model.model.layers.0.self_attn.{prefix}");
    (
        format!("{base}.weight"),
        format!("{base}.scales"),
        format!("{base}.biases"),
    )
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
                    "Usage: metal_rms_norm_qproj_row [model.safetensors] [--tensor-prefix q_proj|k_proj|v_proj] [--warmup N] [--iters N]"
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
    let rms_weight_bytes = header.read_tensor_bytes(RMS_WEIGHT_NAME)?;
    let (weight_name, scales_name, biases_name) = projection_tensor_names(&tensor_prefix);
    let q_weight_bytes = header.read_tensor_bytes(&weight_name)?;
    let q_scales_bytes = header.read_tensor_bytes(&scales_name)?;
    let q_biases_bytes = header.read_tensor_bytes(&biases_name)?;
    let q_weight_entry = header
        .tensor(&weight_name)
        .ok_or("missing projection weight entry")?;
    let q_scales_entry = header
        .tensor(&scales_name)
        .ok_or("missing projection scales entry")?;
    let out_len = usize::try_from(q_weight_entry.shape[0])?;

    let x_buf = runtime.create_buffer_with_bytes(&x_bytes, BufferStorageMode::Private)?;
    let rms_weight_buf =
        runtime.create_buffer_with_bytes(&rms_weight_bytes, BufferStorageMode::Private)?;
    let h_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Private)?;
    let q_weight_buf =
        runtime.create_buffer_with_bytes(&q_weight_bytes, BufferStorageMode::Private)?;
    let q_scales_buf =
        runtime.create_buffer_with_bytes(&q_scales_bytes, BufferStorageMode::Private)?;
    let q_biases_buf =
        runtime.create_buffer_with_bytes(&q_biases_bytes, BufferStorageMode::Private)?;
    let out_buf = runtime.create_buffer(out_len * 2, BufferStorageMode::Private)?;

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

    let n_reads = 4usize;
    let simd_size = 32usize;
    let threadgroup_needed = NORM_LEN.div_ceil(n_reads);
    let simds_needed = threadgroup_needed.div_ceil(simd_size);
    let rms_threadgroup_size = simd_size * simds_needed;
    if rms_threadgroup_size as u64 > rms_pipeline.max_threads_per_threadgroup {
        return Err(format!(
            "rms threadgroup_size {} exceeds pipeline max {}",
            rms_threadgroup_size, rms_pipeline.max_threads_per_threadgroup
        )
        .into());
    }

    let rms_args = MlxRmsNormRowArgs {
        n: NORM_LEN as u32,
        eps: EPS,
    };
    let qproj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: q_weight_entry.shape[1] as u32,
        qparams_per_row: q_scales_entry.shape[1] as u32,
        out_rows: out_len as u32,
    };
    let rms_args_bytes = bytes_of(&rms_args);
    let qproj_args_bytes = bytes_of(&qproj_args);

    let rms_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &x_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &rms_weight_buf,
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
        height: (out_len as u64).div_ceil(8),
        depth: 1,
    };
    let qproj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };

    for _ in 0..warmup_iters {
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
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
    }

    let start = Instant::now();
    for _ in 0..bench_iters {
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
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
    }
    let elapsed = start.elapsed();

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
    runtime.end_command_batch()?;
    runtime.wait_idle()?;

    let h_bytes = runtime.read_buffer(&h_buf, NORM_LEN * 2)?;
    let h_words = h_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    let h_bits = h_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let h_hash = fnv1a64_u32_words(&h_bits);
    if h_bits[..16] != GPU_RMS_NORM_FIRST16_BITS {
        return Err(format!(
            "intermediate rms first16 bits mismatch: got {:08X?} expected {:08X?}",
            &h_bits[..16],
            GPU_RMS_NORM_FIRST16_BITS
        )
        .into());
    }
    if h_hash != GPU_RMS_NORM_HASH {
        return Err(format!(
            "intermediate rms hash mismatch: got 0x{h_hash:016X} expected 0x{GPU_RMS_NORM_HASH:016X}"
        )
        .into());
    }

    let out_bytes = runtime.read_buffer(&out_buf, out_len * 2)?;
    let out_words = out_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    let out_bits = out_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let hash = fnv1a64_u32_words(&out_bits);

    if out_bits[..16] != oracle.expected_first16_bits {
        return Err(format!(
            "first16 bits mismatch: got {:08X?} expected {:08X?}",
            &out_bits[..16],
            oracle.expected_first16_bits
        )
        .into());
    }
    if hash != oracle.expected_hash {
        return Err(format!(
            "full-row hash mismatch: got 0x{hash:016X} expected 0x{:016X}",
            oracle.expected_hash
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("rms_weight_name={RMS_WEIGHT_NAME}");
    println!("tensor_prefix={}", oracle.prefix);
    println!("q_weight_name={}", weight_name);
    println!("eps={EPS}");
    println!("warmup_iters={warmup_iters}");
    println!("bench_iters={bench_iters}");
    println!("rms_threads_per_threadgroup={rms_threadgroup_size}");
    println!("qproj_kernel=kernel_mlx_affine_qmv_row_bf16");
    println!("intermediate_rms_fnv1a64=0x{h_hash:016X}");
    println!("out_len={}", out_words.len());
    println!("total_ns={}", elapsed.as_nanos());
    println!(
        "avg_ns={:.0}",
        elapsed.as_secs_f64() * 1e9 / bench_iters as f64
    );
    println!(
        "avg_us={:.3}",
        elapsed.as_secs_f64() * 1e6 / bench_iters as f64
    );
    println!("full_row_fnv1a64=0x{hash:016X}");
    print!("first16_f32_bits=");
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
