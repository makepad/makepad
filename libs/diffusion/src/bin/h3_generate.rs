//! MiniMax H3 end-to-end t2va generation (debug/bench CLI): TE -> DiT
//! denoise -> video VAE -> frames_u8.npy (+ audio.wav) + timing stats json.
//! The PRODUCT path is the `h3` video backend in libs/asset/ai, which calls
//! the same `h3_generate` pipeline and muxes mp4 via the platform hardware
//! encoder; this bin stays for stage debugging and perf numbers.
//!
//! Usage:
//!   h3-generate --models <MiniMax-H3 dir> --out <dir>
//!               [--prompt "text"]         arbitrary prompt (in-repo tokenizer)
//!               [--image <png>]           fl2va: first-frame keyframe (i2v)
//!               [--dump <oracle dir>]     token ids + seed-parity noise
//!               [--width 640 --height 352 --frames 124 --steps 50 --seed 42]
//!               [--own-noise]             ignore dump noise, use seeded RNG
//!               [--no-audio]              skip the audio VAE decode
//!
//! The dump's forward-0 input rows are the reference's initial noise (t=0 is
//! the noise end), so --dump at the dump's canvas reproduces the reference
//! composition; other canvases fall back to the built-in RNG automatically.
//! With --image on an fl2va dump, the leading condition rows of the dump's
//! forward-0 input are re-used verbatim (anchor parity) and the rest is the
//! reference noise.

use makepad_diffusion::h3::H3_VIDEO_PATCH_DIM;
use makepad_diffusion::h3::H3KeyframeAnchor;
use makepad_diffusion::h3_pipeline::{h3_generate, H3GenerateParams, H3KeyframeInput};
use makepad_diffusion::h3_tokenizer::H3Tokenizer;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

/// Decode a PNG to tightly packed RGB8.
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
    let mut rgb = vec![0u8; w * h * 3];
    for (i, chunk) in pixels.chunks_exact(components).enumerate() {
        match components {
            3 | 4 => rgb[i * 3..i * 3 + 3].copy_from_slice(&chunk[..3]),
            1 => {
                rgb[i * 3] = chunk[0];
                rgb[i * 3 + 1] = chunk[0];
                rgb[i * 3 + 2] = chunk[0];
            }
            _ => return Err(format!("png components {components} unsupported")),
        }
    }
    Ok((rgb, w, h))
}

// --- minimal npy read/write -------------------------------------------------

fn load_npy(path: &Path) -> Result<(Vec<usize>, String, Vec<u8>), String> {
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(format!("{}: not an npy file", path.display()));
    }
    let (header_len, header_start) = if bytes[6] == 1 {
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
    let shape: Vec<usize> = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .map(|text| {
            text.split(',')
                .filter_map(|part| part.trim().parse::<usize>().ok())
                .collect()
        })
        .ok_or_else(|| format!("{}: no shape", path.display()))?;
    Ok((shape, descr, bytes[header_start + header_len..].to_vec()))
}

fn npy_f32(path: &Path) -> Result<Vec<f32>, String> {
    let (_, descr, data) = load_npy(path)?;
    match descr.as_str() {
        "<f4" => Ok(data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        other => Err(format!("{}: descr {other}, expected <f4", path.display())),
    }
}

fn npy_i64(path: &Path) -> Result<Vec<i64>, String> {
    let (_, descr, data) = load_npy(path)?;
    match descr.as_str() {
        "<i8" => Ok(data
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect()),
        other => Err(format!("{}: descr {other}, expected <i8", path.display())),
    }
}

/// Minimal 16-bit stereo WAV writer for the debug artifact; input is planar
/// f32 `[L..., R...]`.
fn write_wav_stereo(path: &Path, planar: &[f32], sample_rate: u32) -> Result<(), String> {
    let half = planar.len() / 2;
    let (left, right) = planar.split_at(half);
    let mut pcm = Vec::with_capacity(half * 2);
    for i in 0..half {
        pcm.push((left[i].clamp(-1.0, 1.0) * 32767.0).round() as i16);
        pcm.push((right[i].clamp(-1.0, 1.0) * 32767.0).round() as i16);
    }
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 4).to_le_bytes()); // byte rate
    out.extend_from_slice(&4u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, out).map_err(|err| format!("{}: {err}", path.display()))
}

