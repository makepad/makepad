use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;

fn usage() -> ! {
    eprintln!("usage: flux-t5-debug-compare <lhs-dir> <rhs-dir>");
    std::process::exit(1);
}

fn f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(format!("expected f32 bytes, got {}", bytes.len()).into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn diff_stats(lhs: &[f32], rhs: &[f32]) -> Result<(f32, f32), Box<dyn std::error::Error>> {
    if lhs.len() != rhs.len() {
        return Err(format!("length mismatch: {} vs {}", lhs.len(), rhs.len()).into());
    }
    let mut max_abs = 0.0f32;
    let mut sum_abs = 0.0f64;
    for (&left, &right) in lhs.iter().zip(rhs.iter()) {
        let diff = (left - right).abs();
        max_abs = max_abs.max(diff);
        sum_abs += diff as f64;
    }
    let mean_abs = if lhs.is_empty() {
        0.0
    } else {
        (sum_abs / lhs.len() as f64) as f32
    };
    Ok((max_abs, mean_abs))
}

fn collect_bin_files(dir: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let mut files = BTreeSet::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("bin") {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            files.insert(name.to_string());
        }
    }
    Ok(files)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let lhs_dir = env::args().nth(1).unwrap_or_else(|| usage());
    let rhs_dir = env::args().nth(2).unwrap_or_else(|| usage());
    let lhs_dir = Path::new(&lhs_dir);
    let rhs_dir = Path::new(&rhs_dir);

    let lhs_files = collect_bin_files(lhs_dir)?;
    let rhs_files = collect_bin_files(rhs_dir)?;
    let common = lhs_files
        .intersection(&rhs_files)
        .cloned()
        .collect::<Vec<_>>();

    if common.is_empty() {
        return Err("no common .bin files found".into());
    }

    for name in common {
        let lhs = f32_bytes_to_vec(&fs::read(lhs_dir.join(&name))?)?;
        let rhs = f32_bytes_to_vec(&fs::read(rhs_dir.join(&name))?)?;
        let (max_abs, mean_abs) = diff_stats(&lhs, &rhs)?;
        println!("{name}: max_abs={max_abs} mean_abs={mean_abs}");
    }

    Ok(())
}
