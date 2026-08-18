//! Native CUDA forward for the pinned RealESRGAN x4plus checkpoint.
//!
//! The implementation follows the released RRDBNet graph literally.  All
//! feature maps are planar `[channel, y * width + x]` (batch 1) f32 tensors;
//! every convolution is 3x3 stride-1 "same".  The only host transfers in a
//! warm upscale are the input upload and the final three-plane download.

use crate::backend::{
    gpu_add, gpu_concat_rows, gpu_conv2d_planar_cached, gpu_device_available,
    gpu_download, gpu_realesrgan_alloc_f16, gpu_realesrgan_alloc_f32,
    gpu_realesrgan_bias_lrelu_f16, gpu_realesrgan_bias_lrelu_f32,
    gpu_realesrgan_conv3x3_f16, gpu_realesrgan_conv3x3_f32, gpu_realesrgan_lrelu,
    gpu_realesrgan_quantize_rgb8_f32, gpu_realesrgan_scale_add,
    gpu_realesrgan_spine_axpb, gpu_upload,
    gpu_upsample_nearest2x, GpuTensor,
};
use crate::realesrgan::{
    RealEsrganCancel, RealEsrganImage, RealEsrganUpscale, RealEsrganWeights,
    REALESRGAN_CACHE_NAMESPACE, REALESRGAN_LRELU_SLOPE, REALESRGAN_NUM_BLOCK,
    REALESRGAN_NUM_FEAT, REALESRGAN_NUM_GROW, REALESRGAN_RESIDUAL_SCALE,
    REALESRGAN_SCALE,
};
use crate::{DiffusionError, ProgressHook, Result};
use std::collections::HashMap;

fn device_error(stage: &str, error: String) -> DiffusionError {
    DiffusionError::model(format!("realesrgan {stage}: {error}"))
}

fn check_cancel(cancel: Option<RealEsrganCancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Debug)]
struct ConvWeight {
    out_channels: usize,
    in_channels: usize,
    weights: Vec<f32>,
    bias: Vec<f32>,
}

enum ForwardOutput {
    Planes(Vec<f32>),
    Rgb8(Vec<u8>),
}

/// Default is the cuDNN f16 fast path (the official CUDA forward is fp16).
/// `MAKEPAD_REALESRGAN_MODE=reference` selects the f32 planar path, which
/// combined with `FLUX_VAE_CONV_GEMM=0` is the pure-f32 parity oracle.
fn fast_mode_enabled() -> bool {
    match std::env::var("MAKEPAD_REALESRGAN_MODE") {
        Ok(value) => value != "reference",
        Err(_) => true,
    }
}

/// Prepared host-side checkpoint.  Convolution tensors stay f32; the shared
/// convolution API performs its own deterministic device packing and caching
/// under `realesrgan::<name>` on first use.
pub(crate) struct RealEsrganModel {
    convs: HashMap<String, ConvWeight>,
}

impl RealEsrganModel {
    pub(crate) fn prepare(
        weights: &RealEsrganWeights,
        cancel: Option<RealEsrganCancel<'_>>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        crate::emit_progress(&mut progress, "load realesrgan", 0.0)?;
        if !gpu_device_available() {
            return Err(DiffusionError::model(
                "native RealESRGAN requires the Makepad CUDA backend",
            ));
        }
        let mut names: Vec<String> = weights.tensor_names().cloned().collect();
        names.sort();
        let mut convs = HashMap::new();
        let total = names.len().max(1);
        for (index, name) in names.into_iter().enumerate() {
            let Some(prefix) = name.strip_suffix(".weight") else {
                continue;
            };
            if index % 64 == 0 {
                check_cancel(cancel)?;
                crate::emit_progress(
                    &mut progress,
                    "load realesrgan checkpoint",
                    index as f64 / total as f64,
                )?;
            }
            let shape = weights.shape(&name)?;
            if shape.len() != 4 || shape[2] != 3 || shape[3] != 3 {
                return Err(DiffusionError::model(format!(
                    "realesrgan conv {name} has unexpected shape {shape:?}"
                )));
            }
            let bias = weights.f32(&format!("{prefix}.bias"))?;
            if bias.len() != shape[0] {
                return Err(DiffusionError::model(format!(
                    "realesrgan conv {prefix} bias length {} != {}",
                    bias.len(),
                    shape[0]
                )));
            }
            convs.insert(
                prefix.to_string(),
                ConvWeight {
                    out_channels: shape[0],
                    in_channels: shape[1],
                    weights: weights.f32(&name)?,
                    bias,
                },
            );
        }
        check_cancel(cancel)?;
        crate::emit_progress(&mut progress, "load realesrgan", 1.0)?;
        Ok(Self { convs })
    }

