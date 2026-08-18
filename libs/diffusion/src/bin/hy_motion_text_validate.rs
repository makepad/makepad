//! Validate native Qwen3-8B HY-Motion conditioning against the fixed oracle.
//!
//! Usage:
//!   hy-motion-text-validate <Qwen3-8B-dir> <oracle-npy-dir>
//!       [layer-oracle-dir] [latest.ckpt]

use std::path::{Path, PathBuf};
use std::time::Instant;

use makepad_diffusion::h3::H3ShardedWeights;
use makepad_diffusion::hy_motion::{
    HY_MOTION_CONTEXT_DIM, HY_MOTION_INPUT_DIM, HY_MOTION_VECTOR_DIM,
};
use makepad_diffusion::hy_motion_text::{
    hy_motion_qwen_encode_precision, hy_motion_qwen_encode_tapped, hy_motion_qwen_evict,
    HyMotionQwenPrecision, HyMotionQwenPrepared, HyMotionQwenTokenizer,
};
use makepad_diffusion::hy_motion_transformer::HyMotionDeviceWeights;
use makepad_diffusion::hy_motion_weights::HyMotionCheckpoint;

const PROMPT: &str = "A person walks forward naturally at a steady pace.";
const EXPECTED_IDS: &[u32] = &[
    151644, 8948, 271, 262, 8116, 5612, 551, 3738, 11379, 1172, 504, 279, 1196, 1467,
    369, 13042, 25, 1917, 11059, 11, 1376, 2487, 28037, 19029, 11, 1973, 80121, 5930,
    11, 34682, 3446, 2866, 11, 47278, 26, 2924, 1707, 62776, 5956, 2687, 4508, 1172,
    421, 3042, 13, 31330, 398, 12322, 2937, 2719, 320, 2359, 73101, 8, 979, 9733, 26,
    653, 537, 7942, 13, 1416, 5248, 6168, 525, 7481, 11, 13216, 279, 1760, 315, 12460,
    6168, 320, 68, 1302, 2572, 6168, 28, 18, 8, 323, 862, 1973, 13, 3155, 537, 17023,
    7402, 3546, 13, 13655, 825, 63594, 14311, 624, 151645, 198, 151644, 872, 198, 32,
    1697, 22479, 4637, 17712, 518, 264, 24020, 17857, 13, 151645, 198,
];

struct Npy {
    shape: Vec<usize>,
    data: Vec<f32>,
}

struct Metrics {
    max_abs: f64,
    mean_abs: f64,
    cosine: f64,
    exact_fraction: f64,
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
    let mut exact = 0usize;
    for (&actual, &expected) in actual.iter().zip(expected) {
        let actual = actual as f64;
        let expected = expected as f64;
        let difference = (actual - expected).abs();
        max_abs = max_abs.max(difference);
        sum_abs += difference;
        dot += actual * expected;
        actual_norm += actual * actual;
        expected_norm += expected * expected;
        if actual == expected {
            exact += 1;
        }
    }
    Ok(Metrics {
        max_abs,
        mean_abs: sum_abs / actual.len() as f64,
        cosine: dot / (actual_norm.sqrt() * expected_norm.sqrt()),
        exact_fraction: exact as f64 / actual.len() as f64,
    })
}

