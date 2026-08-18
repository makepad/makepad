//! FLUX.2 autoencoder: encode AND decode.
//!
//! Same conv skeleton as FLUX.1 (ch 128, mult [1,2,4,4], 2 resblocks, mid
//! attention, GroupNorm+swish) with z_channels 32, deterministic mean-only
//! encode, 2x2 pack to 128 channels, and BatchNorm2d running-stat
//! (de)normalization (`eps=1e-4`, affine=false).
//!
//! Weight names: BFL (`encoder.down` / `decoder.up`) and diffusers
//! (`down_blocks` / `up_blocks`) are both accepted; diffusers `up_blocks.i`
//! maps to BFL `decoder.up.{3-i}` because construction prepends.

use crate::backend::{
    gpu_add, gpu_attention_planar_single, gpu_conv2d_planar_cached, gpu_conv2d_planar_strided,
    gpu_device_available, gpu_download, gpu_group_norm_planar, gpu_pool_cap_override,
    gpu_pool_clear, gpu_silu, gpu_upload, gpu_upsample_nearest2x, GpuTensor,
};
use crate::flux2::{
    flux2_pack_latents, flux2_unpack_latents, Flux2PackedLatents, Flux2VaeConfig, Flux2WeightFile,
};
use crate::{DiffusionError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const VAE_GROUP_NORM_EPSILON: f32 = 1.0e-6;
pub const FLUX2_VAE_NAMESPACE: &str = "flux2-vae";

#[derive(Clone, Debug)]
pub struct Flux2VaeWeights {
    pub path: PathBuf,
    pub config: Flux2VaeConfig,
    tensors: HashMap<String, Vec<f32>>,
    shapes: HashMap<String, Vec<usize>>,
    bn_mean: Vec<f32>,
    bn_var: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Flux2VaeImage {
    pub width: usize,
    pub height: usize,
    /// Planar RGB in `[-1, 1]`, channel-major `[3, H*W]`.
    pub data: Vec<f32>,
}

struct GpuSpatial {
    tensor: GpuTensor,
    width: usize,
    height: usize,
    channels: usize,
}

struct ConvKernel {
    data: Vec<f32>,
    kw: usize,
    kh: usize,
    in_channels: usize,
    out_channels: usize,
}

impl Flux2VaeWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = Flux2WeightFile::load(path)?;
        let mut tensors = HashMap::new();
        let mut shapes = HashMap::new();
        for header in &file.files {
            for (name, info) in &header.tensors {
                if name == "__metadata__" || name.ends_with("num_batches_tracked") {
                    continue;
                }
                let canonical = canonicalize_vae_name(name);
                let values = header.read_f32(name)?;
                let shape: Vec<usize> = info.shape.iter().map(|d| *d as usize).collect();
                let values = reshape_linear_as_1x1(values, &shape);
                tensors.insert(canonical.clone(), values);
                shapes.insert(canonical, normalize_shape(&shape));
            }
        }
        let bn_mean = tensors
            .get("bn.running_mean")
            .cloned()
            .ok_or_else(|| DiffusionError::model("flux2 vae missing bn.running_mean"))?;
        let bn_var = tensors
            .get("bn.running_var")
            .cloned()
            .ok_or_else(|| DiffusionError::model("flux2 vae missing bn.running_var"))?;
        if bn_mean.len() != 128 || bn_var.len() != 128 {
            return Err(DiffusionError::model(format!(
                "flux2 vae bn stats expected 128, got mean={} var={}",
                bn_mean.len(),
                bn_var.len()
            )));
        }
        Ok(Self {
            path: path.to_path_buf(),
            config: Flux2VaeConfig::flux2_dev(),
            tensors,
            shapes,
            bn_mean,
            bn_var,
        })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.tensors.contains_key(name)
    }

    fn values(&self, name: &str) -> Result<&[f32]> {
        self.tensors
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| DiffusionError::model(format!("flux2 vae missing {name}")))
    }

    fn kernel(&self, name: &str) -> Result<ConvKernel> {
        let data = self.values(name)?.to_vec();
        let shape = self
            .shapes
            .get(name)
            .ok_or_else(|| DiffusionError::model(format!("flux2 vae missing shape {name}")))?;
        match shape.as_slice() {
            [out_c, in_c, kh, kw] => Ok(ConvKernel {
                data,
                kw: *kw,
                kh: *kh,
                in_channels: *in_c,
                out_channels: *out_c,
            }),
            [out_c, in_c] => Ok(ConvKernel {
                data,
                kw: 1,
                kh: 1,
                in_channels: *in_c,
                out_channels: *out_c,
            }),
            other => Err(DiffusionError::model(format!(
                "flux2 vae {name} is not a conv kernel: {other:?}"
            ))),
        }
    }

    pub fn bn_mean(&self) -> &[f32] {
        &self.bn_mean
    }

    pub fn bn_var(&self) -> &[f32] {
        &self.bn_var
    }
}

