use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBufferBindingRef, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_mlx::{fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxSafetensorsHeader};
use std::env;
use std::error::Error;
use std::mem::size_of;
use std::path::PathBuf;
use std::slice;
use std::time::Instant;

const GPU_QPROJ_ROW_HASH: u64 = 0xDA70_9B59_F4F7_0892;
const GPU_QPROJ_ROW_FIRST16_BITS: [u32; 16] = [
    0xBF64_0000,
    0x402D_0000,
    0x3F42_0000,
    0xBF6E_0000,
    0x3D19_0000,
    0xBF54_0000,
    0xBF3E_0000,
    0x3FCE_0000,
    0x3DB5_0000,
    0xBEAC_0000,
    0x3F10_0000,
    0x3E8D_0000,
    0x3DCA_0000,
    0x3E88_0000,
    0x3E88_0000,
    0xC019_0000,
];

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

fn tensor_names(prefix: &str) -> (String, String, String) {
    let base = format!("language_model.model.layers.0.self_attn.{prefix}");
    (
        format!("{base}.weight"),
        format!("{base}.scales"),
        format!("{base}.biases"),
    )
}

fn parse_u64_arg(text: &str) -> Result<u64, Box<dyn Error>> {
    if let Some(stripped) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Ok(u64::from_str_radix(stripped, 16)?)
    } else {
        Ok(text.parse::<u64>()?)
    }
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn bytes_from_bf16_words(words: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 2);
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut warmup_iters = 0usize;
    let mut bench_iters = 1usize;
    let mut variant = "exact".to_string();
    let mut tensor_prefix = "q_proj".to_string();
    let mut expected_hash: Option<u64> = None;

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
            "--variant" => {
                variant = args_iter.next().ok_or("--variant expects a value")?;
            }
            "--tensor-prefix" => {
                tensor_prefix = args_iter.next().ok_or("--tensor-prefix expects a value")?;
            }
            "--expected-hash" => {
                let value = args_iter.next().ok_or("--expected-hash expects a value")?;
                expected_hash = Some(parse_u64_arg(&value)?);
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metal_qproj_row [model.safetensors] [--variant exact|simd32|qmv] [--tensor-prefix q_proj|k_proj|v_proj|o_proj] [--expected-hash 0x...] [--warmup N] [--iters N]"
                );
                return Ok(());
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {}", arg).into());
            }
            _ => {
                model_path = PathBuf::from(arg);
            }
        }
    }
    if bench_iters == 0 {
        return Err("--iters must be greater than zero".into());
    }
    if variant != "exact" && variant != "simd32" && variant != "qmv" {
        return Err("--variant must be exact, simd32, or qmv".into());
    }
    if tensor_prefix != "q_proj"
        && tensor_prefix != "k_proj"
        && tensor_prefix != "v_proj"
        && tensor_prefix != "o_proj"
    {
        return Err("--tensor-prefix must be q_proj, k_proj, v_proj, or o_proj".into());
    }

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
    if !runtime.features().has_bfloat {
        return Err("Metal device does not report BF16 support".into());
    }

    let (weight_name, scales_name, biases_name) = tensor_names(&tensor_prefix);

    let weights_entry = header
        .tensor(&weight_name)
        .ok_or("missing projection weight entry")?;
    let scales_entry = header
        .tensor(&scales_name)
        .ok_or("missing projection scales entry")?;

    let n_in = usize::try_from(weights_entry.shape[1] * 8)?;
    let out_rows = usize::try_from(weights_entry.shape[0])?;
    let x_words = gemma4_qproj_case_input_bf16_words(n_in);
    let x_bytes = bytes_from_bf16_words(&x_words);
    let weights = header.read_tensor_bytes(&weight_name)?;
    let scales = header.read_tensor_bytes(&scales_name)?;
    let biases = header.read_tensor_bytes(&biases_name)?;
    let out_len_bytes = out_rows * 2;

    let x_buf = runtime.create_buffer_with_bytes(&x_bytes, BufferStorageMode::Private)?;
    let weights_buf = runtime.create_buffer_with_bytes(&weights, BufferStorageMode::Private)?;
    let scales_buf = runtime.create_buffer_with_bytes(&scales, BufferStorageMode::Private)?;
    let biases_buf = runtime.create_buffer_with_bytes(&biases, BufferStorageMode::Private)?;
    let out_buf = runtime.create_buffer(out_len_bytes, BufferStorageMode::Private)?;

    let (cache_name, base_name, threadgroups_size, threads_per_threadgroup_size) =
        match variant.as_str() {
            "simd32" => (
                "kernel_mlx_affine_qproj_row_bf16_simd32".to_string(),
                "kernel_mlx_affine_qproj_row_bf16_simd32".to_string(),
                MetalSize {
                    width: out_rows as u64,
                    height: 1,
                    depth: 1,
                },
                MetalSize {
                    width: 32,
                    height: 1,
                    depth: 1,
                },
            ),
            "qmv" => (
                "kernel_mlx_affine_qmv_row_bf16".to_string(),
                "kernel_mlx_affine_qmv_row_bf16".to_string(),
                MetalSize {
                    width: 1,
                    height: (out_rows as u64).div_ceil(8),
                    depth: 1,
                },
                MetalSize {
                    width: 32,
                    height: 2,
                    depth: 1,
                },
            ),
            _ => (
                "kernel_mlx_affine_qproj_row_bf16".to_string(),
                "kernel_mlx_affine_qproj_row_bf16".to_string(),
                MetalSize {
                    width: (out_rows as u64).div_ceil(64),
                    height: 1,
                    depth: 1,
                },
                MetalSize {
                    width: 64,
                    height: 1,
                    depth: 1,
                },
            ),
        };
    let pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name,
        base_name,
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;

    let args = MlxAffineQprojRowArgs {
        n_in: n_in as u32,
        weight_words_per_row: weights_entry.shape[1] as u32,
        qparams_per_row: scales_entry.shape[1] as u32,
        out_rows: out_rows as u32,
    };

    let bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &x_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &weights_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &out_buf,
            offset_bytes: 0,
        },
    ];
    let args_bytes = bytes_of(&args);

    for _ in 0..warmup_iters {
        runtime.dispatch_compute(
            &pipeline,
            args_bytes,
            &bindings,
            &[],
            threadgroups_size,
            threads_per_threadgroup_size,
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
            threadgroups_size,
            threads_per_threadgroup_size,
        )?;
        runtime.wait_idle()?;
    }
    let elapsed = start.elapsed();

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

    if tensor_prefix == "q_proj" {
        if out_bits[..16] != GPU_QPROJ_ROW_FIRST16_BITS {
            return Err(format!(
                "first16 bits mismatch: got {:08X?} expected {:08X?}",
                &out_bits[..16],
                GPU_QPROJ_ROW_FIRST16_BITS
            )
            .into());
        }
        let expected = expected_hash.unwrap_or(GPU_QPROJ_ROW_HASH);
        if hash != expected {
            return Err(format!(
                "full-row hash mismatch: got 0x{hash:016X} expected 0x{expected:016X}"
            )
            .into());
        }
    }
    if tensor_prefix != "q_proj" {
        if let Some(expected) = expected_hash {
            if hash != expected {
                return Err(format!(
                    "full-row hash mismatch: got 0x{hash:016X} expected 0x{expected:016X}"
                )
                .into());
            }
        }
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("tensor_prefix={tensor_prefix}");
    println!("variant={variant}");
    println!("warmup_iters={warmup_iters}");
    println!("bench_iters={bench_iters}");
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
    println!(
        "qmm_per_s={:.3}",
        bench_iters as f64 / elapsed.as_secs_f64()
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
