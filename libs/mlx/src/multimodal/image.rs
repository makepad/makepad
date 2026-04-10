use crate::{MlxImageProcessorConfig, MlxModelSnapshot, MlxRtError, Result};
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use makepad_zune_jpeg::JpegDecoder;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct GemmaImagePixels {
    pub width: usize,
    pub height: usize,
    pub pixels_chw: Vec<f32>,
    pub patch_grid_width: usize,
    pub patch_grid_height: usize,
    pub soft_token_count: usize,
}

pub fn load_gemma_image(snapshot: &MlxModelSnapshot, image_path: &Path) -> Result<GemmaImagePixels> {
    let decoded = decode_image_rgb8(image_path)?;
    let processed = preprocess_image(&snapshot.processor_config.image_processor, decoded)?;
    if processed.soft_token_count == 0 {
        return Err(MlxRtError::InvalidModelDir {
            path: image_path.to_path_buf(),
            message: "image preprocess produced zero soft tokens".to_string(),
        });
    }
    Ok(processed)
}

#[derive(Clone, Debug)]
struct DecodedRgbImage {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

fn decode_image_rgb8(path: &Path) -> Result<DecodedRgbImage> {
    let magic = fs::read(path).map_err(|err| MlxRtError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    if magic.len() >= 8 && magic[..8] == [137, 80, 78, 71, 13, 10, 26, 10] {
        return decode_png_rgb8(path);
    }
    if magic.len() >= 3 && magic[0] == 0xFF && magic[1] == 0xD8 && magic[2] == 0xFF {
        return decode_jpeg_rgb8(path);
    }
    Err(MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: "unsupported image format; expected PNG or JPEG".to_string(),
    })
}

fn decode_jpeg_rgb8(path: &Path) -> Result<DecodedRgbImage> {
    let file = fs::File::open(path).map_err(|err| MlxRtError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let reader = BufReader::new(file);
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(reader, options);
    decoder.decode_headers().map_err(|err| MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let info = decoder.info().ok_or_else(|| MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: "jpeg decoder did not expose image info".to_string(),
    })?;
    let pixels = decoder.decode().map_err(|err| MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    Ok(DecodedRgbImage {
        width: info.width as usize,
        height: info.height as usize,
        pixels,
    })
}

fn decode_png_rgb8(path: &Path) -> Result<DecodedRgbImage> {
    let file = fs::File::open(path).map_err(|err| MlxRtError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let reader = BufReader::new(file);
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(reader, options);
    decoder.decode_headers().map_err(|err| MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let info = decoder.info().cloned().ok_or_else(|| MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: "png decoder did not expose image info".to_string(),
    })?;
    let colorspace = decoder.colorspace().ok_or_else(|| MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: "png decoder did not expose colorspace".to_string(),
    })?;
    let pixels = decoder.decode_raw().map_err(|err| MlxRtError::InvalidModelDir {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let rgb_pixels = convert_to_rgb8(path, colorspace, &pixels)?;
    Ok(DecodedRgbImage {
        width: info.width as usize,
        height: info.height as usize,
        pixels: rgb_pixels,
    })
}

fn convert_to_rgb8(path: &Path, colorspace: ColorSpace, pixels: &[u8]) -> Result<Vec<u8>> {
    let components = colorspace.num_components();
    if components == 0 || pixels.len() % components != 0 {
        return Err(MlxRtError::InvalidModelDir {
            path: path.to_path_buf(),
            message: format!(
                "unsupported or malformed PNG colorspace {:?} with {} bytes",
                colorspace,
                pixels.len()
            ),
        });
    }
    let mut out = Vec::with_capacity((pixels.len() / components) * 3);
    for chunk in pixels.chunks_exact(components) {
        match colorspace {
            ColorSpace::RGB => out.extend_from_slice(&chunk[..3]),
            ColorSpace::RGBA => out.extend_from_slice(&chunk[..3]),
            ColorSpace::Luma => out.extend_from_slice(&[chunk[0], chunk[0], chunk[0]]),
            ColorSpace::LumaA => out.extend_from_slice(&[chunk[0], chunk[0], chunk[0]]),
            ColorSpace::BGR => out.extend_from_slice(&[chunk[2], chunk[1], chunk[0]]),
            ColorSpace::BGRA => out.extend_from_slice(&[chunk[2], chunk[1], chunk[0]]),
            ColorSpace::ARGB => out.extend_from_slice(&[chunk[1], chunk[2], chunk[3]]),
            other => {
                return Err(MlxRtError::InvalidModelDir {
                    path: path.to_path_buf(),
                    message: format!("unsupported PNG colorspace {:?}", other),
                });
            }
        }
    }
    Ok(out)
}

fn preprocess_image(
    config: &MlxImageProcessorConfig,
    image: DecodedRgbImage,
) -> Result<GemmaImagePixels> {
    let side_multiple = usize::try_from(config.patch_size.saturating_mul(config.pooling_kernel_size))
        .map_err(|_| MlxRtError::InvalidModelDir {
            path: PathBuf::new(),
            message: "image processor side_multiple overflow".to_string(),
        })?;
    let max_patches = usize::try_from(
        config
            .max_soft_tokens
            .saturating_mul(config.pooling_kernel_size)
            .saturating_mul(config.pooling_kernel_size),
    )
    .map_err(|_| MlxRtError::InvalidModelDir {
        path: PathBuf::new(),
        message: "image processor max_patches overflow".to_string(),
    })?;

    let (target_width, target_height) = if config.do_resize {
        aspect_ratio_preserving_resize_dims(
            image.width,
            image.height,
            usize::try_from(config.patch_size).unwrap_or(16),
            max_patches,
            usize::try_from(config.pooling_kernel_size).unwrap_or(3),
            side_multiple,
        )?
    } else {
        (image.width, image.height)
    };

    let resized_pixels = if target_width == image.width && target_height == image.height {
        image.pixels
    } else {
        resize_rgb8_bicubic(&image.pixels, image.width, image.height, target_width, target_height)
    };

    let pixel_count = target_width
        .checked_mul(target_height)
        .ok_or_else(|| MlxRtError::InvalidModelDir {
            path: PathBuf::new(),
            message: "image pixel count overflow".to_string(),
        })?;
    let mut pixels_chw = vec![0.0f32; pixel_count * 3];
    for y in 0..target_height {
        for x in 0..target_width {
            let src = (y * target_width + x) * 3;
            let dst = y * target_width + x;
            for channel in 0..3 {
                let mut value = resized_pixels[src + channel] as f32;
                if config.do_rescale {
                    value *= config.rescale_factor as f32;
                }
                if config.do_normalize {
                    value = (value - config.image_mean[channel]) / config.image_std[channel];
                }
                pixels_chw[channel * pixel_count + dst] = value;
            }
        }
    }

    let patch_size = usize::try_from(config.patch_size).unwrap_or(16);
    let pooling_kernel_size = usize::try_from(config.pooling_kernel_size).unwrap_or(3);
    let patch_grid_width = target_width / patch_size;
    let patch_grid_height = target_height / patch_size;
    let num_patches = patch_grid_width
        .checked_mul(patch_grid_height)
        .ok_or_else(|| MlxRtError::InvalidModelDir {
            path: PathBuf::new(),
            message: "patch grid overflow".to_string(),
        })?;
    let soft_token_count = num_patches / (pooling_kernel_size * pooling_kernel_size);
    Ok(GemmaImagePixels {
        width: target_width,
        height: target_height,
        pixels_chw,
        patch_grid_width,
        patch_grid_height,
        soft_token_count,
    })
}

fn aspect_ratio_preserving_resize_dims(
    width: usize,
    height: usize,
    patch_size: usize,
    max_patches: usize,
    pooling_kernel_size: usize,
    side_multiple: usize,
) -> Result<(usize, usize)> {
    let target_pixels = (max_patches * patch_size * patch_size) as f64;
    let factor = (target_pixels / (width as f64 * height as f64)).sqrt();
    let mut target_height = ((factor * height as f64) / side_multiple as f64).floor() as usize
        * side_multiple;
    let mut target_width = ((factor * width as f64) / side_multiple as f64).floor() as usize
        * side_multiple;
    if target_height == 0 && target_width == 0 {
        return Err(MlxRtError::InvalidModelDir {
            path: PathBuf::new(),
            message: "attempting to resize to a 0x0 image".to_string(),
        });
    }
    let max_side_length = (max_patches / (pooling_kernel_size * pooling_kernel_size)) * side_multiple;
    if target_height == 0 {
        target_height = side_multiple;
        target_width = (((width as f64 / height as f64).floor() as usize).max(1) * side_multiple)
            .min(max_side_length);
    } else if target_width == 0 {
        target_width = side_multiple;
        target_height = (((height as f64 / width as f64).floor() as usize).max(1) * side_multiple)
            .min(max_side_length);
    }
    Ok((target_width, target_height))
}

fn resize_rgb8_bicubic(
    src: &[u8],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; dst_width * dst_height * 3];
    let scale_x = src_width as f32 / dst_width as f32;
    let scale_y = src_height as f32 / dst_height as f32;
    for dst_y in 0..dst_height {
        let src_y = (dst_y as f32 + 0.5) * scale_y - 0.5;
        let src_y_floor = src_y.floor() as isize;
        for dst_x in 0..dst_width {
            let src_x = (dst_x as f32 + 0.5) * scale_x - 0.5;
            let src_x_floor = src_x.floor() as isize;
            let mut accum = [0.0f32; 3];
            for ky in -1..=2 {
                let sy = clamp_index(src_y_floor + ky, src_height);
                let wy = cubic_weight(src_y - (src_y_floor + ky) as f32);
                for kx in -1..=2 {
                    let sx = clamp_index(src_x_floor + kx, src_width);
                    let wx = cubic_weight(src_x - (src_x_floor + kx) as f32);
                    let weight = wx * wy;
                    let src_index = (sy * src_width + sx) * 3;
                    for channel in 0..3 {
                        accum[channel] += src[src_index + channel] as f32 * weight;
                    }
                }
            }
            let dst_index = (dst_y * dst_width + dst_x) * 3;
            for channel in 0..3 {
                out[dst_index + channel] = accum[channel].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn cubic_weight(distance: f32) -> f32 {
    let a = -0.5f32;
    let x = distance.abs();
    if x <= 1.0 {
        ((a + 2.0) * x * x * x) - ((a + 3.0) * x * x) + 1.0
    } else if x < 2.0 {
        (a * x * x * x) - (5.0 * a * x * x) + (8.0 * a * x) - (4.0 * a)
    } else {
        0.0
    }
}

fn clamp_index(value: isize, limit: usize) -> usize {
    value.clamp(0, limit.saturating_sub(1) as isize) as usize
}