fn normalize_shape(shape: &[usize]) -> Vec<usize> {
    if shape.len() == 2 {
        vec![shape[0], shape[1], 1, 1]
    } else {
        shape.to_vec()
    }
}

fn reshape_linear_as_1x1(values: Vec<f32>, _shape: &[usize]) -> Vec<f32> {
    values
}

/// Map diffusers AutoencoderKLFlux2 names onto BFL canonical names.
pub fn canonicalize_vae_name(name: &str) -> String {
    let name = name
        .strip_prefix("vae.")
        .or_else(|| name.strip_prefix("model."))
        .unwrap_or(name);
    if name == "quant_conv.weight" || name == "quant_conv.bias" {
        return name.replace("quant_conv", "encoder.quant_conv");
    }
    if name == "post_quant_conv.weight" || name == "post_quant_conv.bias" {
        return name.replace("post_quant_conv", "decoder.post_quant_conv");
    }
    if name.starts_with("bn.") || name.starts_with("latent_norm.") || name.starts_with("batch_norm.")
    {
        return name
            .replace("latent_norm.", "bn.")
            .replace("batch_norm.", "bn.");
    }
    let name = name
        .replace("encoder.conv_norm_out", "encoder.norm_out")
        .replace("decoder.conv_norm_out", "decoder.norm_out")
        .replace(".conv_shortcut.", ".nin_shortcut.");

    if let Some(rest) = name.strip_prefix("encoder.down_blocks.") {
        return map_down_block("encoder.down", rest);
    }
    if let Some(rest) = name.strip_prefix("decoder.up_blocks.") {
        return map_up_block(rest);
    }
    if let Some(rest) = name.strip_prefix("encoder.mid_block.") {
        return map_mid_block("encoder.mid", rest);
    }
    if let Some(rest) = name.strip_prefix("decoder.mid_block.") {
        return map_mid_block("decoder.mid", rest);
    }
    name
}

fn map_down_block(prefix: &str, rest: &str) -> String {
    // down_blocks.{i}.resnets.{j}.*  -> {prefix}.{i}.block.{j}.*
    // down_blocks.{i}.downsamplers.0.conv.* -> {prefix}.{i}.downsample.conv.*
    let mut parts = rest.splitn(2, '.');
    let index = parts.next().unwrap_or("0");
    let tail = parts.next().unwrap_or("");
    if let Some(tail) = tail.strip_prefix("resnets.") {
        format!("{prefix}.{index}.block.{tail}")
    } else if let Some(tail) = tail.strip_prefix("downsamplers.0.") {
        format!("{prefix}.{index}.downsample.{tail}")
    } else {
        format!("{prefix}.{index}.{tail}")
    }
}

fn map_up_block(rest: &str) -> String {
    let mut parts = rest.splitn(2, '.');
    let index: usize = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let tail = parts.next().unwrap_or("");
    let bfl = 3usize.saturating_sub(index);
    if let Some(tail) = tail.strip_prefix("resnets.") {
        format!("decoder.up.{bfl}.block.{tail}")
    } else if let Some(tail) = tail.strip_prefix("upsamplers.0.") {
        format!("decoder.up.{bfl}.upsample.{tail}")
    } else {
        format!("decoder.up.{bfl}.{tail}")
    }
}

