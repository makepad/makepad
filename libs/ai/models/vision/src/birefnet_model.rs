//! Native CUDA forward for the pinned BiRefNet HR-matting checkpoint.
//!
//! The implementation follows the released graph literally: two Swin-L
//! backbone passes (1024 and 512), multi-scale concatenation, the context
//! squeeze block, and the deformable ASPP decoder.  Transformer tensors use
//! token-major `[token, channel]` storage; decoder tensors use planar
//! `[channel, y * width + x]` storage.  The only host transfer in a warm
//! forward is the final one-channel matte.

use crate::backend::{
    gpu_add, gpu_birefnet_broadcast, gpu_birefnet_deform_conv2d_cached,
    gpu_birefnet_global_avg_pool, gpu_birefnet_image_to_patches,
    gpu_birefnet_mul_sigmoid_mask, gpu_birefnet_relu,
    gpu_birefnet_resize_bilinear, gpu_birefnet_swin_attention,
    gpu_birefnet_tokens_to_planar, gpu_concat_cols, gpu_concat_rows,
    gpu_conv2d_planar_cached, gpu_download, gpu_gather_cols,
    gpu_gather_rows_colblock, gpu_gelu_erf, gpu_layer_norm_pytorch,
    gpu_linear_nt_cached, gpu_slice_cols, gpu_slice_rows, gpu_upload, gpu_upload_u32,
    GpuLinearPart, GpuTensor,
};
use crate::birefnet::{
    BiRefNetCancel, BiRefNetImage, BiRefNetMatte, BiRefNetWeights,
    BIREFNET_BN_EPS, BIREFNET_CACHE_NAMESPACE, BIREFNET_DEPTHS,
    BIREFNET_HEADS, BIREFNET_INPUT_SIZE, BIREFNET_LN_EPS,
    BIREFNET_WINDOW_SIZE,
};
use crate::{DiffusionError, ProgressHook, Result};
use makepad_ggml::quant::{GGML_TYPE_BF16, GGML_TYPE_F16};
use makepad_ai_loader::MlxDType;
use std::collections::HashMap;

fn device_error(stage: &str, error: String) -> DiffusionError {
    DiffusionError::model(format!("birefnet {stage}: {error}"))
}

fn check_cancel(cancel: Option<BiRefNetCancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Debug)]
struct MatrixWeight {
    shape: Vec<usize>,
    ggml_type: u32,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct FloatWeight {
    shape: Vec<usize>,
    values: Vec<f32>,
}

struct Feature {
    tensor: GpuTensor,
    width: usize,
    height: usize,
}

struct BackboneOutput {
    stages: Vec<Feature>,
}

struct WindowIndices {
    gather: Vec<u32>,
    scatter: Vec<u32>,
    regions: Option<Vec<u32>>,
    windows: usize,
}

/// Prepared host-side checkpoint plus the metadata needed to populate the
/// device cache.  Large 2-D linear tensors remain in their original F16/BF16
/// byte representation.  Convolution tensors are expanded once to f32 because
/// the shared convolution API performs its own deterministic device packing.
pub(crate) struct BiRefNetModel {
    matrices: HashMap<String, MatrixWeight>,
    floats: HashMap<String, FloatWeight>,
    indices: HashMap<String, Vec<i64>>,
}

impl BiRefNetModel {
    pub(crate) fn prepare(
        weights: &BiRefNetWeights,
        cancel: Option<BiRefNetCancel<'_>>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        crate::emit_progress(&mut progress, "load birefnet", 0.0)?;
        if !crate::backend::gpu_device_available() {
            return Err(DiffusionError::model(
                "native BiRefNet requires the Makepad CUDA backend",
            ));
        }

        let mut names: Vec<String> = weights.tensor_names().cloned().collect();
        names.sort();
        let total = names.len().max(1);
        let mut matrices = HashMap::new();
        let mut floats = HashMap::new();
        let mut indices = HashMap::new();
        for (index, name) in names.into_iter().enumerate() {
            if index % 32 == 0 {
                check_cancel(cancel)?;
                crate::emit_progress(
                    &mut progress,
                    "load birefnet checkpoint",
                    index as f64 / total as f64,
                )?;
            }
            let shape = weights.shape(&name)?;
            let dtype = weights.dtype(&name)?;
            if dtype == MlxDType::I64 {
                let values = weights.i64(&name)?;
                indices.insert(name, values);
                continue;
            }
            let is_linear = (shape.len() == 2
                && !name.ends_with("relative_position_bias_table")
                || name == "bb.patch_embed.proj.weight")
                && matches!(dtype, MlxDType::F16 | MlxDType::BF16);
            if is_linear {
                let ggml_type = match dtype {
                    MlxDType::F16 => GGML_TYPE_F16,
                    MlxDType::BF16 => GGML_TYPE_BF16,
                    _ => unreachable!(),
                };
                matrices.insert(
                    name.clone(),
                    MatrixWeight {
                        shape,
                        ggml_type,
                        bytes: weights.bytes(&name)?,
                    },
                );
            } else {
                floats.insert(
                    name.clone(),
                    FloatWeight {
                        shape,
                        values: weights.f32(&name)?,
                    },
                );
            }
        }

        let mut model = Self {
            matrices,
            floats,
            indices,
        };
        model.fold_decoder_batch_norms()?;
        check_cancel(cancel)?;
        crate::emit_progress(&mut progress, "load birefnet", 1.0)?;
        Ok(model)
    }

