//! Validate native HY-Motion CLIP pooling against the fixed oracle.
//!
//! Usage: hy-motion-clip-validate <clip-model-dir> <oracle-npy-dir>

use std::path::{Path, PathBuf};
use std::time::Instant;

use makepad_diffusion::clip::ClipTokenizer;
use makepad_diffusion::hy_motion::HY_MOTION_VECTOR_DIM;
use makepad_diffusion::hy_motion_clip::{
    HyMotionClipConditioner, HY_MOTION_CLIP_TOKENS,
};

const PROMPT: &str = "A person walks forward naturally at a steady pace.";
const EXPECTED_REAL_IDS: &[i32] = &[
    49_406, 320, 2_533, 8_192, 2_342, 12_995, 536, 320, 12_937, 9_450, 269, 49_407,
];

struct Metrics {
    max_abs: f64,
    mean_abs: f64,
    cosine: f64,
}

fn compare(actual: &[f32], expected: &[f32]) -> Result<Metrics, String> {
    if actual.len() != expected.len() {
        return Err(format!(
            "comparison length mismatch: {} vs {}",
            actual.len(),
            expected.len()
        ));
    }
    let mut max_abs = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut actual_norm = 0.0f64;
    let mut expected_norm = 0.0f64;
    for (&actual, &expected) in actual.iter().zip(expected) {
        let actual = actual as f64;
        let expected = expected as f64;
        let difference = (actual - expected).abs();
        max_abs = max_abs.max(difference);
        sum_abs += difference;
        dot += actual * expected;
        actual_norm += actual * actual;
        expected_norm += expected * expected;
    }
    Ok(Metrics {
        max_abs,
        mean_abs: sum_abs / actual.len().max(1) as f64,
        cosine: dot / (actual_norm.sqrt() * expected_norm.sqrt()).max(1.0e-30),
    })
}

fn load_f32_npy(path: &Path) -> Result<(Vec<usize>, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let (header_len, header_start) = if bytes[6] == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        )
    };
    let data_start = header_start + header_len;
    let header = String::from_utf8_lossy(&bytes[header_start..data_start]);
    if !header.contains("'descr': '<f4'") {
        return Err(format!("{}: expected little-endian f32", path.display()));
    }
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    let shape = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    let values = bytes[data_start..]
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    Ok((shape, values))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HY-MOTION-CLIP-VALIDATE-FAILED: {error}");
        std::process::exit(1);
    }
    println!("HY-MOTION-CLIP-VALIDATE-DONE");
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let model_dir = PathBuf::from(arguments.next().ok_or(
        "usage: hy-motion-clip-validate <clip-model-dir> <oracle-npy-dir>",
    )?);
    let oracle_dir = PathBuf::from(arguments.next().ok_or(
        "usage: hy-motion-clip-validate <clip-model-dir> <oracle-npy-dir>",
    )?);
    if arguments.next().is_some() {
        return Err(
            "usage: hy-motion-clip-validate <clip-model-dir> <oracle-npy-dir>".to_string(),
        );
    }

    let load_started = Instant::now();
    let mut conditioner =
        HyMotionClipConditioner::load(model_dir).map_err(|error| error.to_string())?;
    println!(
        "clip_load_s={:.6} loaded_text_tensors={}",
        load_started.elapsed().as_secs_f64(),
        conditioner.loaded_tensor_count()
    );
    if conditioner.loaded_tensor_count() != 196 {
        return Err(format!(
            "CLIP text-only tensor contract mismatch: {}",
            conditioner.loaded_tensor_count()
        ));
    }

    let encode_started = Instant::now();
    let run = conditioner
        .encode(PROMPT)
        .map_err(|error| error.to_string())?;
    let encode_s = encode_started.elapsed().as_secs_f64();
    if run.input_ids.len() != HY_MOTION_CLIP_TOKENS
        || run.eos_index != EXPECTED_REAL_IDS.len() - 1
        || &run.input_ids[..EXPECTED_REAL_IDS.len()] != EXPECTED_REAL_IDS
        || !run.input_ids[EXPECTED_REAL_IDS.len()..]
            .iter()
            .all(|&token| token == ClipTokenizer::PAD_TOKEN_ID)
    {
        return Err(format!(
            "CLIP token parity failed: len={} eos={} ids={:?}",
            run.input_ids.len(),
            run.eos_index,
            &run.input_ids[..run.input_ids.len().min(EXPECTED_REAL_IDS.len())]
        ));
    }
    println!(
        "clip_token_parity=exact tokens={} eos_index={}",
        run.input_ids.len(),
        run.eos_index
    );

    let (shape, expected) = load_f32_npy(&oracle_dir.join("text_vec_raw.npy"))?;
    if expected.len() != HY_MOTION_VECTOR_DIM {
        return Err(format!(
            "oracle CLIP vector shape mismatch: {shape:?} ({} values)",
            expected.len()
        ));
    }
    let metrics = compare(&run.vector, &expected)?;
    println!(
        "clip_pooler n={} max_abs={:.9e} mean_abs={:.9e} cosine={:.12} encode_s={encode_s:.6}",
        run.vector.len(), metrics.max_abs, metrics.mean_abs, metrics.cosine,
    );
    if metrics.cosine < 0.999_99 || !metrics.max_abs.is_finite() {
        return Err(format!(
            "CLIP pooler parity below gate: cosine {:.12}",
            metrics.cosine
        ));
    }

    let warm_started = Instant::now();
    let warm = conditioner
        .encode(PROMPT)
        .map_err(|error| error.to_string())?;
    let warm_s = warm_started.elapsed().as_secs_f64();
    if warm.vector != run.vector {
        return Err("resident CLIP repeat was not bit-exact".to_string());
    }
    println!("clip_warm_encode_s={warm_s:.6} repeat=bit_exact");
    Ok(())
}
