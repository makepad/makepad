use makepad_mlx::MlxIndexedSafetensors;
use std::env;
use std::path::PathBuf;

const EMBED_WEIGHT: &str = "language_model.model.embed_tokens.weight";
const EMBED_SCALES: &str = "language_model.model.embed_tokens.scales";

fn usage() -> &'static str {
    "Usage: nvfp4_repack_check <model_dir> <row> [row ...]"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let model_dir = PathBuf::from(args.next().ok_or_else(|| usage().to_string())?);
    let rows = args
        .map(|arg| arg.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty() {
        return Err(usage().into());
    }

    let weights = MlxIndexedSafetensors::load(&model_dir)?;
    let full = weights.repack_nvfp4_tensor_to_ggml_bytes(EMBED_WEIGHT, EMBED_SCALES)?;

    for row in rows {
        let row_bytes = weights.repack_nvfp4_row_to_ggml_bytes(EMBED_WEIGHT, EMBED_SCALES, row)?;
        let row_len = row_bytes.len();
        let start = usize::try_from(row)?
            .checked_mul(row_len)
            .ok_or("row offset overflow")?;
        let end = start.checked_add(row_len).ok_or("row end overflow")?;
        let full_row = full.get(start..end).ok_or("row slice out of range")?;
        let mismatch = full_row
            .iter()
            .zip(row_bytes.iter())
            .position(|(left, right)| left != right);
        match mismatch {
            Some(index) => {
                println!(
                    "row={row} match=false first_diff={index} full={} row={}",
                    full_row[index], row_bytes[index]
                );
            }
            None => {
                println!("row={row} match=true bytes={row_len}");
            }
        }
    }

    Ok(())
}