    pub(crate) fn matte(
        &self,
        image: BiRefNetImage<'_>,
        cancel: Option<BiRefNetCancel<'_>>,
        mut progress: Option<ProgressHook>,
    ) -> Result<BiRefNetMatte> {
        check_cancel(cancel)?;
        crate::emit_progress(&mut progress, "birefnet preprocess", 0.0)?;
        let input = preprocess_image(image, BIREFNET_INPUT_SIZE, BIREFNET_INPUT_SIZE);
        let image_gpu = gpu_upload(
            &input,
            3,
            BIREFNET_INPUT_SIZE * BIREFNET_INPUT_SIZE,
        )
        .map_err(|error| device_error("upload input", error))?;

        crate::emit_progress(&mut progress, "birefnet backbone 1024", 0.04)?;
        let full = self.backbone(
            &image_gpu,
            BIREFNET_INPUT_SIZE,
            cancel,
            &mut progress,
            0.04,
            0.42,
        )?;
        check_cancel(cancel)?;

        let half_image = gpu_birefnet_resize_bilinear(
            &image_gpu,
            BIREFNET_INPUT_SIZE,
            BIREFNET_INPUT_SIZE,
            BIREFNET_INPUT_SIZE / 2,
            BIREFNET_INPUT_SIZE / 2,
            true,
        )
        .map_err(|error| device_error("resize half input", error))?;
        crate::emit_progress(&mut progress, "birefnet backbone 512", 0.46)?;
        let half = self.backbone(
            &half_image,
            BIREFNET_INPUT_SIZE / 2,
            cancel,
            &mut progress,
            0.46,
            0.30,
        )?;
        drop(half_image);
        check_cancel(cancel)?;

        crate::emit_progress(&mut progress, "birefnet decoder", 0.77)?;
        let logits = self.decode(&image_gpu, full, half, cancel, &mut progress)?;
        check_cancel(cancel)?;
        crate::emit_progress(&mut progress, "birefnet read matte", 0.98)?;
        let logits = gpu_download(&logits)
            .map_err(|error| device_error("download matte", error))?;
        let mut alpha: Vec<f32> = logits
            .into_iter()
            .map(|value| {
                let value = if value >= 0.0 {
                    1.0 / (1.0 + (-value).exp())
                } else {
                    let exp = value.exp();
                    exp / (1.0 + exp)
                };
                value.clamp(0.0, 1.0)
            })
            .collect();
        if image.width != BIREFNET_INPUT_SIZE || image.height != BIREFNET_INPUT_SIZE {
            alpha = resize_plane_align_corners(
                &alpha,
                BIREFNET_INPUT_SIZE,
                BIREFNET_INPUT_SIZE,
                image.width,
                image.height,
            );
        }
        crate::emit_progress(&mut progress, "birefnet matte", 1.0)?;
        BiRefNetMatte::new(image.width, image.height, alpha)
    }

    fn float(&self, name: &str) -> Result<&FloatWeight> {
        self.floats.get(name).ok_or_else(|| {
            DiffusionError::model(format!("birefnet floating tensor {name} is missing"))
        })
    }

    fn values(&self, name: &str) -> Result<&[f32]> {
        Ok(&self.float(name)?.values)
    }

    fn matrix(&self, name: &str) -> Result<&MatrixWeight> {
        self.matrices.get(name).ok_or_else(|| {
            DiffusionError::model(format!("birefnet matrix tensor {name} is missing"))
        })
    }

    fn linear(&self, input: &GpuTensor, prefix: &str) -> Result<GpuTensor> {
        let weight_name = format!("{prefix}.weight");
        let weight = self.matrix(&weight_name)?;
        if weight.shape.len() != 2
            || weight.shape[1] != input.cols()
            || weight.shape[0] == 0
        {
            return Err(DiffusionError::model(format!(
                "birefnet linear {prefix} shape {:?} cannot consume {}x{}",
                weight.shape,
                input.rows(),
                input.cols()
            )));
        }
        let bias_name = format!("{prefix}.bias");
        let bias = self
            .floats
            .get(&bias_name)
            .map(|weight| weight.values.as_slice())
            .unwrap_or(&[]);
        let part = GpuLinearPart {
            bt_ggml_type: weight.ggml_type,
            n: weight.shape[0],
            cache_key: &weight_name,
            bytes: &weight.bytes,
        };
        gpu_linear_nt_cached(input, BIREFNET_CACHE_NAMESPACE, &[part], bias)
            .map_err(|error| device_error(prefix, error))
    }

    fn layer_norm(&self, input: &GpuTensor, prefix: &str) -> Result<GpuTensor> {
        gpu_layer_norm_pytorch(
            input,
            self.values(&format!("{prefix}.weight"))?,
            self.values(&format!("{prefix}.bias"))?,
            BIREFNET_LN_EPS,
        )
        .map_err(|error| device_error(prefix, error))
    }