fn write_npy_u8(path: &Path, shape: &[usize], data: &[u8]) -> Result<(), String> {
    let shape_text = shape
        .iter()
        .map(|dim| format!("{dim},"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut header = format!("{{'descr': '|u1', 'fortran_order': False, 'shape': ({shape_text}), }}");
    let unpadded = 10 + header.len() + 1;
    header.push_str(&" ".repeat((64 - unpadded % 64) % 64));
    header.push('\n');
    let mut file = std::fs::File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
    file.write_all(b"\x93NUMPY\x01\x00").map_err(|err| err.to_string())?;
    file.write_all(&(header.len() as u16).to_le_bytes()).map_err(|err| err.to_string())?;
    file.write_all(header.as_bytes()).map_err(|err| err.to_string())?;
    file.write_all(data).map_err(|err| err.to_string())?;
    Ok(())
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
    if let Err(err) = run(&opts) {
        eprintln!("h3-generate FAILED: {err}");
        std::process::exit(1);
    }
    println!("H3-GENERATE-DONE");
}

fn opt_usize(opts: &HashMap<String, String>, name: &str, default: usize) -> usize {
    opts.get(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn run(opts: &HashMap<String, String>) -> Result<(), String> {
    let models = PathBuf::from(
        opts.get("models").map(String::as_str).unwrap_or(r"C:\ai\models\MiniMax-H3"),
    );
    let out_dir =
        PathBuf::from(opts.get("out").map(String::as_str).unwrap_or(r"C:\ai\out\h3_ours"));
    std::fs::create_dir_all(&out_dir).map_err(|err| err.to_string())?;
    let dump = opts.get("dump").map(PathBuf::from);

    let width = opt_usize(opts, "width", 640);
    let height = opt_usize(opts, "height", 352);
    let frames = opt_usize(opts, "frames", 124);
    let steps = opt_usize(opts, "steps", 50);
    let seed = opt_usize(opts, "seed", 42) as u64;
    let own_noise = opts.contains_key("own-noise");
    let act16 = std::env::var("H3_ACT_F16").map(|v| v != "0").unwrap_or(false);

    // Token ids: --prompt through the in-repo Qwen tokenizer port, or from
    // the dump (fixed r1 prompt) for oracle-parity runs.
    let token_ids: Vec<u32> = if let Some(prompt) = opts.get("prompt").filter(|p| !p.is_empty()) {
        let tokenizer = H3Tokenizer::load(&models.join("tokenizer")).map_err(|e| e.to_string())?;
        let ids = tokenizer.encode(prompt);
        println!("prompt: {} tokens (in-repo tokenizer)", ids.len());
        ids
    } else {
        let dump_dir = dump
            .as_ref()
            .ok_or("--prompt \"text\" or --dump <oracle dir> required (token ids source)")?;
        let ids: Vec<u32> = npy_i64(&dump_dir.join("te_token_ids.npy"))?
            .iter()
            .map(|id| *id as u32)
            .collect();
        // An fl2va dump stores the FULL presentation (label + vision block +
        // prompt); the pipeline assembles the presentation itself, so keep
        // only the prompt tail when a keyframe run reads dump ids.
        if opts.get("image").filter(|p| !p.is_empty()).is_some() {
            match ids
                .iter()
                .position(|id| *id == makepad_diffusion::h3_pipeline::H3_TOKEN_VISION_END)
            {
                Some(end) => {
                    println!(
                        "prompt: {} tokens (dump presentation tail after vision block)",
                        ids.len() - end - 1
                    );
                    ids[end + 1..].to_vec()
                }
                None => ids,
            }
        } else {
            ids
        }
    };

    // fl2va keyframes: --image <png> is the FIRST frame, --last-image <png>
    // the LAST one. Either, both or neither; both switch the workflow to
    // fl2va, and they are packed first-then-last like upstream.
    let mut keyframes: Vec<H3KeyframeInput> = Vec::new();
    for (flag, anchor) in [
        ("image", H3KeyframeAnchor::First),
        ("last-image", H3KeyframeAnchor::Last),
    ] {
        let Some(path) = opts.get(flag).filter(|p| !p.is_empty()) else {
            continue;
        };
        let (rgb, w, h) = load_png_rgb(Path::new(path))?;
        let tokenizer = H3Tokenizer::load(&models.join("tokenizer")).map_err(|e| e.to_string())?;
        let picture_label_ids = tokenizer.encode(&format!("<Picture {}>: ", keyframes.len() + 1));
        println!(
            "keyframe[{anchor:?}]: {path} {w}x{h} -> canvas {width}x{height} (label {} tokens)",
            picture_label_ids.len()
        );
        keyframes.push(H3KeyframeInput {
            rgb,
            width: w,
            height: h,
            anchor,
            picture_label_ids,
        });
    }

    // Reference noise when the canvas matches the dump geometry. On an fl2va
    // dump the forward-0 video rows are [condition rows | pure noise]: the
    // condition rows anchor verbatim (oracle parity), the tail is the noise.
    let mut video_noise = None;
    let mut audio_noise = None;
    let mut condition_rows_override = None;
    if !own_noise {
        if let Some(rows) = dump
            .as_ref()
            .and_then(|dir| npy_f32(&dir.join("dit_in_video_rows.npy")).ok())
        {
            let lw = width / 16;
            let lh = height / 16;
            let aligned = {
                let mut f = frames.max(1);
                while f % 17 != 5 {
                    f += 1;
                }
                f
            };
            let latent_frames = (aligned - 5) / 17 * 5 + 2;
            let rows_per_frame = (lh / 2) * (lw / 2);
            let expected = latent_frames * rows_per_frame * H3_VIDEO_PATCH_DIM;
            let cond_expected = keyframes.len() * rows_per_frame * H3_VIDEO_PATCH_DIM;
            if rows.len() == cond_expected + expected {
                println!("noise: torch seed-parity rows from dump");
                if cond_expected > 0 {
                    println!("keyframe: condition rows from dump (anchor parity)");
                    condition_rows_override = Some(rows[..cond_expected].to_vec());
                }
                video_noise = Some(rows[cond_expected..].to_vec());
                if let Some(audio) = dump
                    .as_ref()
                    .and_then(|dir| npy_f32(&dir.join("dit_in_audio_rows.npy")).ok())
                {
                    audio_noise = Some(audio);
                }
            } else {
                println!(
                    "noise: dump rows {} != expected {} for this canvas/workflow — own RNG",
                    rows.len(),
                    cond_expected + expected
                );
            }
        }
    }
    if video_noise.is_none() {
        println!("noise: own RNG seed {seed} (not torch-parity)");
    }
    let noise_source = if video_noise.is_some() { "dump" } else { "own-rng" };

    // Quantized-tier overrides: --dit-gguf/--te-gguf or --dit-nvfp4/--te-nvfp4
    // component files, optional --video-vae file and --audio-vae dir, and
    // --staged for the 24/32GB sequential-residency mode.
    let component = |gguf_key: &str, nvfp4_key: &str| -> Option<makepad_diffusion::h3_pipeline::H3ComponentFile> {
        use makepad_diffusion::h3_pipeline::{H3ComponentFile, H3WeightFormat};
        if let Some(path) = opts.get(gguf_key) {
            return Some(H3ComponentFile {
                path: std::path::PathBuf::from(path),
                format: H3WeightFormat::Gguf,
            });
        }
        opts.get(nvfp4_key).map(|path| H3ComponentFile {
            path: std::path::PathBuf::from(path),
            format: H3WeightFormat::Nvfp4,
        })
    };
    let dit_file = component("dit-gguf", "dit-nvfp4");
    let te_file = component("te-gguf", "te-nvfp4");
    let video_vae_path = opts.get("video-vae").map(std::path::PathBuf::from);
    let audio_vae_dir = opts.get("audio-vae").map(std::path::PathBuf::from);
    let model_set = if dit_file.is_some()
        || te_file.is_some()
        || video_vae_path.is_some()
        || audio_vae_dir.is_some()
    {
        Some(makepad_diffusion::h3_pipeline::H3ModelSet {
            dit: dit_file,
            text_encoder: te_file,
            video_vae_path,
            audio_vae_dir,
        })
    } else {
        None
    };

    let params = H3GenerateParams {
        width,
        height,
        num_frames: frames,
        num_inference_steps: steps,
        token_ids,
        seed,
        keyframes,
        video_noise_rows: video_noise,
        audio_noise_rows: audio_noise,
        condition_rows_override,
        act16,
        decode_audio: !opts.contains_key("no-audio"),
        model_set,
        staged_residency: opts.contains_key("staged"),
    };

    let output = h3_generate(&models, &params, |line| {
        println!("{line}");
        let _ = std::io::stdout().flush();
    })
    .map_err(|err| err.to_string())?;

    // Frames + stats.
    let frames_path = out_dir.join("frames_u8.npy");
    write_npy_u8(
        &frames_path,
        &[output.num_frames, output.height, output.width, 3],
        &output.frames_rgb8,
    )?;
    if let Some(planar) = &output.audio_planar {
        let wav_path = out_dir.join("audio.wav");
        write_wav_stereo(&wav_path, planar, output.audio_sample_rate)?;
        println!("audio -> {}", wav_path.display());
    }
    let t = &output.timings;
    let warm = t.warm_forward_s().unwrap_or(0.0);
    let forwards_list = t
        .forwards_s
        .iter()
        .map(|s| format!("{s:.3}"))
        .collect::<Vec<_>>()
        .join(", ");
    let denoise_total: f64 = t.forwards_s.iter().sum();
    let stats = format!(
        "{{\n \"canvas\": [{}, {}, {}],\n \"steps\": {},\n \"seed\": {},\n \"noise\": \"{}\",\n \
         \"te_load_s\": {:.2},\n \"te_encode_s\": {:.2},\n \"dit_load_s\": {:.2},\n \
         \"denoise_total_s\": {:.2},\n \"warm_s_per_forward\": {:.3},\n \
         \"vae_load_s\": {:.2},\n \"vae_decode_s\": {:.2},\n \"audio_decode_s\": {:.2},\n \"total_s\": {:.1},\n \
         \"forwards_s\": [{}]\n}}\n",
        output.width,
        output.height,
        output.num_frames,
        steps,
        seed,
        noise_source,
        t.te_load_s,
        t.te_encode_s,
        t.dit_load_s,
        denoise_total,
        warm,
        t.vae_load_s,
        t.vae_decode_s,
        t.audio_decode_s,
        t.total_s,
        forwards_list,
    );
    std::fs::write(out_dir.join("h3_ours_stats.json"), &stats).map_err(|err| err.to_string())?;
    println!(
        "totals: te {:.1}+{:.1}s dit_load {:.1}s denoise {:.1}s (warm {:.3} s/fwd) vae {:.1}+{:.1}s total {:.1}s",
        t.te_load_s,
        t.te_encode_s,
        t.dit_load_s,
        denoise_total,
        warm,
        t.vae_load_s,
        t.vae_decode_s,
        t.total_s,
    );
    println!("frames -> {}", frames_path.display());
    Ok(())
}
