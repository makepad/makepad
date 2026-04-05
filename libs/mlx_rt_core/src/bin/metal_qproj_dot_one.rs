use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBufferBindingRef, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_mlx_rt_core::{gemma4_qproj_case_input_bf16_words, MlxSafetensorsHeader};
use std::env;
use std::error::Error;
use std::mem::size_of;
use std::path::PathBuf;
use std::slice;

const WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.weight";
const SCALES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.scales";
const BIASES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.biases";
const OUTPUT_LEN: usize = 2_816;
const GPU_QPROJ_FIRST_OUTPUT_BITS: u32 = 0xBF64_0000;

#[repr(C)]
struct MlxAffineDequantRowArgs {
    n: u32,
}

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
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
    let model_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_model_path);

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
    if !runtime.features().has_bfloat {
        return Err("Metal device does not report BF16 support".into());
    }

    let x_words = gemma4_qproj_case_input_bf16_words(OUTPUT_LEN);
    let x_bytes = bytes_from_bf16_words(&x_words);
    let weights = header.read_rank2_row_bytes(WEIGHT_NAME, 0)?;
    let scales = header.read_rank2_row_bytes(SCALES_NAME, 0)?;
    let biases = header.read_rank2_row_bytes(BIASES_NAME, 0)?;

    let x_buf = runtime.create_buffer_with_bytes(&x_bytes, BufferStorageMode::Private)?;
    let weights_buf = runtime.create_buffer_with_bytes(&weights, BufferStorageMode::Private)?;
    let scales_buf = runtime.create_buffer_with_bytes(&scales, BufferStorageMode::Private)?;
    let biases_buf = runtime.create_buffer_with_bytes(&biases, BufferStorageMode::Private)?;
    let out_buf = runtime.create_buffer(2, BufferStorageMode::Private)?;

    let pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qproj_dot_bf16".to_string(),
        base_name: "kernel_mlx_affine_qproj_dot_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;

    let args = MlxAffineDequantRowArgs {
        n: OUTPUT_LEN as u32,
    };
    runtime.dispatch_compute(
        &pipeline,
        bytes_of(&args),
        &[
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
        ],
        &[],
        MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        },
        MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        },
    )?;
    runtime.wait_idle()?;

    let out_bytes = runtime.read_buffer(&out_buf, 2)?;
    let out_word = u16::from_le_bytes([out_bytes[0], out_bytes[1]]);
    let out_bits = bf16_word_to_f32(out_word).to_bits();

    if out_bits != GPU_QPROJ_FIRST_OUTPUT_BITS {
        return Err(format!(
            "first output mismatch: got 0x{out_bits:08X} expected 0x{GPU_QPROJ_FIRST_OUTPUT_BITS:08X}"
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("first_output_f32_bits=0x{out_bits:08X}");
    println!("status=ok");

    Ok(())
}