    fn conv(
        &self,
        input: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
        pad: usize,
    ) -> Result<GpuTensor> {
        let weight_name = format!("{prefix}.weight");
        let weight = self.float(&weight_name)?;
        if weight.shape.len() != 4 || weight.shape[1] != input.rows() {
            return Err(DiffusionError::model(format!(
                "birefnet conv {prefix} shape {:?} cannot consume {}x{}",
                weight.shape,
                input.rows(),
                input.cols()
            )));
        }
        let bias_name = format!("{prefix}.bias");
        let bias = self.values(&bias_name)?;
        gpu_conv2d_planar_cached(
            input,
            width,
            height,
            BIREFNET_CACHE_NAMESPACE,
            &weight_name,
            &weight.values,
            bias,
            weight.shape[0],
            weight.shape[3],
            weight.shape[2],
            pad,
            pad,
        )
        .map_err(|error| device_error(prefix, error))
    }

    fn backbone(
        &self,
        image: &GpuTensor,
        size: usize,
        cancel: Option<BiRefNetCancel<'_>>,
        progress: &mut Option<ProgressHook>,
        progress_base: f64,
        progress_span: f64,
    ) -> Result<BackboneOutput> {
        check_cancel(cancel)?;
        let patch_size = size / 4;
        let mut hidden = self.patch_embed(image, size, patch_size)?;
        hidden = self.layer_norm(&hidden, "bb.patch_embed.norm")?;

        let total_blocks: usize = BIREFNET_DEPTHS.iter().sum();
        let mut completed_blocks = 0usize;
        let mut width = patch_size;
        let mut height = patch_size;
        let mut stages = Vec::with_capacity(4);
        for stage in 0..4 {
            for block in 0..BIREFNET_DEPTHS[stage] {
                check_cancel(cancel)?;
                let prefix = format!("bb.layers.{stage}.blocks.{block}");
                hidden = self.swin_block(
                    hidden,
                    width,
                    height,
                    BIREFNET_HEADS[stage],
                    block % 2 == 1,
                    &prefix,
                )?;
                completed_blocks += 1;
                crate::emit_progress(
                    progress,
                    "birefnet swin",
                    progress_base
                        + progress_span * completed_blocks as f64 / total_blocks as f64,
                )?;
            }

            let stage_norm = self.layer_norm(&hidden, &format!("bb.norm{stage}"))?;
            let planar = gpu_birefnet_tokens_to_planar(&stage_norm)
                .map_err(|error| device_error("stage tokens to planar", error))?;
            drop(stage_norm);
            stages.push(Feature {
                tensor: planar,
                width,
                height,
            });

            if stage < 3 {
                hidden = self.patch_merge(hidden, width, height, stage)?;
                width /= 2;
                height /= 2;
            }
        }
        Ok(BackboneOutput { stages })
    }

    /// Unfold the exact 4x4/stride-4 patch grid into token rows and apply the
    /// checkpoint's Conv2d weights as one linear.  This avoids evaluating the
    /// other 15 stride phases (roughly ten billion unnecessary FMAs at 1024).
    fn patch_embed(
        &self,
        image: &GpuTensor,
        image_size: usize,
        patch_size: usize,
    ) -> Result<GpuTensor> {
        let mut columns = Vec::with_capacity(3 * 4 * 4);
        for channel in 0..3 {
            let channel_plane = gpu_slice_rows(image, channel, 1)
                .map_err(|error| device_error("patch channel", error))?;
            for ky in 0..4 {
                for kx in 0..4 {
                    let mut indices = Vec::with_capacity(patch_size * patch_size);
                    for y in 0..patch_size {
                        for x in 0..patch_size {
                            indices.push(((4 * y + ky) * image_size + 4 * x + kx) as u32);
                        }
                    }
                    let planar = gpu_gather_cols(&channel_plane, &indices)
                        .map_err(|error| device_error("patch unfold", error))?;
                    columns.push(
                        gpu_birefnet_tokens_to_planar(&planar)
                            .map_err(|error| device_error("patch column", error))?,
                    );
                }
            }
        }
        let refs: Vec<&GpuTensor> = columns.iter().collect();
        let patches = gpu_concat_cols(&refs)
            .map_err(|error| device_error("patch concat", error))?;
        drop(columns);
        let name = "bb.patch_embed.proj.weight";
        let weight = self.matrix(name)?;
        if weight.shape != [192, 3, 4, 4] {
            return Err(DiffusionError::model(format!(
                "birefnet patch weight shape {:?}",
                weight.shape
            )));
        }
        let part = GpuLinearPart {
            bt_ggml_type: weight.ggml_type,
            n: weight.shape[0],
            cache_key: name,
            bytes: &weight.bytes,
        };
        gpu_linear_nt_cached(
            &patches,
            BIREFNET_CACHE_NAMESPACE,
            &[part],
            self.values("bb.patch_embed.proj.bias")?,
        )
        .map_err(|error| device_error("patch projection", error))
    }

