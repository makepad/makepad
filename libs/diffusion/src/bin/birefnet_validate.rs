//! Native BiRefNet validator/oracle harness.
//!
//! `birefnet-validate <model.safetensors> [oracle-dir]`
//!
//! With only a checkpoint this performs the full native architecture/weight
//! inventory. With an oracle directory it additionally expects raw native
//! validator inputs (`input_rgb8.bin`, `input_shape.txt`) and the reference
//! soft matte (`alpha_f32.bin`) and reports cosine/max/MAE/IoU.  The oracle is
//! data only; this binary never launches Python or Torch.

use makepad_diffusion::birefnet::{BiRefNet, BiRefNetImage, BiRefNetWeights};
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

fn read_png_rgb(path: &Path) -> Result<(Vec<u8>, usize, usize, Option<Vec<f32>>), String> {
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
    let mut alpha = (components >= 4).then(|| Vec::with_capacity(pixel_count));
    for (source, target) in pixels.chunks_exact(components).zip(rgb.chunks_exact_mut(3)) {
        target.copy_from_slice(&source[..3]);
        if let Some(alpha) = alpha.as_mut() {
            alpha.push(f32::from(source[3]) / 255.0);
        }
    }
    Ok((rgb, info.width as usize, info.height as usize, alpha))
}

fn write_alpha_png(path: &Path, alpha: &[f32], width: usize, height: usize) -> Result<(), String> {
    let expected = width
        .checked_mul(height)
        .ok_or_else(|| "alpha png size overflow".to_string())?;
    if alpha.len() != expected {
        return Err(format!(
            "alpha png expected {expected} values, got {}",
            alpha.len()
        ));
    }
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

fn metrics(actual: &[f32], oracle: &[f32]) -> Result<(), String> {
    if actual.len() != oracle.len() {
        return Err(format!("matte length {} != oracle {}", actual.len(), oracle.len()));
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
    println!("PARITY cosine={cosine:.9} max_abs={max:.9} mae={mae:.9} iou50={iou:.9}");
    Ok(())
}

fn matte_stats(alpha: &[f32]) {
    let mut minimum = f32::INFINITY;
    let mut maximum = f32::NEG_INFINITY;
    let mut sum = 0.0f64;
    let mut foreground = 0usize;
    for &value in alpha {
        minimum = minimum.min(value);
        maximum = maximum.max(value);
        sum += f64::from(value);
        foreground += usize::from(value >= 0.5);
    }
    println!(
        "MATTE min={minimum:.9} max={maximum:.9} mean={:.9} foreground50={:.9}",
        sum / alpha.len().max(1) as f64,
        foreground as f64 / alpha.len().max(1) as f64
    );
}

fn run(model_path: PathBuf, input_spec: Option<PathBuf>) -> Result<(), String> {
    let load = Instant::now();
    let weights = BiRefNetWeights::load(&model_path).map_err(|err| err.to_string())?;
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
    let (pixels, width, height, channels, oracle_alpha, output_path, png_output_path) = if is_png {
        let (pixels, width, height, embedded_alpha) = read_png_rgb(&input_spec)?;
        let output_path = input_spec.with_extension("native-alpha-f32.bin");
        let png_output_path = Some(input_spec.with_extension("native-alpha.png"));
        if embedded_alpha.is_some() {
            println!("REFERENCE embedded PNG alpha");
        }
        (
            pixels,
            width,
            height,
            3,
            embedded_alpha,
            output_path,
            png_output_path,
        )
    } else {
        let pixels = fs::read(input_spec.join("input_rgb8.bin"))
            .map_err(|err| format!("input_rgb8.bin: {err}"))?;
        let (width, height, channels) = shape(&input_spec.join("input_shape.txt"))?;
        let oracle_alpha = Some(read_f32(&input_spec.join("alpha_f32.bin"))?);
        let output_path = input_spec.join("alpha_native_f32.bin");
        (
            pixels,
            width,
            height,
            channels,
            oracle_alpha,
            output_path,
            None,
        )
    };
    let input = BiRefNetImage::new(&pixels, width, height, channels).map_err(|err| err.to_string())?;

    let prepare = Instant::now();
    let mut load_progress = |stage: &str, fraction: f64| {
        eprintln!("@P {fraction:.6} {stage}");
        Ok(())
    };
    let never_cancel = || false;
    let model = BiRefNet::prepare_controlled(
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
    let matte = model
        .matte_controlled(input, Some(&never_cancel), Some(&mut infer_progress))
        .map_err(|err| err.to_string())?;
    println!("INFER_MS {:.3}", infer.elapsed().as_secs_f64() * 1000.0);
    matte_stats(&matte.alpha);
    if let Some(oracle_alpha) = oracle_alpha {
        metrics(&matte.alpha, &oracle_alpha)?;
    }
    fs::write(
        &output_path,
        matte.alpha.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>(),
    )
    .map_err(|err| format!("write native alpha: {err}"))?;
    println!("OUTPUT {}", output_path.display());
    if let Some(png_output_path) = png_output_path {
        write_alpha_png(&png_output_path, &matte.alpha, width, height)?;
        println!("OUTPUT_PNG {}", png_output_path.display());
    }
    Ok(())
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(model) = args.next() else {
        eprintln!("usage: birefnet-validate <model.safetensors> [oracle-dir|input.png]");
        std::process::exit(2);
    };
    let input = args.next().map(PathBuf::from);
    if args.next().is_some() {
        eprintln!("usage: birefnet-validate <model.safetensors> [oracle-dir|input.png]");
        std::process::exit(2);
    }
    if let Err(err) = run(PathBuf::from(model), input) {
        eprintln!("ERROR: {err}");
        std::process::exit(1);
    }
}
