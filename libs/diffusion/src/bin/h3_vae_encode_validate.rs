//! MiniMax H3 VAE ENCODER validation against the diffusers i2v oracle dump
//! (h3_i2v_dump.py -> C:\ai\out\dump_i2v):
//!
//! * resize: our PIL-LANCZOS port on the input png vs the dumped canvas
//!   (keyframe_canvas_u8.npy) — gate max diff <= 2 and >= 99% exact-or-+-1
//!   (the port is byte-exact in unit tests; the gate allows png decoder
//!   slack).
//! * encode: the normalized input vs vae_enc_in.npy (~exact), then the tiled
//!   spatial encode vs vae_enc_moments.npy — gate cosine >= 0.999 (the
//!   reference encoder ran in bf16; expect bf16-class deltas). CUDA only.
//! * arith: 0.999 * cond_latents_0 + 0.001 * cond_noise_0 vs cond_noised_0
//!   (max_abs < 1e-6 — cross-checks the pipeline's scale_noise math), plus
//!   an INFORMATIONAL mean-only posterior (eps = 0) vs cond_latents_0
//!   (shows the posterior std scale; the reference sampled real noise).
//!
//! Usage: h3-vae-encode-validate --dump <dir> --models <dir> [--image <png>]
//!        [--stage resize|encode|arith|all]
//!
//! Exit 0 only if every REQUESTED stage's gate passes. resize/arith run
//! CPU-only (mac ok); encode needs the CUDA box.

use makepad_diffusion::h3::H3ShardedWeights;
use makepad_diffusion::h3_image::resize_rgb_lanczos3;
use makepad_diffusion::h3_vae::{
    h3_vae_condition_latents, h3_vae_encode_keyframe_moments, h3_vae_normalize_canvas,
    H3VaeEncoderPrepared,
};
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};

// --- minimal .npy reader (h3_validate.rs) -----------------------------------

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
            "<f2" => Ok(self
                .data
                .chunks_exact(2)
                .map(|c| makepad_diffusion::h3::f16_word_to_f32(u16::from_le_bytes([c[0], c[1]])))
                .collect()),
            other => Err(format!("npy descr {other} not f32-convertible")),
        }
    }

    fn as_u8(&self) -> Result<&[u8], String> {
        match self.descr.as_str() {
            "|u1" | "<u1" | "u1" => Ok(&self.data),
            other => Err(format!("npy descr {other} not u8")),
        }
    }
}

// --- helpers -----------------------------------------------------------------

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
        return Err(format!("png components {components} unsupported (need rgb)"));
    }
    let mut rgb = vec![0u8; w * h * 3];
    for (i, chunk) in pixels.chunks_exact(components).enumerate() {
        rgb[i * 3..i * 3 + 3].copy_from_slice(&chunk[..3]);
    }
    Ok((rgb, w, h))
}

struct Stats {
    max_abs: f64,
    cosine: f64,
}

fn compare(name: &str, ours: &[f32], reference: &[f32]) -> Result<Stats, String> {
    if ours.len() != reference.len() {
        return Err(format!(
            "[{name}] LENGTH MISMATCH ours {} ref {}",
            ours.len(),
            reference.len()
        ));
    }
    let mut max_abs = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (a, b) in ours.iter().zip(reference.iter()) {
        let a = *a as f64;
        let b = *b as f64;
        let diff = (a - b).abs();
        max_abs = max_abs.max(diff);
        sum_abs += diff;
        dot += a * b;
        norm_a += a * a;
        norm_b += b * b;
    }
    let cosine = dot / (norm_a.sqrt() * norm_b.sqrt()).max(1e-30);
    let mean_abs = sum_abs / ours.len() as f64;
    println!("[{name}] n={} max_abs={max_abs:.6e} mean_abs={mean_abs:.6e} cosine={cosine:.8}", ours.len());
    Ok(Stats { max_abs, cosine })
}