fn map_mid_block(prefix: &str, rest: &str) -> String {
    if let Some(tail) = rest.strip_prefix("resnets.0.") {
        return format!("{prefix}.block_1.{tail}");
    }
    if let Some(tail) = rest.strip_prefix("resnets.1.") {
        return format!("{prefix}.block_2.{tail}");
    }
    if let Some(tail) = rest.strip_prefix("attentions.0.") {
        let tail = tail
            .replace("group_norm.", "norm.")
            .replace("to_q.", "q.")
            .replace("to_k.", "k.")
            .replace("to_v.", "v.")
            .replace("to_out.0.", "proj_out.");
        return format!("{prefix}.attn_1.{tail}");
    }
    format!("{prefix}.{rest}")
}

fn namespace(weights: &Flux2VaeWeights) -> String {
    format!("{FLUX2_VAE_NAMESPACE}:{}", weights.path.display())
}

pub fn flux2_vae_bn_normalize(packed: &mut Flux2PackedLatents, weights: &Flux2VaeWeights) {
    let plane = packed.token_count();
    let eps = weights.config.batch_norm_eps;
    for channel in 0..packed.channels {
        let mean = weights.bn_mean[channel];
        let std = (weights.bn_var[channel] + eps).sqrt();
        let start = channel * plane;
        for value in &mut packed.data[start..start + plane] {
            *value = (*value - mean) / std;
        }
    }
}

pub fn flux2_vae_bn_denormalize(packed: &mut Flux2PackedLatents, weights: &Flux2VaeWeights) {
    let plane = packed.token_count();
    let eps = weights.config.batch_norm_eps;
    for channel in 0..packed.channels {
        let mean = weights.bn_mean[channel];
        let std = (weights.bn_var[channel] + eps).sqrt();
        let start = channel * plane;
        for value in &mut packed.data[start..start + plane] {
            *value = *value * std + mean;
        }
    }
}

pub fn flux2_vae_encode(
    weights: &Flux2VaeWeights,
    image: &Flux2VaeImage,
) -> Result<Flux2PackedLatents> {
    if !gpu_device_available() {
        return Err(DiffusionError::workflow(
            "flux2 vae encode requires CUDA (MAKEPAD_GGML_REQUIRE_CUDA=1)",
        ));
    }
    if image.data.len() != 3 * image.width * image.height {
        return Err(DiffusionError::workflow(format!(
            "flux2 vae encode expected {} RGB values, got {}",
            3 * image.width * image.height,
            image.data.len()
        )));
    }
    gpu_pool_clear();
    gpu_pool_cap_override(Some(2048 * 1024 * 1024));
    let ns = namespace(weights);
    let mut hidden = GpuSpatial {
        tensor: gpu_upload(&image.data, 3, image.width * image.height)
            .map_err(DiffusionError::model)?,
        width: image.width,
        height: image.height,
        channels: 3,
    };
    hidden = conv2d(weights, &ns, &hidden, "encoder.conv_in.weight", "encoder.conv_in.bias", 1, 1)?;
    for level in 0..4 {
        let prefix = format!("encoder.down.{level}");
        for block in 0..2 {
            hidden = resnet(weights, &ns, &hidden, &format!("{prefix}.block.{block}"))?;
        }
        if weights.has_tensor(&format!("{prefix}.downsample.conv.weight")) {
            hidden = downsample(weights, &ns, &hidden, &format!("{prefix}.downsample.conv"))?;
        }
    }
    hidden = resnet(weights, &ns, &hidden, "encoder.mid.block_1")?;
    hidden = mid_attn(weights, &ns, &hidden, "encoder.mid.attn_1")?;
    hidden = resnet(weights, &ns, &hidden, "encoder.mid.block_2")?;
    hidden = group_norm(weights, &ns, &hidden, "encoder.norm_out.weight", "encoder.norm_out.bias")?;
    hidden = silu(&hidden)?;
    hidden = conv2d(weights, &ns, &hidden, "encoder.conv_out.weight", "encoder.conv_out.bias", 1, 1)?;
    if weights.has_tensor("encoder.quant_conv.weight") {
        hidden = conv2d(
            weights,
            &ns,
            &hidden,
            "encoder.quant_conv.weight",
            "encoder.quant_conv.bias",
            0,
            0,
        )?;
    }
    let moments = gpu_download(&hidden.tensor).map_err(DiffusionError::model)?;
    let z_channels = weights.config.z_channels as usize;
    let plane = hidden.width * hidden.height;
    if moments.len() != 2 * z_channels * plane {
        return Err(DiffusionError::model(format!(
            "flux2 vae encode moments {} != 2*{z_channels}*{plane}",
            moments.len()
        )));
    }
    let mean = &moments[..z_channels * plane];
    let mut packed = flux2_pack_latents(mean, hidden.width, hidden.height, z_channels)?;
    flux2_vae_bn_normalize(&mut packed, weights);
    drop(hidden);
    gpu_pool_clear();
    gpu_pool_cap_override(None);
    Ok(packed)
}