    fn swin_block(
        &self,
        hidden: GpuTensor,
        width: usize,
        height: usize,
        heads: usize,
        shifted: bool,
        prefix: &str,
    ) -> Result<GpuTensor> {
        let channels = hidden.cols();
        let shortcut = hidden;
        let normalized = self.layer_norm(&shortcut, &format!("{prefix}.norm1"))?;
        let indices = build_window_indices(width, height, shifted);
        let gather_gpu = gpu_upload_u32(&indices.gather)
            .map_err(|error| device_error("window gather indices", error))?;
        let windows = gpu_gather_rows_colblock(&normalized, &gather_gpu, None, channels)
            .map_err(|error| device_error("window partition", error))?;
        drop(gather_gpu);
        drop(normalized);

        let qkv = self.linear(&windows, &format!("{prefix}.attn.qkv"))?;
        drop(windows);
        let q = gpu_slice_cols(&qkv, 0, channels)
            .map_err(|error| device_error("swin q", error))?;
        let k = gpu_slice_cols(&qkv, channels, channels)
            .map_err(|error| device_error("swin k", error))?;
        let v = gpu_slice_cols(&qkv, 2 * channels, channels)
            .map_err(|error| device_error("swin v", error))?;
        drop(qkv);

        let relative_bias = self.relative_position_bias(prefix, heads)?;
        let regions_gpu = indices
            .regions
            .as_ref()
            .map(|regions| gpu_upload_u32(regions))
            .transpose()
            .map_err(|error| device_error("window regions", error))?;
        let attended = gpu_birefnet_swin_attention(
            &q,
            &k,
            &v,
            BIREFNET_CACHE_NAMESPACE,
            &format!("{prefix}.attn"),
            &relative_bias,
            regions_gpu.as_ref(),
            indices.windows,
            heads,
            BIREFNET_WINDOW_SIZE * BIREFNET_WINDOW_SIZE,
        )
        .map_err(|error| device_error("swin attention", error))?;
        drop(q);
        drop(k);
        drop(v);
        drop(regions_gpu);
        let projected = self.linear(&attended, &format!("{prefix}.attn.proj"))?;
        drop(attended);
        let scatter_gpu = gpu_upload_u32(&indices.scatter)
            .map_err(|error| device_error("window scatter indices", error))?;
        let projected = gpu_gather_rows_colblock(&projected, &scatter_gpu, None, channels)
            .map_err(|error| device_error("window reverse", error))?;
        drop(scatter_gpu);
        let residual = gpu_add(&shortcut, &projected)
            .map_err(|error| device_error("swin attention residual", error))?;
        drop(shortcut);
        drop(projected);

        let mlp_input = self.layer_norm(&residual, &format!("{prefix}.norm2"))?;
        let mlp = self.linear(&mlp_input, &format!("{prefix}.mlp.fc1"))?;
        drop(mlp_input);
        let mlp = gpu_gelu_erf(&mlp).map_err(|error| device_error("swin gelu", error))?;
        let mlp = self.linear(&mlp, &format!("{prefix}.mlp.fc2"))?;
        gpu_add(&residual, &mlp).map_err(|error| device_error("swin mlp residual", error))
    }

    fn patch_merge(
        &self,
        hidden: GpuTensor,
        width: usize,
        height: usize,
        stage: usize,
    ) -> Result<GpuTensor> {
        let channels = hidden.cols();
        let out_width = width / 2;
        let out_height = height / 2;
        let mut gathered = Vec::with_capacity(4);
        for (dy, dx) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
            let mut indices = Vec::with_capacity(out_width * out_height);
            for y in 0..out_height {
                for x in 0..out_width {
                    indices.push(((2 * y + dy) * width + 2 * x + dx) as u32);
                }
            }
            let indices = gpu_upload_u32(&indices)
                .map_err(|error| device_error("patch merge indices", error))?;
            let part = gpu_gather_rows_colblock(&hidden, &indices, None, channels)
                .map_err(|error| device_error("patch merge gather", error))?;
            gathered.push(part);
        }
        drop(hidden);
        let refs: Vec<&GpuTensor> = gathered.iter().collect();
        let merged = gpu_concat_cols(&refs)
            .map_err(|error| device_error("patch merge concat", error))?;
        drop(gathered);
        let prefix = format!("bb.layers.{stage}.downsample");
        let merged = self.layer_norm(&merged, &format!("{prefix}.norm"))?;
        self.linear(&merged, &format!("{prefix}.reduction"))
    }

    fn relative_position_bias(&self, prefix: &str, heads: usize) -> Result<Vec<f32>> {
        let table = self.values(&format!(
            "{prefix}.attn.relative_position_bias_table"
        ))?;
        let indices = self
            .indices
            .get(&format!("{prefix}.attn.relative_position_index"))
            .ok_or_else(|| {
                DiffusionError::model(format!(
                    "birefnet relative-position indices missing for {prefix}"
                ))
            })?;
        let tokens = BIREFNET_WINDOW_SIZE * BIREFNET_WINDOW_SIZE;
        if indices.len() != tokens * tokens || table.len() % heads != 0 {
            return Err(DiffusionError::model(format!(
                "birefnet malformed relative-position tensors for {prefix}"
            )));
        }
        let table_rows = table.len() / heads;
        let mut bias = vec![0.0f32; heads * tokens * tokens];
        for head in 0..heads {
            for query in 0..tokens {
                for key in 0..tokens {
                    let row = usize::try_from(indices[query * tokens + key]).map_err(|_| {
                        DiffusionError::model("negative birefnet relative-position index")
                    })?;
                    if row >= table_rows {
                        return Err(DiffusionError::model(
                            "birefnet relative-position index out of range",
                        ));
                    }
                    bias[(head * tokens + query) * tokens + key] = table[row * heads + head];
                }
            }
        }
        Ok(bias)
    }

