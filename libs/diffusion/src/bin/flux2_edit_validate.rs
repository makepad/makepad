//! FLUX.2-klein-4B image-edit parity validator.
//!
//! Compares the native CUDA port against locked Python oracle dumps at
//! `C:\ai\flux2edit\dumps\` (or `--dumps`). Gates:
//!   input_ids          exact
//!   packed_latents     max_abs / cosine
//!   mid-step residual  max_abs / cosine
//!   decoded PNG        max_abs over f32 RGB (after banker u8)
//!   warm edit time     not slower than oracle `warm_ms` (when present)
//!
//! Usage:
//!   flux2-edit-validate --models <dir> --dumps <dir> [--strict]
//!
//! Fail-closed without CUDA.

use makepad_diffusion::backend::gpu_device_available;
use makepad_diffusion::flux2_pipeline::{
    flux2_klein_paths_from_root, Flux2EditRequest, Flux2KleinPipeline,
};
use makepad_diffusion::flux2_vae::{flux2_image_from_rgb_u8, flux2_image_to_rgb_u8};
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::io::BufReader;
use std::path::{Path, PathBuf};

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
    let header = String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
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
            "<f2" | "<e2" => Ok(self
                .data
                .chunks_exact(2)
                .map(|c| {
                    let word = u16::from_le_bytes([c[0], c[1]]);
                    f32::from_bits((word as u32) << 16)
                })
                .collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }

    fn as_i64(&self) -> Result<Vec<i64>, String> {
        match self.descr.as_str() {
            "<i8" => Ok(self
                .data
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
                .collect()),
            "<i4" => Ok(self
                .data
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64)
                .collect()),
            other => Err(format!("npy descr {other} not int")),
        }
    }
}

fn compare(name: &str, got: &[f32], exp: &[f32]) -> Result<(f64, f64), String> {
    if got.len() != exp.len() {
        return Err(format!(
            "{name}: length {} vs oracle {}",
            got.len(),
            exp.len()
        ));
    }
    let mut max_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (a, b) in got.iter().zip(exp.iter()) {
        let d = (*a as f64 - *b as f64).abs();
        if d > max_abs {
            max_abs = d;
        }
        dot += *a as f64 * *b as f64;
        na += *a as f64 * *a as f64;
        nb += *b as f64 * *b as f64;
    }
    let cosine = if na > 0.0 && nb > 0.0 {
        dot / (na.sqrt() * nb.sqrt())
    } else {
        0.0
    };
    Ok((max_abs, cosine))
}

fn load_png_rgb(path: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let file = std::fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let reader = BufReader::new(file);
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(reader, options);
    decoder.decode_headers().map_err(|err| format!("{err:?}"))?;
    let info = decoder.info().cloned().ok_or("png: no info")?;
    let colorspace = decoder.colorspace().ok_or("png: no colorspace")?;
    let pixels = decoder.decode_raw().map_err(|err| format!("{err:?}"))?;
    let components = colorspace.num_components();
    let (w, h) = (info.width as usize, info.height as usize);
    if components < 3 {
        return Err(format!("png components {components} unsupported"));
    }
    let mut rgb = vec![0u8; w * h * 3];
    for (i, chunk) in pixels.chunks_exact(components).enumerate() {
        rgb[i * 3..i * 3 + 3].copy_from_slice(&chunk[..3]);
    }
    Ok((rgb, w, h))
}

fn parse_args() -> Result<(PathBuf, PathBuf, bool), String> {
    let mut models = None;
    let mut dumps = None;
    let mut strict = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--models" => models = args.next().map(PathBuf::from),
            "--dumps" => dumps = args.next().map(PathBuf::from),
            "--strict" => strict = true,
            other => return Err(format!("unknown arg {other}")),
        }
    }
    Ok((
        models.unwrap_or_else(|| PathBuf::from(r"C:\ai\flux2edit\weights\FLUX.2-klein-4B")),
        dumps.unwrap_or_else(|| PathBuf::from(r"C:\ai\flux2edit\dumps\jacket_red")),
        strict,
    ))
}