fn load_f32_npy(path: &Path) -> Result<Npy, String> {
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
    let data = bytes[data_start..]
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    Ok(Npy { shape, data })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HY-MOTION-TEXT-VALIDATE-FAILED: {error}");
        std::process::exit(1);
    }
    println!("HY-MOTION-TEXT-VALIDATE-DONE");
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let model_dir = PathBuf::from(arguments.next().ok_or(
        "usage: hy-motion-text-validate <Qwen3-8B-dir> <oracle-npy-dir>",
    )?);
    let oracle_dir = PathBuf::from(arguments.next().ok_or(
        "usage: hy-motion-text-validate <Qwen3-8B-dir> <oracle-npy-dir> [layer-oracle-dir] [latest.ckpt]",
    )?);
    let layer_oracle_dir = arguments.next().map(PathBuf::from);
    let checkpoint_path = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        return Err(
            "usage: hy-motion-text-validate <Qwen3-8B-dir> <oracle-npy-dir> [layer-oracle-dir] [latest.ckpt]"
                .to_string(),
        );
    }
    let precision = match std::env::var("HY_MOTION_QWEN_PRECISION") {
        Ok(value) if value.eq_ignore_ascii_case("f16") => HyMotionQwenPrecision::F16,
        _ => HyMotionQwenPrecision::Bf16,
    };
    println!("qwen_precision={precision:?}");

    let tokenizer_started = Instant::now();
    let tokenizer = HyMotionQwenTokenizer::load(&model_dir).map_err(|error| error.to_string())?;
    let tokens = tokenizer.tokenize(PROMPT).map_err(|error| error.to_string())?;
    if tokens.input_ids != EXPECTED_IDS
        || tokens.crop_start != 101
        || tokens.text_tokens != 12
        || tokens.pad_token_id != 151_643
    {
        return Err(format!(
            "token parity failed: ids={} crop={} real={} pad={} first_mismatch={:?}",
            tokens.input_ids.len(),
            tokens.crop_start,
            tokens.text_tokens,
            tokens.pad_token_id,
            tokens
                .input_ids
                .iter()
                .zip(EXPECTED_IDS)
                .position(|(actual, expected)| actual != expected)
        ));
    }
    println!(
        "token_parity=exact full_tokens={} crop_start={} text_tokens={} tokenizer_s={:.6}",
        tokens.input_ids.len(),
        tokens.crop_start,
        tokens.text_tokens,
        tokenizer_started.elapsed().as_secs_f64()
    );

    let headers_started = Instant::now();
    let weights = H3ShardedWeights::load(&model_dir).map_err(|error| error.to_string())?;
    let prepared = HyMotionQwenPrepared::prepare(&weights).map_err(|error| error.to_string())?;
    println!(
        "qwen_contract_s={:.6} disk_bytes={}",
        headers_started.elapsed().as_secs_f64(),
        weights.total_disk_bytes()
    );

    let encode_started = Instant::now();
    let (run, taps, operator_taps) = if layer_oracle_dir.is_some() {
        hy_motion_qwen_encode_tapped(
            &weights,
            &prepared,
            &tokens,
            &[0, 1, 2, 9, 18, 27, 35, 36],
            precision,
        )
        .map_err(|error| error.to_string())?
    } else {
        (
            hy_motion_qwen_encode_precision(&weights, &prepared, &tokens, precision)
                .map_err(|error| error.to_string())?,
            Vec::new(),
            Vec::new(),
        )
    };
    let encode_s = encode_started.elapsed().as_secs_f64();
    let expected = load_f32_npy(&oracle_dir.join("text_ctxt_raw.npy"))?;
    if expected.shape != [1, 128, HY_MOTION_CONTEXT_DIM] {
        return Err(format!("oracle context shape mismatch: {:?}", expected.shape));
    }
    let expected = &expected.data[..run.text_tokens * HY_MOTION_CONTEXT_DIM];
    if expected.len() != run.context.len() {
        return Err(format!(
            "context length mismatch: {} vs {}",
            run.context.len(),
            expected.len()
        ));
    }
    let metrics = compare(&run.context, expected)?;
    println!(
        "qwen_context n={} max_abs={:.9e} mean_abs={:.9e} cosine={:.12} exact_fraction={:.6} encode_s={encode_s:.6}",
        run.context.len(),
        metrics.max_abs,
        metrics.mean_abs,
        metrics.cosine,
        metrics.exact_fraction,
    );
    if let Some(layer_oracle_dir) = &layer_oracle_dir {
        for tap in &taps {
            let expected = load_f32_npy(
                &layer_oracle_dir.join(format!("hidden_{:02}.npy", tap.stage)),
            )?;
            if expected.shape != [1, tokens.input_ids.len(), HY_MOTION_CONTEXT_DIM]
                || tap.hidden_states.len() < expected.data.len()
            {
                return Err(format!(
                    "Qwen stage {} oracle shape mismatch: {:?}",
                    tap.stage, expected.shape
                ));
            }
            let stage_metrics = compare(
                &tap.hidden_states[..expected.data.len()],
                &expected.data,
            )?;
            println!(
                "qwen_stage={:02} max_abs={:.9e} mean_abs={:.9e} cosine={:.12} exact_fraction={:.6}",
                tap.stage,
                stage_metrics.max_abs,
                stage_metrics.mean_abs,
                stage_metrics.cosine,
                stage_metrics.exact_fraction,
            );
        }
        for tap in &operator_taps {
            let expected =
                load_f32_npy(&layer_oracle_dir.join(format!("{}.npy", tap.name)))?;
            let (actual_values, expected_values) = match expected.shape.as_slice() {
                [1, rows, cols] if *rows <= tap.rows && *cols == tap.cols => (
                    &tap.values[..*rows * tap.cols],
                    expected.data.as_slice(),
                ),
                [1, rows, heads, head_dim]
                    if *rows <= tap.rows && *heads * *head_dim == tap.cols =>
                (
                    &tap.values[..*rows * tap.cols],
                    &expected.data[..*rows * tap.cols],
                ),
                _ => {
                    return Err(format!(
                        "Qwen operator {} oracle shape mismatch: {:?}, native {}x{}",
                        tap.name, expected.shape, tap.rows, tap.cols
                    ));
                }
            };
            let operator_metrics = compare(actual_values, expected_values)?;
            println!(
                "qwen_operator={} max_abs={:.9e} mean_abs={:.9e} cosine={:.12} exact_fraction={:.6}",
                tap.name,
                operator_metrics.max_abs,
                operator_metrics.mean_abs,
                operator_metrics.cosine,
                operator_metrics.exact_fraction,
            );
        }
    }
    if metrics.cosine < 0.999 || !metrics.max_abs.is_finite() {
        return Err(format!(
            "Qwen context parity below gate: cosine {:.12}",
            metrics.cosine
        ));
    }

    // The first pass includes streaming 16 GB of sharded BF16 weights to the
    // device cache. A second pass is the production steady-state timing and
    // must be deterministic with the resident weights.
    let warm_started = Instant::now();
    let warm = hy_motion_qwen_encode_precision(&weights, &prepared, &tokens, precision)
        .map_err(|error| error.to_string())?;
    let warm_s = warm_started.elapsed().as_secs_f64();
    if warm.context != run.context {
        let first_mismatch = warm
            .context
            .iter()
            .zip(&run.context)
            .position(|(warm, cold)| warm.to_bits() != cold.to_bits());
        return Err(format!(
            "resident Qwen repeat was not deterministic: first_mismatch={first_mismatch:?}"
        ));
    }
    println!("qwen_warm_encode_s={warm_s:.6} repeat=bit_exact");

    if let Some(checkpoint_path) = checkpoint_path {
        const MOTION_TOKENS: usize = 120;

        // HY-Motion consumes only the cropped host context. Match the
        // released pipeline's residency policy by evicting Qwen before the
        // full-f32 DiT is uploaded, then measure the resulting trajectory.
        let evict_started = Instant::now();
        let evicted = hy_motion_qwen_evict().map_err(|error| error.to_string())?;
        println!(
            "qwen_evict_tensors={evicted} qwen_evict_s={:.6}",
            evict_started.elapsed().as_secs_f64()
        );

        let vector = load_f32_npy(&oracle_dir.join("text_vec_raw.npy"))?;
        if vector.data.len() != HY_MOTION_VECTOR_DIM {
            return Err(format!(
                "oracle CLIP vector shape mismatch: {:?} ({} values)",
                vector.shape,
                vector.data.len()
            ));
        }
        let noise = load_f32_npy(&oracle_dir.join("noise.npy"))?;
        if noise.data.len() != 360 * HY_MOTION_INPUT_DIM {
            return Err(format!(
                "oracle noise shape mismatch: {:?} ({} values)",
                noise.shape,
                noise.data.len()
            ));
        }
        let expected_sampled = load_f32_npy(&oracle_dir.join("sampled_trimmed.npy"))?;
        if expected_sampled.data.len() != MOTION_TOKENS * HY_MOTION_INPUT_DIM {
            return Err(format!(
                "oracle sampled shape mismatch: {:?} ({} values)",
                expected_sampled.shape,
                expected_sampled.data.len()
            ));
        }

        let checkpoint_started = Instant::now();
        let mut checkpoint =
            HyMotionCheckpoint::open(checkpoint_path).map_err(|error| error.to_string())?;
        let device_weights =
            HyMotionDeviceWeights::load_full(&mut checkpoint).map_err(|error| error.to_string())?;
        println!(
            "dit_checkpoint_upload_s={:.6}",
            checkpoint_started.elapsed().as_secs_f64()
        );
        let prepared = device_weights
            .prepare_cfg(&run.context, &vector.data, MOTION_TOKENS)
            .map_err(|error| error.to_string())?;
        let trajectory_started = Instant::now();
        let sampled = device_weights
            .sample_cfg_euler(
                &noise.data[..MOTION_TOKENS * HY_MOTION_INPUT_DIM],
                &prepared,
                50,
                5.0,
            )
            .map_err(|error| error.to_string())?;
        let trajectory_s = trajectory_started.elapsed().as_secs_f64();
        let trajectory = compare(&sampled, &expected_sampled.data)?;
        println!(
            "qwen_conditioned_trajectory n={} max_abs={:.9e} mean_abs={:.9e} cosine={:.12} trajectory_50_step_s={trajectory_s:.6}",
            sampled.len(),
            trajectory.max_abs,
            trajectory.mean_abs,
            trajectory.cosine,
        );
        if trajectory.cosine < 0.999_9 || !trajectory.max_abs.is_finite() {
            return Err(format!(
                "Qwen-conditioned trajectory parity below gate: cosine {:.12}",
                trajectory.cosine
            ));
        }
    }
    Ok(())
}
