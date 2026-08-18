//! MiniMax H3 stage validation against the diffusers oracle dump
//! (h3_dump.py -> C:\ai\out\dump_m0 for t2va, h3_i2v_dump.py ->
//! C:\ai\out\dump_i2v for fl2va — auto-detected from the dump's
//! condition-row count). Compares, per stage, our own device forward against
//! the reference tensors: packing/rope (exact math), the text encoder final
//! hidden (t2va dumps; fl2va TE validation lives in h3-vision-validate),
//! the fl2va condition-row arithmetic, and the DiT forward-0 velocities.
//!
//! Usage:
//!   h3-validate --dump <dir> --models <MiniMax-H3 dir> [--stage layout|te|fl2va|dit|all]

use makepad_diffusion::h3::{
    h3_build_packed_layout, h3_build_row_timesteps_cond, h3_patchify_video_latents,
    h3_rope_tables, h3_scale_noise, H3ShardedWeights, H3_KEYFRAME_NOISE_AUG, H3_ROT_HALF,
};
use makepad_diffusion::h3_text::{h3_text_encode, h3_text_encoder_evict, H3TextEncoderPrepared};
use makepad_diffusion::h3_transformer::{h3_dit_forward, H3DitPrepared};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// --- minimal .npy reader ---------------------------------------------------

struct Npy {
    shape: Vec<usize>,
    descr: String,
    data: Vec<u8>,
}

fn load_npy(path: &Path) -> Result<Npy, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let major = bytes[6];
    let (header_len, header_start) = if major == 1 {
        (
            u16::from_le_bytes([bytes[8], bytes[9]]) as usize,
            10usize,
        )
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        )
    };
    let header = String::from_utf8_lossy(&bytes[header_start..header_start + header_len])
        .to_string();
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(|| format!("{}: no descr", path.display()))?
        .to_string();
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
    Ok(Npy {
        shape,
        descr,
        data: bytes[header_start + header_len..].to_vec(),
    })
}

impl Npy {
    fn as_f32(&self) -> Result<Vec<f32>, String> {
        match self.descr.as_str() {
            "<f4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            "<f8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| {
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
                })
                .collect()),
            "<f2" => Ok(self
                .data
                .chunks_exact(2)
                .map(|c| {
                    makepad_diffusion::h3::f16_word_to_f32(u16::from_le_bytes([c[0], c[1]]))
                })
                .collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }

    fn as_f64(&self) -> Result<Vec<f64>, String> {
        match self.descr.as_str() {
            "<f8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect()),
            other => Err(format!("npy descr {other} not f64")),
        }
    }

    fn as_i64(&self) -> Result<Vec<i64>, String> {
        match self.descr.as_str() {
            "<i8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect()),
            other => Err(format!("npy descr {other} not i64")),
        }
    }
}