    fn decode(
        &self,
        image: &GpuTensor,
        full: BackboneOutput,
        half: BackboneOutput,
        cancel: Option<BiRefNetCancel<'_>>,
        progress: &mut Option<ProgressHook>,
    ) -> Result<GpuTensor> {
        if full.stages.len() != 4 || half.stages.len() != 4 {
            return Err(DiffusionError::model(
                "birefnet backbone did not produce four stages",
            ));
        }
        let mut stages = Vec::with_capacity(4);
        for (full, half) in full.stages.into_iter().zip(half.stages) {
            let half = gpu_birefnet_resize_bilinear(
                &half.tensor,
                half.width,
                half.height,
                full.width,
                full.height,
                true,
            )
            .map_err(|error| device_error("resize half feature", error))?;
            let tensor = gpu_concat_rows(&full.tensor, &half)
                .map_err(|error| device_error("multi-scale feature concat", error))?;
            stages.push(Feature {
                tensor,
                width: full.width,
                height: full.height,
            });
        }
        check_cancel(cancel)?;

        // Context concatenation onto x4: resized x1/x2/x3 followed by x4.
        let x4_width = stages[3].width;
        let x4_height = stages[3].height;
        let mut context_parts = Vec::with_capacity(4);
        for stage in stages.iter().take(3) {
            context_parts.push(
                gpu_birefnet_resize_bilinear(
                    &stage.tensor,
                    stage.width,
                    stage.height,
                    x4_width,
                    x4_height,
                    true,
                )
                .map_err(|error| device_error("context resize", error))?,
            );
        }
        let mut x4_context = context_parts.remove(0);
        for part in context_parts {
            x4_context = gpu_concat_rows(&x4_context, &part)
                .map_err(|error| device_error("context concat", error))?;
        }
        x4_context = gpu_concat_rows(&x4_context, &stages[3].tensor)
            .map_err(|error| device_error("context x4 concat", error))?;
        let mut x4 = self.basic_decoder_block(
            &x4_context,
            x4_width,
            x4_height,
            "squeeze_module.0",
        )?;
        drop(x4_context);
        crate::emit_progress(progress, "birefnet squeeze", 0.80)?;
        check_cancel(cancel)?;

        // Decoder block 4.
        let ipt5 = self.input_patch_branch(image, x4_width, x4_height, "decoder.ipt_blk5")?;
        x4 = gpu_concat_rows(&x4, &ipt5)
            .map_err(|error| device_error("decoder ipt5 concat", error))?;
        let mut p4 = self.basic_decoder_block(
            &x4,
            x4_width,
            x4_height,
            "decoder.decoder_block4",
        )?;
        p4 = self.gradient_attention(&p4, x4_width, x4_height, 4)?;
        crate::emit_progress(progress, "birefnet decoder 4", 0.84)?;
        check_cancel(cancel)?;

        // Decoder block 3.
        let x3 = &stages[2];
        let up4 = gpu_birefnet_resize_bilinear(
            &p4,
            x4_width,
            x4_height,
            x3.width,
            x3.height,
            true,
        )
        .map_err(|error| device_error("decoder up4", error))?;
        let lateral4 = self.conv(
            &x3.tensor,
            x3.width,
            x3.height,
            "decoder.lateral_block4.conv",
            0,
        )?;
        let mut p3_input = gpu_add(&up4, &lateral4)
            .map_err(|error| device_error("decoder lateral4", error))?;
        let ipt4 = self.input_patch_branch(image, x3.width, x3.height, "decoder.ipt_blk4")?;
        p3_input = gpu_concat_rows(&p3_input, &ipt4)
            .map_err(|error| device_error("decoder ipt4 concat", error))?;
        let mut p3 = self.basic_decoder_block(
            &p3_input,
            x3.width,
            x3.height,
            "decoder.decoder_block3",
        )?;
        p3 = self.gradient_attention(&p3, x3.width, x3.height, 3)?;
        crate::emit_progress(progress, "birefnet decoder 3", 0.88)?;
        check_cancel(cancel)?;

        // Decoder block 2.
        let x2 = &stages[1];
        let up3 = gpu_birefnet_resize_bilinear(
            &p3,
            x3.width,
            x3.height,
            x2.width,
            x2.height,
            true,
        )
        .map_err(|error| device_error("decoder up3", error))?;
        let lateral3 = self.conv(
            &x2.tensor,
            x2.width,
            x2.height,
            "decoder.lateral_block3.conv",
            0,
        )?;
        let mut p2_input = gpu_add(&up3, &lateral3)
            .map_err(|error| device_error("decoder lateral3", error))?;
        let ipt3 = self.input_patch_branch(image, x2.width, x2.height, "decoder.ipt_blk3")?;
        p2_input = gpu_concat_rows(&p2_input, &ipt3)
            .map_err(|error| device_error("decoder ipt3 concat", error))?;
        let mut p2 = self.basic_decoder_block(
            &p2_input,
            x2.width,
            x2.height,
            "decoder.decoder_block2",
        )?;
        p2 = self.gradient_attention(&p2, x2.width, x2.height, 2)?;
        crate::emit_progress(progress, "birefnet decoder 2", 0.92)?;
        check_cancel(cancel)?;

        // Decoder block 1 and full-resolution head.
        let x1 = &stages[0];
        let up2 = gpu_birefnet_resize_bilinear(
            &p2,
            x2.width,
            x2.height,
            x1.width,
            x1.height,
            true,
        )
        .map_err(|error| device_error("decoder up2", error))?;
        let lateral2 = self.conv(
            &x1.tensor,
            x1.width,
            x1.height,
            "decoder.lateral_block2.conv",
            0,
        )?;
        let mut p1_input = gpu_add(&up2, &lateral2)
            .map_err(|error| device_error("decoder lateral2", error))?;
        let ipt2 = self.input_patch_branch(image, x1.width, x1.height, "decoder.ipt_blk2")?;
        p1_input = gpu_concat_rows(&p1_input, &ipt2)
            .map_err(|error| device_error("decoder ipt2 concat", error))?;
        let p1 = self.basic_decoder_block(
            &p1_input,
            x1.width,
            x1.height,
            "decoder.decoder_block1",
        )?;
        let p1 = gpu_birefnet_resize_bilinear(
            &p1,
            x1.width,
            x1.height,
            BIREFNET_INPUT_SIZE,
            BIREFNET_INPUT_SIZE,
            true,
        )
        .map_err(|error| device_error("decoder up1", error))?;
        let ipt1 = self.input_patch_branch(
            image,
            BIREFNET_INPUT_SIZE,
            BIREFNET_INPUT_SIZE,
            "decoder.ipt_blk1",
        )?;
        let p1 = gpu_concat_rows(&p1, &ipt1)
            .map_err(|error| device_error("decoder ipt1 concat", error))?;
        crate::emit_progress(progress, "birefnet output", 0.97)?;
        self.conv(
            &p1,
            BIREFNET_INPUT_SIZE,
            BIREFNET_INPUT_SIZE,
            "decoder.conv_out1.0",
            0,
        )
    }

