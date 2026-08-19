//! Device forward for the v4.26 IFNet.
//!
//! Structurally identical to the portable reference in [`crate::rife_cpu`] —
//! same planar `[C, H*W]` f32 layout, same op order, same f32 precision — so
//! the reference stays a usable parity oracle (`MAKEPAD_RIFE_MODE=reference`).
//!
//! Only the ops that had no counterpart in the shared CUDA store are new
//! (`gpu_rife_warp`, `gpu_rife_conv_transpose2d`, `gpu_rife_res_conv`,
//! `gpu_rife_scale`, `gpu_rife_fill`, `gpu_rife_merge_rgb8`, all in
//! `libs/ai/cuda/kernels/rife.cu`); convolution, bilinear resize, pixel
//! shuffle, row slicing and concatenation are the existing shared kernels.
//!
//! Every weight lands in the device weight cache under the `rife::` prefix,
//! so a warm service pays the ~22 MB upload once and
//! [`crate::rife::unload_rife`] retires the whole namespace.
//!
//! This path cannot run on macOS (no CUDA); it is compiled everywhere and
//! fails closed through the shared `gpu_*` stubs.

use crate::backend::{
    gpu_add, gpu_birefnet_resize_bilinear, gpu_concat_rows_many, gpu_conv2d_planar_strided,
    gpu_pixel_shuffle_planar, gpu_realesrgan_lrelu, gpu_rife_conv_transpose2d,
    gpu_rife_fill, gpu_rife_merge_rgb8, gpu_rife_res_conv, gpu_rife_scale, gpu_rife_warp,
    gpu_slice_rows, gpu_upload, GpuTensor,
};
use crate::rife::{
    padded_extent, ConvWeight, DeconvWeight, IfBlockWeights, RifeFramePair, RifeModelWeights,
    RifeScale, RIFE_CACHE_NAMESPACE, RIFE_LASTCONV_PLANES, RIFE_LRELU_SLOPE,
};
use crate::rife_cpu::pack_padded_rgb8;
use crate::{DiffusionError, Result};

fn device_error(stage: &str, error: String) -> DiffusionError {
    DiffusionError::model(format!("rife {stage}: {error}"))
}

/// Extent of a planar tensor as the kernels see it.
#[derive(Clone, Copy)]
struct Extent {
    width: usize,
    height: usize,
}

impl Extent {
    fn plane(self) -> usize {
        self.width * self.height
    }
}

fn conv2d(
    x: &GpuTensor,
    extent: Extent,
    weight: &ConvWeight,
    key: &str,
) -> Result<(GpuTensor, Extent)> {
    if x.rows() != weight.in_channels {
        return Err(DiffusionError::model(format!(
            "rife conv {key} expects {} channels, got {}",
            weight.in_channels,
            x.rows()
        )));
    }
    let out = Extent {
        width: (extent.width + 2 * weight.pad).saturating_sub(weight.kw) / weight.stride + 1,
        height: (extent.height + 2 * weight.pad).saturating_sub(weight.kh) / weight.stride + 1,
    };
    let tensor = gpu_conv2d_planar_strided(
        x,
        extent.width,
        extent.height,
        out.width,
        out.height,
        RIFE_CACHE_NAMESPACE,
        key,
        &weight.weights,
        &weight.bias,
        weight.out_channels,
        weight.kw,
        weight.kh,
        weight.pad,
        weight.pad,
        weight.stride,
        weight.stride,
    )
    .map_err(|error| device_error(key, error))?;
    Ok((tensor, out))
}

fn conv2d_lrelu(
    x: &GpuTensor,
    extent: Extent,
    weight: &ConvWeight,
    key: &str,
) -> Result<(GpuTensor, Extent)> {
    let (tensor, out) = conv2d(x, extent, weight, key)?;
    let activated = gpu_realesrgan_lrelu(&tensor, RIFE_LRELU_SLOPE)
        .map_err(|error| device_error("lrelu", error))?;
    Ok((activated, out))
}

fn conv_transpose2d(
    x: &GpuTensor,
    extent: Extent,
    weight: &DeconvWeight,
    key: &str,
) -> Result<(GpuTensor, Extent)> {
    if x.rows() != weight.in_channels {
        return Err(DiffusionError::model(format!(
            "rife deconv {key} expects {} channels, got {}",
            weight.in_channels,
            x.rows()
        )));
    }
    let out = Extent {
        width: (extent.width - 1) * weight.stride + weight.kw - 2 * weight.pad,
        height: (extent.height - 1) * weight.stride + weight.kh - 2 * weight.pad,
    };
    let tensor = gpu_rife_conv_transpose2d(
        x,
        extent.width,
        extent.height,
        RIFE_CACHE_NAMESPACE,
        key,
        &weight.weights,
        &weight.bias,
        weight.out_channels,
        weight.kw,
        weight.kh,
        weight.pad,
        weight.stride,
    )
    .map_err(|error| device_error(key, error))?;
    Ok((tensor, out))
}