/// The dumped canvas: (h, w, 3) u8. Returns (bytes, w, h).
fn load_canvas(dump: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let canvas = load_npy(&dump.join("keyframe_canvas_u8.npy"))?;
    let bytes = canvas.as_u8()?;
    match canvas.shape.as_slice() {
        [h, w, 3] => Ok((bytes.to_vec(), *w, *h)),
        other => Err(format!("keyframe_canvas_u8 shape {other:?}, expected (h, w, 3)")),
    }
}

// --- stages -------------------------------------------------------------------

fn stage_resize(dump: &Path, image: Option<&Path>) -> Result<(), String> {
    let image = image.ok_or(
        "resize stage needs --image <input png> (the picture the dump run resized, e.g. \
         i2v_input_960x544.png)",
    )?;
    let (src, src_w, src_h) = load_png_rgb(image)?;
    let (canvas, dst_w, dst_h) = load_canvas(dump)?;
    println!("resize: {src_w}x{src_h} -> {dst_w}x{dst_h}");
    let ours = resize_rgb_lanczos3(&src, src_w, src_h, dst_w, dst_h);
    if ours.len() != canvas.len() {
        return Err(format!("resize length mismatch ours {} ref {}", ours.len(), canvas.len()));
    }
    let mut exact = 0usize;
    let mut within1 = 0usize;
    let mut max_diff = 0u32;
    for (a, b) in ours.iter().zip(canvas.iter()) {
        let diff = (*a as i32 - *b as i32).unsigned_abs();
        max_diff = max_diff.max(diff);
        if diff == 0 {
            exact += 1;
        } else if diff == 1 {
            within1 += 1;
        }
    }
    let total = ours.len();
    let frac_ok = (exact + within1) as f64 / total as f64;
    println!(
        "[resize] exact {exact}/{total} ({:.4}%), +-1 {within1}, max_diff {max_diff}",
        100.0 * exact as f64 / total as f64
    );
    if max_diff <= 2 && frac_ok >= 0.99 {
        println!("[resize] PASS");
        Ok(())
    } else {
        Err(format!(
            "[resize] FAIL: max_diff {max_diff} (gate <= 2), exact-or-+-1 {:.4}% (gate >= 99%)",
            100.0 * frac_ok
        ))
    }
}

fn stage_encode(dump: &Path, models: &Path) -> Result<(), String> {
    let (canvas, width, height) = load_canvas(dump)?;
    println!("encode: canvas {width}x{height}");

    // Our normalized input vs the dump's — this is pure CPU arithmetic and
    // should agree to f32 rounding (~1e-7).
    let enc_in = load_npy(&dump.join("vae_enc_in.npy"))?;
    let reference_in = enc_in.as_f32()?;
    let ours_in = h3_vae_normalize_canvas(&canvas, width, height);
    let stats = compare("encode.input", &ours_in, &reference_in)?;
    if stats.max_abs > 1e-5 {
        return Err(format!(
            "[encode.input] FAIL: max_abs {:.3e} (gate 1e-5) — pixel convention drifted",
            stats.max_abs
        ));
    }

    let vae_dir = models.join("vae");
    println!("loading VAE encoder from {}", vae_dir.display());
    let weights = H3ShardedWeights::load(&vae_dir).map_err(|err| err.to_string())?;
    let prepared = H3VaeEncoderPrepared::prepare(&weights).map_err(|err| err.to_string())?;
    let start = std::time::Instant::now();
    let moments = h3_vae_encode_keyframe_moments(&prepared, &canvas, width, height)
        .map_err(|err| err.to_string())?;
    println!("encode: {:.2}s -> (48, {}, {})", start.elapsed().as_secs_f64(), height / 16, width / 16);

    let reference = load_npy(&dump.join("vae_enc_moments.npy"))?;
    println!("vae_enc_moments: {:?} {}", reference.shape, reference.descr);
    let reference = reference.as_f32()?;
    let stats = compare("encode.moments", &moments, &reference)?;
    if stats.cosine >= 0.999 {
        println!("[encode] PASS");
        Ok(())
    } else {
        Err(format!("[encode] FAIL: cosine {:.6} (gate >= 0.999)", stats.cosine))
    }
}

