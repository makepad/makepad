//! Native SAM 3.1 Multiplex validator.
//!
//! `sam3-validate <model.safetensors> [oracle-dir|input.png] [prompt] [warm-runs]`
//!
//! With only a checkpoint this inventories the Comfy-Org multiplex header
//! (no GPU). With an image it runs the native detector and writes packed
//! f32 masks plus a grayscale PNG. An oracle directory may contain
//! `alpha_f32.bin` / encoder-decoder taps for cosine/max/MAE/IoU.

use makepad_diffusion::backend::gpu_perf_stats;
use makepad_diffusion::sam3::{
    preprocess_image, Sam3, Sam3Image, Sam3Prompt, Sam3Weights, SAM3_EXECUTION, SAM3_MODEL_SHA256,
    SAM3_MODEL_SIZE,
};
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::{DecoderOptions, EncoderOptions};
use makepad_zune_png::{PngDecoder, PngEncoder};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn read_f32(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() % 4 != 0 {
        return Err(format!("{} is not packed f32", path.display()));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn read_png_rgb(path: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let file = fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(BufReader::new(file), options);
    decoder
        .decode_headers()
        .map_err(|err| format!("{} png header: {err:?}", path.display()))?;
    let info = decoder
        .info()
        .cloned()
        .ok_or_else(|| format!("{} png has no info", path.display()))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| format!("{} png has no colorspace", path.display()))?;
    let pixels = decoder
        .decode_raw()
        .map_err(|err| format!("{} png decode: {err:?}", path.display()))?;
    let components = colorspace.num_components();
    if components < 3 {
        return Err(format!(
            "{} png has {components} components; RGB/RGBA required",
            path.display()
        ));
    }
    let pixel_count = info.width as usize * info.height as usize;
    let mut rgb = vec![0u8; pixel_count * 3];
    for (source, target) in pixels.chunks_exact(components).zip(rgb.chunks_exact_mut(3)) {
        target.copy_from_slice(&source[..3]);
    }
    Ok((rgb, info.width as usize, info.height as usize))
}

fn write_alpha_png(path: &Path, alpha: &[f32], width: usize, height: usize) -> Result<(), String> {
    let pixels: Vec<u8> = alpha
        .iter()
        .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::Luma);
    let mut encoder = PngEncoder::new(&pixels, options);
    let mut output = Vec::new();
    encoder
        .encode(&mut output)
        .map_err(|err| format!("{} png encode: {err:?}", path.display()))?;
    fs::write(path, output).map_err(|err| format!("{}: {err}", path.display()))
}