fn resize(x: &GpuTensor, from: Extent, to: Extent) -> Result<GpuTensor> {
    if from.width == to.width && from.height == to.height {
        return gpu_slice_rows(x, 0, x.rows()).map_err(|error| device_error("view", error));
    }
    gpu_birefnet_resize_bilinear(x, from.width, from.height, to.width, to.height, false)
        .map_err(|error| device_error("resize", error))
}

/// The reference clamps `Head`'s input (and IFNet's images) to `[0, 1]`.
/// Here the images are `u8 / 255` with zero padding, so the clamp is
/// provably a no-op and no kernel is spent on it — the portable reference
/// still performs it, which keeps the two paths comparable on any input.
fn head_forward(
    image: &GpuTensor,
    extent: Extent,
    weights: &crate::rife::HeadWeights,
) -> Result<GpuTensor> {
    let (x, small) = conv2d_lrelu(image, extent, &weights.cnn0, "encode.cnn0")?;
    let (x, small) = conv2d_lrelu(&x, small, &weights.cnn1, "encode.cnn1")?;
    let (x, small) = conv2d_lrelu(&x, small, &weights.cnn2, "encode.cnn2")?;
    let (x, full) = conv_transpose2d(&x, small, &weights.cnn3, "encode.cnn3")?;
    if full.width != extent.width || full.height != extent.height {
        return Err(DiffusionError::model(format!(
            "rife encode returned {}x{}, expected {}x{}",
            full.width, full.height, extent.width, extent.height
        )));
    }
    Ok(x)
}

/// One IFBlock; returns `(flow, mask, feat)` at the full extent.
fn if_block_forward(
    x: &GpuTensor,
    extent: Extent,
    flow: Option<&GpuTensor>,
    weights: &IfBlockWeights,
    index: usize,
    scale: f32,
) -> Result<(GpuTensor, GpuTensor, GpuTensor)> {
    let small = Extent {
        width: (extent.width as f32 / scale) as usize,
        height: (extent.height as f32 / scale) as usize,
    };
    if small.width == 0 || small.height == 0 {
        return Err(DiffusionError::model(
            "rife if_block: canvas too small for the requested scale",
        ));
    }
    let downscaled = resize(x, extent, small)?;
    let input = match flow {
        None => downscaled,
        Some(flow) => {
            let down = resize(flow, extent, small)?;
            let scaled = gpu_rife_scale(&down, 1.0 / scale)
                .map_err(|error| device_error("flow downscale", error))?;
            gpu_concat_rows_many(&[&downscaled, &scaled])
                .map_err(|error| device_error("flow concat", error))?
        }
    };
    if input.rows() != weights.in_planes {
        return Err(DiffusionError::model(format!(
            "rife block {index} expects {} input planes, got {}",
            weights.in_planes,
            input.rows()
        )));
    }
    let (feat, quarter) = conv2d_lrelu(
        &input,
        small,
        &weights.conv0_a,
        &format!("blocks.{index}.conv0.0.0"),
    )?;
    let (mut feat, quarter) = conv2d_lrelu(
        &feat,
        quarter,
        &weights.conv0_b,
        &format!("blocks.{index}.conv0.1.0"),
    )?;
    for (res_index, res) in weights.convblock.iter().enumerate() {
        let key = format!("blocks.{index}.convblock.{res_index}.conv");
        let (convolved, _) = conv2d(&feat, quarter, &res.conv, &key)?;
        feat = gpu_rife_res_conv(&convolved, &feat, &res.beta, RIFE_LRELU_SLOPE)
            .map_err(|error| device_error(&key, error))?;
    }
    let (tmp, half) = conv_transpose2d(
        &feat,
        quarter,
        &weights.lastconv,
        &format!("blocks.{index}.lastconv.0"),
    )?;
    let tmp = gpu_pixel_shuffle_planar(
        &tmp,
        half.width,
        half.height,
        RIFE_LASTCONV_PLANES,
        2,
        &vec![0.0f32; RIFE_LASTCONV_PLANES],
    )
    .map_err(|error| device_error("pixel shuffle", error))?;
    let shuffled = Extent {
        width: half.width * 2,
        height: half.height * 2,
    };
    if shuffled.width != small.width || shuffled.height != small.height {
        return Err(DiffusionError::model(format!(
            "rife block {index} produced {}x{}, expected {}x{}",
            shuffled.width, shuffled.height, small.width, small.height
        )));
    }
    let tmp = resize(&tmp, shuffled, extent)?;
    let flow_raw = gpu_slice_rows(&tmp, 0, 4).map_err(|error| device_error("slice flow", error))?;
    let flow_out =
        gpu_rife_scale(&flow_raw, scale).map_err(|error| device_error("flow upscale", error))?;
    let mask = gpu_slice_rows(&tmp, 4, 1).map_err(|error| device_error("slice mask", error))?;
    let feat_out = gpu_slice_rows(&tmp, 5, RIFE_LASTCONV_PLANES - 5)
        .map_err(|error| device_error("slice feat", error))?;
    // `mask`/`feat_out` are views into `tmp`; a `* 1.0` copies them into
    // their own pool buffers so `tmp` is released at the end of the block
    // while they stay alive as inputs to the next one.
    let mask = gpu_rife_scale(&mask, 1.0).map_err(|e| device_error("materialize mask", e))?;
    let feat_out =
        gpu_rife_scale(&feat_out, 1.0).map_err(|e| device_error("materialize feat", e))?;
    Ok((flow_out, mask, feat_out))
}

