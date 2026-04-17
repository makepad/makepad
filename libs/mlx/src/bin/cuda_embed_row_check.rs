use makepad_ggml::backend::cuda::CudaRuntime;
use makepad_ggml::quant::{get_rows_ggml_bytes_cpu, GGML_TYPE_NVFP4};
use makepad_mlx::MlxIndexedSafetensors;
use std::env;
use std::path::PathBuf;

const EMBED_WEIGHT: &str = "language_model.model.embed_tokens.weight";
const EMBED_SCALES: &str = "language_model.model.embed_tokens.scales";

fn bf16_round_to_f32(value: f32) -> f32 {
    f32::from_bits(value.to_bits() & 0xFFFF_0000)
}

fn usage() -> &'static str {
    "Usage: cuda_embed_row_check <model_dir> <row> [row ...]"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let model_dir = PathBuf::from(args.next().ok_or_else(|| usage().to_string())?);
    let rows = args
        .map(|arg| arg.parse::<u32>())
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty() {
        return Err(usage().into());
    }

    let weights = MlxIndexedSafetensors::load(&model_dir)?;
    let packed = weights.repack_nvfp4_tensor_to_ggml_bytes(EMBED_WEIGHT, EMBED_SCALES)?;
    let cuda = CudaRuntime::load()?;
    let embed_weight = cuda.load_bytes(&packed)?;
    let hidden = weights.snapshot.config.text_config.hidden_size as usize;
    let embed_scale = bf16_round_to_f32((hidden as f32).sqrt());
    let row_buf = cuda.alloc_f32(hidden)?;

    for row in rows {
        cuda.nvfp4_get_row_f32_offset(&embed_weight, &row_buf, 0, hidden, row as usize)?;
        cuda.scale_f32_inplace(&row_buf, embed_scale, hidden)?;
        let gpu = cuda.read_f32s(&row_buf, hidden)?;
        let mut cpu = get_rows_ggml_bytes_cpu(
            &packed,
            GGML_TYPE_NVFP4,
            hidden,
            weights.snapshot.config.text_config.vocab_size as usize,
            &[row as i32],
        )
        .ok_or("CPU NVFP4 get_rows failed")?;
        for value in &mut cpu {
            *value = bf16_round_to_f32(*value * embed_scale);
        }
        let mut max_abs_diff = 0.0f32;
        let mut max_index = 0usize;
        for (index, (&left, &right)) in gpu.iter().zip(cpu.iter()).enumerate() {
            let diff = (left - right).abs();
            if diff > max_abs_diff {
                max_abs_diff = diff;
                max_index = index;
            }
        }
        println!(
            "row={row} max_abs_diff={max_abs_diff:.8} index={max_index} gpu={:.8} cpu={:.8}",
            gpu[max_index], cpu[max_index]
        );
    }

    Ok(())
}
