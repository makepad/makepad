//! Native RealESRGAN x4plus validator/bench harness.
//!
//! `realesrgan-validate <model.safetensors> [oracle-dir|input.png] [--bench N]`
//!
//! With only a checkpoint this performs the full architecture/weight
//! inventory.  With an oracle directory it expects `input_rgb8.bin`,
//! `input_shape.txt` (width height channels) and the reference pre-clamp CHW
//! dump `out_f32.bin`, reports cosine/max/MAE plus 8-bit pixel deltas, and
//! writes the native output next to the oracle's.  With a PNG it upscales and
//! writes `<input>.native-x4.png`.  `--bench N` times N warm end-to-end
//! upscales (u8 host -> u8 host, the same protocol as the oracle timing).
//! The oracle is data only; this binary never launches Python or Torch.

use makepad_diffusion::realesrgan::{
    RealEsrgan, RealEsrganImage, RealEsrganUpscale, RealEsrganWeights,
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

fn shape(path: &Path) -> Result<(usize, usize, usize), String> {
    let text = fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let values: Vec<usize> = text
        .split_whitespace()
        .map(|value| value.parse::<usize>().map_err(|err| err.to_string()))
        .collect::<Result<_, _>>()?;
    match values.as_slice() {
        [width, height, channels] => Ok((*width, *height, *channels)),
        _ => Err(format!("{} must contain: width height channels", path.display())),
    }
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

fn write_rgb_png(path: &Path, rgb: &[u8], width: usize, height: usize) -> Result<(), String> {
    if rgb.len() != width * height * 3 {
        return Err("rgb png buffer size mismatch".to_string());
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGB);
    let mut encoder = PngEncoder::new(rgb, options);
    let mut output = Vec::new();
    encoder
        .encode(&mut output)
        .map_err(|err| format!("{} png encode: {err:?}", path.display()))?;
    fs::write(path, output).map_err(|err| format!("{}: {err}", path.display()))
}

fn quantize(planes: &[f32], plane: usize) -> Vec<u8> {
    let mut rgb = vec![0u8; plane * 3];
    for pixel in 0..plane {
        for channel in 0..3 {
            rgb[pixel * 3 + channel] =
                (planes[channel * plane + pixel].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    rgb
}

fn metrics(label: &str, actual: &[f32], oracle: &[f32], plane: usize) -> Result<(), String> {
    if actual.len() != oracle.len() {
        return Err(format!(
            "{label} length {} != oracle {}",
            actual.len(),
            oracle.len()
        ));
    }
    let mut dot = 0.0f64;
    let mut aa = 0.0f64;
    let mut bb = 0.0f64;
    let mut abs = 0.0f64;
    let mut max = 0.0f32;
    for (&a, &b) in actual.iter().zip(oracle) {
        dot += f64::from(a) * f64::from(b);
        aa += f64::from(a) * f64::from(a);
        bb += f64::from(b) * f64::from(b);
        let d = (a - b).abs();
        abs += f64::from(d);
        max = max.max(d);
    }
    let cosine = dot / (aa.sqrt() * bb.sqrt()).max(f64::MIN_POSITIVE);
    let mae = abs / actual.len().max(1) as f64;
    let native8 = quantize(actual, plane);
    let oracle8 = quantize(oracle, plane);
    let mut pix_diff = 0usize;
    let mut max8 = 0u8;
    for (pixel, (a, b)) in native8.chunks_exact(3).zip(oracle8.chunks_exact(3)).enumerate() {
        let _ = pixel;
        if a != b {
            pix_diff += 1;
            for (&x, &y) in a.iter().zip(b) {
                max8 = max8.max(x.abs_diff(y));
            }
        }
    }
    println!(
        "PARITY-{label} cosine={cosine:.9} max_abs={max:.9} mae={mae:.9} \
         pix_diff={pix_diff}/{plane} max8={max8}"
    );
    Ok(())
}

fn run(
    model_path: PathBuf,
    input_spec: Option<PathBuf>,
    bench: usize,
) -> Result<(), String> {
    let load = Instant::now();
    let weights = RealEsrganWeights::load(&model_path).map_err(|err| err.to_string())?;
    println!(
        "WEIGHTS tensors={} bytes={} load_ms={:.3}",
        weights.tensor_names().count(),
        weights.file_len(),
        load.elapsed().as_secs_f64() * 1000.0
    );
    let Some(input_spec) = input_spec else {
        println!("INVENTORY-OK");
        return Ok(());
    };
    let is_png = input_spec
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    let (pixels, width, height, channels, oracle, oracle16, out_bin, out_png) = if is_png {
        let (pixels, width, height) = read_png_rgb(&input_spec)?;
        (
            pixels,
            width,
            height,
            3,
            None,
            None,
            input_spec.with_extension("native-x4-f32.bin"),
            input_spec.with_extension("native-x4.png"),
        )
    } else {
        let pixels = fs::read(input_spec.join("input_rgb8.bin"))
            .map_err(|err| format!("input_rgb8.bin: {err}"))?;
        let (width, height, channels) = shape(&input_spec.join("input_shape.txt"))?;
        let oracle = Some(read_f32(&input_spec.join("out_f32.bin"))?);
        let oracle16 = input_spec
            .join("out_fp16_f32.bin")
            .exists()
            .then(|| read_f32(&input_spec.join("out_fp16_f32.bin")))
            .transpose()?;
        (
            pixels,
            width,
            height,
            channels,
            oracle,
            oracle16,
            input_spec.join("out_native_f32.bin"),
            input_spec.join("native.png"),
        )
    };
    let input = RealEsrganImage::new(&pixels, width, height, channels)
        .map_err(|err| err.to_string())?;

    let prepare = Instant::now();
    let mut load_progress = |stage: &str, fraction: f64| {
        eprintln!("@P {fraction:.6} {stage}");
        Ok(())
    };
    let never_cancel = || false;
    let model = RealEsrgan::prepare_controlled(
        &weights,
        Some(&never_cancel),
        Some(&mut load_progress),
    )
    .map_err(|err| err.to_string())?;
    println!("PREPARE_MS {:.3}", prepare.elapsed().as_secs_f64() * 1000.0);

    let infer = Instant::now();
    let mut infer_progress = |stage: &str, fraction: f64| {
        eprintln!("@P {fraction:.6} {stage}");
        Ok(())
    };
    let upscale: RealEsrganUpscale = model
        .upscale_controlled(input, Some(&never_cancel), Some(&mut infer_progress))
        .map_err(|err| err.to_string())?;
    println!("INFER_MS {:.3}", infer.elapsed().as_secs_f64() * 1000.0);
    let out_plane = upscale.width * upscale.height;

    if let Some(oracle) = &oracle {
        metrics("F32", &upscale.planes, oracle, out_plane)?;
    }
    if let Some(oracle16) = &oracle16 {
        metrics("FP16", &upscale.planes, oracle16, out_plane)?;
    }

    fs::write(
        &out_bin,
        upscale
            .planes
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .map_err(|err| format!("write native output: {err}"))?;
    println!("OUTPUT {}", out_bin.display());
    write_rgb_png(&out_png, &upscale.rgb8(), upscale.width, upscale.height)?;
    println!("OUTPUT_PNG {}", out_png.display());

    if bench > 0 {
        // Warm end-to-end: u8 host pixels in, quantized u8 host pixels out —
        // the same protocol as the recorded oracle timings.
        let mut times = Vec::with_capacity(bench);
        for _ in 0..bench {
            let start = Instant::now();
            let rgb = model
                .upscale_rgb8_controlled(input, Some(&never_cancel), None)
                .map_err(|err| err.to_string())?;
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(rgb);
        }
        let mut sorted = times.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let line = times
            .iter()
            .map(|t| format!("{t:.2}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("WARM_MS median={median:.2} min={:.2} all=[{line}]", sorted[0]);
    }
    Ok(())
}

fn main() {
    let mut model = None;
    let mut input = None;
    let mut bench = 0usize;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bench" {
            let Some(value) = args.next().and_then(|v| v.to_str().map(str::to_string)) else {
                eprintln!("--bench requires a count");
                std::process::exit(2);
            };
            bench = match value.parse() {
                Ok(value) => value,
                Err(_) => {
                    eprintln!("--bench requires a count");
                    std::process::exit(2);
                }
            };
        } else if model.is_none() {
            model = Some(PathBuf::from(arg));
        } else if input.is_none() {
            input = Some(PathBuf::from(arg));
        } else {
            eprintln!(
                "usage: realesrgan-validate <model.safetensors> [oracle-dir|input.png] [--bench N]"
            );
            std::process::exit(2);
        }
    }
    let Some(model) = model else {
        eprintln!(
            "usage: realesrgan-validate <model.safetensors> [oracle-dir|input.png] [--bench N]"
        );
        std::process::exit(2);
    };
    if let Err(err) = run(model, input, bench) {
        eprintln!("ERROR: {err}");
        std::process::exit(1);
    }
}
