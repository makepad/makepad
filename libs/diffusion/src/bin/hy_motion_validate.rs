//! Stagewise FULL HY-Motion validation against `hy_oracle.py` dumps.
//!
//! Usage:
//!   hy-motion-validate <latest.ckpt> <dump-directory> [refiner]

use std::path::{Path, PathBuf};
use std::time::Instant;

use makepad_diffusion::backend::{gpu_concat_rows, gpu_download, gpu_upload};
use makepad_diffusion::hy_motion::{
    hy_motion_rope_tables, HyMotionPackedShape, HY_MOTION_CONTEXT_DIM,
    HY_MOTION_DOUBLE_LAYERS, HY_MOTION_HEAD_DIM, HY_MOTION_HIDDEN,
    HY_MOTION_INPUT_DIM, HY_MOTION_ROPE_THETA, HY_MOTION_SINGLE_LAYERS,
    HY_MOTION_VECTOR_DIM,
};
use makepad_diffusion::hy_motion_transformer::{mean_rows, HyMotionDeviceWeights};
use makepad_diffusion::hy_motion_weights::HyMotionCheckpoint;

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        )
    };
    let data_start = header_start + header_len;
    if data_start > bytes.len() {
        return Err(format!("{}: truncated npy header", path.display()));
    }
    let header = String::from_utf8_lossy(&bytes[header_start..data_start]);
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: no dtype", path.display()))?
        .to_string();
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
    Ok(Npy {
        shape,
        descr,
        data: bytes[data_start..].to_vec(),
    })
}

impl Npy {
    fn f32(&self) -> Result<Vec<f32>, String> {
        if self.descr != "<f4" {
            return Err(format!("expected <f4 npy, got {}", self.descr));
        }
        Ok(self
            .data
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect())
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    max_abs: f64,
    mean_abs: f64,
    cosine: f64,
}

fn compare(name: &str, actual: &[f32], expected: &[f32]) -> Result<Metrics, String> {
    if actual.len() != expected.len() {
        return Err(format!(
            "{name}: length {} != {}",
            actual.len(),
            expected.len()
        ));
    }
    let mut max_abs = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut norm_actual = 0.0f64;
    let mut norm_expected = 0.0f64;
    for (&actual, &expected) in actual.iter().zip(expected) {
        let actual = actual as f64;
        let expected = expected as f64;
        let difference = (actual - expected).abs();
        max_abs = max_abs.max(difference);
        sum_abs += difference;
        dot += actual * expected;
        norm_actual += actual * actual;
        norm_expected += expected * expected;
    }
    let metrics = Metrics {
        max_abs,
        mean_abs: sum_abs / actual.len().max(1) as f64,
        cosine: dot / (norm_actual.sqrt() * norm_expected.sqrt()).max(1.0e-30),
    };
    println!(
        "{name}: n={} max_abs={:.9e} mean_abs={:.9e} cosine={:.12}",
        actual.len(), metrics.max_abs, metrics.mean_abs, metrics.cosine
    );
    Ok(metrics)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HY-MOTION-VALIDATE-FAILED: {error}");
        std::process::exit(1);
    }
    println!("HY-MOTION-VALIDATE-DONE");
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let checkpoint_path = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: hy-motion-validate <latest.ckpt> <dump-directory> [refiner|full]")?,
    );
    let dump = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: hy-motion-validate <latest.ckpt> <dump-directory> [refiner|full]")?,
    );
    let stage = arguments.next().unwrap_or_else(|| "refiner".to_string());
    if stage != "refiner" && stage != "full" {
        return Err(format!("unsupported stage {stage:?}; expected refiner|full"));
    }

    // The oracle is pure f32. Attention's optional f16 GEMM inputs are an
    // optimization to validate only after the f32 stage is green.
    std::env::set_var("FLUX_ATTN_F16", "0");

    let started = Instant::now();
    let mut checkpoint = HyMotionCheckpoint::open(&checkpoint_path)
        .map_err(|error| error.to_string())?;
    let weights = if stage == "full" {
        HyMotionDeviceWeights::load_full(&mut checkpoint)
    } else {
        HyMotionDeviceWeights::load_text_refiner(&mut checkpoint)
    }
    .map_err(|error| error.to_string())?;
    println!(
        "loaded_parameters={} load_s={:.6}",
        weights.parameter_count(),
        started.elapsed().as_secs_f64()
    );

    if stage == "full" {
        validate_full(&weights, &dump)
    } else {
        validate_refiner(&weights, &dump)
    }
}