    pub(crate) fn upscale(
        &self,
        image: RealEsrganImage<'_>,
        cancel: Option<RealEsrganCancel<'_>>,
        mut progress: Option<ProgressHook>,
    ) -> Result<RealEsrganUpscale> {
        match self.forward(image, cancel, &mut progress, false)? {
            ForwardOutput::Planes(planes) => RealEsrganUpscale::new(
                image.width * REALESRGAN_SCALE,
                image.height * REALESRGAN_SCALE,
                planes,
            ),
            ForwardOutput::Rgb8(_) => unreachable!("planes requested"),
        }
    }

    /// Warm-path artifact: quantization and interleave happen on device in
    /// fast mode, so only the final RGB8 bytes cross PCIe.
    pub(crate) fn upscale_rgb8(
        &self,
        image: RealEsrganImage<'_>,
        cancel: Option<RealEsrganCancel<'_>>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<u8>> {
        match self.forward(image, cancel, &mut progress, true)? {
            ForwardOutput::Rgb8(rgb) => Ok(rgb),
            ForwardOutput::Planes(planes) => Ok(RealEsrganUpscale::new(
                image.width * REALESRGAN_SCALE,
                image.height * REALESRGAN_SCALE,
                planes,
            )?
            .rgb8()),
        }
    }

    fn forward(
        &self,
        image: RealEsrganImage<'_>,
        cancel: Option<RealEsrganCancel<'_>>,
        progress: &mut Option<ProgressHook>,
        want_rgb8: bool,
    ) -> Result<ForwardOutput> {
        check_cancel(cancel)?;
        crate::emit_progress(progress, "realesrgan preprocess", 0.0)?;
        let plane = image.width * image.height;
        let mut input = vec![0.0f32; 3 * plane];
        for pixel in 0..plane {
            for channel in 0..3 {
                input[channel * plane + pixel] =
                    f32::from(image.pixels[pixel * image.channels + channel]) / 255.0;
            }
        }
        if fast_mode_enabled() {
            self.forward_f16(&input, image.width, image.height, cancel, progress, want_rgb8)
        } else {
            let planes =
                self.forward_reference(&input, image.width, image.height, cancel, progress)?;
            Ok(ForwardOutput::Planes(planes))
        }
    }