fn stage_arith(dump: &Path) -> Result<(), String> {
    let latents = load_npy(&dump.join("cond_latents_0.npy"))?;
    println!("cond_latents_0: {:?} {}", latents.shape, latents.descr);
    let latents = latents.as_f32()?;
    let noise = load_npy(&dump.join("cond_noise_0.npy"))?.as_f32()?;
    let noised = load_npy(&dump.join("cond_noised_0.npy"))?.as_f32()?;

    // Cross-check of the pipeline's scale_noise math (t * x0 + (1 - t) *
    // noise at t = 0.999 — the actual code lives in h3_pipeline; this pins
    // the reference semantics the coordinator implements against).
    let scaled: Vec<f32> = latents
        .iter()
        .zip(noise.iter())
        .map(|(x0, n)| 0.999f32 * x0 + 0.001f32 * n)
        .collect();
    let stats = compare("arith.scale_noise", &scaled, &noised)?;
    let pass = stats.max_abs < 1e-6;
    println!(
        "[arith.scale_noise] {} (0.999*cond_latents_0 + 0.001*cond_noise_0 vs cond_noised_0, gate max_abs < 1e-6)",
        if pass { "PASS" } else { "FAIL" }
    );

    // INFORMATIONAL: mean-only posterior (eps = 0) vs the reference's
    // SAMPLED latents — the gap shows the posterior std scale, not an error.
    match load_npy(&dump.join("vae_enc_moments.npy")).and_then(|npy| {
        let moments = npy.as_f32()?;
        let (lh, lw) = match npy.shape.as_slice() {
            [1, 48, 1, lh, lw] | [48, 1, lh, lw] => (*lh, *lw),
            [48, lh, lw] => (*lh, *lw),
            other => return Err(format!("vae_enc_moments shape {other:?}")),
        };
        let eps = vec![0.0f32; 24 * lh * lw];
        let mean_only =
            h3_vae_condition_latents(&moments, lh, lw, &eps).map_err(|err| err.to_string())?;
        compare("arith.mean_only(informational)", &mean_only, &latents)?;
        Ok(())
    }) {
        Ok(()) => {}
        Err(err) => println!("[arith.mean_only] skipped: {err}"),
    }

    if pass {
        Ok(())
    } else {
        Err(format!("[arith] FAIL: scale_noise max_abs {:.3e} (gate < 1e-6)", stats.max_abs))
    }
}

// --- main ---------------------------------------------------------------------

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
            .unwrap_or(r"C:\ai\out\dump_i2v"),
    );
    let models = PathBuf::from(
        opts.get("models")
            .map(String::as_str)
            .unwrap_or(r"C:\ai\models\MiniMax-H3"),
    );
    let stage = opts.get("stage").map(String::as_str).unwrap_or("all").to_string();
    // Default --image: the known copies of the dump run's input picture.
    let image: Option<PathBuf> = opts
        .get("image")
        .map(PathBuf::from)
        .or_else(|| {
            [
                r"C:\ai\i2v_input_960x544.png",
                "local/agent_state/minimax-h3-files/i2v_input_960x544.png",
            ]
            .iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
        });

    let run_stage = |name: &str| stage == "all" || stage == name;
    let mut failed = false;
    if run_stage("resize") {
        match stage_resize(&dump, image.as_deref()) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("resize stage FAILED: {err}");
                failed = true;
            }
        }
    }
    if run_stage("encode") {
        match stage_encode(&dump, &models) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("encode stage FAILED: {err}");
                failed = true;
            }
        }
    }
    if run_stage("arith") {
        match stage_arith(&dump) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("arith stage FAILED: {err}");
                failed = true;
            }
        }
    }
    if !["all", "resize", "encode", "arith"].contains(&stage.as_str()) {
        eprintln!("unknown --stage {stage} (resize|encode|arith|all)");
        failed = true;
    }
    if failed {
        std::process::exit(1);
    }
    println!("H3-VAE-ENCODE-VALIDATE-DONE");
}