fn validate_refiner(weights: &HyMotionDeviceWeights, dump: &Path) -> Result<(), String> {
    let conditional_raw = load_npy(&dump.join("text_ctxt_raw.npy"))?;
    let null_raw = load_npy(&dump.join("null_ctxt_input.npy"))?;
    let expected_context = load_npy(&dump.join("trim0.ctxt_encoder.npy"))?;
    let expected_refined = load_npy(&dump.join("trim0.text_refiner.npy"))?;
    if conditional_raw.shape != [1, 128, HY_MOTION_CONTEXT_DIM]
        || null_raw.shape != [1, 1, HY_MOTION_CONTEXT_DIM]
        || expected_context.shape != [2, 12, HY_MOTION_HIDDEN]
        || expected_refined.shape != [2, 12, HY_MOTION_HIDDEN]
    {
        return Err(format!(
            "oracle shape mismatch: context={:?} null={:?} projected={:?} refined={:?}",
            conditional_raw.shape,
            null_raw.shape,
            expected_context.shape,
            expected_refined.shape
        ));
    }
    let conditional_raw = conditional_raw.f32()?;
    let null_row = null_raw.f32()?;
    let expected_context = expected_context.f32()?;
    let expected_refined = expected_refined.f32()?;
    let text_tokens = 12;
    let branch_len = text_tokens * HY_MOTION_HIDDEN;
    let mut projected_pair = Vec::with_capacity(2 * branch_len);
    let mut refined_pair = Vec::with_capacity(2 * branch_len);

    for branch in 0..2 {
        let raw = if branch == 0 {
            null_row.repeat(text_tokens)
        } else {
            let mut rows = Vec::with_capacity(text_tokens * HY_MOTION_CONTEXT_DIM);
            for row in 0..text_tokens {
                let start = row * HY_MOTION_CONTEXT_DIM;
                rows.extend_from_slice(&conditional_raw[start..start + HY_MOTION_CONTEXT_DIM]);
            }
            rows
        };
        let raw = gpu_upload(&raw, text_tokens, HY_MOTION_CONTEXT_DIM)
            .map_err(|error| error.to_string())?;
        let projected = weights
            .context_projection(&raw)
            .map_err(|error| error.to_string())?;
        let projected_host = gpu_download(&projected).map_err(|error| error.to_string())?;
        let context_mean = mean_rows(&projected_host, text_tokens, HY_MOTION_HIDDEN)
            .map_err(|error| error.to_string())?;
        let refined = weights
            .text_refiner(&projected, &context_mean, 0.0)
            .map_err(|error| error.to_string())?;
        projected_pair.extend(projected_host);
        refined_pair.extend(gpu_download(&refined).map_err(|error| error.to_string())?);
    }

    let projected = compare("trim0.ctxt_encoder", &projected_pair, &expected_context)?;
    let refined = compare("trim0.text_refiner", &refined_pair, &expected_refined)?;
    if projected.cosine < 0.999_999 || refined.cosine < 0.999_99 {
        return Err(format!(
            "refiner parity below gate: projected cosine {:.12}, refined cosine {:.12}",
            projected.cosine, refined.cosine
        ));
    }
    Ok(())
}

