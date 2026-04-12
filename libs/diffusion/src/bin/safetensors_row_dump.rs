use makepad_mlx::{MlxDType, MlxSafetensorsHeader};
use std::env;

fn usage() -> ! {
    eprintln!("usage: safetensors-row-dump <file.safetensors> <tensor-name> <row-index> [count]");
    std::process::exit(1);
}

fn f16_to_f32(bits: u16) -> f32 {
    makepad_ggml::f16_to_f32(bits)
}

fn bf16_to_f32(bits: u16) -> f32 {
    makepad_ggml::bf16_to_f32(bits)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).unwrap_or_else(|| usage());
    let tensor_name = env::args().nth(2).unwrap_or_else(|| usage());
    let row_index: usize = env::args()
        .nth(3)
        .unwrap_or_else(|| usage())
        .parse()?;
    let count: usize = env::args().nth(4).map(|value| value.parse()).transpose()?.unwrap_or(8);

    let header = MlxSafetensorsHeader::load(&path)?;
    let entry = header
        .tensor(&tensor_name)
        .ok_or_else(|| format!("missing tensor '{}'", tensor_name))?;
    if entry.shape.len() != 2 {
        return Err(format!("tensor '{}' is not rank-2: {:?}", tensor_name, entry.shape).into());
    }

    let rows = usize::try_from(entry.shape[0])?;
    let cols = usize::try_from(entry.shape[1])?;
    if row_index >= rows {
        return Err(format!("row {} out of range for {} rows", row_index, rows).into());
    }
    let bytes = header.read_tensor_bytes(&tensor_name)?;

    println!("tensor: {}", tensor_name);
    println!("dtype: {:?} shape: [{}, {}]", entry.dtype, rows, cols);

    match entry.dtype {
        MlxDType::F16 => {
            let start = row_index * cols * 2;
            let end = start + cols * 2;
            let values = bytes[start..end]
                .chunks_exact(2)
                .take(count.min(cols))
                .map(|chunk| f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
                .collect::<Vec<_>>();
            println!("row[{row_index}][0..{}]: {:?}", values.len(), values);
        }
        MlxDType::BF16 => {
            let start = row_index * cols * 2;
            let end = start + cols * 2;
            let values = bytes[start..end]
                .chunks_exact(2)
                .take(count.min(cols))
                .map(|chunk| bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
                .collect::<Vec<_>>();
            println!("row[{row_index}][0..{}]: {:?}", values.len(), values);
        }
        MlxDType::F32 => {
            let start = row_index * cols * 4;
            let end = start + cols * 4;
            let values = bytes[start..end]
                .chunks_exact(4)
                .take(count.min(cols))
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect::<Vec<_>>();
            println!("row[{row_index}][0..{}]: {:?}", values.len(), values);
        }
        other => return Err(format!("unsupported dtype {:?}", other).into()),
    }

    Ok(())
}