    /// cuDNN fast forward.  All 345 dense-block convolutions run in f16 over
    /// one persistent 256-row planar buffer: with batch 1 a planar row block
    /// is a contiguous NCHW tensor, so every "concat" in the released graph
    /// is a pointer offset and the epilogue kernels fold bias/LeakyReLU/
    /// residual scaling in place.  Zero concat copies, zero im2col slabs.
    /// The residual spine and the four-conv upsample head run in f32 (FMA
    /// cuDNN, exact epilogues) so their rounding never reaches the locked
    /// output envelope; conv inputs stay f16 like the official forward.
    fn forward_f16(
        &self,
        input: &[f32],
        width: usize,
        height: usize,
        cancel: Option<RealEsrganCancel<'_>>,
        progress: &mut Option<ProgressHook>,
        want_rgb8: bool,
    ) -> Result<ForwardOutput> {
        const NS: &str = REALESRGAN_CACHE_NAMESPACE;
        let feat = REALESRGAN_NUM_FEAT;
        let grow = REALESRGAN_NUM_GROW;
        let slope = REALESRGAN_LRELU_SLOPE;
        let rscale = REALESRGAN_RESIDUAL_SCALE;
        let plane = width * height;
        let err = device_error;

        // `conv_first` runs in f32 on the unrounded input: its rounding
        // would ride through all 23 blocks and the trunk skip, the most
        // amplified position in the graph — and it costs ~a millisecond.
        let input32 = gpu_upload(input, 3, plane).map_err(|e| err("upload input", e))?;
        let fea32 =
            self.conv_f32(&input32, 3, width, height, "conv_first", feat)?;
        drop(input32);
        self.bias_f32(&fea32, "conv_first", 1.0)?;
        check_cancel(cancel)?;

        // The residual spine (RDB/RRDB residuals and the trunk) accumulates
        // in f32 — `x32`/`save32`/`fea32` — while every dense-block conv
        // still reads the f16 `wide` view, exactly the tensors the official
        // forward feeds its convs.  Spine rounding compounding across 23
        // blocks is what pushed the output tail past the official envelope.
        let wide = gpu_realesrgan_alloc_f16(feat + 4 * grow + feat, plane)
            .map_err(|e| err("alloc wide", e))?;
        let x32 = gpu_realesrgan_alloc_f32(feat, plane).map_err(|e| err("alloc x32", e))?;
        let save32 =
            gpu_realesrgan_alloc_f32(feat, plane).map_err(|e| err("alloc save32", e))?;
        gpu_realesrgan_spine_axpb(
            &fea32, Some(&fea32), None, &x32, Some((&wide, 0)), feat, NS, "", &[], 0.0,
        )
        .map_err(|e| err("seed wide", e))?;

        for block in 0..REALESRGAN_NUM_BLOCK {
            check_cancel(cancel)?;
            gpu_realesrgan_spine_axpb(
                &x32, Some(&x32), None, &save32, None, feat, NS, "", &[], 0.0,
            )
            .map_err(|e| err("save rrdb input", e))?;
            for rdb in 1..=3 {
                let prefix = format!("body.{block}.rdb{rdb}");
                for conv in 1..=4usize {
                    let in_channels = feat + (conv - 1) * grow;
                    let name = format!("{prefix}.conv{conv}");
                    self.conv_f16(&wide, in_channels, width, height, &name, grow, &wide, in_channels)?;
                    self.bias_f16(&wide, in_channels, grow, &name, slope)?;
                }
                let name = format!("{prefix}.conv5");
                let dense = feat + 4 * grow;
                self.conv_f16(&wide, dense, width, height, &name, feat, &wide, dense)?;
                let bias = &self.weight(&name)?.bias;
                gpu_realesrgan_spine_axpb(
                    &x32,
                    None,
                    Some((&wide, dense)),
                    &x32,
                    Some((&wide, 0)),
                    feat,
                    NS,
                    &name,
                    bias,
                    rscale,
                )
                .map_err(|e| err("rdb residual", e))?;
            }
            gpu_realesrgan_spine_axpb(
                &save32, Some(&x32), None, &x32, Some((&wide, 0)), feat, NS, "", &[], rscale,
            )
            .map_err(|e| err("rrdb residual", e))?;
            crate::emit_progress(
                progress,
                "realesrgan body",
                0.02 + 0.86 * (block + 1) as f64 / REALESRGAN_NUM_BLOCK as f64,
            )?;
        }
        drop(save32);
        drop(wide);

        // `conv_body` reads the exact f32 spine and stays f32: its output
        // lands directly on the trunk skip that the whole head amplifies.
        let body32 = self.conv_f32(&x32, feat, width, height, "conv_body", feat)?;
        drop(x32);
        let body_bias = &self.weight("conv_body")?.bias;
        gpu_realesrgan_spine_axpb(
            &fea32,
            Some(&body32),
            None,
            &fea32,
            None,
            feat,
            NS,
            "conv_body",
            body_bias,
            1.0,
        )
        .map_err(|e| err("trunk", e))?;
        drop(body32);
        check_cancel(cancel)?;
        crate::emit_progress(progress, "realesrgan upsample", 0.90)?;

        // The upsample head runs in true f32 (cuDNN FMA convs + exact
        // bias/lrelu): the official fp16 forward keeps rounding through
        // conv_up1..conv_last, so an exact head puts every tail metric
        // strictly inside the locked envelope.
        let up = gpu_upsample_nearest2x(&fea32, width, height)
            .map_err(|e| err("upsample 2x", e))?;
        drop(fea32);
        let (width2, height2) = (width * 2, height * 2);
        let c1 = self.conv_f32(&up, feat, width2, height2, "conv_up1", feat)?;
        drop(up);
        self.bias_f32(&c1, "conv_up1", slope)?;
        check_cancel(cancel)?;

        let up2 = gpu_upsample_nearest2x(&c1, width2, height2)
            .map_err(|e| err("upsample 4x", e))?;
        drop(c1);
        let (width4, height4) = (width * 4, height * 4);
        let c2 = self.conv_f32(&up2, feat, width4, height4, "conv_up2", feat)?;
        drop(up2);
        self.bias_f32(&c2, "conv_up2", slope)?;
        check_cancel(cancel)?;
        crate::emit_progress(progress, "realesrgan head", 0.95)?;

        let c3 = self.conv_f32(&c2, feat, width4, height4, "conv_hr", feat)?;
        drop(c2);
        self.bias_f32(&c3, "conv_hr", slope)?;

        let out = self.conv_f32(&c3, feat, width4, height4, "conv_last", 3)?;
        drop(c3);
        self.bias_f32(&out, "conv_last", 1.0)?;
        check_cancel(cancel)?;
        crate::emit_progress(progress, "realesrgan download", 0.98)?;

        let result = if want_rgb8 {
            ForwardOutput::Rgb8(
                gpu_realesrgan_quantize_rgb8_f32(&out).map_err(|e| err("quantize rgb8", e))?,
            )
        } else {
            ForwardOutput::Planes(
                gpu_download(&out).map_err(|e| err("download output", e))?,
            )
        };
        crate::emit_progress(progress, "realesrgan upscale", 1.0)?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn conv_f16(
        &self,
        input: &GpuTensor,
        in_channels: usize,
        width: usize,
        height: usize,
        name: &str,
        out_channels: usize,
        output: &GpuTensor,
        out_row_offset: usize,
    ) -> Result<()> {
        let weight = self.weight(name)?;
        if weight.in_channels != in_channels || weight.out_channels != out_channels {
            return Err(DiffusionError::model(format!(
                "realesrgan conv {name} is {}x{}, requested {in_channels}x{out_channels}",
                weight.in_channels, weight.out_channels
            )));
        }
        gpu_realesrgan_conv3x3_f16(
            input,
            in_channels,
            width,
            height,
            REALESRGAN_CACHE_NAMESPACE,
            name,
            &weight.weights,
            out_channels,
            output,
            out_row_offset,
        )
        .map_err(|error| device_error(name, error))
    }

    fn bias_f16(
        &self,
        tensor: &GpuTensor,
        row_offset: usize,
        channels: usize,
        name: &str,
        slope: f32,
    ) -> Result<()> {
        let bias = &self.weight(name)?.bias;
        gpu_realesrgan_bias_lrelu_f16(
            tensor,
            row_offset,
            channels,
            REALESRGAN_CACHE_NAMESPACE,
            name,
            bias,
            slope,
        )
        .map_err(|error| device_error("bias epilogue", error))
    }

    fn conv_f32(
        &self,
        input: &GpuTensor,
        in_channels: usize,
        width: usize,
        height: usize,
        name: &str,
        out_channels: usize,
    ) -> Result<GpuTensor> {
        let weight = self.weight(name)?;
        if weight.in_channels != in_channels || weight.out_channels != out_channels {
            return Err(DiffusionError::model(format!(
                "realesrgan conv {name} is {}x{}, requested {in_channels}x{out_channels}",
                weight.in_channels, weight.out_channels
            )));
        }
        gpu_realesrgan_conv3x3_f32(
            input,
            in_channels,
            width,
            height,
            REALESRGAN_CACHE_NAMESPACE,
            name,
            &weight.weights,
            out_channels,
        )
        .map_err(|error| device_error(name, error))
    }

    fn bias_f32(&self, tensor: &GpuTensor, name: &str, slope: f32) -> Result<()> {
        let bias = &self.weight(name)?.bias;
        gpu_realesrgan_bias_lrelu_f32(
            tensor,
            REALESRGAN_CACHE_NAMESPACE,
            name,
            bias,
            slope,
        )
        .map_err(|error| device_error("bias epilogue", error))
    }

    fn forward_reference(
        &self,
        input: &[f32],
        width: usize,
        height: usize,
        cancel: Option<RealEsrganCancel<'_>>,
        progress: &mut Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let plane = width * height;
        let image_gpu = gpu_upload(input, 3, plane)
            .map_err(|error| device_error("upload input", error))?;

        let fea = self.conv(&image_gpu, width, height, "conv_first")?;
        drop(image_gpu);
        check_cancel(cancel)?;

        let mut hidden = self.rrdb(&fea, width, height, 0)?;
        crate::emit_progress(
            progress,
            "realesrgan body",
            0.02 + 0.86 / REALESRGAN_NUM_BLOCK as f64,
        )?;
        for block in 1..REALESRGAN_NUM_BLOCK {
            check_cancel(cancel)?;
            hidden = self.rrdb(&hidden, width, height, block)?;
            crate::emit_progress(
                progress,
                "realesrgan body",
                0.02 + 0.86 * (block + 1) as f64 / REALESRGAN_NUM_BLOCK as f64,
            )?;
        }
        let body = self.conv(&hidden, width, height, "conv_body")?;
        drop(hidden);
        let trunk = gpu_add(&fea, &body).map_err(|error| device_error("trunk", error))?;
        drop(fea);
        drop(body);
        check_cancel(cancel)?;
        crate::emit_progress(progress, "realesrgan upsample", 0.90)?;

        let up = gpu_upsample_nearest2x(&trunk, width, height)
            .map_err(|error| device_error("upsample 2x", error))?;
        drop(trunk);
        let up = self.conv_lrelu(&up, width * 2, height * 2, "conv_up1")?;
        check_cancel(cancel)?;
        let up = gpu_upsample_nearest2x(&up, width * 2, height * 2)
            .map_err(|error| device_error("upsample 4x", error))?;
        let up = self.conv_lrelu(&up, width * 4, height * 4, "conv_up2")?;
        check_cancel(cancel)?;
        crate::emit_progress(progress, "realesrgan head", 0.95)?;
        let hr = self.conv_lrelu(&up, width * 4, height * 4, "conv_hr")?;
        drop(up);
        let out = self.conv(&hr, width * 4, height * 4, "conv_last")?;
        drop(hr);
        check_cancel(cancel)?;
        crate::emit_progress(progress, "realesrgan download", 0.98)?;
        let planes = gpu_download(&out)
            .map_err(|error| device_error("download output", error))?;
        crate::emit_progress(progress, "realesrgan upscale", 1.0)?;
        Ok(planes)
    }

    fn weight(&self, name: &str) -> Result<&ConvWeight> {
        self.convs.get(name).ok_or_else(|| {
            DiffusionError::model(format!("realesrgan conv {name} is missing"))
        })
    }

    fn conv(
        &self,
        input: &GpuTensor,
        width: usize,
        height: usize,
        name: &str,
    ) -> Result<GpuTensor> {
        let weight = self.weight(name)?;
        if weight.in_channels != input.rows() {
            return Err(DiffusionError::model(format!(
                "realesrgan conv {name} expects {} channels, got {}",
                weight.in_channels,
                input.rows()
            )));
        }
        gpu_conv2d_planar_cached(
            input,
            width,
            height,
            REALESRGAN_CACHE_NAMESPACE,
            name,
            &weight.weights,
            &weight.bias,
            weight.out_channels,
            3,
            3,
            1,
            1,
        )
        .map_err(|error| device_error(name, error))
    }

    fn conv_lrelu(
        &self,
        input: &GpuTensor,
        width: usize,
        height: usize,
        name: &str,
    ) -> Result<GpuTensor> {
        let out = self.conv(input, width, height, name)?;
        gpu_realesrgan_lrelu(&out, REALESRGAN_LRELU_SLOPE)
            .map_err(|error| device_error("lrelu", error))
    }

    /// One residual dense block: five convs over incrementally concatenated
    /// features, LeakyReLU on the first four, 0.2-scaled residual.
    fn rdb(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
    ) -> Result<GpuTensor> {
        let first = self.conv_lrelu(x, width, height, &format!("{prefix}.conv1"))?;
        let mut cat = gpu_concat_rows(x, &first)
            .map_err(|error| device_error("rdb concat", error))?;
        drop(first);
        for conv in 2..=4 {
            let grown = self.conv_lrelu(&cat, width, height, &format!("{prefix}.conv{conv}"))?;
            cat = gpu_concat_rows(&cat, &grown)
                .map_err(|error| device_error("rdb concat", error))?;
        }
        debug_assert_eq!(
            cat.rows(),
            REALESRGAN_NUM_FEAT + 4 * REALESRGAN_NUM_GROW
        );
        let delta = self.conv(&cat, width, height, &format!("{prefix}.conv5"))?;
        drop(cat);
        gpu_realesrgan_scale_add(x, &delta, REALESRGAN_RESIDUAL_SCALE)
            .map_err(|error| device_error("rdb residual", error))
    }

    fn rrdb(
        &self,
        x: &GpuTensor,
        width: usize,
        height: usize,
        block: usize,
    ) -> Result<GpuTensor> {
        let mut hidden = self.rdb(x, width, height, &format!("body.{block}.rdb1"))?;
        hidden = self.rdb(&hidden, width, height, &format!("body.{block}.rdb2"))?;
        hidden = self.rdb(&hidden, width, height, &format!("body.{block}.rdb3"))?;
        gpu_realesrgan_scale_add(x, &hidden, REALESRGAN_RESIDUAL_SCALE)
            .map_err(|error| device_error("rrdb residual", error))
    }
}