/// FLUX2_VAE_DUMP=dir: dump decoder stage outputs as rows=channels x
/// cols=plane .f32 files (the DiT dump format) for oracle bisection.
fn vae_dump(name: &str, spatial: &GpuSpatial) {
    let Ok(dir) = std::env::var("FLUX2_VAE_DUMP") else {
        return;
    };
    if dir.is_empty() {
        return;
    }
    let Ok(values) = gpu_download(&spatial.tensor) else {
        return;
    };
    let path = std::path::Path::new(&dir).join(format!("{name}.f32"));
    let mut bytes = Vec::with_capacity(8 + values.len() * 4);
    bytes.extend_from_slice(&(spatial.channels as u32).to_le_bytes());
    bytes.extend_from_slice(&((spatial.width * spatial.height) as u32).to_le_bytes());
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(path, bytes);
}

pub fn flux2_vae_decode(
    weights: &Flux2VaeWeights,
    packed: &Flux2PackedLatents,
) -> Result<Flux2VaeImage> {
    if !gpu_device_available() {
        return Err(DiffusionError::workflow(
            "flux2 vae decode requires CUDA (MAKEPAD_GGML_REQUIRE_CUDA=1)",
        ));
    }
    let z_channels = weights.config.z_channels as usize;
    let mut denorm = packed.clone();
    flux2_vae_bn_denormalize(&mut denorm, weights);
    let unpacked = flux2_unpack_latents(&denorm, z_channels)?;
    let width = packed.width * 2;
    let height = packed.height * 2;
    gpu_pool_clear();
    gpu_pool_cap_override(Some(2048 * 1024 * 1024));
    let ns = namespace(weights);
    let mut hidden = GpuSpatial {
        tensor: gpu_upload(&unpacked, z_channels, width * height).map_err(DiffusionError::model)?,
        width,
        height,
        channels: z_channels,
    };
    vae_dump("vae_in", &hidden);
    if weights.has_tensor("decoder.post_quant_conv.weight") {
        hidden = conv2d(
            weights,
            &ns,
            &hidden,
            "decoder.post_quant_conv.weight",
            "decoder.post_quant_conv.bias",
            0,
            0,
        )?;
    }
    hidden = conv2d(weights, &ns, &hidden, "decoder.conv_in.weight", "decoder.conv_in.bias", 1, 1)?;
    vae_dump("vae_conv_in", &hidden);
    hidden = resnet(weights, &ns, &hidden, "decoder.mid.block_1")?;
    vae_dump("vae_mid_b1", &hidden);
    hidden = mid_attn(weights, &ns, &hidden, "decoder.mid.attn_1")?;
    vae_dump("vae_mid_attn", &hidden);
    hidden = resnet(weights, &ns, &hidden, "decoder.mid.block_2")?;
    vae_dump("vae_mid_b2", &hidden);
    for stage in (0..=3).rev() {
        let prefix = format!("decoder.up.{stage}");
        for block in 0..=2 {
            hidden = resnet(weights, &ns, &hidden, &format!("{prefix}.block.{block}"))?;
        }
        if weights.has_tensor(&format!("{prefix}.upsample.conv.weight")) {
            hidden = upsample(&hidden)?;
            hidden = conv2d(
                weights,
                &ns,
                &hidden,
                &format!("{prefix}.upsample.conv.weight"),
                &format!("{prefix}.upsample.conv.bias"),
                1,
                1,
            )?;
        }
        vae_dump(&format!("vae_up{stage}"), &hidden);
    }
    hidden = group_norm(weights, &ns, &hidden, "decoder.norm_out.weight", "decoder.norm_out.bias")?;
    vae_dump("vae_norm_out", &hidden);
    hidden = silu(&hidden)?;
    hidden = conv2d(weights, &ns, &hidden, "decoder.conv_out.weight", "decoder.conv_out.bias", 1, 1)?;
    vae_dump("vae_out", &hidden);
    let data = gpu_download(&hidden.tensor).map_err(DiffusionError::model)?;
    let image = Flux2VaeImage {
        width: hidden.width,
        height: hidden.height,
        data,
    };
    drop(hidden);
    gpu_pool_clear();
    gpu_pool_cap_override(None);
    Ok(image)
}