    fn input_patch_branch(
        &self,
        image: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
    ) -> Result<GpuTensor> {
        let patches = gpu_birefnet_image_to_patches(
            image,
            BIREFNET_INPUT_SIZE,
            BIREFNET_INPUT_SIZE,
            width,
            height,
        )
        .map_err(|error| device_error("image patches", error))?;
        let hidden = self.conv(&patches, width, height, &format!("{prefix}.conv1"), 1)?;
        self.conv(
            &hidden,
            width,
            height,
            &format!("{prefix}.conv_out"),
            1,
        )
    }

    fn basic_decoder_block(
        &self,
        input: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
    ) -> Result<GpuTensor> {
        let hidden = self.conv(input, width, height, &format!("{prefix}.conv_in"), 1)?;
        let hidden = gpu_birefnet_relu(&hidden)
            .map_err(|error| device_error("decoder relu in", error))?;
        let hidden = self.aspp_deformable(
            &hidden,
            width,
            height,
            &format!("{prefix}.dec_att"),
        )?;
        self.conv(&hidden, width, height, &format!("{prefix}.conv_out"), 1)
    }

    fn aspp_deformable(
        &self,
        input: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
    ) -> Result<GpuTensor> {
        let mut branches = Vec::with_capacity(5);
        branches.push(self.deform_branch(
            input,
            width,
            height,
            &format!("{prefix}.aspp1"),
            1,
        )?);
        for (index, kernel) in [1usize, 3, 7].into_iter().enumerate() {
            branches.push(self.deform_branch(
                input,
                width,
                height,
                &format!("{prefix}.aspp_deforms.{index}"),
                kernel,
            )?);
        }
        let pooled = gpu_birefnet_global_avg_pool(input)
            .map_err(|error| device_error("aspp global pool", error))?;
        let pooled = self.conv(
            &pooled,
            1,
            1,
            &format!("{prefix}.global_avg_pool.1"),
            0,
        )?;
        let pooled = gpu_birefnet_relu(&pooled)
            .map_err(|error| device_error("aspp global relu", error))?;
        branches.push(
            gpu_birefnet_broadcast(&pooled, width * height)
                .map_err(|error| device_error("aspp global broadcast", error))?,
        );
        let mut joined = branches.remove(0);
        for branch in branches {
            joined = gpu_concat_rows(&joined, &branch)
                .map_err(|error| device_error("aspp concat", error))?;
        }
        let output = self.conv(&joined, width, height, &format!("{prefix}.conv1"), 0)?;
        gpu_birefnet_relu(&output).map_err(|error| device_error("aspp output relu", error))
    }

    fn deform_branch(
        &self,
        input: &GpuTensor,
        width: usize,
        height: usize,
        prefix: &str,
        kernel: usize,
    ) -> Result<GpuTensor> {
        let deform = format!("{prefix}.atrous_conv");
        let offset = self.conv(
            input,
            width,
            height,
            &format!("{deform}.offset_conv"),
            kernel / 2,
        )?;
        let modulator = self.conv(
            input,
            width,
            height,
            &format!("{deform}.modulator_conv"),
            kernel / 2,
        )?;
        let weight_name = format!("{deform}.regular_conv.weight");
        let weight = self.float(&weight_name)?;
        let bias_name = format!("{deform}.regular_conv.bias");
        let bias = self.values(&bias_name)?;
        let output = gpu_birefnet_deform_conv2d_cached(
            input,
            &offset,
            &modulator,
            width,
            height,
            BIREFNET_CACHE_NAMESPACE,
            &weight_name,
            &weight.values,
            bias,
            weight.shape[0],
            kernel,
        )
        .map_err(|error| device_error("deform convolution", error))?;
        gpu_birefnet_relu(&output).map_err(|error| device_error("deform relu", error))
    }

