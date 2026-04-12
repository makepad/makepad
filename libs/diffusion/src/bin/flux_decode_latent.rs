use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{
    unpack_flux_latents_nchw, ComfyModelRoots, FluxLatentShape, FluxPromptToImagePlan,
};
use makepad_diffusion::flux_schedule::{FLUX_VAE_SCALING_FACTOR, FLUX_VAE_SHIFT_FACTOR};
use makepad_diffusion::flux_vae::{CompiledFluxVaeMetal, LoadedFluxVaeWeights};
use makepad_ggml::backend::metal::MetalRuntime;
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;
use std::env;
use std::fs;
use std::path::Path;

fn usage() -> ! {
    eprintln!(
        "usage: flux-decode-latent <workflow.json> <comfy-root-or-model-root> <latent.bin> <output.png> [width height]"
    );
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let root = env::args().nth(2).unwrap_or_else(|| usage());
    let latent_path = env::args().nth(3).unwrap_or_else(|| usage());
    let output_path = env::args().nth(4).unwrap_or_else(|| usage());
    let width = env::args().nth(5).map(|value| value.parse::<u32>()).transpose()?;
    let height = env::args().nth(6).map(|value| value.parse::<u32>()).transpose()?;

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let width = width.unwrap_or(256);
    let height = height.unwrap_or(width);
    let latent_shape = FluxLatentShape::from_image_size(width, height)?;

    let latent_bytes = fs::read(Path::new(&latent_path))?;
    let latent_values = f32_bytes_to_vec(&latent_bytes)?;
    let latent_format = env::var("FLUX_LATENT_FORMAT").unwrap_or_else(|_| {
        if latent_path.contains(".packed.") {
            "packed".to_string()
        } else {
            "nchw".to_string()
        }
    });

    let mut latents_nchw = match latent_format.as_str() {
        "packed" => unpack_flux_latents_nchw(
            &latent_values,
            1,
            latent_shape.latent_height,
            latent_shape.latent_width,
        )?,
        "nchw" => latent_values,
        other => {
            return Err(format!("unsupported FLUX_LATENT_FORMAT '{}'", other).into());
        }
    };

    let expected = 16usize
        .checked_mul(latent_shape.latent_width as usize)
        .and_then(|value| value.checked_mul(latent_shape.latent_height as usize))
        .ok_or("latent size overflow")?;
    if latents_nchw.len() != expected {
        return Err(format!(
            "latent decode expected {} values for 1x16x{}x{}, got {}",
            expected,
            latent_shape.latent_height,
            latent_shape.latent_width,
            latents_nchw.len()
        )
        .into());
    }

    for value in &mut latents_nchw {
        *value = (*value / FLUX_VAE_SCALING_FACTOR) + FLUX_VAE_SHIFT_FACTOR;
    }
    let latents_whcb = nchw_to_whcb(
        &latents_nchw,
        1,
        16,
        latent_shape.latent_height as usize,
        latent_shape.latent_width as usize,
    )?;

    let vae_path = plan
        .bundle
        .vae_path
        .as_ref()
        .ok_or("workflow bundle does not include vae")?;
    let runtime = MetalRuntime::new().map_err(|err| format!("vae metal runtime init failed: {err}"))?;
    let mut vae = LoadedFluxVaeWeights::load(vae_path)?;
    let compiled = CompiledFluxVaeMetal::compile_with_runtime(runtime, &mut vae, latent_shape)?;
    let image = compiled.execute(&vae, &latents_whcb)?;
    fs::write(
        Path::new(&output_path),
        encode_png_rgb(&image.image, image.width, image.height)?,
    )?;

    println!("workflow: {}", workflow.path.display());
    println!("latent: {}", latent_path);
    println!("format: {}", latent_format);
    println!("output: {}", output_path);
    println!("size: {}x{}", width, height);

    Ok(())
}

fn f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(format!(
            "expected f32 byte length, got {} bytes",
            bytes.len()
        )
        .into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn nchw_to_whcb(
    input: &[f32],
    batch: usize,
    channels: usize,
    height: usize,
    width: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let expected = batch * channels * height * width;
    if input.len() != expected {
        return Err(format!(
            "nchw_to_whcb expected {} values, got {}",
            expected,
            input.len()
        )
        .into());
    }
    Ok(input.to_vec())
}

fn encode_png_rgb(image_whcb: &[f32], width: usize, height: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let expected = width * height * 3;
    if image_whcb.len() != expected {
        return Err(format!(
            "png encode expected {} float values, got {}",
            expected,
            image_whcb.len()
        )
        .into());
    }
    let plane = width * height;
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let pixel = y * width + x;
            let r = to_u8(image_whcb[pixel]);
            let g = to_u8(image_whcb[plane + pixel]);
            let b = to_u8(image_whcb[(plane * 2) + pixel]);
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut encoder = PngEncoder::new(&pixels, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| format!("png encode failed: {err:?}"))?;
    Ok(out)
}

fn to_u8(value: f32) -> u8 {
    let scaled = ((value + 1.0) * 127.5).round();
    scaled.clamp(0.0, 255.0) as u8
}
