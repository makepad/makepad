use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBufferBindingRef, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_mlx::{fnv1a64_u32_words, MlxSafetensorsHeader};
use std::env;
use std::error::Error;
use std::mem::size_of;
use std::path::PathBuf;
use std::slice;

const WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.weight";
const SCALES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.scales";
const BIASES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.biases";
const OUTPUT_LEN: usize = 2_816;
const GPU_DEQUANT_ROW_HASH: u64 = 0x2D44_4223_7EE7_C10F;
const GPU_DEQUANT_ROW_FIRST16_BITS: [u32; 16] = [
    0x3BD9_0000,
    0x3CD9_0000,
    0x0000_0000,
    0x0000_0000,
    0xBBD9_0000,
    0x3C59_0000,
    0xBC59_0000,
    0x0000_0000,
    0x3D08_0000,
    0x3BD9_0000,
    0x3CD9_0000,
    0xBD59_0000,
    0x3BD9_0000,
    0xBBD9_0000,
    0x3CD9_0000,
    0xBCD9_0000,
];

#[repr(C)]
struct MlxAffineDequantRowArgs {
    n: u32,
    embed_scale: f32,
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

    let weights = header.read_rank2_row_bytes(WEIGHT_NAME, 0)?;
    let scales = header.read_rank2_row_bytes(SCALES_NAME, 0)?;
    let biases = header.read_rank2_row_bytes(BIASES_NAME, 0)?;
    let out_len_bytes = OUTPUT_LEN * 2;

    let weights_buf = runtime.create_buffer_with_bytes(&weights, BufferStorageMode::Private)?;
    let scales_buf = runtime.create_buffer_with_bytes(&scales, BufferStorageMode::Private)?;
    let biases_buf = runtime.create_buffer_with_bytes(&biases, BufferStorageMode::Private)?;
    let out_buf = runtime.create_buffer(out_len_bytes, BufferStorageMode::Private)?;

    let pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_dequant_row_bf16".to_string(),
        base_name: "kernel_mlx_affine_dequant_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;

    let threads_per_threadgroup = 64u64;
    let threadgroups = (OUTPUT_LEN as u64).div_ceil(threads_per_threadgroup);
    let args = MlxAffineDequantRowArgs {
        n: OUTPUT_LEN as u32,
        embed_scale: 1.0,
    };

    runtime.dispatch_compute(
        &pipeline,
        bytes_of(&args),
        &[
            MetalBufferBindingRef {
                index: 1,
                buffer: &weights_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: &scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: &biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: &out_buf,
                offset_bytes: 0,
            },
        ],
        &[],
        MetalSize {
            width: threadgroups,
            height: 1,
            depth: 1,
        },
        MetalSize {
            width: threads_per_threadgroup,
            height: 1,
            depth: 1,
        },
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

    if out_bits[..16] != GPU_DEQUANT_ROW_FIRST16_BITS {
        return Err(format!(
            "first16 bits mismatch: got {:08X?} expected {:08X?}",
            &out_bits[..16],
            GPU_DEQUANT_ROW_FIRST16_BITS
        )
        .into());
    }
    if hash != GPU_DEQUANT_ROW_HASH {
        return Err(format!(
            "full-row hash mismatch: got 0x{hash:016X} expected 0x{GPU_DEQUANT_ROW_HASH:016X}"
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("out_len={}", out_words.len());
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