    fn gradient_attention(
        &self,
        input: &GpuTensor,
        width: usize,
        height: usize,
        level: usize,
    ) -> Result<GpuTensor> {
        let prefix = format!("decoder.gdt_convs_{level}");
        let hidden = self.conv(input, width, height, &format!("{prefix}.0"), 1)?;
        let hidden = gpu_birefnet_relu(&hidden)
            .map_err(|error| device_error("gradient relu", error))?;
        let logits = self.conv(
            &hidden,
            width,
            height,
            &format!("decoder.gdt_convs_attn_{level}.0"),
            0,
        )?;
        gpu_birefnet_mul_sigmoid_mask(input, &logits)
            .map_err(|error| device_error("gradient attention", error))
    }

    /// Fold the decoder's inference BatchNorm layers into the immediately
    /// preceding convolution.  This is algebraically exact and mirrors the
    /// published GGML implementation while removing dozens of full feature-
    /// plane affine passes from every request.
    fn fold_decoder_batch_norms(&mut self) -> Result<()> {
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut add_block = |prefix: String| {
            pairs.push((format!("{prefix}.conv_in"), format!("{prefix}.bn_in")));
            pairs.push((format!("{prefix}.conv_out"), format!("{prefix}.bn_out")));
            let attention = format!("{prefix}.dec_att");
            pairs.push((
                format!("{attention}.aspp1.atrous_conv.regular_conv"),
                format!("{attention}.aspp1.bn"),
            ));
            for index in 0..3 {
                pairs.push((
                    format!(
                        "{attention}.aspp_deforms.{index}.atrous_conv.regular_conv"
                    ),
                    format!("{attention}.aspp_deforms.{index}.bn"),
                ));
            }
            pairs.push((
                format!("{attention}.global_avg_pool.1"),
                format!("{attention}.global_avg_pool.2"),
            ));
            pairs.push((
                format!("{attention}.conv1"),
                format!("{attention}.bn1"),
            ));
        };
        add_block("squeeze_module.0".to_string());
        for level in 1..=4 {
            add_block(format!("decoder.decoder_block{level}"));
        }
        for level in 2..=4 {
            pairs.push((
                format!("decoder.gdt_convs_{level}.0"),
                format!("decoder.gdt_convs_{level}.1"),
            ));
        }
        for (conv, batch_norm) in pairs {
            self.fold_batch_norm(&conv, &batch_norm)?;
        }
        Ok(())
    }

    fn fold_batch_norm(&mut self, conv: &str, batch_norm: &str) -> Result<()> {
        let gamma = self.values(&format!("{batch_norm}.weight"))?.to_vec();
        let beta = self.values(&format!("{batch_norm}.bias"))?.to_vec();
        let mean = self
            .values(&format!("{batch_norm}.running_mean"))?
            .to_vec();
        let variance = self
            .values(&format!("{batch_norm}.running_var"))?
            .to_vec();
        let channels = gamma.len();
        if beta.len() != channels || mean.len() != channels || variance.len() != channels {
            return Err(DiffusionError::model(format!(
                "birefnet BatchNorm {batch_norm} vector length mismatch"
            )));
        }
        let weight_name = format!("{conv}.weight");
        let bias_name = format!("{conv}.bias");
        let mut bias = self
            .floats
            .get(&bias_name)
            .map(|weight| weight.values.clone())
            .unwrap_or_else(|| vec![0.0; channels]);
        let weight = self.floats.get_mut(&weight_name).ok_or_else(|| {
            DiffusionError::model(format!("birefnet convolution {weight_name} missing"))
        })?;
        if weight.shape.len() != 4 || weight.shape[0] != channels {
            return Err(DiffusionError::model(format!(
                "birefnet BatchNorm {batch_norm} does not match {:?}",
                weight.shape
            )));
        }
        let per_channel = weight.values.len() / channels;
        for channel in 0..channels {
            let scale = gamma[channel] / (variance[channel] + BIREFNET_BN_EPS).sqrt();
            for value in &mut weight.values
                [channel * per_channel..(channel + 1) * per_channel]
            {
                *value *= scale;
            }
            bias[channel] = bias[channel] * scale + beta[channel] - mean[channel] * scale;
        }
        self.floats.insert(
            bias_name,
            FloatWeight {
                shape: vec![channels],
                values: bias,
            },
        );
        Ok(())
    }
}