/// RGB `[0,1]` u8 -> VAE input `[-1,1]` planar.
pub fn flux2_image_from_rgb_u8(rgb: &[u8], width: usize, height: usize) -> Result<Flux2VaeImage> {
    if rgb.len() != width * height * 3 {
        return Err(DiffusionError::workflow(format!(
            "flux2 image expected {} bytes, got {}",
            width * height * 3,
            rgb.len()
        )));
    }
    let plane = width * height;
    let mut data = vec![0.0f32; 3 * plane];
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 3;
            let dest = y * width + x;
            data[dest] = rgb[src] as f32 / 127.5 - 1.0;
            data[plane + dest] = rgb[src + 1] as f32 / 127.5 - 1.0;
            data[2 * plane + dest] = rgb[src + 2] as f32 / 127.5 - 1.0;
        }
    }
    Ok(Flux2VaeImage {
        width,
        height,
        data,
    })
}

/// VAE output `[-1,1]` planar -> RGB u8 with banker's rounding.
pub fn flux2_image_to_rgb_u8(image: &Flux2VaeImage) -> Vec<u8> {
    let plane = image.width * image.height;
    let mut rgb = vec![0u8; plane * 3];
    for i in 0..plane {
        let r = banker_u8(image.data[i] * 0.5 + 0.5);
        let g = banker_u8(image.data[plane + i] * 0.5 + 0.5);
        let b = banker_u8(image.data[2 * plane + i] * 0.5 + 0.5);
        rgb[i * 3] = r;
        rgb[i * 3 + 1] = g;
        rgb[i * 3 + 2] = b;
    }
    rgb
}

fn banker_u8(unit: f32) -> u8 {
    let scaled = (unit.clamp(0.0, 1.0) * 255.0) as f64;
    let floor = scaled.floor();
    let frac = scaled - floor;
    let rounded = if frac > 0.5 {
        floor + 1.0
    } else if frac < 0.5 {
        floor
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    };
    rounded as u8
}

fn conv2d(
    weights: &Flux2VaeWeights,
    ns: &str,
    input: &GpuSpatial,
    weight_name: &str,
    bias_name: &str,
    pad_x: usize,
    pad_y: usize,
) -> Result<GpuSpatial> {
    let kernel = weights.kernel(weight_name)?;
    let bias = match weights.values(bias_name) {
        Ok(values) => values.to_vec(),
        Err(_) => vec![0.0f32; kernel.out_channels],
    };
    if kernel.in_channels != input.channels {
        return Err(DiffusionError::model(format!(
            "flux2 vae {weight_name} in={} input={}",
            kernel.in_channels, input.channels
        )));
    }
    let tensor = gpu_conv2d_planar_cached(
        &input.tensor,
        input.width,
        input.height,
        ns,
        weight_name,
        &kernel.data,
        &bias,
        kernel.out_channels,
        kernel.kw,
        kernel.kh,
        pad_x,
        pad_y,
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuSpatial {
        tensor,
        width: input.width,
        height: input.height,
        channels: kernel.out_channels,
    })
}

