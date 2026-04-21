use crate::{Splat, SplatError, SplatFileFormat, SplatHigherOrderSh, SplatScene};
use makepad_micro_serde::*;
use makepad_webp::WebPDecoder;
use makepad_zip_file::zip_read_central_directory;
use std::{
    collections::HashMap,
    io::{BufReader, Cursor},
};

const SH_C0: f32 = 0.28209479177387814;

#[derive(Clone, Debug, DeJson)]
struct SogMeta {
    version: u32,
    count: usize,
    means: SogMeans,
    scales: SogCodebookBlock,
    quats: SogFileBlock,
    sh0: SogCodebookBlock,
    #[rename(shN)]
    sh_n: Option<SogHigherShBlock>,
    antialias: Option<bool>,
}

#[derive(Clone, Debug, DeJson)]
struct SogMeans {
    mins: [f32; 3],
    maxs: [f32; 3],
    files: Vec<String>,
}

#[derive(Clone, Debug, DeJson)]
struct SogCodebookBlock {
    codebook: Vec<f32>,
    files: Vec<String>,
}

#[derive(Clone, Debug, DeJson)]
struct SogFileBlock {
    files: Vec<String>,
}

#[derive(Clone, Debug, DeJson)]
struct SogHigherShBlock {
    count: usize,
    bands: usize,
    codebook: Vec<f32>,
    files: Vec<String>,
}

#[derive(Clone, Debug)]
struct DecodedImage {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl DecodedImage {
    fn pixel_count(&self) -> usize {
        self.width * self.height
    }