fn load_npy(path: &Path) -> Result<(Vec<usize>, String, Vec<u8>), String> {
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
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
    let header = String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
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

fn npy_f32(path: &Path) -> Result<(Vec<usize>, Vec<f32>), String> {
    let (shape, descr, data) = load_npy(path)?;
    match descr.as_str() {
        "<f4" => Ok((
            shape,
            data.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )),
        other => Err(format!("{}: descr {other}, expected <f4", path.display())),
    }
}

fn npy_u8(path: &Path) -> Result<(Vec<usize>, Vec<u8>), String> {
    let (shape, descr, data) = load_npy(path)?;
    match descr.as_str() {
        "|u1" | "u1" => Ok((shape, data)),
        other if other.ends_with("u1") => Ok((shape, data)),
        other => Err(format!("{}: descr {other}, expected u1", path.display())),
    }
}

fn metrics_named(name: &str, actual: &[f32], oracle: &[f32]) -> Result<(f64, f32, f64, f64), String> {
    if actual.len() != oracle.len() {
        return Err(format!(
            "{name} length {} != oracle {}",
            actual.len(),
            oracle.len()
        ));
    }
    let mut dot = 0.0f64;
    let mut aa = 0.0f64;
    let mut bb = 0.0f64;
    let mut abs = 0.0f64;
    let mut max = 0.0f32;
    let mut intersection = 0usize;
    let mut union = 0usize;
    for (&a, &b) in actual.iter().zip(oracle) {
        dot += f64::from(a) * f64::from(b);
        aa += f64::from(a) * f64::from(a);
        bb += f64::from(b) * f64::from(b);
        let d = (a - b).abs();
        abs += f64::from(d);
        max = max.max(d);
        let af = a >= 0.5;
        let bf = b >= 0.5;
        intersection += usize::from(af && bf);
        union += usize::from(af || bf);
    }
    let cosine = dot / (aa.sqrt() * bb.sqrt()).max(f64::MIN_POSITIVE);
    let mae = abs / actual.len().max(1) as f64;
    let iou = intersection as f64 / union.max(1) as f64;
    println!(
        "PARITY {name} cosine={cosine:.9} max_abs={max:.9} mae={mae:.9} iou50={iou:.9}"
    );
    Ok((cosine, max, mae, iou))
}

fn metrics(actual: &[f32], oracle: &[f32]) -> Result<(), String> {
    let (cosine, max, mae, iou) = metrics_named("mask", actual, oracle)?;
    println!(
        "TOLERANCE fp16/bf16: cosine>=0.995 mae<=0.02 iou50>=0.90 (dump-locked); max_abs ignored on binary 05"
    );
    // 05 dumps are binary 0/1. A handful of edge pixels make max_abs=1.0 even at IoU 0.998.
    if cosine < 0.995 || mae > 0.02 || iou < 0.90 {
        println!("GATE fail");
    } else {
        println!("GATE pass");
    }
    Ok(())
}

fn is_oracle_npy_dir(path: &Path) -> bool {
    path.is_dir()
        && (path.join("01_preprocessed_rgb_01_nchw.npy").is_file()
            || path.join("05_p0_cat_soft_masks.npy").is_file()
            || path.join("manifest.json").is_file())
}

fn run(
    model_path: PathBuf,
    input_spec: Option<PathBuf>,
    prompt: String,
    warm_runs: usize,
) -> Result<(), String> {
    let load = Instant::now();
    let weights = Sam3Weights::load(&model_path).map_err(|err| err.to_string())?;
    println!(
        "WEIGHTS tensors={} bytes={} sha256_pin={} size_pin={} load_ms={:.3}",
        weights.tensor_names().count(),
        weights.file_len(),
        SAM3_MODEL_SHA256,
        SAM3_MODEL_SIZE,
        load.elapsed().as_secs_f64() * 1000.0
    );
    println!(
        "EXECUTION backend={} weights={} activations={} accumulation={}",
        SAM3_EXECUTION.compute_backend,
        SAM3_EXECUTION.weight_precision,
        SAM3_EXECUTION.activation_precision,
        SAM3_EXECUTION.accumulation_precision
    );
    let Some(input_spec) = input_spec else {
        println!("INVENTORY-OK");
        return Ok(());
    };

    let mut oracle_dir: Option<PathBuf> = None;
    let (pixels, width, height, oracle, output_dir) = if is_oracle_npy_dir(&input_spec) {
        oracle_dir = Some(input_spec.clone());
        let rgb_npy = input_spec.join("00_fixture_rgb_u8.npy");
        let (pixels, width, height) = if rgb_npy.is_file() {
            let (shape, data) = npy_u8(&rgb_npy)?;
            match shape.as_slice() {
                [h, w, 3] => (data, *w, *h),
                other => {
                    return Err(format!(
                        "00_fixture_rgb_u8.npy shape {other:?} expected [H,W,3]"
                    ))
                }
            }
        } else {
            let png = if input_spec.join("00_fixture_rgb.png").is_file() {
                input_spec.join("00_fixture_rgb.png")
            } else if Path::new(r"C:\ai\sam3\fixtures\cat_still.png").is_file() {
                PathBuf::from(r"C:\ai\sam3\fixtures\cat_still.png")
            } else {
                return Err("oracle dumps dir has no RGB fixture".into());
            };
            read_png_rgb(&png)?
        };
        let oracle = if input_spec.join("05_p0_cat_soft_masks.npy").is_file() {
            let (shape, data) = npy_f32(&input_spec.join("05_p0_cat_soft_masks.npy"))?;
            println!("ORACLE soft_masks shape={shape:?} n={}", data.len());
            Some(data)
        } else {
            None
        };
        let output_dir = PathBuf::from(r"C:\ai\sam3\native_out");
        fs::create_dir_all(&output_dir)
            .map_err(|err| format!("create {}: {err}", output_dir.display()))?;
        (pixels, width, height, oracle, output_dir)
    } else if input_spec.is_dir() {
        let pixels = fs::read(input_spec.join("input_rgb8.bin"))
            .map_err(|err| format!("input_rgb8.bin: {err}"))?;
        let shape = fs::read_to_string(input_spec.join("input_shape.txt"))
            .map_err(|err| format!("input_shape.txt: {err}"))?;
        let values: Vec<usize> = shape
            .split_whitespace()
            .map(|v| v.parse().map_err(|err| format!("shape: {err}")))
            .collect::<Result<_, _>>()?;
        let (width, height) = match values.as_slice() {
            [w, h, _] | [w, h] => (*w, *h),
            _ => return Err("input_shape.txt must contain width height".into()),
        };
        let oracle = input_spec
            .join("alpha_f32.bin")
            .exists()
            .then(|| read_f32(&input_spec.join("alpha_f32.bin")))
            .transpose()?;
        (pixels, width, height, oracle, input_spec)
    } else {
        let (pixels, width, height) = read_png_rgb(&input_spec)?;
        let output_dir = input_spec
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sam3-native");
        fs::create_dir_all(&output_dir)
            .map_err(|err| format!("create {}: {err}", output_dir.display()))?;
        (pixels, width, height, None, output_dir)
    };

    let parsed = Sam3Prompt::parse(&prompt).map_err(|err| err.to_string())?;
    println!("PROMPT phrases={}", parsed.phrases.len());
    let image = Sam3Image::rgb8(&pixels, width, height).map_err(|err| err.to_string())?;
    let preprocessed = preprocess_image(image).map_err(|err| err.to_string())?;
    println!(
        "INPUT original={}x{} processed={}x{} src={}x{}",
        width,
        height,
        preprocessed.width,
        preprocessed.height,
        preprocessed.src_width,
        preprocessed.src_height
    );
    if let Some(dir) = oracle_dir.as_ref() {
        let pre_path = dir.join("01_preprocessed_rgb_01_nchw.npy");
        if pre_path.is_file() {
            let (shape, data) = npy_f32(&pre_path)?;
            println!("ORACLE preprocess shape={shape:?}");
            let _ = metrics_named("preprocess", &preprocessed.pixels, &data)?;
        }
    }

    let vram = gpu_perf_stats(false);
    let baseline_free = vram.mem_free_bytes;
    println!(
        "VRAM_BASELINE free_mb={} total_mb={}",
        baseline_free / (1 << 20),
        vram.mem_total_bytes / (1 << 20)
    );
    let prepare = Instant::now();
    let model = Sam3::prepare(&weights).map_err(|err| err.to_string())?;
    println!("PREPARE_MS {:.3}", prepare.elapsed().as_secs_f64() * 1000.0);

    let run_count = warm_runs.max(1);
    let mut durations = Vec::with_capacity(run_count);
    let mut last = None;
    for index in 0..run_count {
        let start = Instant::now();
        let mask = model
            .segment_preprocessed(&preprocessed, &parsed, None, None)
            .map_err(|err| err.to_string())?;
        let seconds = start.elapsed().as_secs_f64();
        println!("RUN index={index} seconds={seconds:.9} detections={}", mask.scores.len());
        durations.push(seconds);
        last = Some(mask);
    }
    let mask = last.expect("at least one native SAM3 run");
    let vram = gpu_perf_stats(false);
    println!(
        "VRAM_AFTER free_mb={} used_delta_mb={}",
        vram.mem_free_bytes / (1 << 20),
        baseline_free.saturating_sub(vram.mem_free_bytes) / (1 << 20)
    );
    let mut measured = if durations.len() > 1 {
        durations[1..].to_vec()
    } else {
        durations
    };
    measured.sort_by(f64::total_cmp);
    println!("WARM_MEDIAN_SECONDS {:.9}", measured[measured.len() / 2]);
    if let Some(oracle) = oracle {
        metrics(&mask.alpha, &oracle)?;
        let _ = metrics_named("05_soft_masks", &mask.alpha, &oracle);
    } else {
        println!("PARITY skipped (no oracle soft mask)");
    }
    if let Some(dir) = oracle_dir.as_ref() {
        let union_path = dir.join("05_p0_cat_union_binary.npy");
        if union_path.is_file() {
            let (_, want_u8) = npy_u8(&union_path)?;
            let want: Vec<f32> = want_u8.iter().map(|&v| v as f32).collect();
            if want.len() == mask.alpha.len() {
                let _ = metrics_named("05_union_binary", &mask.alpha, &want);
            } else {
                println!(
                    "PARITY 05_union_binary skipped length {} != {}",
                    mask.alpha.len(),
                    want.len()
                );
            }
        }
    }
    println!(
        "DETECTIONS kept={} raw={} top_score={}",
        mask.scores.len(),
        mask.raw_scores.len(),
        mask.raw_scores
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max)
    );
    if let Some(dir) = oracle_dir.as_ref() {
        if !mask.raw_scores.is_empty() && dir.join("04_p0_cat_scores.npy").is_file() {
            let (_, want) = npy_f32(&dir.join("04_p0_cat_scores.npy"))?;
            let _ = metrics_named("04_scores", &mask.raw_scores, &want);
        }
        if !mask.raw_boxes_xyxy.is_empty() && dir.join("04_p0_cat_boxes.npy").is_file() {
            let (_, want) = npy_f32(&dir.join("04_p0_cat_boxes.npy"))?;
            let got: Vec<f32> = mask.raw_boxes_xyxy.iter().flatten().copied().collect();
            let _ = metrics_named("04_boxes", &got, &want);
        }
        if !mask.raw_boxes_xyxy.is_empty() && dir.join("04_p0_cat_kept_boxes.npy").is_file() {
            let (shape, want) = npy_f32(&dir.join("04_p0_cat_kept_boxes.npy"))?;
            if let Some(best) = mask.raw_scores
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
            {
                let got = mask.raw_boxes_xyxy[best];
                println!(
                    "KEPT_BOX native={got:?} oracle_shape={shape:?} oracle={:?}",
                    &want[..want.len().min(4)]
                );
            }
        }
    }
    if let Some(dir) = oracle_dir.as_ref() {
        for (name, file) in [
            ("native_trunk", "native_trunk.f32"),
            ("native_fpn0", "native_fpn0.f32"),
            ("native_fpn1", "native_fpn1.f32"),
            ("native_fpn2", "native_fpn2.f32"),
            ("native_text", "native_text.f32"),
        ] {
            let tap = output_dir.join(file);
            if !tap.is_file() {
                continue;
            }
            let oracle_name = match name {
                "native_trunk" => "03_image_trunk.npy",
                "native_fpn0" => "03_image_fpn_0.npy",
                "native_fpn1" => "03_image_fpn_1.npy",
                "native_fpn2" => "03_image_fpn_2.npy",
                "native_text" => "02_p0_cat_text_emb_resized.npy",
                _ => continue,
            };
            let oracle_path = dir.join(oracle_name);
            if !oracle_path.is_file() {
                continue;
            }
            let actual = read_f32(&tap)?;
            let (_, want) = npy_f32(&oracle_path)?;
            let _ = metrics_named(name, &actual, &want);
        }
    }
    fs::write(
        output_dir.join("alpha_native_f32.bin"),
        mask.alpha.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
    )
    .map_err(|err| format!("write native alpha: {err}"))?;
    write_alpha_png(
        &output_dir.join("alpha_native.png"),
        &mask.alpha,
        mask.width,
        mask.height,
    )?;
    println!("OUTPUT {}", output_dir.display());
    Ok(())
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(model) = args.next() else {
        eprintln!(
            "usage: sam3-validate <model.safetensors> [oracle-dir|input.png] [prompt] [warm-runs]"
        );
        std::process::exit(2);
    };
    let input = args.next().map(PathBuf::from);
    let prompt = args
        .next()
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cat".to_string());
    let warm_runs = args
        .next()
        .and_then(|v| v.to_string_lossy().parse().ok())
        .unwrap_or(3);
    if let Err(err) = run(PathBuf::from(model), input, prompt, warm_runs) {
        eprintln!("ERROR: {err}");
        std::process::exit(1);
    }
}