fn downsample(
    weights: &Flux2VaeWeights,
    ns: &str,
    input: &GpuSpatial,
    prefix: &str,
) -> Result<GpuSpatial> {
    // BFL Downsample: pad (0,1,0,1) then conv 3x3 stride 2 pad 0. The
    // strided kernel skips out-of-bounds taps, so running it unpadded with
    // pad 0 IS the right/bottom zero-pad — no host pad round trip needed.
    let kernel = weights.kernel(&format!("{prefix}.weight"))?;
    let bias = weights.values(&format!("{prefix}.bias"))?.to_vec();
    let out_w = input.width / 2;
    let out_h = input.height / 2;
    let tensor = gpu_conv2d_planar_strided(
        &input.tensor,
        input.width,
        input.height,
        out_w,
        out_h,
        ns,
        &format!("{prefix}.weight"),
        &kernel.data,
        &bias,
        kernel.out_channels,
        kernel.kw,
        kernel.kh,
        0,
        0,
        2,
        2,
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuSpatial {
        tensor,
        width: out_w,
        height: out_h,
        channels: kernel.out_channels,
    })
}

fn group_norm(
    weights: &Flux2VaeWeights,
    ns: &str,
    input: &GpuSpatial,
    weight_name: &str,
    bias_name: &str,
) -> Result<GpuSpatial> {
    let gamma = weights.values(weight_name)?;
    let beta = weights.values(bias_name)?;
    let tensor = gpu_group_norm_planar(
        &input.tensor,
        input.width,
        input.height,
        32,
        ns,
        weight_name,
        gamma,
        beta,
        VAE_GROUP_NORM_EPSILON,
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuSpatial {
        tensor,
        width: input.width,
        height: input.height,
        channels: input.channels,
    })
}

fn silu(input: &GpuSpatial) -> Result<GpuSpatial> {
    Ok(GpuSpatial {
        tensor: gpu_silu(&input.tensor).map_err(DiffusionError::model)?,
        width: input.width,
        height: input.height,
        channels: input.channels,
    })
}

fn add(lhs: &GpuSpatial, rhs: &GpuSpatial) -> Result<GpuSpatial> {
    Ok(GpuSpatial {
        tensor: gpu_add(&lhs.tensor, &rhs.tensor).map_err(DiffusionError::model)?,
        width: lhs.width,
        height: lhs.height,
        channels: lhs.channels,
    })
}

fn upsample(input: &GpuSpatial) -> Result<GpuSpatial> {
    Ok(GpuSpatial {
        tensor: gpu_upsample_nearest2x(&input.tensor, input.width, input.height)
            .map_err(DiffusionError::model)?,
        width: input.width * 2,
        height: input.height * 2,
        channels: input.channels,
    })
}

fn resnet(
    weights: &Flux2VaeWeights,
    ns: &str,
    input: &GpuSpatial,
    prefix: &str,
) -> Result<GpuSpatial> {
    let hidden = group_norm(
        weights,
        ns,
        input,
        &format!("{prefix}.norm1.weight"),
        &format!("{prefix}.norm1.bias"),
    )?;
    let hidden = silu(&hidden)?;
    let hidden = conv2d(
        weights,
        ns,
        &hidden,
        &format!("{prefix}.conv1.weight"),
        &format!("{prefix}.conv1.bias"),
        1,
        1,
    )?;
    let hidden = group_norm(
        weights,
        ns,
        &hidden,
        &format!("{prefix}.norm2.weight"),
        &format!("{prefix}.norm2.bias"),
    )?;
    let hidden = silu(&hidden)?;
    let hidden = conv2d(
        weights,
        ns,
        &hidden,
        &format!("{prefix}.conv2.weight"),
        &format!("{prefix}.conv2.bias"),
        1,
        1,
    )?;
    if weights.has_tensor(&format!("{prefix}.nin_shortcut.weight")) {
        let residual = conv2d(
            weights,
            ns,
            input,
            &format!("{prefix}.nin_shortcut.weight"),
            &format!("{prefix}.nin_shortcut.bias"),
            0,
            0,
        )?;
        add(&residual, &hidden)
    } else {
        add(input, &hidden)
    }
}