    fn channel(&self, pixel: usize, channel: usize) -> Result<u8, SplatError> {
        if channel > 3 {
            return Err(SplatError::InvalidData(
                "channel index out of range".to_string(),
            ));
        }
        let byte_index = pixel
            .checked_mul(4)
            .and_then(|offset| offset.checked_add(channel))
            .ok_or_else(|| {
                SplatError::InvalidData(
                    "image channel index overflow while decoding SOG".to_string(),
                )
            })?;
        self.rgba.get(byte_index).copied().ok_or_else(|| {
            SplatError::InvalidData(
                "image channel index out of bounds while decoding SOG".to_string(),
            )
        })
    }
}

pub fn load_sog_from_bytes(bytes: &[u8]) -> Result<SplatScene, SplatError> {
    let files = unzip_all_entries(bytes)?;
    let meta_bytes = files
        .get("meta.json")
        .ok_or_else(|| SplatError::MissingField("meta.json".to_string()))?;
    let meta_text = std::str::from_utf8(meta_bytes)?;
    let meta = SogMeta::deserialize_json_lenient(meta_text)?;

    if meta.version != 2 {
        return Err(SplatError::Unsupported(format!(
            "only SOG version 2 is supported (found {})",
            meta.version
        )));
    }

    if meta.means.files.len() < 2 {
        return Err(SplatError::InvalidData(
            "means.files must contain two webp files".to_string(),
        ));
    }
    if meta.scales.files.is_empty() {
        return Err(SplatError::InvalidData(
            "scales.files must contain one webp file".to_string(),
        ));
    }
    if meta.quats.files.is_empty() {
        return Err(SplatError::InvalidData(
            "quats.files must contain one webp file".to_string(),
        ));
    }
    if meta.sh0.files.is_empty() {
        return Err(SplatError::InvalidData(
            "sh0.files must contain one webp file".to_string(),
        ));
    }

    let means_l = decode_webp(
        files
            .get(&meta.means.files[0])
            .ok_or_else(|| SplatError::MissingField(meta.means.files[0].clone()))?,
    )?;
    let means_u = decode_webp(
        files
            .get(&meta.means.files[1])
            .ok_or_else(|| SplatError::MissingField(meta.means.files[1].clone()))?,
    )?;
    let scales = decode_webp(
        files
            .get(&meta.scales.files[0])
            .ok_or_else(|| SplatError::MissingField(meta.scales.files[0].clone()))?,
    )?;
    let quats = decode_webp(
        files
            .get(&meta.quats.files[0])
            .ok_or_else(|| SplatError::MissingField(meta.quats.files[0].clone()))?,
    )?;
    let sh0 = decode_webp(
        files
            .get(&meta.sh0.files[0])
            .ok_or_else(|| SplatError::MissingField(meta.sh0.files[0].clone()))?,
    )?;

    let expected_pixels = meta.count;
    let source_pixels = means_l
        .pixel_count()
        .min(means_u.pixel_count())
        .min(scales.pixel_count())
        .min(quats.pixel_count())
        .min(sh0.pixel_count());

    if source_pixels < expected_pixels {
        return Err(SplatError::InvalidData(format!(
            "SOG metadata count={} exceeds available image pixels={}",
            expected_pixels, source_pixels
        )));
    }

    let mut scene = SplatScene::empty(SplatFileFormat::Sog);
    scene.antialias = meta.antialias.unwrap_or(false);
    scene.splats.reserve(meta.count);

    for i in 0..meta.count {
        let x = decode_mean_component(i, 0, &means_l, &means_u, &meta.means)?;
        let y = decode_mean_component(i, 1, &means_l, &means_u, &meta.means)?;
        let z = decode_mean_component(i, 2, &means_l, &means_u, &meta.means)?;

        let scale = [
            decode_scale_component(i, 0, &scales, &meta.scales.codebook)?,
            decode_scale_component(i, 1, &scales, &meta.scales.codebook)?,
            decode_scale_component(i, 2, &scales, &meta.scales.codebook)?,
        ];

        let rotation = decode_quaternion(i, &quats)?;

        let color = [
            decode_sh0_component(i, 0, &sh0, &meta.sh0.codebook)?,
            decode_sh0_component(i, 1, &sh0, &meta.sh0.codebook)?,
            decode_sh0_component(i, 2, &sh0, &meta.sh0.codebook)?,
            sh0.channel(i, 3)? as f32 / 255.0,
        ];

        scene.splats.push(Splat {
            position: [x, y, z],
            scale,
            rotation,
            color,
        });
    }

    if let Some(shn_meta) = &meta.sh_n {
        scene.higher_order_sh = Some(decode_higher_order_sh(shn_meta, &files, meta.count)?);
    }

    scene.recompute_bounds();
    Ok(scene)
}

fn decode_mean_component(
    pixel_index: usize,
    channel: usize,
    low: &DecodedImage,
    high: &DecodedImage,
    means: &SogMeans,
) -> Result<f32, SplatError> {
    let low_byte = low.channel(pixel_index, channel)? as u16;
    let high_byte = high.channel(pixel_index, channel)? as u16;
    let quantized = low_byte + (high_byte << 8);
    let normalized = quantized as f32 / 65535.0;
    let encoded = lerp(means.mins[channel], means.maxs[channel], normalized);
    Ok(unlog_position(encoded))
}

fn decode_codebook_component(
    pixel_index: usize,
    channel: usize,
    image: &DecodedImage,
    codebook: &[f32],
) -> Result<f32, SplatError> {
    if codebook.is_empty() {
        return Err(SplatError::InvalidData(
            "empty codebook in SOG metadata".to_string(),
        ));
    }
    let code_index = image.channel(pixel_index, channel)? as usize;
    codebook.get(code_index).copied().ok_or_else(|| {
        SplatError::InvalidData(format!(
            "codebook index {} out of bounds {}",
            code_index,
            codebook.len()
        ))
    })
}

fn decode_sh0_component(
    pixel_index: usize,
    channel: usize,
    image: &DecodedImage,
    codebook: &[f32],
) -> Result<f32, SplatError> {
    let sh_value = decode_codebook_component(pixel_index, channel, image, codebook)?;
    Ok((0.5 + sh_value * SH_C0).clamp(0.0, 1.0))
}

fn decode_scale_component(
    pixel_index: usize,
    channel: usize,
    image: &DecodedImage,
    codebook: &[f32],
) -> Result<f32, SplatError> {
    // SOG scales are stored in log-space, matching common Gaussian splat PLY exports.
    let log_scale = decode_codebook_component(pixel_index, channel, image, codebook)?;
    Ok(log_scale.exp())
}

fn decode_quaternion(pixel_index: usize, quats: &DecodedImage) -> Result<[f32; 4], SplatError> {
    // Match SOG v2's packed quaternion convention used by Spark / PlayCanvas.
    let sqrt2 = 2.0_f32.sqrt();
    let r0 = (quats.channel(pixel_index, 0)? as f32 / 255.0 - 0.5) * sqrt2;
    let r1 = (quats.channel(pixel_index, 1)? as f32 / 255.0 - 0.5) * sqrt2;
    let r2 = (quats.channel(pixel_index, 2)? as f32 / 255.0 - 0.5) * sqrt2;
    let rr = (1.0 - r0 * r0 - r1 * r1 - r2 * r2).max(0.0).sqrt();

    let order = quats.channel(pixel_index, 3)? as i32 - 252;
    if !(0..=3).contains(&order) {
        return Err(SplatError::InvalidData(format!(
            "invalid packed quaternion channel marker {}",
            order + 252
        )));
    }

    let mut q = [
        if order == 0 {
            r0
        } else if order == 1 {
            rr
        } else {
            r1
        },
        if order <= 1 {
            r1
        } else if order == 2 {
            rr
        } else {
            r2
        },
        if order <= 2 { r2 } else { rr },
        if order == 0 { rr } else { r0 },
    ];

    let len2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if len2 > f32::EPSILON {
        let inv_len = len2.sqrt().recip();
        q[0] *= inv_len;
        q[1] *= inv_len;
        q[2] *= inv_len;
        q[3] *= inv_len;
    } else {
        q = [0.0, 0.0, 0.0, 1.0];
    }

    Ok(q)
}

fn decode_higher_order_sh(
    shn_meta: &SogHigherShBlock,
    files: &HashMap<String, Vec<u8>>,
    splat_count: usize,
) -> Result<SplatHigherOrderSh, SplatError> {
    if shn_meta.files.len() < 2 {
        return Err(SplatError::InvalidData(
            "shN.files must contain centroid and label textures".to_string(),
        ));
    }

    let centroids = decode_webp(
        files
            .get(&shn_meta.files[0])
            .ok_or_else(|| SplatError::MissingField(shn_meta.files[0].clone()))?,
    )?;
    let labels = decode_webp(
        files
            .get(&shn_meta.files[1])
            .ok_or_else(|| SplatError::MissingField(shn_meta.files[1].clone()))?,
    )?;

    let coeffs_per_channel = match shn_meta.bands {
        1 => 3,
        2 => 8,
        3 => 15,
        other => {
            return Err(SplatError::Unsupported(format!(
                "unsupported shN bands count {other}"
            )))
        }
    };

    let label_pixel_count = labels.pixel_count();
    if label_pixel_count < splat_count {
        return Err(SplatError::InvalidData(format!(
            "shN label pixels {} < splat count {}",
            label_pixel_count, splat_count
        )));
    }

    if shn_meta.codebook.is_empty() {
        return Err(SplatError::InvalidData(
            "shN.codebook must not be empty".to_string(),
        ));
    }

    let mut coeffs = vec![0.0_f32; splat_count * coeffs_per_channel * 3];

    for splat_index in 0..splat_count {
        let label = labels.channel(splat_index, 0)? as usize
            + ((labels.channel(splat_index, 1)? as usize) << 8);
        if label >= shn_meta.count {
            return Err(SplatError::InvalidData(format!(
                "shN label {} out of range {}",
                label, shn_meta.count
            )));
        }

        let centroids_per_row = 64;
        for coeff_index in 0..coeffs_per_channel {
            let u = (label % centroids_per_row) * coeffs_per_channel + coeff_index;
            let v = label / centroids_per_row;
            if u >= centroids.width || v >= centroids.height {
                return Err(SplatError::InvalidData(
                    "shN centroid lookup out of texture bounds".to_string(),
                ));
            }
            let centroid_pixel = v * centroids.width + u;
            let r_i = centroids.channel(centroid_pixel, 0)? as usize;
            let g_i = centroids.channel(centroid_pixel, 1)? as usize;
            let b_i = centroids.channel(centroid_pixel, 2)? as usize;

            let base = (splat_index * coeffs_per_channel + coeff_index) * 3;
            coeffs[base] = *shn_meta.codebook.get(r_i).ok_or_else(|| {
                SplatError::InvalidData("shN centroid r codebook index out of range".to_string())
            })?;
            coeffs[base + 1] = *shn_meta.codebook.get(g_i).ok_or_else(|| {
                SplatError::InvalidData("shN centroid g codebook index out of range".to_string())
            })?;
            coeffs[base + 2] = *shn_meta.codebook.get(b_i).ok_or_else(|| {
                SplatError::InvalidData("shN centroid b codebook index out of range".to_string())
            })?;
        }
    }

    Ok(SplatHigherOrderSh {
        bands: shn_meta.bands,
        coeffs_per_channel,
        coeffs,
    })
}

fn decode_webp(data: &[u8]) -> Result<DecodedImage, SplatError> {
    let cursor = Cursor::new(data);
    let mut decoder = WebPDecoder::new(BufReader::new(cursor))?;
    let (width_u32, height_u32) = decoder.dimensions();
    let width = width_u32 as usize;
    let height = height_u32 as usize;
    let out_size = decoder.output_buffer_size().ok_or_else(|| {
        SplatError::InvalidData("webp output size overflow while decoding SOG".to_string())
    })?;

    let mut decoded = vec![0u8; out_size];
    decoder.read_image(&mut decoded)?;

    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| SplatError::InvalidData("webp dimension overflow".to_string()))?;
    if pixel_count == 0 {
        return Err(SplatError::InvalidData(
            "zero-sized webp image in SOG payload".to_string(),
        ));
    }
    let channels = decoded.len() / pixel_count;
    if channels != 3 && channels != 4 {
        return Err(SplatError::InvalidData(format!(
            "unsupported webp channel count {} (expected 3 or 4)",
            channels
        )));
    }

    let rgba = if channels == 4 {
        decoded
    } else {
        let mut rgba = vec![0u8; pixel_count * 4];
        for i in 0..pixel_count {
            let src = i * 3;
            let dst = i * 4;
            rgba[dst] = decoded[src];
            rgba[dst + 1] = decoded[src + 1];
            rgba[dst + 2] = decoded[src + 2];
            rgba[dst + 3] = 255;
        }
        rgba
    };

    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

fn unzip_all_entries(bytes: &[u8]) -> Result<HashMap<String, Vec<u8>>, SplatError> {
    let mut cursor = Cursor::new(bytes);
    let central_directory = zip_read_central_directory(&mut cursor)?;
    let mut out = HashMap::new();

    for header in central_directory.file_headers {
        let data = header.extract(&mut cursor)?;
        out.insert(header.file_name, data);
    }

    Ok(out)
}

fn lerp(min_v: f32, max_v: f32, t: f32) -> f32 {
    min_v + (max_v - min_v) * t
}

fn unlog_position(encoded: f32) -> f32 {
    let abs_value = encoded.abs();
    let unlogged = abs_value.exp() - 1.0;
    if encoded < 0.0 {
        -unlogged
    } else {
        unlogged
    }
}