fn compare(name: &str, ours: &[f32], reference: &[f32]) {
    if ours.len() != reference.len() {
        println!("[{name}] LENGTH MISMATCH ours {} ref {}", ours.len(), reference.len());
        return;
    }
    let mut max_abs = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    let mut ref_max = 0.0f64;
    for (a, b) in ours.iter().zip(reference.iter()) {
        let a = *a as f64;
        let b = *b as f64;
        let diff = (a - b).abs();
        max_abs = max_abs.max(diff);
        sum_abs += diff;
        dot += a * b;
        norm_a += a * a;
        norm_b += b * b;
        ref_max = ref_max.max(b.abs());
    }
    let cosine = dot / (norm_a.sqrt() * norm_b.sqrt()).max(1e-30);
    println!(
        "[{name}] n={} max_abs={max_abs:.6e} mean_abs={:.6e} cosine={cosine:.8} ref_max={ref_max:.3e}",
        ours.len(),
        sum_abs / ours.len() as f64,
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts: HashMap<String, String> = HashMap::new();
    let mut key: Option<String> = None;
    for arg in &args[1..] {
        if let Some(name) = arg.strip_prefix("--") {
            key = Some(name.to_string());
            opts.entry(name.to_string()).or_default();
        } else if let Some(name) = key.take() {
            opts.insert(name, arg.clone());
        }
    }
    let dump = PathBuf::from(opts.get("dump").map(String::as_str).unwrap_or(r"C:\ai\out\dump_m0"));
    let models =
        PathBuf::from(opts.get("models").map(String::as_str).unwrap_or(r"C:\ai\models\MiniMax-H3"));
    let stage = opts.get("stage").map(String::as_str).unwrap_or("all").to_string();

    if let Err(err) = run(&dump, &models, &stage) {
        eprintln!("h3-validate FAILED: {err}");
        std::process::exit(1);
    }
    println!("H3-VALIDATE-DONE");
}

fn opts_loop() -> usize {
    // --loop N is parsed generically into OPTS; read from env fallback too.
    std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == "--loop")
        .and_then(|pair| pair[1].parse().ok())
        .or_else(|| std::env::var("H3_LOOP").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

fn run(dump: &Path, models: &Path, stage: &str) -> Result<(), String> {
    let load = |name: &str| load_npy(&dump.join(name));

    // Reference layout inputs (always needed).
    let position_ids = load("dit_in_position_ids.npy")?;
    let token_tags = load("dit_in_token_tags.npy")?.as_i64()?;
    let text_indices = load("dit_in_text_indices.npy")?.as_i64()?;
    let audio_indices = load("dit_in_audio_indices.npy")?.as_i64()?;
    let video_indices = load("dit_in_video_indices.npy")?.as_i64()?;
    let n_text = text_indices.len();
    let n_audio_rows = audio_indices.len();
    // fl2va packs the keyframe conditioning rows in FRONT of the video
    // indices: everything below the first audio row is a conditioning row.
    let first_audio = audio_indices.first().copied().unwrap_or(i64::MAX);
    let n_condition_rows = video_indices
        .iter()
        .filter(|index| **index < first_audio)
        .count();
    let n_video_rows = video_indices.len() - n_condition_rows;
    let seq = token_tags.len();
    println!(
        "layout: seq={seq} text={n_text} cond_rows={n_condition_rows} audio_rows={n_audio_rows} video_rows={n_video_rows}"
    );

    // Recover the latent geometry from the dump video rows + known canvas.
    // 640x352 -> latent 40x22 -> 20x11 = 220 rows/frame, 37 frames.
    let (latent_h, latent_w) = (22usize, 40usize);
    let rows_per_frame = (latent_h / 2) * (latent_w / 2);
    let n_latent_frames = n_video_rows / rows_per_frame;
    let n_keyframes = n_condition_rows / rows_per_frame;
    let n_audio_latents = n_audio_rows / 2;
    let text_tags: Vec<u8> = token_tags[..n_text].iter().map(|t| *t as u8).collect();
    let layout = h3_build_packed_layout(
        &text_tags,
        n_latent_frames,
        latent_h,
        latent_w,
        n_audio_latents,
        n_keyframes,
    )
    .map_err(|err| err.to_string())?;
    if layout.sequence_length != seq {
        return Err(format!(
            "layout seq mismatch: ours {} ref {seq}",
            layout.sequence_length
        ));
    }

    if stage == "layout" || stage == "all" {
        // Exact f64 position grid comparison.
        let ref_pos = position_ids.as_f64()?;
        let mut max_abs = 0.0f64;
        for (a, b) in layout.position_ids.iter().zip(ref_pos.iter()) {
            max_abs = max_abs.max((a - b).abs());
        }
        println!("[layout.position_ids] max_abs={max_abs:.3e} (f64, expect ~0)");
        let tags_ok = layout
            .token_tags
            .iter()
            .zip(token_tags.iter())
            .all(|(a, b)| *a as i64 == *b);
        println!("[layout.token_tags] equal={tags_ok}");

        // Rope tables vs the reference rope module output (seq, 96 doubled).
        // Optional: the fl2va dump does not tap the rope module (the table
        // math was validated against the m0 dump; position_ids above are the
        // fl2va-specific part).
        if load("dit_rope_0.npy").is_err() {
            println!("[layout.rope] dit_rope_0.npy not in dump — skipped (validated on dump_m0)");
        } else {
        let tables = h3_rope_tables(&layout.position_ids);
        let ref_cos = load("dit_rope_0.npy")?.as_f32()?;
        let ref_sin = load("dit_rope_1.npy")?.as_f32()?;
        let mut ours_cos = vec![0.0f32; seq * H3_ROT_HALF];
        ours_cos.copy_from_slice(&tables.cos);
        // reference row stride 96: first 48 = the shared half.
        let ref_cos_half: Vec<f32> = (0..seq)
            .flat_map(|row| ref_cos[row * 96..row * 96 + 48].to_vec())
            .collect();
        let ref_sin_half: Vec<f32> = (0..seq)
            .flat_map(|row| ref_sin[row * 96..row * 96 + 48].to_vec())
            .collect();
        compare("layout.rope_cos", &ours_cos, &ref_cos_half);
        compare("layout.rope_sin", &tables.sin, &ref_sin_half);
        }
    }

    if (stage == "fl2va" || stage == "all") && n_condition_rows > 0 {
        // Condition-row arithmetic: normalized cond latents + the reference's
        // own noise draw -> scale_noise at 0.999 -> patchify -> must equal
        // both the dumped noised latents and the leading DiT video rows.
        let cond_latents = load("cond_latents_0.npy")?.as_f32()?;
        let cond_noise = load("cond_noise_0.npy")?.as_f32()?;
        let cond_noised_ref = load("cond_noised_0.npy")?.as_f32()?;
        let noised = h3_scale_noise(&cond_latents, H3_KEYFRAME_NOISE_AUG, &cond_noise)
            .map_err(|err| err.to_string())?;
        compare("fl2va.cond_noised", &noised, &cond_noised_ref);
        let rows = h3_patchify_video_latents(&noised, 1, latent_h, latent_w)
            .map_err(|err| err.to_string())?;
        let dit_video_rows = load("dit_in_video_rows.npy")?.as_f32()?;
        compare(
            "fl2va.cond_rows_vs_dit_input",
            &rows,
            &dit_video_rows[..n_condition_rows * 96],
        );
    }

    if (stage == "te" || stage == "all") && n_condition_rows > 0 {
        println!("te: fl2va dump (vision tokens) — validated by h3-vision-validate, skipping");
    }
    if (stage == "te" || stage == "all") && n_condition_rows == 0 {
        let te_dir = models.join("text_encoder");
        println!("loading TE shards from {}", te_dir.display());
        let te_weights = H3ShardedWeights::load(&te_dir).map_err(|err| err.to_string())?;
        let prepared = H3TextEncoderPrepared::prepare(&te_weights).map_err(|err| err.to_string())?;
        let token_ids: Vec<u32> = load("te_token_ids.npy")?
            .as_i64()?
            .iter()
            .map(|id| *id as u32)
            .collect();
        let start = std::time::Instant::now();
        let hidden = h3_text_encode(&te_weights, &prepared, &token_ids)
            .map_err(|err| err.to_string())?;
        println!("te encode: {:.2}s ({} tokens)", start.elapsed().as_secs_f64(), token_ids.len());
        let reference = load("prompt_embeds.npy")
            .or_else(|_| load("te_hidden_50.npy"))?
            .as_f32()?;
        compare("te.hidden50", &hidden, &reference);
        let freed = h3_text_encoder_evict().map_err(|err| err.to_string())?;
        println!("te cache evicted: {freed} buffers");
    }

    if stage == "dit" || stage == "all" {
        let tr_dir = models.join("transformer");
        println!("loading DiT shards from {}", tr_dir.display());
        let weights = H3ShardedWeights::load(&tr_dir).map_err(|err| err.to_string())?;
        let start = std::time::Instant::now();
        let prepared = H3DitPrepared::prepare(&weights).map_err(|err| err.to_string())?;
        println!("dit prepare: {:.2}s", start.elapsed().as_secs_f64());

        let video_rows = load("dit_in_video_rows.npy")?.as_f32()?;
        let audio_rows = load("dit_in_audio_rows.npy")?.as_f32()?;
        let text_embeds = load("dit_in_text.npy")?.as_f32()?;
        let timesteps = load("dit_f0_timestep.npy")?.as_f32()?;
        // forward 0 timesteps: reconstruct video/audio pair from the
        // distinct list + indices (video rows' value, audio rows' value);
        // fl2va pins the conditioning rows at max(video_t, 0.999).
        let ref_indices0 = load("dit_f0_timestep_indices.npy")?.as_i64()?;
        let video_t = timesteps[ref_indices0[layout.video_start] as usize];
        let audio_t = timesteps[ref_indices0[layout.audio_start] as usize];
        let plan = h3_build_row_timesteps_cond(
            &layout,
            video_t,
            audio_t,
            video_t.max(H3_KEYFRAME_NOISE_AUG),
        );
        // Sanity: our distinct set must match the reference's.
        compare("dit.timestep_values", &plan.values, &timesteps);
        let ref_indices = load("dit_f0_timestep_indices.npy")?.as_i64()?;
        let idx_ok = plan
            .indices
            .iter()
            .zip(ref_indices.iter())
            .all(|(a, b)| *a as i64 == *b);
        println!("[dit.timestep_indices] equal={idx_ok}");

        let rope = h3_rope_tables(&layout.position_ids);
        let act16 = std::env::var("H3_ACT_F16").map(|v| v != "0").unwrap_or(false);
        let start = std::time::Instant::now();
        let out = h3_dit_forward(
            &weights,
            &prepared,
            &layout,
            &plan,
            &rope,
            &video_rows,
            &audio_rows,
            &text_embeds,
            act16,
            &[0, 9, 49],
        )
        .map_err(|err| err.to_string())?;
        println!("dit forward: {:.2}s (act16={act16})", start.elapsed().as_secs_f64());

        compare(
            "dit.video_velocity",
            &out.video_velocity,
            &load("dit_f0_video_vel.npy")?.as_f32()?,
        );
        compare(
            "dit.audio_velocity",
            &out.audio_velocity,
            &load("dit_f0_audio_vel.npy")?.as_f32()?,
        );
        if let Some(refiner) = &out.debug.refiner_out {
            compare("dit.refiner_out", refiner, &load("dit_refiner_out.npy")?.as_f32()?);
        }
        if let Some(temb) = &out.debug.temb {
            compare("dit.temb", temb, &load("dit_temb.npy")?.as_f32()?);
        }
        for (layer, values) in &out.debug.block_outputs {
            let name = format!("dit_block{layer}_out.npy");
            if let Ok(reference) = load(&name) {
                compare(&format!("dit.block{layer}"), values, &reference.as_f32()?);
            }
        }
        // Warm denoise-loop timing: --loop N re-runs the forward (no debug
        // taps) and reports s/forward against the 2.033 s/fwd oracle
        // (640x352x124, diffusers bf16 on this box).
        let loop_count: usize = opts_loop();
        if loop_count > 0 {
            let mut times = Vec::with_capacity(loop_count);
            for _ in 0..loop_count {
                let start = std::time::Instant::now();
                let _ = h3_dit_forward(
                    &weights,
                    &prepared,
                    &layout,
                    &plan,
                    &rope,
                    &video_rows,
                    &audio_rows,
                    &text_embeds,
                    act16,
                    &[],
                )
                .map_err(|err| err.to_string())?;
                times.push(start.elapsed().as_secs_f64());
            }
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            let min = times.iter().cloned().fold(f64::INFINITY, f64::min);
            println!(
                "[dit.loop] n={loop_count} mean={mean:.3}s/fwd min={min:.3}s/fwd (act16={act16}, oracle 2.033 s/fwd @640x352x124)"
            );
        }

        if let Some(adaln) = &out.debug.block_adaln0 {
            // Reference chunks: dit_block0_adaln_{0..5}, each (n_t*3, 5376).
            // Ours: (m_pad, 96768) with chunk c of (t, mod) at
            // [t, mod*32256 + c*5376 ..].
            let n_t = plan.values.len();
            for chunk in 0..6 {
                let name = format!("dit_block0_adaln_{chunk}.npy");
                let Ok(reference) = load(&name) else { continue };
                let reference = reference.as_f32()?;
                let mut ours = Vec::with_capacity(n_t * 3 * 5376);
                for t in 0..n_t {
                    for modality in 0..3 {
                        let base = t * (6 * 5376 * 3) + modality * (6 * 5376) + chunk * 5376;
                        ours.extend_from_slice(&adaln[base..base + 5376]);
                    }
                }
                compare(&format!("dit.adaln0.chunk{chunk}"), &ours, &reference);
            }
        }
    }

    Ok(())
}