fn main() {
    if let Err(err) = run() {
        eprintln!("flux2-edit-validate: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if !gpu_device_available() {
        return Err("CUDA is required; refusing CPU/Metal fallback".into());
    }
    let (models, dumps, strict) = parse_args()?;
    if !dumps.is_dir() {
        return Err(format!(
            "oracle dump dir missing: {} (run ref_dump_flux2_klein_edit.py first)",
            dumps.display()
        ));
    }
    let paths = flux2_klein_paths_from_root(&models).map_err(|err| err.to_string())?;
    let mut pipe = Flux2KleinPipeline::load(paths).map_err(|err| err.to_string())?;

    // FLUX2_DECODE_ORACLE=1: decode the oracle's own final latents through
    // the native VAE and exit — isolates VAE decode error from denoise
    // trajectory drift when the decoded PNG looks systematically off.
    if std::env::var("FLUX2_DECODE_ORACLE").as_deref() == Ok("1") {
        let latents = load_npy(&dumps.join("latents_final_packed.npy"))?.as_f32()?;
        let tokens = latents.len() / 128;
        let side = (tokens as f64).sqrt() as usize;
        let packed = makepad_diffusion::flux2::Flux2PackedLatents::from_tokens(
            &latents, side, side, 128,
        )
        .map_err(|err| err.to_string())?;
        let image = makepad_diffusion::flux2_vae::flux2_vae_decode(&pipe.vae, &packed)
            .map_err(|err| err.to_string())?;
        let (mut lo, mut hi, mut sum) = (f32::INFINITY, f32::NEG_INFINITY, 0.0f64);
        for &v in &image.data {
            lo = lo.min(v);
            hi = hi.max(v);
            sum += v as f64;
        }
        println!(
            "decoded data range: min={lo:.4} max={hi:.4} mean={:.4}",
            sum / image.data.len() as f64
        );
        let rgb = flux2_image_to_rgb_u8(&image);
        // encode_png_rgb's to_u8 maps [-1,1] -> [0,255].
        let mut planar = vec![0.0f32; rgb.len()];
        let plane = image.width * image.height;
        for i in 0..plane {
            planar[i] = rgb[i * 3] as f32 * (2.0 / 255.0) - 1.0;
            planar[plane + i] = rgb[i * 3 + 1] as f32 * (2.0 / 255.0) - 1.0;
            planar[2 * plane + i] = rgb[i * 3 + 2] as f32 * (2.0 / 255.0) - 1.0;
        }
        let png = makepad_diffusion::flux_pipeline::encode_png_rgb(
            &planar,
            image.width,
            image.height,
        )
        .map_err(|err| err.to_string())?;
        let out = dumps.join("oracle_latents_native_decoded.png");
        std::fs::write(&out, &png).map_err(|err| err.to_string())?;
        println!("wrote {}", out.display());
        return Ok(());
    }

    let meta_path = dumps.join("meta.json");
    let meta: serde_lite::Meta = if meta_path.is_file() {
        let text = std::fs::read_to_string(&meta_path).map_err(|err| err.to_string())?;
        serde_lite::parse_meta(&text)
    } else {
        serde_lite::Meta {
            prompt: "change the jacket to red leather".into(),
            width: 512,
            height: 512,
            steps: 4,
            seed: 7,
            warm_ms: Some(671.0),
        }
    };

    let ref_png = [
        dumps.join("reference.png"),
        PathBuf::from(r"C:\ai\flux2edit\fixtures\jacket_input.png"),
        models.join("editing.jpg"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| "missing reference PNG".to_string())?;
    let (rgb, rw, rh) = load_png_rgb(&ref_png)?;
    let reference = flux2_image_from_rgb_u8(&rgb, rw, rh).map_err(|err| err.to_string())?;

    let noise = ["noise_latents_packed.npy", "noise.npy"]
        .iter()
        .map(|name| dumps.join(name))
        .find(|p| p.is_file());
    let noise = match noise {
        Some(path) => Some(load_npy(&path)?.as_f32()?),
        None => None,
    };

    let request = Flux2EditRequest {
        prompt: meta.prompt.clone(),
        width: meta.width,
        height: meta.height,
        steps: meta.steps,
        seed: meta.seed,
        references: vec![reference],
        noise: noise.clone(),
        teacher_ref_tokens: None,
        teacher_embeds: None,
        init: None,
    };
    let result = pipe.edit(&request).map_err(|err| err.to_string())?;
    // Warm pass: DiT weights are now cached; time this loop only.
    let warm = pipe.edit(&request).map_err(|err| format!("warm: {err}"))?;

    let mut failed = 0usize;
    let mut rows = Vec::new();

    if dumps.join("input_ids.npy").is_file() {
        let npy = load_npy(&dumps.join("input_ids.npy"))?;
        let oracle: Vec<i64> = if npy.descr.contains("f") {
            npy.as_f32()?
                .into_iter()
                .map(|v| v.round() as i64)
                .collect()
        } else {
            npy.as_i64()?
        };
        let got: Vec<i64> = result.input_ids.iter().map(|id| *id as i64).collect();
        let exact = got == oracle;
        rows.push(format!(
            "input_ids            exact={} native_len={} oracle_len={} real_len={}",
            exact,
            got.len(),
            oracle.len(),
            result.real_len
        ));
        if !exact {
            failed += 1;
        }
    } else {
        rows.push("input_ids            SKIP (no dump)".into());
        if strict {
            failed += 1;
        }
    }

    gate_f32(
        &mut rows,
        &mut failed,
        "sigmas",
        &dumps.join("sigmas.npy"),
        &result.sigmas,
        1e-6,
        0.999999,
        strict,
    )?;
    gate_f32(
        &mut rows,
        &mut failed,
        "prompt_embeds",
        &dumps.join("prompt_embeds.npy"),
        &result.prompt_embeds,
        5e-2,
        0.999,
        false,
    )?;
    let packed_tokens = result.ref_packed.to_tokens();
    gate_f32(
        &mut rows,
        &mut failed,
        "image_latents_packed",
        &dumps.join("image_latents_packed.npy"),
        &packed_tokens,
        8e-2,
        0.999,
        strict,
    )?;
    gate_f32(
        &mut rows,
        &mut failed,
        "image_latents_unpacked",
        &dumps.join("image_latents_unpacked.npy"),
        &result.ref_packed.data,
        8e-2,
        0.999,
        false,
    )?;
    let step0 = result.step_residuals.first().cloned().unwrap_or_default();
    gate_f32(
        &mut rows,
        &mut failed,
        "residual_step0",
        &dumps.join("residual_step0.npy"),
        &step0,
        5e-2,
        0.995,
        strict,
    )?;
    if dumps.join("residual_step0.npy").is_file() && !step0.is_empty() {
        if let Ok(exp) = load_npy(&dumps.join("residual_step0.npy")).and_then(|n| n.as_f32()) {
            if exp.len() == step0.len() && exp.len() % 128 == 0 {
                let tokens = exp.len() / 128;
                let mut worst = 0.0f64;
                let mut worst_i = 0usize;
                let mut over = 0usize;
                for token in 0..tokens {
                    let mut m = 0.0f64;
                    for c in 0..128 {
                        let d = (step0[token * 128 + c] as f64 - exp[token * 128 + c] as f64).abs();
                        if d > m {
                            m = d;
                        }
                    }
                    if m > 5e-2 {
                        over += 1;
                    }
                    if m > worst {
                        worst = m;
                        worst_i = token;
                    }
                }
                rows.push(format!(
                    "residual_step0_tok   worst={worst:.5} @token={worst_i}/{tokens} over_5e-2={over}"
                ));
            }
        }
    }
    if dumps.join("image_latents_packed.npy").is_file() && dumps.join("prompt_embeds.npy").is_file()
    {
        let teacher_ref = load_npy(&dumps.join("image_latents_packed.npy"))?.as_f32()?;
        let teacher_emb = load_npy(&dumps.join("prompt_embeds.npy"))?.as_f32()?;
        let teacher_req = Flux2EditRequest {
            prompt: meta.prompt.clone(),
            width: meta.width,
            height: meta.height,
            steps: 1,
            seed: meta.seed,
            references: vec![],
            noise: noise.clone(),
            teacher_ref_tokens: Some(teacher_ref),
            teacher_embeds: Some(teacher_emb),
            init: None,
        };
        let teacher = pipe
            .edit(&teacher_req)
            .map_err(|err| format!("teacher: {err}"))?;
        let t0 = teacher.step_residuals.first().cloned().unwrap_or_default();
        gate_f32(
            &mut rows,
            &mut failed,
            "residual_teacher",
            &dumps.join("residual_step0.npy"),
            &t0,
            5e-2,
            0.995,
            false,
        )?;
    }

    if dumps.join("latents_final_packed.npy").is_file() {
        let packed_final = result.packed_latents.to_tokens();
        gate_f32(
            &mut rows,
            &mut failed,
            "latents_final_packed",
            &dumps.join("latents_final_packed.npy"),
            &packed_final,
            8e-2,
            0.999,
            false,
        )?;
    }

    let decoded_png = ["edited.png", "decoded.png"]
        .iter()
        .map(|name| dumps.join(name))
        .find(|p| p.is_file());
    if let Some(decoded_png) = decoded_png {
        let (oracle_rgb, ow, oh) = load_png_rgb(&decoded_png)?;
        let got_rgb = flux2_image_to_rgb_u8(&result.image);
        if ow != result.image.width || oh != result.image.height {
            rows.push(format!(
                "decoded_png          FAIL size {}x{} vs {}x{}",
                result.image.width, result.image.height, ow, oh
            ));
            failed += 1;
        } else {
            let got: Vec<f32> = got_rgb.iter().map(|v| *v as f32).collect();
            let exp: Vec<f32> = oracle_rgb.iter().map(|v| *v as f32).collect();
            let (max_abs, cosine) = compare("decoded_png", &got, &exp)?;
            let ok = max_abs <= 8.0 && cosine >= 0.99;
            rows.push(format!(
                "decoded_png          max_abs={max_abs:.3} cosine={cosine:.6} {}",
                if ok { "PASS" } else { "FAIL" }
            ));
            if !ok {
                failed += 1;
            }
        }
    } else {
        rows.push("decoded_png          SKIP (no dump)".into());
        if strict {
            failed += 1;
        }
    }

    if let Some(oracle_ms) = meta.warm_ms {
        let ok = warm.warm_ms <= oracle_ms * 1.05 + 5.0;
        rows.push(format!(
            "warm_ms              native={:.1} oracle={:.1} cold={:.1} {}",
            warm.warm_ms,
            oracle_ms,
            result.warm_ms,
            if ok { "PASS" } else { "FAIL" }
        ));
        if !ok {
            failed += 1;
        }
    } else {
        rows.push(format!(
            "warm_ms              native={:.1} (no oracle timing)",
            result.warm_ms
        ));
    }

    // End-to-end warm generate vs the official 4090 stage meta (jacket/seed7):
    // TE 0.35 s, VAE encode 0.38 s, denoise 0.671 s, VAE decode 5.02 s.
    // Gates: decode alone under 5.02 s, encode+denoise+decode under 6.07 s.
    const ORACLE_TE_MS: f64 = 350.0;
    const ORACLE_ENCODE_MS: f64 = 380.0;
    const ORACLE_DECODE_MS: f64 = 5020.0;
    const ORACLE_E2E_MS: f64 = ORACLE_ENCODE_MS + 671.0 + ORACLE_DECODE_MS;
    rows.push(format!(
        "warm_stage_ms        te={:.1} encode={:.1} denoise={:.1} decode={:.1} png={:.1} total={:.1}",
        warm.te_ms, warm.encode_ms, warm.warm_ms, warm.decode_ms, warm.png_ms, warm.total_ms
    ));
    {
        let ok = warm.decode_ms <= ORACLE_DECODE_MS;
        rows.push(format!(
            "warm_decode_ms       native={:.1} oracle={ORACLE_DECODE_MS:.1} {}",
            warm.decode_ms,
            if ok { "PASS" } else { "FAIL" }
        ));
        if !ok {
            failed += 1;
        }
    }
    {
        let e2e = warm.encode_ms + warm.warm_ms + warm.decode_ms;
        let ok = e2e <= ORACLE_E2E_MS;
        rows.push(format!(
            "warm_e2e_ms          native={e2e:.1} (enc+denoise+dec) oracle={ORACLE_E2E_MS:.1} {}",
            if ok { "PASS" } else { "FAIL" }
        ));
        if !ok {
            failed += 1;
        }
        let full = warm.te_ms + e2e;
        let full_oracle = ORACLE_TE_MS + ORACLE_E2E_MS;
        rows.push(format!(
            "warm_full_ms         native={full:.1} (te+enc+denoise+dec) oracle={full_oracle:.1} {}",
            if full <= full_oracle { "PASS" } else { "FAIL" }
        ));
    }

    println!("flux2-edit-validate");
    for row in &rows {
        println!("  {row}");
    }
    let out = dumps.join("native_decoded.png");
    std::fs::write(&out, &result.png).ok();
    println!("  wrote {}", out.display());
    if failed > 0 {
        return Err(format!("{failed} gate(s) failed"));
    }
    println!("  ALL GATES PASSED");
    Ok(())
}

fn gate_f32(
    rows: &mut Vec<String>,
    failed: &mut usize,
    name: &str,
    path: &Path,
    got: &[f32],
    max_abs_tol: f64,
    cosine_tol: f64,
    strict: bool,
) -> Result<(), String> {
    if !path.is_file() {
        rows.push(format!("{name:<20} SKIP (no dump)"));
        if strict {
            *failed += 1;
        }
        return Ok(());
    }
    let exp = load_npy(path)?.as_f32()?;
    let (max_abs, cosine) = compare(name, got, &exp)?;
    let ok = max_abs <= max_abs_tol && cosine >= cosine_tol;
    rows.push(format!(
        "{name:<20} max_abs={max_abs:.5} cosine={cosine:.6} {}",
        if ok { "PASS" } else { "FAIL" }
    ));
    if !ok {
        *failed += 1;
    }
    Ok(())
}

mod serde_lite {
    pub struct Meta {
        pub prompt: String,
        pub width: u32,
        pub height: u32,
        pub steps: usize,
        pub seed: u64,
        pub warm_ms: Option<f64>,
    }

    pub fn parse_meta(text: &str) -> Meta {
        let mut meta = Meta {
            prompt: "change the sky to sunset orange".into(),
            width: 512,
            height: 512,
            steps: 4,
            seed: 7,
            warm_ms: None,
        };
        for line in text.lines() {
            if let Some(v) = json_str(line, "prompt") {
                meta.prompt = v;
            }
            if let Some(v) = json_num(line, "width") {
                meta.width = v as u32;
            }
            if let Some(v) = json_num(line, "height") {
                meta.height = v as u32;
            }
            if let Some(v) = json_num(line, "steps") {
                meta.steps = v as usize;
            }
            if let Some(v) = json_num(line, "seed") {
                meta.seed = v as u64;
            }
            if let Some(v) = json_num(line, "warm_ms") {
                meta.warm_ms = Some(v);
            }
            if let Some(v) = json_num(line, "denoise_s") {
                meta.warm_ms = Some(v * 1000.0);
            }
        }
        meta
    }

    fn json_str(line: &str, key: &str) -> Option<String> {
        let pat = format!("\"{key}\"");
        let rest = line.split(&pat).nth(1)?;
        let rest = rest.split(':').nth(1)?;
        let rest = rest.trim().trim_start_matches('"');
        let value = rest.split('"').next()?;
        Some(value.to_string())
    }

    fn json_num(line: &str, key: &str) -> Option<f64> {
        let pat = format!("\"{key}\"");
        let rest = line.split(&pat).nth(1)?;
        let rest = rest.split(':').nth(1)?;
        let token = rest
            .trim()
            .trim_end_matches(',')
            .split(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .next()?;
        token.parse().ok()
    }
}