/// Device end-to-end interpolation of one frame pair.
pub fn interpolate_rgb8(
    weights: &RifeModelWeights,
    pair: RifeFramePair<'_>,
    timestep: f32,
    scale: RifeScale,
) -> Result<Vec<u8>> {
    let (padded_w, padded_h) = padded_extent(pair.width, pair.height, scale);
    let extent = Extent {
        width: padded_w,
        height: padded_h,
    };
    let plane = extent.plane();
    let host0 = pack_padded_rgb8(pair.frame0, pair.width, pair.height, padded_w, padded_h);
    let host1 = pack_padded_rgb8(pair.frame1, pair.width, pair.height, padded_w, padded_h);
    let img0 =
        gpu_upload(&host0.data, 3, plane).map_err(|error| device_error("upload frame0", error))?;
    let img1 =
        gpu_upload(&host1.data, 3, plane).map_err(|error| device_error("upload frame1", error))?;
    drop(host0);
    drop(host1);

    let f0 = head_forward(&img0, extent, &weights.encode)?;
    let f1 = head_forward(&img1, extent, &weights.encode)?;
    let time =
        gpu_rife_fill(1, plane, timestep).map_err(|error| device_error("timestep", error))?;

    let mut warped0 = gpu_slice_rows(&img0, 0, 3).map_err(|e| device_error("view img0", e))?;
    let mut warped1 = gpu_slice_rows(&img1, 0, 3).map_err(|e| device_error("view img1", e))?;
    let mut flow: Option<GpuTensor> = None;
    let mut mask = gpu_rife_fill(1, plane, 0.0).map_err(|e| device_error("mask init", e))?;
    let mut feat = gpu_rife_fill(RIFE_LASTCONV_PLANES - 5, plane, 0.0)
        .map_err(|e| device_error("feat init", e))?;
    let scale_list = scale.scale_list();

    for (index, block) in weights.blocks.iter().enumerate() {
        let step = scale_list[index];
        let updated = match &flow {
            None => {
                let input = gpu_concat_rows_many(&[&img0, &img1, &f0, &f1, &time])
                    .map_err(|error| device_error("block0 concat", error))?;
                let (flow_out, mask_out, feat_out) =
                    if_block_forward(&input, extent, None, block, index, step)?;
                mask = mask_out;
                feat = feat_out;
                flow_out
            }
            Some(current) => {
                let flow01 =
                    gpu_slice_rows(current, 0, 2).map_err(|e| device_error("slice flow01", e))?;
                let flow23 =
                    gpu_slice_rows(current, 2, 2).map_err(|e| device_error("slice flow23", e))?;
                let wf0 = gpu_rife_warp(&f0, &flow01, extent.width, extent.height)
                    .map_err(|error| device_error("warp f0", error))?;
                let wf1 = gpu_rife_warp(&f1, &flow23, extent.width, extent.height)
                    .map_err(|error| device_error("warp f1", error))?;
                let input = gpu_concat_rows_many(&[
                    &warped0, &warped1, &wf0, &wf1, &time, &mask, &feat,
                ])
                .map_err(|error| device_error("block concat", error))?;
                let (delta, mask_out, feat_out) =
                    if_block_forward(&input, extent, Some(current), block, index, step)?;
                mask = mask_out;
                feat = feat_out;
                gpu_add(current, &delta).map_err(|error| device_error("flow update", error))?
            }
        };
        let flow01 =
            gpu_slice_rows(&updated, 0, 2).map_err(|e| device_error("slice flow01", e))?;
        let flow23 =
            gpu_slice_rows(&updated, 2, 2).map_err(|e| device_error("slice flow23", e))?;
        warped0 = gpu_rife_warp(&img0, &flow01, extent.width, extent.height)
            .map_err(|error| device_error("warp img0", error))?;
        warped1 = gpu_rife_warp(&img1, &flow23, extent.width, extent.height)
            .map_err(|error| device_error("warp img1", error))?;
        flow = Some(updated);
    }

    gpu_rife_merge_rgb8(
        &warped0,
        &warped1,
        &mask,
        padded_w,
        padded_h,
        pair.width,
        pair.height,
    )
    .map_err(|error| device_error("merge", error))
}