fn validate_full(weights: &HyMotionDeviceWeights, dump: &Path) -> Result<(), String> {
    const MOTION_TOKENS: usize = 120;
    const TEXT_TOKENS: usize = 12;

    let conditional_context = load_npy(&dump.join("text_ctxt_raw.npy"))?.f32()?;
    let null_context = load_npy(&dump.join("null_ctxt_input.npy"))?.f32()?;
    let conditional_vector = load_npy(&dump.join("text_vec_raw.npy"))?.f32()?;
    let null_vector = load_npy(&dump.join("null_vtxt_feat.npy"))?.f32()?;
    let noise = load_npy(&dump.join("noise.npy"))?.f32()?;
    if conditional_context.len() != 128 * HY_MOTION_CONTEXT_DIM
        || null_context.len() != HY_MOTION_CONTEXT_DIM
        || conditional_vector.len() != HY_MOTION_VECTOR_DIM
        || null_vector.len() != HY_MOTION_VECTOR_DIM
        || noise.len() != 360 * HY_MOTION_INPUT_DIM
    {
        return Err("full-stage input tensor shape mismatch".to_string());
    }

    let latent = noise[..MOTION_TOKENS * HY_MOTION_INPUT_DIM].to_vec();
    let packed = HyMotionPackedShape::new(MOTION_TOKENS, TEXT_TOKENS)
        .map_err(|error| error.to_string())?;
    let (rope_cos, rope_sin) = hy_motion_rope_tables(
        &packed.rope_positions,
        HY_MOTION_HEAD_DIM,
        HY_MOTION_ROPE_THETA,
    )
    .map_err(|error| error.to_string())?;
    let rope_cos = gpu_upload(
        &rope_cos,
        packed.total_tokens(),
        HY_MOTION_HEAD_DIM / 2,
    )
    .map_err(|error| error.to_string())?;
    let rope_sin = gpu_upload(
        &rope_sin,
        packed.total_tokens(),
        HY_MOTION_HEAD_DIM / 2,
    )
    .map_err(|error| error.to_string())?;

    let stage_names = [
        "trim0.input_encoder",
        "trim0.timestep_encoder",
        "trim0.vtxt_encoder",
        "trim0.ctxt_encoder",
        "trim0.text_refiner",
        "trim0.double0.0",
        "trim0.double0.1",
        "trim0.double8.0",
        "trim0.double8.1",
        "trim0.single0",
        "trim0.single17",
        "trim0.final_layer",
    ];
    let mut actual = std::collections::BTreeMap::<&str, Vec<f32>>::new();
    for name in stage_names {
        actual.insert(name, Vec::new());
    }

    let forward_started = Instant::now();
    for branch in 0..2 {
        let context = if branch == 0 {
            null_context.repeat(TEXT_TOKENS)
        } else {
            conditional_context[..TEXT_TOKENS * HY_MOTION_CONTEXT_DIM].to_vec()
        };
        let vector = if branch == 0 {
            &null_vector
        } else {
            &conditional_vector
        };

        let latent_gpu = gpu_upload(&latent, MOTION_TOKENS, HY_MOTION_INPUT_DIM)
            .map_err(|error| error.to_string())?;
        let context_gpu = gpu_upload(&context, TEXT_TOKENS, HY_MOTION_CONTEXT_DIM)
            .map_err(|error| error.to_string())?;
        let vector_gpu = gpu_upload(vector, 1, HY_MOTION_VECTOR_DIM)
            .map_err(|error| error.to_string())?;

        let mut motion = weights
            .input_projection(&latent_gpu)
            .map_err(|error| error.to_string())?;
        actual
            .get_mut("trim0.input_encoder")
            .unwrap()
            .extend(gpu_download(&motion).map_err(|error| error.to_string())?);
        let context_projected = weights
            .context_projection(&context_gpu)
            .map_err(|error| error.to_string())?;
        let context_host =
            gpu_download(&context_projected).map_err(|error| error.to_string())?;
        actual
            .get_mut("trim0.ctxt_encoder")
            .unwrap()
            .extend_from_slice(&context_host);
        let context_mean = mean_rows(&context_host, TEXT_TOKENS, HY_MOTION_HIDDEN)
            .map_err(|error| error.to_string())?;
        let mut text = weights
            .text_refiner(&context_projected, &context_mean, 0.0)
            .map_err(|error| error.to_string())?;
        actual
            .get_mut("trim0.text_refiner")
            .unwrap()
            .extend(gpu_download(&text).map_err(|error| error.to_string())?);

        let time = weights
            .timestep_projection(0.0)
            .map_err(|error| error.to_string())?;
        actual
            .get_mut("trim0.timestep_encoder")
            .unwrap()
            .extend(gpu_download(&time).map_err(|error| error.to_string())?);
        let vector = weights
            .vector_projection(&vector_gpu)
            .map_err(|error| error.to_string())?;
        actual
            .get_mut("trim0.vtxt_encoder")
            .unwrap()
            .extend(gpu_download(&vector).map_err(|error| error.to_string())?);
        let adapter = makepad_diffusion::backend::gpu_add(&time, &vector)
            .map_err(|error| error.to_string())?;

        for layer in 0..HY_MOTION_DOUBLE_LAYERS {
            (motion, text) = weights
                .double_block(
                    layer,
                    motion,
                    text,
                    &adapter,
                    &rope_cos,
                    &rope_sin,
                )
                .map_err(|error| error.to_string())?;
            if layer == 0 || layer + 1 == HY_MOTION_DOUBLE_LAYERS {
                let index = if layer == 0 { 0 } else { 8 };
                actual
                    .get_mut(if index == 0 {
                        "trim0.double0.0"
                    } else {
                        "trim0.double8.0"
                    })
                    .unwrap()
                    .extend(gpu_download(&motion).map_err(|error| error.to_string())?);
                actual
                    .get_mut(if index == 0 {
                        "trim0.double0.1"
                    } else {
                        "trim0.double8.1"
                    })
                    .unwrap()
                    .extend(gpu_download(&text).map_err(|error| error.to_string())?);
            }
        }

        let mut joint = gpu_concat_rows(&motion, &text).map_err(|error| error.to_string())?;
        for layer in 0..HY_MOTION_SINGLE_LAYERS {
            joint = weights
                .single_block(
                    layer,
                    joint,
                    MOTION_TOKENS,
                    &adapter,
                    &rope_cos,
                    &rope_sin,
                )
                .map_err(|error| error.to_string())?;
            if layer == 0 {
                actual
                    .get_mut("trim0.single0")
                    .unwrap()
                    .extend(gpu_download(&joint).map_err(|error| error.to_string())?);
            } else if layer + 1 == HY_MOTION_SINGLE_LAYERS {
                actual
                    .get_mut("trim0.single17")
                    .unwrap()
                    .extend(gpu_download(&joint).map_err(|error| error.to_string())?);
            }
        }
        let motion = makepad_diffusion::backend::gpu_slice_rows(&joint, 0, MOTION_TOKENS)
            .map_err(|error| error.to_string())?;
        let output = weights
            .final_layer(&motion, &adapter)
            .map_err(|error| error.to_string())?;
        actual
            .get_mut("trim0.final_layer")
            .unwrap()
            .extend(gpu_download(&output).map_err(|error| error.to_string())?);
    }
    println!("full_debug_forward_s={:.6}", forward_started.elapsed().as_secs_f64());

    let mut minimum_cosine = 1.0f64;
    for name in stage_names {
        let expected = load_npy(&dump.join(format!("{name}.npy")))?.f32()?;
        let metrics = compare(name, actual.get(name).unwrap(), &expected)?;
        minimum_cosine = minimum_cosine.min(metrics.cosine);
    }
    if minimum_cosine < 0.999_9 {
        return Err(format!(
            "full-stage parity below gate: minimum cosine {minimum_cosine:.12}"
        ));
    }

    let null_context_trimmed = null_context.repeat(TEXT_TOKENS);
    let conditional_context_trimmed =
        conditional_context[..TEXT_TOKENS * HY_MOTION_CONTEXT_DIM].to_vec();
    let run_pair = || -> Result<Vec<f32>, String> {
        let mut pair = weights
            .forward(
                &latent,
                &null_context_trimmed,
                &null_vector,
                0.0,
                MOTION_TOKENS,
                TEXT_TOKENS,
            )
            .map_err(|error| error.to_string())?;
        pair.extend(
            weights
                .forward(
                    &latent,
                    &conditional_context_trimmed,
                    &conditional_vector,
                    0.0,
                    MOTION_TOKENS,
                    TEXT_TOKENS,
                )
                .map_err(|error| error.to_string())?,
        );
        Ok(pair)
    };
    let warm = run_pair()?;
    let expected_final = load_npy(&dump.join("trim0.final_layer.npy"))?.f32()?;
    compare("trim0.forward_api", &warm, &expected_final)?;
    let iterations = 10;
    let benchmark = Instant::now();
    let mut checksum = 0.0f64;
    for _ in 0..iterations {
        checksum += run_pair()?.iter().map(|&value| value as f64).sum::<f64>();
    }
    let elapsed = benchmark.elapsed().as_secs_f64();
    println!(
        "warm_pair_ms={:.6} projected_50_step_s={:.6} iterations={} checksum={:.9}",
        elapsed * 1000.0 / iterations as f64,
        elapsed * 50.0 / iterations as f64,
        iterations,
        checksum
    );

    let prepared_shape = HyMotionDeviceWeights::prepare_shape(MOTION_TOKENS, TEXT_TOKENS)
        .map_err(|error| error.to_string())?;
    let prepared_null = weights
        .prepare_branch(&null_context_trimmed, &null_vector, TEXT_TOKENS)
        .map_err(|error| error.to_string())?;
    let prepared_conditional = weights
        .prepare_branch(
            &conditional_context_trimmed,
            &conditional_vector,
            TEXT_TOKENS,
        )
        .map_err(|error| error.to_string())?;
    let run_prepared_pair = || -> Result<Vec<f32>, String> {
        let mut pair = weights
            .forward_prepared(&latent, 0.0, &prepared_null, &prepared_shape)
            .map_err(|error| error.to_string())?;
        pair.extend(
            weights
                .forward_prepared(&latent, 0.0, &prepared_conditional, &prepared_shape)
                .map_err(|error| error.to_string())?,
        );
        Ok(pair)
    };
    let prepared_warm = run_prepared_pair()?;
    compare("trim0.prepared_api", &prepared_warm, &expected_final)?;
    let benchmark = Instant::now();
    let mut checksum = 0.0f64;
    for _ in 0..iterations {
        checksum += run_prepared_pair()?
            .iter()
            .map(|&value| value as f64)
            .sum::<f64>();
    }
    let elapsed = benchmark.elapsed().as_secs_f64();
    println!(
        "prepared_pair_ms={:.6} projected_50_step_s={:.6} iterations={} checksum={:.9}",
        elapsed * 1000.0 / iterations as f64,
        elapsed * 50.0 / iterations as f64,
        iterations,
        checksum
    );

    let prepared_cfg = weights
        .prepare_cfg(
            &conditional_context_trimmed,
            &conditional_vector,
            MOTION_TOKENS,
        )
        .map_err(|error| error.to_string())?;
    let trajectory_started = Instant::now();
    let sampled = weights
        .sample_cfg_euler(&latent, &prepared_cfg, 50, 5.0)
        .map_err(|error| error.to_string())?;
    let trajectory_s = trajectory_started.elapsed().as_secs_f64();
    let expected_sampled = load_npy(&dump.join("sampled_trimmed.npy"))?.f32()?;
    let trajectory = compare("sampled_trimmed", &sampled, &expected_sampled)?;
    println!(
        "trajectory_50_step_s={trajectory_s:.6} trajectory_cosine={:.12}",
        trajectory.cosine
    );
    if trajectory.cosine < 0.999_999 {
        return Err(format!(
            "trajectory parity below gate: cosine {:.12}",
            trajectory.cosine
        ));
    }

    let perf = makepad_diffusion::backend::gpu_perf_stats(false);
    println!(
        "cuda_mem_used_end_mb={:.3} cuda_mem_free_mb={:.3} cuda_mem_total_mb={:.3}",
        (perf.mem_total_bytes.saturating_sub(perf.mem_free_bytes)) as f64 / 1_048_576.0,
        perf.mem_free_bytes as f64 / 1_048_576.0,
        perf.mem_total_bytes as f64 / 1_048_576.0,
    );
    Ok(())
}