fn build_window_indices(width: usize, height: usize, shifted: bool) -> WindowIndices {
    let window = BIREFNET_WINDOW_SIZE;
    let shift = if shifted { window / 2 } else { 0 };
    let padded_width = width.div_ceil(window) * window;
    let padded_height = height.div_ceil(window) * window;
    let windows_w = padded_width / window;
    let windows_h = padded_height / window;
    let windows = windows_w * windows_h;
    let tokens = window * window;
    let mut gather = Vec::with_capacity(windows * tokens);
    let mut regions = shifted.then(|| Vec::with_capacity(windows * tokens));
    for window_index in 0..windows {
        let window_y = window_index / windows_w;
        let window_x = window_index % windows_w;
        for position in 0..tokens {
            let local_y = position / window;
            let local_x = position % window;
            let grid_y = window_y * window + local_y;
            let grid_x = window_x * window + local_x;
            let source_y = (grid_y + shift) % padded_height;
            let source_x = (grid_x + shift) % padded_width;
            gather.push(if source_y < height && source_x < width {
                (source_y * width + source_x) as u32
            } else {
                u32::MAX
            });
            if let Some(regions) = &mut regions {
                let region = |coordinate: usize, length: usize| {
                    if coordinate < length - window {
                        0u32
                    } else if coordinate < length - shift {
                        1u32
                    } else {
                        2u32
                    }
                };
                regions.push(region(grid_y, padded_height) * 3 + region(grid_x, padded_width));
            }
        }
    }
    let mut scatter = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let shifted_y = (y + padded_height - shift) % padded_height;
            let shifted_x = (x + padded_width - shift) % padded_width;
            let window_y = shifted_y / window;
            let window_x = shifted_x / window;
            let local_y = shifted_y % window;
            let local_x = shifted_x % window;
            scatter.push(
                ((window_y * windows_w + window_x) * tokens + local_y * window + local_x)
                    as u32,
            );
        }
    }
    WindowIndices {
        gather,
        scatter,
        regions,
        windows,
    }
}

fn preprocess_image(image: BiRefNetImage<'_>, out_width: usize, out_height: usize) -> Vec<f32> {
    let plane = out_width * out_height;
    let mut output = vec![0.0f32; 3 * plane];
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];
    for out_y in 0..out_height {
        let source_y = ((out_y as f32 + 0.5) * image.height as f32 / out_height as f32
            - 0.5)
            .clamp(0.0, (image.height - 1) as f32);
        let y0 = source_y.floor() as usize;
        let y1 = (y0 + 1).min(image.height - 1);
        let fy = source_y - y0 as f32;
        for out_x in 0..out_width {
            let source_x = ((out_x as f32 + 0.5) * image.width as f32 / out_width as f32
                - 0.5)
                .clamp(0.0, (image.width - 1) as f32);
            let x0 = source_x.floor() as usize;
            let x1 = (x0 + 1).min(image.width - 1);
            let fx = source_x - x0 as f32;
            for channel in 0..3 {
                let sample = |x: usize, y: usize| {
                    image.pixels[(y * image.width + x) * image.channels + channel] as f32
                        / 255.0
                };
                let top = sample(x0, y0) * (1.0 - fx) + sample(x1, y0) * fx;
                let bottom = sample(x0, y1) * (1.0 - fx) + sample(x1, y1) * fx;
                let value = top * (1.0 - fy) + bottom * fy;
                output[channel * plane + out_y * out_width + out_x] =
                    (value - mean[channel]) / std[channel];
            }
        }
    }
    output
}

fn resize_plane_align_corners(
    input: &[f32],
    in_width: usize,
    in_height: usize,
    out_width: usize,
    out_height: usize,
) -> Vec<f32> {
    let mut output = vec![0.0; out_width * out_height];
    let scale_y = if out_height > 1 {
        (in_height - 1) as f32 / (out_height - 1) as f32
    } else {
        0.0
    };
    let scale_x = if out_width > 1 {
        (in_width - 1) as f32 / (out_width - 1) as f32
    } else {
        0.0
    };
    for y in 0..out_height {
        let fy = y as f32 * scale_y;
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(in_height - 1);
        let wy = fy - y0 as f32;
        for x in 0..out_width {
            let fx = x as f32 * scale_x;
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(in_width - 1);
            let wx = fx - x0 as f32;
            let top = input[y0 * in_width + x0] * (1.0 - wx)
                + input[y0 * in_width + x1] * wx;
            let bottom = input[y1 * in_width + x0] * (1.0 - wx)
                + input[y1 * in_width + x1] * wx;
            output[y * out_width + x] = top * (1.0 - wy) + bottom * wy;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_indices_cover_each_real_token_once() {
        for &(width, height) in &[(256, 256), (128, 128), (64, 64), (32, 32), (16, 16)] {
            for shifted in [false, true] {
                let indices = build_window_indices(width, height, shifted);
                let mut seen = vec![0usize; width * height];
                for &index in &indices.gather {
                    if index != u32::MAX {
                        seen[index as usize] += 1;
                    }
                }
                assert!(seen.iter().all(|&count| count == 1));
                assert_eq!(indices.scatter.len(), width * height);
                for (token, &window_row) in indices.scatter.iter().enumerate() {
                    assert_eq!(indices.gather[window_row as usize], token as u32);
                }
            }
        }
    }

    #[test]
    fn preprocess_ignores_existing_alpha() {
        let rgb = [255, 0, 0, 0, 255, 0];
        let rgba = [255, 0, 0, 0, 0, 255, 0, 255];
        let rgb = preprocess_image(BiRefNetImage::rgb8(&rgb, 2, 1).unwrap(), 2, 1);
        let rgba = preprocess_image(BiRefNetImage::rgba8(&rgba, 2, 1).unwrap(), 2, 1);
        assert_eq!(rgb, rgba);
    }

    #[test]
    fn align_corners_resize_keeps_endpoints() {
        let resized = resize_plane_align_corners(&[0.0, 1.0, 2.0, 3.0], 2, 2, 3, 3);
        assert_eq!(resized[0], 0.0);
        assert_eq!(resized[2], 1.0);
        assert_eq!(resized[6], 2.0);
        assert_eq!(resized[8], 3.0);
        assert_eq!(resized[4], 1.5);
    }
}
