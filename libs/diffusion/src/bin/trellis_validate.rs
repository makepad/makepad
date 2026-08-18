//! TRELLIS 2 stage validation against the reference oracle dump
//! (t2_dump.py -> C:\ai\out\t2dump). Stages:
//!   dino  — DINOv3 conditioner embedding parity (512 + 1024)
//!   ssfwd — one SS flow DiT forward (pos + neg) vs dump forward 0/1
//!   ss    — full SS stage: 12-step CFG sampling -> ss_dec -> voxel coords
//!           (the EXACT-match gate)
//!
//! Usage:
//!   trellis-validate --dump <dir> --models <TRELLIS.2-4B dir>
//!                    [--dino <model.safetensors>] [--ssdec <ss_dec.safetensors>]
//!                    [--stage dino|ssfwd|ss|all]

use makepad_diffusion::trellis::{
    t2_quantize_unique_coords, t2_rope_tables, t2_ss_grid_coords, TrellisWeights,
    T2_SHAPE_SAMPLER, T2_SLAT_CHANNELS, T2_SS_CHANNELS, T2_SS_SAMPLER, T2_SS_TOKENS,
};
use makepad_diffusion::trellis_dino::T2Dino;
use makepad_diffusion::trellis_dit::{t2_upload_cond, t2_upload_rope, T2Dit};
use makepad_diffusion::trellis_pipeline::{t2_chw_to_tokens, t2_run_ss, t2_step_plan};
use makepad_diffusion::trellis_slat::T2SparseDec;
use makepad_diffusion::trellis_vae::T2SsDec;
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
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        )
    };
    let header =
        String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
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
            "|u1" => Ok(self.data.iter().map(|b| *b as f32).collect()),
            "|b1" => Ok(self.data.iter().map(|b| *b as f32).collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }

    fn as_i32(&self) -> Result<Vec<i32>, String> {
        match self.descr.as_str() {
            "<i4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            "<i8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| {
                    i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as i32
                })
                .collect()),
            other => Err(format!("npy descr {other} not i32-convertible")),
        }
    }
}

