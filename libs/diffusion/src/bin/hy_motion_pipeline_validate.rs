//! Full persistent HY-Motion prompt -> conditioning -> latent -> 3D motion
//! validation and same-box timing.
//!
//! Usage:
//!   hy-motion-pipeline-validate <Qwen3-8B-dir> <CLIP-dir> <latest.ckpt>
//!       <dump_wooden-dir> <oracle-npy-dir>

use std::path::{Path, PathBuf};
use std::time::Instant;

use makepad_diffusion::hy_motion::{
    HY_MOTION_CONTEXT_DIM, HY_MOTION_INPUT_DIM, HY_MOTION_VECTOR_DIM,
};
use makepad_diffusion::hy_motion_pipeline::{
    HyMotionGenerateParams, HyMotionModelPaths, HyMotionPipeline,
    HyMotionRunControl,
};
use makepad_diffusion::DiffusionError;

const PROMPT: &str = "A person walks forward naturally at a steady pace.";
const FRAMES: usize = 120;
const REFERENCE_COMPLETE_WARM_S: f64 = 0.2793 + 1.307_997_6 + 0.2019;
const REFERENCE_CACHED_WARM_S: f64 = 1.307_997_6 + 0.2019;

struct Npy {
    shape: Vec<usize>,
    data: Vec<f32>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
        _ => return Err(format!("{}: unsupported npy version {major}", path.display())),
    };
    let data_start = header_start + header_len;
    if data_start > bytes.len() {
        return Err(format!("{}: truncated npy header", path.display()));
    }
    let header = String::from_utf8_lossy(&bytes[header_start..data_start]);
    if !header.contains("'descr': '<f4'") || header.contains("'fortran_order': True") {
        return Err(format!(
            "{}: expected little-endian C-order f32 npy",
            path.display()
        ));
    }
    let shape_text = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    let shape: Vec<usize> = shape_text
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect();
    let data: Vec<f32> = bytes[data_start..]
        .chunks_exact(4)
        .map(|value| f32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .collect();
    if data.len() != shape.iter().product::<usize>() {
        return Err(format!(
            "{}: payload length {} disagrees with shape {:?}",
            path.display(),
            data.len(),
            shape
        ));
    }
    Ok(Npy { shape, data })
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
        actual.len(),
        metrics.max_abs,
        metrics.mean_abs,
        metrics.cosine
    );
    Ok(metrics)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("HY-MOTION-PIPELINE-VALIDATE-FAILED: {error}");
        std::process::exit(1);
    }
    println!("HY-MOTION-PIPELINE-VALIDATE-DONE");
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let usage = "usage: hy-motion-pipeline-validate <Qwen3-8B-dir> <CLIP-dir> <latest.ckpt> <dump_wooden-dir> <oracle-npy-dir>";
    let paths = HyMotionModelPaths::new(
        PathBuf::from(arguments.next().ok_or(usage)?),
        PathBuf::from(arguments.next().ok_or(usage)?),
        PathBuf::from(arguments.next().ok_or(usage)?),
        PathBuf::from(arguments.next().ok_or(usage)?),
    );
    let oracle_dir = PathBuf::from(arguments.next().ok_or(usage)?);
    if arguments.next().is_some() {
        return Err(usage.to_string());
    }

    let load_started = Instant::now();
    let (mut pipeline, load) = HyMotionPipeline::load(&paths).map_err(|error| error.to_string())?;
    println!(
        "pipeline_load_s={:.6} qwen_contract_s={:.6} clip_load_s={:.6} dit_upload_s={:.6} wooden_load_s={:.6}",
        load_started.elapsed().as_secs_f64(),
        load.prompt_encoder.qwen_contract_s,
        load.prompt_encoder.clip_load_s,
        load.sampler.checkpoint_upload_s,
        load.sampler.wooden_load_s,
    );

    let expected_context = load_npy(&oracle_dir.join("text_ctxt_raw.npy"))?;
    if expected_context.shape != [1, 128, HY_MOTION_CONTEXT_DIM] {
        return Err(format!(
            "text context oracle shape mismatch: {:?}",
            expected_context.shape
        ));
    }
    let expected_vector = load_npy(&oracle_dir.join("text_vec_raw.npy"))?;
    if expected_vector.data.len() != HY_MOTION_VECTOR_DIM {
        return Err(format!(
            "CLIP oracle shape mismatch: {:?}",
            expected_vector.shape
        ));
    }

    // First prompt populates Qwen/CLIP CUDA caches while the full DiT is
    // already resident: this is the real combined peak/cold behavior.
    let cold = pipeline
        .encode_prompt_uncached(PROMPT, &mut HyMotionRunControl::default())
        .map_err(|error| error.to_string())?;
    let text_tokens = cold.conditioning.text_tokens();
    let qwen_cold = compare(
        "pipeline.qwen_cold",
        &cold.conditioning.context,
        &expected_context.data[..text_tokens * HY_MOTION_CONTEXT_DIM],
    )?;
    let clip_cold = compare(
        "pipeline.clip_cold",
        &cold.conditioning.vector,
        &expected_vector.data,
    )?;
    println!(
        "pipeline_prompt_cold_s={:.6} qwen_s={:.6} clip_s={:.6} text_tokens={text_tokens}",
        cold.timing.total_s, cold.timing.qwen_encode_s, cold.timing.clip_encode_s
    );

    // Same prompt, explicitly uncached at the host level: isolates warm
    // model residency instead of the pipeline's exact prompt cache.
    let warm = pipeline
        .encode_prompt_uncached(PROMPT, &mut HyMotionRunControl::default())
        .map_err(|error| error.to_string())?;
    if warm.conditioning.context != cold.conditioning.context
        || warm.conditioning.vector != cold.conditioning.vector
    {
        return Err("resident conditioner repeat was not bit-exact".to_string());
    }
    println!(
        "pipeline_prompt_warm_s={:.6} qwen_s={:.6} clip_s={:.6} repeat=bit_exact",
        warm.timing.total_s, warm.timing.qwen_encode_s, warm.timing.clip_encode_s
    );

    let noise = load_npy(&oracle_dir.join("noise.npy"))?;
    if noise.shape != [1, 360, HY_MOTION_INPUT_DIM] {
        return Err(format!("noise oracle shape mismatch: {:?}", noise.shape));
    }
    let params = HyMotionGenerateParams {
        frames: FRAMES,
        steps: 50,
        guidance: 5.0,
        seed: 123,
        initial_latent: Some(noise.data[..FRAMES * HY_MOTION_INPUT_DIM].to_vec()),
        smooth: true,
    };

    // No host cache was installed by encode_prompt_uncached, so this is a
    // complete warm NEW-prompt-equivalent encode + sample + decode timing.
    let complete = pipeline
        .generate(PROMPT, &params)
        .map_err(|error| error.to_string())?;
    if complete.conditioning_cache_hit {
        return Err("first complete pipeline run unexpectedly hit host cache".to_string());
    }
    let expected_sampled = load_npy(&oracle_dir.join("sampled_trimmed.npy"))?;
    let sampled = compare(
        "pipeline.sampled_trimmed",
        &complete.normalized_latent,
        &expected_sampled.data,
    )?;
    let latent_denorm = compare(
        "pipeline.decoded.latent_denorm",
        &complete.decoded.latent_denorm,
        &load_npy(&oracle_dir.join("decoded.latent_denorm.npy"))?.data,
    )?;
    let rotations = compare(
        "pipeline.decoded.rot6d",
        &complete.decoded.rotations_6d,
        &load_npy(&oracle_dir.join("decoded.rot6d.npy"))?.data,
    )?;
    let translations = compare(
        "pipeline.decoded.transl",
        &complete.decoded.translations,
        &load_npy(&oracle_dir.join("decoded.transl.npy"))?.data,
    )?;
    let roots = compare(
        "pipeline.decoded.root_rotations_mat",
        &complete.decoded.root_rotation_matrices,
        &load_npy(&oracle_dir.join("decoded.root_rotations_mat.npy"))?.data,
    )?;
    let keypoints = compare(
        "pipeline.decoded.keypoints3d",
        &complete.decoded.keypoints_3d,
        &load_npy(&oracle_dir.join("decoded.keypoints3d.npy"))?.data,
    )?;
    println!(
        "pipeline_complete_warm_s={:.6} prompt_s={:.6} prepare_s={:.6} denoise_s={:.6} decode_s={:.6}",
        complete.timing.total_s,
        complete.timing.prompt.total_s,
        complete.timing.sample.prepare_s,
        complete.timing.sample.denoise_s,
        complete.timing.sample.decode_s,
    );
    let speedup = REFERENCE_COMPLETE_WARM_S / complete.timing.total_s;
    println!(
        "pipeline_reference_complete_s={REFERENCE_COMPLETE_WARM_S:.6} speedup={speedup:.4}x"
    );
    if complete.timing.total_s > REFERENCE_COMPLETE_WARM_S {
        return Err(format!(
            "full pipeline speed below gate: native {:.6}s > reference {REFERENCE_COMPLETE_WARM_S:.6}s",
            complete.timing.total_s
        ));
    }

    let skeleton = pipeline.skeleton();
    if skeleton.joint_names.len() != 52
        || skeleton.rest_joints.len() != 52
        || skeleton.parents.len() != 52
        || skeleton.parents[0] != -1
        || skeleton.joint_names[0] != "Pelvis"
        || skeleton.joint_names[21] != "R_Wrist"
    {
        return Err("native retarget skeleton contract mismatch".to_string());
    }
    for frame in 0..FRAMES {
        let local_root = &complete.decoded.local_rotation_matrices
            [frame * 22 * 9..frame * 22 * 9 + 9];
        let root = &complete.decoded.root_rotation_matrices[frame * 9..frame * 9 + 9];
        if local_root != root {
            return Err(format!(
                "retarget local/root matrix mismatch at frame {frame}"
            ));
        }
    }
    println!(
        "retarget_contract=green joints={} active_local_matrices={} root_translation=separate",
        skeleton.joint_names.len(),
        complete.decoded.local_rotation_matrices.len() / 9,
    );

    // The second generate must use the exact host conditioning cache and be
    // fully deterministic through the resident sampler.
    let cached = pipeline
        .generate(PROMPT, &params)
        .map_err(|error| error.to_string())?;
    if !cached.conditioning_cache_hit
        || cached.normalized_latent != complete.normalized_latent
        || cached.decoded.keypoints_3d != complete.decoded.keypoints_3d
    {
        return Err("cached complete pipeline repeat was not bit-exact".to_string());
    }
    println!(
        "pipeline_cached_s={:.6} denoise_s={:.6} decode_s={:.6} repeat=bit_exact",
        cached.timing.total_s,
        cached.timing.sample.denoise_s,
        cached.timing.sample.decode_s,
    );
    if cached.timing.total_s > REFERENCE_CACHED_WARM_S {
        return Err(format!(
            "cached pipeline speed below gate: native {:.6}s > reference {REFERENCE_CACHED_WARM_S:.6}s",
            cached.timing.total_s
        ));
    }

    // Cancellation must unwind within one Qwen layer and leave its resident
    // cache reusable by the next request.
    let qwen_cancelled = std::cell::Cell::new(false);
    let mut qwen_phase = |phase: &str, done: usize, _total: usize| {
        if phase == "qwen-encode" && done >= 2 {
            qwen_cancelled.set(true);
        }
    };
    let qwen_cancel = || qwen_cancelled.get();
    let mut qwen_control = HyMotionRunControl {
        on_phase: Some(&mut qwen_phase),
        cancel: Some(&qwen_cancel),
    };
    match pipeline.encode_prompt_uncached("A person jumps once.", &mut qwen_control) {
        Err(DiffusionError::Cancelled) => {}
        Err(error) => return Err(format!("Qwen cancel returned wrong error: {error}")),
        Ok(_) => return Err("Qwen cancellation did not stop the encode".to_string()),
    }
    let qwen_recovered = pipeline
        .encode_prompt_uncached(PROMPT, &mut HyMotionRunControl::default())
        .map_err(|error| error.to_string())?;
    if qwen_recovered.conditioning.context != complete.conditioning.context
        || qwen_recovered.conditioning.vector != complete.conditioning.vector
    {
        return Err("Qwen cancellation recovery changed conditioning".to_string());
    }
    println!("pipeline_qwen_cancel_recovery=green");

    // Denoise cancellation is checked before every Euler step. A subsequent
    // full run must remain bit-exact, proving partially advanced host latent
    // state never leaks into the persistent sampler.
    let denoise_cancelled = std::cell::Cell::new(false);
    let mut denoise_phase = |phase: &str, done: usize, _total: usize| {
        if phase == "denoise" && done >= 2 {
            denoise_cancelled.set(true);
        }
    };
    let denoise_cancel = || denoise_cancelled.get();
    let mut denoise_control = HyMotionRunControl {
        on_phase: Some(&mut denoise_phase),
        cancel: Some(&denoise_cancel),
    };
    match pipeline.generate_with_control(PROMPT, &params, &mut denoise_control) {
        Err(DiffusionError::Cancelled) => {}
        Err(error) => return Err(format!("denoise cancel returned wrong error: {error}")),
        Ok(_) => return Err("denoise cancellation did not stop the sample".to_string()),
    }
    let recovered = pipeline
        .generate(PROMPT, &params)
        .map_err(|error| error.to_string())?;
    if recovered.normalized_latent != complete.normalized_latent
        || recovered.decoded.local_rotation_matrices
            != complete.decoded.local_rotation_matrices
    {
        return Err("denoise cancellation recovery was not bit-exact".to_string());
    }
    println!("pipeline_denoise_cancel_recovery=green");

    let soak_runs = std::env::var("HY_MOTION_SOAK_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(3);
    let mut soak_max_s = 0.0f64;
    for run_index in 0..soak_runs {
        let repeat = pipeline
            .generate(PROMPT, &params)
            .map_err(|error| format!("soak run {run_index}: {error}"))?;
        soak_max_s = soak_max_s.max(repeat.timing.total_s);
        if !repeat.conditioning_cache_hit
            || repeat.normalized_latent != complete.normalized_latent
            || repeat.decoded.local_rotation_matrices
                != complete.decoded.local_rotation_matrices
            || repeat.timing.total_s > REFERENCE_CACHED_WARM_S
        {
            return Err(format!(
                "soak run {run_index} failed determinism/cache/speed gate ({:.6}s)",
                repeat.timing.total_s
            ));
        }
    }
    println!(
        "pipeline_soak_runs={soak_runs} max_s={soak_max_s:.6} repeat=bit_exact speed_gate_s={REFERENCE_CACHED_WARM_S:.6}"
    );

    if qwen_cold.cosine < 0.999
        || clip_cold.cosine < 0.999_99
        || sampled.cosine < 0.999_9
        || [
            latent_denorm.cosine,
            rotations.cosine,
            translations.cosine,
            roots.cosine,
            keypoints.cosine,
        ]
        .into_iter()
        .any(|cosine| cosine < 0.999)
    {
        return Err("full pipeline numeric parity below gate".to_string());
    }
    Ok(())
}
