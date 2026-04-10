use makepad_ggml::backend::metal::{BufferStorageMode, MetalRuntime};
use makepad_mlx::{gemma4_qproj_case_input_bf16_words, MlxSafetensorsHeader};
use std::env;
use std::error::Error;
use std::path::PathBuf;

const WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.weight";
const SCALES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.scales";
const BIASES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.biases";

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn bytes_from_bf16_words(words: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 2);
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

fn fnv1a64_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn verify_roundtrip(
    runtime: &MetalRuntime,
    label: &str,
    bytes: &[u8],
) -> Result<(usize, u64), Box<dyn Error>> {
    let buffer = runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?;
    let readback = runtime.read_buffer(&buffer, bytes.len())?;
    if readback != bytes {
        return Err(format!("{label} Metal readback mismatch").into());
    }
    Ok((bytes.len(), fnv1a64_bytes(bytes)))
}

fn main() -> Result<(), Box<dyn Error>> {
    let model_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_model_path);

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
    let info = runtime.backend_info();
    let features = runtime.features();

    let x_words = gemma4_qproj_case_input_bf16_words(2_816);
    let x_bytes = bytes_from_bf16_words(&x_words);
    let weight_row_bytes = header.read_rank2_row_bytes(WEIGHT_NAME, 0)?;
    let scales_row_bytes = header.read_rank2_row_bytes(SCALES_NAME, 0)?;
    let biases_row_bytes = header.read_rank2_row_bytes(BIASES_NAME, 0)?;

    let (x_len, x_hash) = verify_roundtrip(&runtime, "x", &x_bytes)?;
    let (weight_len, weight_hash) = verify_roundtrip(&runtime, "weight_row0", &weight_row_bytes)?;
    let (scales_len, scales_hash) = verify_roundtrip(&runtime, "scales_row0", &scales_row_bytes)?;
    let (biases_len, biases_hash) = verify_roundtrip(&runtime, "biases_row0", &biases_row_bytes)?;

    runtime.wait_idle()?;

    println!("backend={}", info.name);
    println!("backend_description={}", info.description);
    println!(
        "features=bf16:{} tensor:{} simdgroup_mm:{}",
        features.has_bfloat, features.has_tensor, features.has_simdgroup_mm
    );
    println!("model_path={}", model_path.display());
    println!("x_len_bytes={x_len}");
    println!("x_fnv1a64=0x{x_hash:016X}");
    println!("weight_row0_len_bytes={weight_len}");
    println!("weight_row0_fnv1a64=0x{weight_hash:016X}");
    println!("scales_row0_len_bytes={scales_len}");
    println!("scales_row0_fnv1a64=0x{scales_hash:016X}");
    println!("biases_row0_len_bytes={biases_len}");
    println!("biases_row0_fnv1a64=0x{biases_hash:016X}");
    println!("status=ok");

    Ok(())
}