fn compare(name: &str, ours: &[f32], reference: &[f32]) -> f64 {
    if ours.len() != reference.len() {
        println!(
            "[{name}] LENGTH MISMATCH ours {} ref {}",
            ours.len(),
            reference.len()
        );
        return 0.0;
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
    cosine
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
    let dump = PathBuf::from(
        opts.get("dump")
            .map(String::as_str)
            .unwrap_or(r"C:\ai\out\t2dump"),
    );
    let models = PathBuf::from(
        opts.get("models")
            .map(String::as_str)
            .unwrap_or(r"C:\ai\ComfyUI\models\microsoft\TRELLIS.2-4B"),
    );
    let dino = PathBuf::from(opts.get("dino").map(String::as_str).unwrap_or(
        r"C:\ai\ComfyUI\models\facebook\dinov3-vitl16-pretrain-lvd1689m\model.safetensors",
    ));
    let ssdec = PathBuf::from(opts.get("ssdec").map(String::as_str).unwrap_or(
        r"C:\ai\ComfyUI\models\microsoft\TRELLIS-image-large\ckpts\ss_dec_conv3d_16l8_fp16.safetensors",
    ));
    let stage = opts
        .get("stage")
        .map(String::as_str)
        .unwrap_or("all")
        .to_string();

    if let Err(err) = run(&dump, &models, &dino, &ssdec, &stage) {
        eprintln!("trellis-validate FAILED: {err}");
        std::process::exit(1);
    }
    println!("TRELLIS-VALIDATE-DONE");
}

fn run(
    dump: &Path,
    models: &Path,
    dino_path: &Path,
    ssdec_path: &Path,
    stage: &str,
) -> Result<(), String> {
    let load = |name: &str| load_npy(&dump.join(name));

    if stage == "dino" || stage == "all" {
        println!("loading dinov3 from {}", dino_path.display());
        let weights = TrellisWeights::load(dino_path).map_err(|err| err.to_string())?;
        let start = std::time::Instant::now();
        let dino = T2Dino::prepare(&weights).map_err(|err| err.to_string())?;
        println!("dino prepare: {:.2}s", start.elapsed().as_secs_f64());
        for res in [512usize, 1024] {
            let input = load(&format!("cond_input_{res}.npy"))?;
            let pixels = input.as_f32()?;
            let start = std::time::Instant::now();
            let ours = dino
                .forward_normalized(&pixels, res)
                .map_err(|err| err.to_string())?;
            println!(
                "dino {res}: {:.2}s ({} tokens)",
                start.elapsed().as_secs_f64(),
                ours.len() / 1024
            );
            let reference = load(&format!("cond_{res}.npy"))?.as_f32()?;
            compare(&format!("dino.cond_{res}"), &ours, &reference);
        }
    }

    if stage == "ssfwd" || stage == "ss" || stage == "ssteach" || stage == "ssdeconly" || stage == "all" {
        let ss_path = models.join("ckpts").join("ss_flow_img_dit_1_3B_64_bf16.safetensors");
        println!("loading ss flow from {}", ss_path.display());
        let weights = TrellisWeights::load(&ss_path).map_err(|err| err.to_string())?;
        let start = std::time::Instant::now();
        let dit = T2Dit::prepare(weights, "t2ss", T2_SS_CHANNELS, T2_SS_CHANNELS)
            .map_err(|err| err.to_string())?;
        println!("ss flow prepare: {:.2}s", start.elapsed().as_secs_f64());

        let cond_host = load("cond_512.npy")?.as_f32()?;
        let cond = t2_upload_cond(&cond_host).map_err(|err| err.to_string())?;
        let neg_host = vec![0.0f32; cond_host.len()];
        let neg_cond = t2_upload_cond(&neg_host).map_err(|err| err.to_string())?;

        if stage == "ssfwd" || stage == "all" {
            let rope = t2_upload_rope(&t2_rope_tables(&t2_ss_grid_coords()))
                .map_err(|err| err.to_string())?;
            let x_ref = load("ss_fwd00_x.npy")?.as_f32()?;
            let x_tokens = t2_chw_to_tokens(&x_ref, T2_SS_CHANNELS, T2_SS_TOKENS);
            let plan = t2_step_plan(&T2_SS_SAMPLER);
            let t0 = plan[0].1;
            let start = std::time::Instant::now();
            let (velocity, _) = dit
                .forward(&x_tokens, T2_SS_TOKENS, (1000.0 * t0) as f32, &cond, &rope, None, &[])
                .map_err(|err| err.to_string())?;
            println!("ss forward: {:.3}s (t={t0:.6})", start.elapsed().as_secs_f64());
            let v_ref = load("ss_fwd00_v.npy")?.as_f32()?;
            let v_ref_tokens = t2_chw_to_tokens(&v_ref, T2_SS_CHANNELS, T2_SS_TOKENS);
            compare("ss.fwd0_velocity", &velocity, &v_ref_tokens);
            // Negative (zero-cond) forward = dump forward 1.
            let (neg_velocity, _) = dit
                .forward(&x_tokens, T2_SS_TOKENS, (1000.0 * t0) as f32, &neg_cond, &rope, None, &[])
                .map_err(|err| err.to_string())?;
            if let Ok(nv_ref) = load("ss_fwd01_v.npy") {
                let nv_tokens = t2_chw_to_tokens(&nv_ref.as_f32()?, T2_SS_CHANNELS, T2_SS_TOKENS);
                compare("ss.fwd1_velocity", &neg_velocity, &nv_tokens);
            }
        }

        if stage == "ssteach" {
            // Teacher-forced per-forward parity: feed the REFERENCE x_t at
            // every forward (pos + neg per CFG step) and compare velocities.
            // Separates model-forward bugs from chaotic trajectory drift.
            let rope = t2_upload_rope(&t2_rope_tables(&t2_ss_grid_coords()))
                .map_err(|err| err.to_string())?;
            let mut fwd = 0usize;
            for (step, t, _t_prev, cfg_active) in t2_step_plan(&T2_SS_SAMPLER) {
                let calls: &[bool] = if cfg_active { &[true, false] } else { &[true] };
                for positive in calls {
                    let x_name = format!("ss_fwd{fwd:02}_x.npy");
                    let v_name = format!("ss_fwd{fwd:02}_v.npy");
                    let (Ok(x_ref), Ok(v_ref)) = (load(&x_name), load(&v_name)) else {
                        println!("[ssteach] missing {x_name}, stopping at fwd {fwd}");
                        break;
                    };
                    let x_tokens =
                        t2_chw_to_tokens(&x_ref.as_f32()?, T2_SS_CHANNELS, T2_SS_TOKENS);
                    let c = if *positive { &cond } else { &neg_cond };
                    let (velocity, _) = dit
                        .forward(&x_tokens, T2_SS_TOKENS, (1000.0 * t) as f32, c, &rope, None, &[])
                        .map_err(|err| err.to_string())?;
                    let v_tokens =
                        t2_chw_to_tokens(&v_ref.as_f32()?, T2_SS_CHANNELS, T2_SS_TOKENS);
                    compare(
                        &format!("ssteach.f{fwd:02}_s{step}_{}", if *positive { "pos" } else { "neg" }),
                        &velocity,
                        &v_tokens,
                    );
                    fwd += 1;
                }
            }
        }

        if stage == "ssdeconly" {
            // Decoder-only exact gate: reference z_s through OUR ss_dec.
            println!("loading ss_dec from {}", ssdec_path.display());
            let dec_weights = TrellisWeights::load(ssdec_path).map_err(|err| err.to_string())?;
            let ss_dec = T2SsDec::prepare(&dec_weights).map_err(|err| err.to_string())?;
            let z_ref = load("ssdec_in.npy")?.as_f32()?;
            let z_tokens = t2_chw_to_tokens(&z_ref, T2_SS_CHANNELS, T2_SS_TOKENS);
            let start = std::time::Instant::now();
            let logits = ss_dec.decode(&z_tokens).map_err(|err| err.to_string())?;
            println!("ss_dec decode: {:.2}s", start.elapsed().as_secs_f64());
            let logits_ref = load("ssdec_out.npy")?.as_f32()?;
            compare("ssdeconly.logits", &logits, &logits_ref);
            let ours = makepad_diffusion::trellis::t2_occupancy_coords(&logits, 64, 32)
                .map_err(|err| err.to_string())?;
            let theirs = makepad_diffusion::trellis::t2_occupancy_coords(&logits_ref, 64, 32)
                .map_err(|err| err.to_string())?;
            // Sign agreement rate on the raw logits (threshold robustness).
            let flips = logits
                .iter()
                .zip(logits_ref.iter())
                .filter(|(a, b)| (**a > 0.0) != (**b > 0.0))
                .count();
            println!(
                "[ssdeconly.coords] ours={} ref={} EXACT={} logit_sign_flips={flips}/262144",
                ours.len(),
                theirs.len(),
                ours == theirs
            );
        }

        if stage == "ss" || stage == "all" {
            println!("loading ss_dec from {}", ssdec_path.display());
            let dec_weights = TrellisWeights::load(ssdec_path).map_err(|err| err.to_string())?;
            let start = std::time::Instant::now();
            let ss_dec = T2SsDec::prepare(&dec_weights).map_err(|err| err.to_string())?;
            println!("ss_dec prepare: {:.2}s", start.elapsed().as_secs_f64());

            let noise = load("randn_00.npy")?;
            if noise.shape != vec![1, 8, 16, 16, 16] {
                return Err(format!("randn_00 unexpected shape {:?}", noise.shape));
            }
            let noise_tokens =
                t2_chw_to_tokens(&noise.as_f32()?, T2_SS_CHANNELS, T2_SS_TOKENS);
            let start = std::time::Instant::now();
            let out = t2_run_ss(
                &dit,
                &ss_dec,
                &noise_tokens,
                &cond,
                &neg_cond,
                &T2_SS_SAMPLER,
                32,
                |fwd, t, _x, _v| {
                    if fwd % 6 == 0 {
                        println!("  ss fwd {fwd} t={t:.6}");
                    }
                },
            )
            .map_err(|err| err.to_string())?;
            println!("ss stage: {:.2}s", start.elapsed().as_secs_f64());

            // Final latent vs the decoder input the reference saw.
            let z_ref = load("ssdec_in.npy")?.as_f32()?;
            let z_ref_tokens = t2_chw_to_tokens(&z_ref, T2_SS_CHANNELS, T2_SS_TOKENS);
            compare("ss.z_latent", &out.z_tokens, &z_ref_tokens);
            // Decoder logits (64^3): reference layout (1, 1, 64, 64, 64) is
            // already x, y, z lex.
            let logits_ref = load("ssdec_out.npy")?.as_f32()?;
            compare("ss.dec_logits", &out.logits, &logits_ref);

            // THE GATE: exact voxel coords at res 32.
            let coords_ref = load("slat_lr_coords.npy")?;
            let coords_ref_values = coords_ref.as_i32()?;
            let n_ref = coords_ref.shape[0];
            let mut ref_list = Vec::with_capacity(n_ref);
            for row in 0..n_ref {
                ref_list.push([
                    coords_ref_values[row * 4 + 1],
                    coords_ref_values[row * 4 + 2],
                    coords_ref_values[row * 4 + 3],
                ]);
            }
            let exact = out.coords == ref_list;
            let ours_set: std::collections::HashSet<[i32; 3]> =
                out.coords.iter().copied().collect();
            let ref_set: std::collections::HashSet<[i32; 3]> = ref_list.iter().copied().collect();
            let missing = ref_set.difference(&ours_set).count();
            let extra = ours_set.difference(&ref_set).count();
            println!(
                "[ss.coords] ours={} ref={} EXACT={exact} missing={missing} extra={extra}",
                out.coords.len(),
                ref_list.len()
            );
        }
    }

    if stage == "slat" || stage == "all" {
        run_slat(dump, models, &load)?;
    }

    if stage == "decode" || stage == "all" {
        run_decode(dump, models, &load)?;
    }

    if stage == "tex" {
        run_tex(dump, models, &load)?;
    }

    println!("TRELLIS-VALIDATE-OK");
    Ok(())
}

fn run_tex(
    _dump: &Path,
    models: &Path,
    load: &dyn Fn(&str) -> Result<Npy, String>,
) -> Result<(), String> {
    use makepad_diffusion::trellis::{
        T2_SHAPE_SLAT_MEAN, T2_SHAPE_SLAT_STD, T2_TEX_IN_CHANNELS, T2_TEX_SAMPLER,
        T2_TEX_SLAT_MEAN, T2_TEX_SLAT_STD,
    };
    let coords = load_coords_npy(&load("shape_slat_final_coords.npy")?)?;
    let tokens = coords.len();
    println!("tex tokens: {tokens}");
    let cond_host = load("cond_1024.npy")?.as_f32()?;
    let cond = t2_upload_cond(&cond_host).map_err(|err| err.to_string())?;
    let rope = t2_upload_rope(&t2_rope_tables(&coords)).map_err(|err| err.to_string())?;

    let tex_path = models.join("ckpts").join("slat_flow_imgshape2tex_dit_1_3B_1024_bf16.safetensors");
    let weights = TrellisWeights::load(&tex_path).map_err(|err| err.to_string())?;
    let dit = T2Dit::prepare(weights, "t2tex", T2_TEX_IN_CHANNELS, T2_SLAT_CHANNELS)
        .map_err(|err| err.to_string())?;

    // concat cond = shape slat renormalized with the SHAPE stats.
    let shape_slat = load("shape_slat_final_feats.npy")?.as_f32()?;
    let mut concat = shape_slat;
    for row in concat.chunks_exact_mut(T2_SLAT_CHANNELS) {
        for (value, (mean, std)) in row
            .iter_mut()
            .zip(T2_SHAPE_SLAT_MEAN.iter().zip(T2_SHAPE_SLAT_STD.iter()))
        {
            *value = (*value - mean) / std;
        }
    }
    if let Ok(ref_concat) = load("tex_concat_cond.npy") {
        compare("tex.concat_cond", &concat, &ref_concat.as_f32()?);
    }

    // Teacher-forced forward 0.
    let x0 = load("tex_fwd00_x.npy")?.as_f32()?;
    let mut full = vec![0.0f32; tokens * T2_TEX_IN_CHANNELS];
    for token in 0..tokens {
        full[token * 64..token * 64 + 32]
            .copy_from_slice(&x0[token * 32..(token + 1) * 32]);
        full[token * 64 + 32..(token + 1) * 64]
            .copy_from_slice(&concat[token * 32..(token + 1) * 32]);
    }
    let plan = t2_step_plan(&T2_TEX_SAMPLER);
    let start = std::time::Instant::now();
    let (velocity, _) = dit
        .forward(&full, tokens, (1000.0 * plan[0].1) as f32, &cond, &rope, None, &[])
        .map_err(|err| err.to_string())?;
    println!("tex forward: {:.3}s", start.elapsed().as_secs_f64());
    compare("tex.fwd0_velocity", &velocity, &load("tex_fwd00_v.npy")?.as_f32()?);

    // Full 12-forward sampling from the reference noise.
    let mut x = load("randn_03.npy")?.as_f32()?;
    let start = std::time::Instant::now();
    makepad_diffusion::trellis_pipeline::t2_sample_flow_concat(
        &dit,
        &mut x,
        tokens,
        Some(&concat),
        &cond,
        &cond, // strength 1.0: neg never used
        &rope,
        &T2_TEX_SAMPLER,
        false,
        |_, _, _, _| {},
    )
    .map_err(|err| err.to_string())?;
    println!("tex sampling: {:.2}s (12 fwd)", start.elapsed().as_secs_f64());
    for row in x.chunks_exact_mut(T2_SLAT_CHANNELS) {
        for (value, (mean, std)) in row
            .iter_mut()
            .zip(T2_TEX_SLAT_MEAN.iter().zip(T2_TEX_SLAT_STD.iter()))
        {
            *value = *value * std + mean;
        }
    }
    compare("tex.final", &x, &load("tex_slat_final_feats.npy")?.as_f32()?);

    // Tex decode with the reference guide subs.
    let mut guide: Vec<Vec<[bool; 8]>> = Vec::new();
    for i in 0..4 {
        let mask = load(&format!("sub{i}_mask.npy"))?.as_f32()?;
        guide.push(
            mask.chunks_exact(8)
                .map(|row| {
                    let mut m = [false; 8];
                    for (dst, v) in m.iter_mut().zip(row) {
                        *dst = *v > 0.5;
                    }
                    m
                })
                .collect(),
        );
    }
    let dec_path = models.join("ckpts").join("tex_dec_next_dc_f16c32_fp16.safetensors");
    let dec_weights = TrellisWeights::load(&dec_path).map_err(|err| err.to_string())?;
    let dec = T2SparseDec::prepare(dec_weights, "t2tdec", 6, false).map_err(|err| err.to_string())?;
    // Use the REFERENCE tex slat so the decode gate is isolated.
    let tex_ref = load("tex_slat_final_feats.npy")?.as_f32()?;
    let start = std::time::Instant::now();
    let (mut feats, out_coords, _subs) = dec
        .decode(&tex_ref, coords, Some(&guide))
        .map_err(|err| err.to_string())?;
    println!(
        "tex decode: {:.2}s -> {} voxels",
        start.elapsed().as_secs_f64(),
        out_coords.len()
    );
    for value in &mut feats {
        *value = *value * 0.5 + 0.5;
    }
    let ref_coords = load_coords_npy(&load("tex_voxels_coords.npy")?)?;
    coords_gate("tex.voxel_coords", &out_coords, &ref_coords);
    let ref_feats = load("tex_voxels_feats.npy")?.as_f32()?;
    if feats.len() == ref_feats.len() {
        compare("tex.voxel_feats", &feats, &ref_feats);
    }
    Ok(())
}

fn load_coords_npy(npy: &Npy) -> Result<Vec<[i32; 3]>, String> {
    let values = npy.as_i32()?;
    let cols = npy.shape[1];
    if cols != 4 && cols != 3 {
        return Err(format!("coords npy has {cols} cols"));
    }
    let skip = cols - 3;
    Ok(values
        .chunks_exact(cols)
        .map(|row| [row[skip], row[skip + 1], row[skip + 2]])
        .collect())
}

fn coords_gate(name: &str, ours: &[[i32; 3]], reference: &[[i32; 3]]) {
    let exact = ours == reference;
    let ours_set: std::collections::HashSet<[i32; 3]> = ours.iter().copied().collect();
    let ref_set: std::collections::HashSet<[i32; 3]> = reference.iter().copied().collect();
    let missing = ref_set.difference(&ours_set).count();
    let extra = ours_set.difference(&ref_set).count();
    println!(
        "[{name}] ours={} ref={} EXACT={exact} missing={missing} extra={extra}",
        ours.len(),
        reference.len()
    );
}

fn run_slat(
    _dump: &Path,
    models: &Path,
    load: &dyn Fn(&str) -> Result<Npy, String>,
) -> Result<(), String> {
    let cond_host = load("cond_512.npy")?.as_f32()?;
    let cond = t2_upload_cond(&cond_host).map_err(|err| err.to_string())?;

    // LR flow: teacher-forced forward 0 on the reference coords.
    let lr_coords = load_coords_npy(&load("slat_lr_coords.npy")?)?;
    println!("slat LR tokens: {}", lr_coords.len());
    let rope = t2_upload_rope(&t2_rope_tables(&lr_coords)).map_err(|err| err.to_string())?;
    let lr_path = models.join("ckpts").join("slat_flow_img2shape_dit_1_3B_512_bf16.safetensors");
    let weights = TrellisWeights::load(&lr_path).map_err(|err| err.to_string())?;
    let start = std::time::Instant::now();
    let dit = T2Dit::prepare(weights, "t2lr", T2_SLAT_CHANNELS, T2_SLAT_CHANNELS)
        .map_err(|err| err.to_string())?;
    println!("lr prepare: {:.2}s", start.elapsed().as_secs_f64());
    let plan = t2_step_plan(&T2_SHAPE_SAMPLER);
    let x0 = load("slat_lr_fwd00_x.npy")?.as_f32()?;
    let start = std::time::Instant::now();
    let (velocity, _) = dit
        .forward(&x0, lr_coords.len(), (1000.0 * plan[0].1) as f32, &cond, &rope, None, &[])
        .map_err(|err| err.to_string())?;
    println!("lr forward: {:.3}s", start.elapsed().as_secs_f64());
    compare("slat.lr_fwd0_velocity", &velocity, &load("slat_lr_fwd00_v.npy")?.as_f32()?);

    // Full LR sampling from the reference noise on the reference coords.
    let noise = load("randn_01.npy")?.as_f32()?;
    let neg_host = vec![0.0f32; cond_host.len()];
    let neg_cond = t2_upload_cond(&neg_host).map_err(|err| err.to_string())?;
    let mut x = noise;
    let start = std::time::Instant::now();
    makepad_diffusion::trellis_pipeline::t2_sample_flow(
        &dit,
        &mut x,
        lr_coords.len(),
        &cond,
        &neg_cond,
        &rope,
        &T2_SHAPE_SAMPLER,
        false, // sparse population std in the CFG rescale
        |_, _, _, _| {},
    )
    .map_err(|err| err.to_string())?;
    println!("lr sampling: {:.2}s (21 fwd)", start.elapsed().as_secs_f64());
    // Post-normalize like the pipeline.
    use makepad_diffusion::trellis::{T2_SHAPE_SLAT_MEAN, T2_SHAPE_SLAT_STD};
    for row in x.chunks_exact_mut(T2_SLAT_CHANNELS) {
        for (value, (mean, std)) in row
            .iter_mut()
            .zip(T2_SHAPE_SLAT_MEAN.iter().zip(T2_SHAPE_SLAT_STD.iter()))
        {
            *value = *value * std + mean;
        }
    }
    let lr_final_ref = load("slat_lr_final_feats.npy")?.as_f32()?;
    compare("slat.lr_final", &x, &lr_final_ref);

    // Cascade upsample: REFERENCE LR latent through OUR shape decoder's
    // first 4 stages -> hr coords -> quantize/unique EXACT gate.
    let dec_path = models.join("ckpts").join("shape_dec_next_dc_f16c32_fp16.safetensors");
    let dec_weights = TrellisWeights::load(&dec_path).map_err(|err| err.to_string())?;
    let start = std::time::Instant::now();
    let dec = T2SparseDec::prepare(dec_weights, "t2sdec", 7, true).map_err(|err| err.to_string())?;
    println!("shape_dec prepare: {:.2}s", start.elapsed().as_secs_f64());
    let start = std::time::Instant::now();
    let hr_coords = dec
        .upsample(&lr_final_ref, lr_coords.clone(), 4)
        .map_err(|err| err.to_string())?;
    println!("upsample: {:.2}s -> {} coords @512", start.elapsed().as_secs_f64(), hr_coords.len());
    let hr_ref = load_coords_npy(&load("upsample_hr_coords.npy")?)?;
    coords_gate("slat.upsample_coords", &hr_coords, &hr_ref);
    let quant = t2_quantize_unique_coords(&hr_coords, 512, 1024);
    let hr_tokens_ref = load_coords_npy(&load("slat_hr_coords.npy")?)?;
    coords_gate("slat.hr_token_coords", &quant, &hr_tokens_ref);

    // HR flow: teacher-forced forward 0 on the reference HR coords.
    let cond1024_host = load("cond_1024.npy")?.as_f32()?;
    let cond1024 = t2_upload_cond(&cond1024_host).map_err(|err| err.to_string())?;
    let hr_rope = t2_upload_rope(&t2_rope_tables(&hr_tokens_ref)).map_err(|err| err.to_string())?;
    let hr_path = models.join("ckpts").join("slat_flow_img2shape_dit_1_3B_1024_bf16.safetensors");
    let weights = TrellisWeights::load(&hr_path).map_err(|err| err.to_string())?;
    let dit_hr = T2Dit::prepare(weights, "t2hr", T2_SLAT_CHANNELS, T2_SLAT_CHANNELS)
        .map_err(|err| err.to_string())?;
    let x0 = load("slat_hr_fwd00_x.npy")?.as_f32()?;
    let start = std::time::Instant::now();
    let (velocity, _) = dit_hr
        .forward(&x0, hr_tokens_ref.len(), (1000.0 * plan[0].1) as f32, &cond1024, &hr_rope, None, &[])
        .map_err(|err| err.to_string())?;
    println!("hr forward: {:.3}s ({} tokens)", start.elapsed().as_secs_f64(), hr_tokens_ref.len());
    compare("slat.hr_fwd0_velocity", &velocity, &load("slat_hr_fwd00_v.npy")?.as_f32()?);
    Ok(())
}

fn run_decode(
    _dump: &Path,
    models: &Path,
    load: &dyn Fn(&str) -> Result<Npy, String>,
) -> Result<(), String> {
    // Shape decode from the REFERENCE final shape slat.
    let slat = load("shape_slat_final_feats.npy")?.as_f32()?;
    let coords = load_coords_npy(&load("shape_slat_final_coords.npy")?)?;
    println!("decode: {} tokens", coords.len());
    let dec_path = models.join("ckpts").join("shape_dec_next_dc_f16c32_fp16.safetensors");
    let dec_weights = TrellisWeights::load(&dec_path).map_err(|err| err.to_string())?;
    let dec = T2SparseDec::prepare(dec_weights, "t2sdec", 7, true).map_err(|err| err.to_string())?;
    let start = std::time::Instant::now();
    let (feats, out_coords, subs) = dec
        .decode(&slat, coords, None)
        .map_err(|err| err.to_string())?;
    println!(
        "shape decode: {:.2}s -> {} voxels",
        start.elapsed().as_secs_f64(),
        out_coords.len()
    );

    // Reference decoder output + coords.
    let ref_feats = load("shape_dec_out.npy")?.as_f32()?;
    let ref_coords = load_coords_npy(&load("fdg_in_coords.npy")?)?;
    coords_gate("decode.voxel_coords", &out_coords, &ref_coords);
    if feats.len() == ref_feats.len() {
        compare("decode.shape_feats", &feats, &ref_feats);
    }
    // Subdivision masks per stage.
    for (i, masks) in subs.iter().enumerate() {
        if let Ok(ref_mask) = load(&format!("sub{i}_mask.npy")) {
            let ref_values: Vec<f32> = ref_mask.as_f32()?;
            let ours: Vec<f32> = masks
                .iter()
                .flat_map(|m| m.iter().map(|b| if *b { 1.0f32 } else { 0.0 }))
                .collect();
            let equal = ours.len() == ref_values.len()
                && ours.iter().zip(ref_values.iter()).all(|(a, b)| a == b);
            let diff = ours
                .iter()
                .zip(ref_values.iter())
                .filter(|(a, b)| a != b)
                .count();
            println!("[decode.sub{i}] n={} equal={equal} diff={diff}", ours.len());
        }
    }

    // FDG fields + dual-grid mesh vs the reference.
    use makepad_diffusion::trellis_mesh::{t2_dual_grid_to_mesh, t2_fdg_fields, t2_mesh_to_glb};
    let fields = t2_fdg_fields(&feats).map_err(|err| err.to_string())?;
    let dualv: Vec<f32> = fields.dual_vertices.iter().flatten().copied().collect();
    compare("decode.fdg_dualv", &dualv, &load("fdg_in_dualv.npy")?.as_f32()?);
    let start = std::time::Instant::now();
    let mesh = t2_dual_grid_to_mesh(&out_coords, &fields, 1024).map_err(|err| err.to_string())?;
    println!(
        "dual grid: {:.2}s -> {} verts {} faces",
        start.elapsed().as_secs_f64(),
        mesh.vertices.len(),
        mesh.faces.len()
    );
    let ref_verts = load("mesh_raw_vertices.npy")?;
    let ref_faces = load("mesh_raw_faces.npy")?;
    println!(
        "[decode.mesh] ours V={} F={} ref V={} F={}",
        mesh.vertices.len(),
        mesh.faces.len(),
        ref_verts.shape[0],
        ref_faces.shape[0]
    );
    let flat_verts: Vec<f32> = mesh.vertices.iter().flatten().copied().collect();
    let rv = ref_verts.as_f32()?;
    if flat_verts.len() == rv.len() {
        compare("decode.mesh_vertices", &flat_verts, &rv);
    }
    let flat_faces: Vec<i32> = mesh
        .faces
        .iter()
        .flatten()
        .map(|v| *v as i32)
        .collect();
    let rf = ref_faces.as_i32()?;
    if flat_faces.len() == rf.len() {
        let equal = flat_faces == rf;
        let diff = flat_faces.iter().zip(rf.iter()).filter(|(a, b)| a != b).count();
        println!("[decode.mesh_faces] equal={equal} diff={diff}/{}", flat_faces.len());
    }

    // First our-stack GLB.
    let glb = t2_mesh_to_glb(&mesh);
    let out = r"C:\ai\out\crown_ours_untextured.glb";
    std::fs::write(out, &glb).map_err(|err| err.to_string())?;
    println!("[decode.glb] wrote {out} ({} bytes)", glb.len());
    Ok(())
}