fn mid_attn(
    weights: &Flux2VaeWeights,
    ns: &str,
    input: &GpuSpatial,
    prefix: &str,
) -> Result<GpuSpatial> {
    let hidden = group_norm(
        weights,
        ns,
        input,
        &format!("{prefix}.norm.weight"),
        &format!("{prefix}.norm.bias"),
    )?;
    let q = conv2d(
        weights,
        ns,
        &hidden,
        &format!("{prefix}.q.weight"),
        &format!("{prefix}.q.bias"),
        0,
        0,
    )?;
    let k = conv2d(
        weights,
        ns,
        &hidden,
        &format!("{prefix}.k.weight"),
        &format!("{prefix}.k.bias"),
        0,
        0,
    )?;
    let v = conv2d(
        weights,
        ns,
        &hidden,
        &format!("{prefix}.v.weight"),
        &format!("{prefix}.v.bias"),
        0,
        0,
    )?;
    let scale = 1.0 / (q.channels as f32).sqrt();
    let attn = gpu_attention_planar_single(&q.tensor, &k.tensor, &v.tensor, scale)
        .map_err(DiffusionError::model)?;
    let attn = GpuSpatial {
        tensor: attn,
        width: input.width,
        height: input.height,
        channels: input.channels,
    };
    let attn = conv2d(
        weights,
        ns,
        &attn,
        &format!("{prefix}.proj_out.weight"),
        &format!("{prefix}.proj_out.bias"),
        0,
        0,
    )?;
    add(input, &attn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_map_diffusers_to_bfl() {
        assert_eq!(
            canonicalize_vae_name("encoder.down_blocks.2.resnets.1.conv1.weight"),
            "encoder.down.2.block.1.conv1.weight"
        );
        assert_eq!(
            canonicalize_vae_name("decoder.up_blocks.0.resnets.2.norm1.weight"),
            "decoder.up.3.block.2.norm1.weight"
        );
        assert_eq!(
            canonicalize_vae_name("decoder.up_blocks.3.upsamplers.0.conv.weight"),
            "decoder.up.0.upsample.conv.weight"
        );
        assert_eq!(
            canonicalize_vae_name("decoder.mid_block.attentions.0.to_q.weight"),
            "decoder.mid.attn_1.q.weight"
        );
        assert_eq!(
            canonicalize_vae_name("decoder.mid_block.attentions.0.to_out.0.weight"),
            "decoder.mid.attn_1.proj_out.weight"
        );
        assert_eq!(
            canonicalize_vae_name("quant_conv.weight"),
            "encoder.quant_conv.weight"
        );
        assert_eq!(
            canonicalize_vae_name("encoder.down_blocks.1.resnets.0.conv_shortcut.weight"),
            "encoder.down.1.block.0.nin_shortcut.weight"
        );
    }

    #[test]
    fn banker_round_half_even() {
        assert_eq!(banker_u8(0.5 / 255.0), 0);
        assert_eq!(banker_u8(1.5 / 255.0), 2);
        assert_eq!(banker_u8(2.5 / 255.0), 2);
    }

    #[test]
    fn local_vae_loads_when_downloaded() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/models/flux2/flux2-vae.safetensors");
        if !path.is_file() {
            eprintln!("flux2 vae file missing, skip");
            return;
        }
        let weights = Flux2VaeWeights::load(&path).expect("load vae");
        assert_eq!(weights.bn_mean().len(), 128);
        assert!(weights.has_tensor("decoder.conv_in.weight"));
        assert!(weights.has_tensor("decoder.up.3.block.0.conv1.weight"));
        assert!(weights.has_tensor("decoder.mid.attn_1.q.weight"));
        assert!(weights.has_tensor("encoder.quant_conv.weight"));
        assert!(weights.has_tensor("decoder.post_quant_conv.weight"));
        assert!(weights.has_tensor("encoder.down.0.downsample.conv.weight"));
    }
}
