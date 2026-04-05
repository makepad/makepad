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

const WEIGHT_NAME: &str = "language_model.model.layers.0.input_layernorm.weight";
const OUTPUT_LEN: usize = 2_816;
const EPS: f32 = 1e-6;
const GPU_RMS_NORM_ROW_HASH: u64 = 0xBF5E_A05B_53DF_E923;
const GPU_RMS_NORM_ROW_FIRST16_BITS: [u32; 16] = [
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

#[repr(C)]
struct MlxRmsNormRowArgs {
    n: u32,
    eps: f32,
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
                    "Usage: metal_rms_norm_row [model.safetensors] [--warmup N] [--iters N]"
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

    let x_words = gemma4_qproj_case_input_bf16_words(OUTPUT_LEN);
    let x_bytes = bytes_from_bf16_words(&x_words);
    let weight_bytes = header.read_tensor_bytes(WEIGHT_NAME)?;
    let out_len_bytes = OUTPUT_LEN * 2;

    let x_buf = runtime.create_buffer_with_bytes(&x_bytes, BufferStorageMode::Private)?;
    let weight_buf = runtime.create_buffer_with_bytes(&weight_bytes, BufferStorageMode::Private)?;
    let out_buf = runtime.create_buffer(out_len_bytes, BufferStorageMode::Private)?;

    let pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;

    let n_reads = 4usize;
    let simd_size = 32usize;
    let threadgroup_needed = OUTPUT_LEN.div_ceil(n_reads);
    let simds_needed = threadgroup_needed.div_ceil(simd_size);
    let threadgroup_size = simd_size * simds_needed;
    if threadgroup_size as u64 > pipeline.max_threads_per_threadgroup {
        return Err(format!(
            "threadgroup_size {} exceeds pipeline max {}",
            threadgroup_size, pipeline.max_threads_per_threadgroup
        )
        .into());
    }

    let args = MlxRmsNormRowArgs {
        n: OUTPUT_LEN as u32,
        eps: EPS,
    };
    let args_bytes = bytes_of(&args);
    let bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &x_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &out_buf,
            offset_bytes: 0,
        },
    ];
    let threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let threads_per_threadgroup = MetalSize {
        width: threadgroup_size as u64,
        height: 1,
        depth: 1,
    };

    for _ in 0..warmup_iters {
        runtime.dispatch_compute(
            &pipeline,
            args_bytes,
            &bindings,
            &[],
            threadgroups,
            threads_per_threadgroup,
        )?;
        runtime.wait_idle()?;
    }

    let start = Instant::now();
    for _ in 0..bench_iters {
        runtime.dispatch_compute(
            &pipeline,
            args_bytes,
            &bindings,
            &[],
            threadgroups,
            threads_per_threadgroup,
        )?;
        runtime.wait_idle()?;
    }
    let elapsed = start.elapsed();

    runtime.dispatch_compute(
        &pipeline,
        args_bytes,
        &bindings,
        &[],
        threadgroups,
        threads_per_threadgroup,
    )?;
    runtime.wait_idle()?;

    let out_bytes = runtime.read_buffer(&out_buf, out_len_bytes)?;
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

    if out_bits[..16] != GPU_RMS_NORM_ROW_FIRST16_BITS {
        return Err(format!(
            "first16 bits mismatch: got {:08X?} expected {:08X?}",
            &out_bits[..16],
            GPU_RMS_NORM_ROW_FIRST16_BITS
        )
        .into());
    }
    if hash != GPU_RMS_NORM_ROW_HASH {
        return Err(format!(
            "full-row hash mismatch: got 0x{hash:016X} expected 0x{GPU_RMS_NORM_ROW_HASH:016X}"
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("weight_name={WEIGHT_NAME}");
    println!("eps={EPS}");
    println!("warmup_iters={warmup_iters}");
    println!("bench_iters={bench_iters}");
    println!("threads_per_threadgroup={threadgroup_size}");
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
