//! Native SAM 3.1 Multiplex foundation (Comfy-Org FP16 checkpoint).
//!
//! The architecture realised here is implemented after the Apache-2.0
//! Hugging Face `transformers` SAM 3 implementation —
//! `src/transformers/models/sam3/{modeling,configuration,image_processing,processing}_sam3.py`,
//! "Copyright 2025 The Meta AI Authors and The HuggingFace Team". Runtime
//! numerics are matched against reference tensor dumps of the Comfy-Org
//! multiplex checkpoint pipeline this port targets. See
//! `../THIRD_PARTY_NOTICES.md`; our own code stays MIT with the crate.
//!
//! Weights are SAM-Licensed. The operator pulls them at runtime from the
//! Comfy-Org repack; this repository never redistributes them.
//!
//! Production identity is `Comfy-Org/sam3.1` `checkpoints/sam3.1_multiplex_fp16.safetensors`.
//! Facebook TOS checkpoints (`facebook/sam3*`) are never fetched or accepted.
//! The detector is a Perception-Encoder ViT-L/14 + CLIP-L text tower + 6-layer
//! fusion encoder + 200-query DETR decoder with a presence token. Tracker
//! weights are inventoried (SAM 3.1 multiplex, 16 object slots) but the
//! production domain is single-image PCS.
//!
//! Fail closed: wrong tensor names/shapes, missing prompt, missing GPU, or a
//! non-Comfy multiplex header all refuse before a generate.

use crate::{DiffusionError, ProgressHook, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

pub const SAM3_REPO: &str = "Comfy-Org/sam3.1";
pub const SAM3_REVISION: &str = "f38cd62b71494b53ac2b56ca36e24f3c8d565581";
pub const SAM3_MODEL_PATH: &str = "checkpoints/sam3.1_multiplex_fp16.safetensors";
pub const SAM3_MODEL_SIZE: u64 = 1_745_546_848;
pub const SAM3_MODEL_SHA256: &str =
    "9ba99c92703c2e8b4f47de2d34a539bb8e18923049e238b780d70dbe6368eb03";

pub const SAM3_INPUT_SIZE: usize = 1008;
pub const SAM3_PATCH: usize = 14;
pub const SAM3_VISION_DIM: usize = 1024;
pub const SAM3_VISION_DEPTH: usize = 32;
pub const SAM3_VISION_HEADS: usize = 16;
pub const SAM3_VISION_MLP: usize = 4736;
pub const SAM3_VISION_WINDOW: usize = 24;
pub const SAM3_POS_BASE: usize = 24;
pub const SAM3_TEXT_DIM: usize = 1024;
pub const SAM3_TEXT_DEPTH: usize = 24;
pub const SAM3_TEXT_HEADS: usize = 16;
pub const SAM3_TEXT_MLP: usize = 4096;
pub const SAM3_TEXT_CTX: usize = 32;
pub const SAM3_TEXT_VOCAB: usize = 49_408;
pub const SAM3_DETECTOR_DIM: usize = 256;
pub const SAM3_DETECTOR_HEADS: usize = 8;
pub const SAM3_FUSION_DEPTH: usize = 6;
pub const SAM3_DECODER_DEPTH: usize = 6;
pub const SAM3_NUM_QUERIES: usize = 200;
pub const SAM3_MULTIPLEX_SLOTS: usize = 16;
pub const SAM3_LN_EPS: f32 = 1e-5;
pub const SAM3_SCORE_THRESH: f32 = 0.5;
/// Interactive-head refinement passes per accepted detection. Two is the
/// value the reference dumps were produced at.
pub const SAM3_REFINE_ITERS: usize = 2;
pub const SAM3_CACHE_NAMESPACE: &str = "sam3-1-multiplex";

/// PE L++ windowed layers keep 24×24 windows; these four layers are global.
pub const SAM3_GLOBAL_LAYERS: [usize; 4] = [7, 15, 23, 31];

/// SAM3's CLIP tokenizer pads with 0, not with EOS. Confirmed against the
/// reference tokenizer dumps.
pub const SAM3_PAD_ID: i32 = 0;

pub type Sam3Cancel<'a> = &'a dyn Fn() -> bool;

fn f16_word_to_f32(word: u16) -> f32 {
    let sign = ((word >> 15) & 1) as u32;
    let exp = ((word >> 10) & 0x1f) as u32;
    let frac = (word & 0x3ff) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            let mut exp32 = 127 - 15 + 1;
            let mut frac32 = frac;
            while frac32 & 0x400 == 0 {
                frac32 <<= 1;
                exp32 -= 1;
            }
            frac32 &= 0x3ff;
            (sign << 31) | (exp32 << 23) | (frac32 << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xff << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

fn check_cancel(cancel: Option<Sam3Cancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

/// Multiplex phrase: `cat:1` means concept `cat`, at most one instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sam3Phrase {
    pub text: String,
    pub max_instances: Option<usize>,
}

/// Parsed SAM 3.1 multiplex prompt (`eye:2, window panels:4`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sam3Prompt {
    pub phrases: Vec<Sam3Phrase>,
}

impl Sam3Prompt {
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DiffusionError::workflow(
                "sam3 prompt is required (multiplex syntax, e.g. \"cat:1\")",
            ));
        }
        let mut phrases = Vec::new();
        for part in trimmed.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (text, max_instances) = match part.rsplit_once(':') {
                Some((text, count)) if !text.trim().is_empty() && count.bytes().all(|b| b.is_ascii_digit()) => {
                    let parsed = count.parse::<usize>().map_err(|_| {
                        DiffusionError::workflow(format!("sam3 prompt count is invalid: {part:?}"))
                    })?;
                    if parsed == 0 {
                        return Err(DiffusionError::workflow(format!(
                            "sam3 prompt count must be >= 1: {part:?}"
                        )));
                    }
                    (text.trim().to_string(), Some(parsed))
                }
                _ => (part.to_string(), None),
            };
            if text.chars().count() > 128 {
                return Err(DiffusionError::workflow(
                    "sam3 phrase is longer than 128 characters",
                ));
            }
            phrases.push(Sam3Phrase {
                text,
                max_instances,
            });
        }
        if phrases.is_empty() {
            return Err(DiffusionError::workflow("sam3 prompt has no phrases"));
        }
        if phrases.len() > SAM3_MULTIPLEX_SLOTS {
            return Err(DiffusionError::workflow(format!(
                "sam3 multiplex accepts at most {SAM3_MULTIPLEX_SLOTS} phrases"
            )));
        }
        Ok(Self { phrases })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sam3Image<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

impl<'a> Sam3Image<'a> {
    pub fn rgb8(pixels: &'a [u8], width: usize, height: usize) -> Result<Self> {
        Self::new(pixels, width, height, 3)
    }

    pub fn rgba8(pixels: &'a [u8], width: usize, height: usize) -> Result<Self> {
        Self::new(pixels, width, height, 4)
    }

    pub fn new(
        pixels: &'a [u8],
        width: usize,
        height: usize,
        channels: usize,
    ) -> Result<Self> {
        if width == 0 || height == 0 || !(channels == 3 || channels == 4) {
            return Err(DiffusionError::workflow(
                "sam3 image must be non-empty RGB8 or RGBA8",
            ));
        }
        let expected = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(channels))
            .ok_or_else(|| DiffusionError::workflow("sam3 image size overflow"))?;
        if pixels.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "sam3 image byte length {} != {width}*{height}*{channels}",
                pixels.len()
            )));
        }
        Ok(Self {
            pixels,
            width,
            height,
            channels,
        })
    }
}

/// Soft instance-union mask at the original input size. Values are finite
/// and clamped to 0..=1; no thresholding is applied to the mask itself.
#[derive(Clone, Debug)]
pub struct Sam3Mask {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<f32>,
    pub boxes_xyxy: Vec<[f32; 4]>,
    pub scores: Vec<f32>,
    pub phrases: Vec<String>,
    /// All 200 detector logits (pre-threshold), for 04-dump parity.
    pub raw_scores: Vec<f32>,
    pub raw_boxes_xyxy: Vec<[f32; 4]>,
}

impl Sam3Mask {
    pub fn new(
        width: usize,
        height: usize,
        alpha: Vec<f32>,
        boxes_xyxy: Vec<[f32; 4]>,
        scores: Vec<f32>,
        phrases: Vec<String>,
    ) -> Result<Self> {
        Self::with_raw(
            width,
            height,
            alpha,
            boxes_xyxy,
            scores,
            phrases,
            Vec::new(),
            Vec::new(),
        )
    }

    pub fn with_raw(
        width: usize,
        height: usize,
        alpha: Vec<f32>,
        boxes_xyxy: Vec<[f32; 4]>,
        scores: Vec<f32>,
        phrases: Vec<String>,
        raw_scores: Vec<f32>,
        raw_boxes_xyxy: Vec<[f32; 4]>,
    ) -> Result<Self> {
        if alpha.len() != width.saturating_mul(height)
            || alpha.iter().any(|value| !value.is_finite())
            || boxes_xyxy.len() != scores.len()
            || scores.len() != phrases.len()
            || scores.iter().any(|value| !value.is_finite())
            || raw_boxes_xyxy.len() != raw_scores.len()
            || raw_scores.iter().any(|value| !value.is_finite())
        {
            return Err(DiffusionError::model(
                "sam3 returned an invalid mask or detection list",
            ));
        }
        Ok(Self {
            width,
            height,
            alpha,
            boxes_xyxy,
            scores,
            phrases,
            raw_scores,
            raw_boxes_xyxy,
        })
    }

    pub fn alpha_u8(&self) -> Vec<u8> {
        self.alpha
            .iter()
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sam3ExecutionInfo {
    pub compute_backend: &'static str,
    pub weight_precision: &'static str,
    pub activation_precision: &'static str,
    pub accumulation_precision: &'static str,
}

pub const SAM3_EXECUTION: Sam3ExecutionInfo = Sam3ExecutionInfo {
    compute_backend: "makepad-ggml-cuda",
    weight_precision: "checkpoint f16, linears bf16, rest f32",
    activation_precision: "gemm operands bf16, rest f32",
    accumulation_precision: "f32",
};

/// Thin direct reader around the pinned Comfy-Org multiplex checkpoint.
pub struct Sam3Weights {
    pub path: PathBuf,
    header: MlxSafetensorsHeader,
}

impl Sam3Weights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let header = MlxSafetensorsHeader::load(&path).map_err(|err| {
            DiffusionError::model(format!("sam3 weights {}: {err:?}", path.display()))
        })?;
        let weights = Self { path, header };
        weights.validate_architecture()?;
        Ok(weights)
    }

    pub fn file_len(&self) -> u64 {
        self.header.file_len
    }

    pub fn has(&self, name: &str) -> bool {
        self.header.tensors.contains_key(name)
    }

    pub fn tensor_names(&self) -> impl Iterator<Item = &String> {
        self.header.tensors.keys()
    }

    pub fn shape(&self, name: &str) -> Result<Vec<usize>> {
        let entry = self.entry(name)?;
        entry
            .shape
            .iter()
            .map(|&value| {
                usize::try_from(value).map_err(|_| {
                    DiffusionError::model(format!(
                        "sam3 tensor {name} dimension {value} exceeds usize"
                    ))
                })
            })
            .collect()
    }

    pub fn dtype(&self, name: &str) -> Result<MlxDType> {
        Ok(self.entry(name)?.dtype)
    }

    pub fn bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.header.read_tensor_bytes(name).map_err(|err| {
            DiffusionError::model(format!("sam3 read tensor {name}: {err:?}"))
        })
    }

    pub fn f32(&self, name: &str) -> Result<Vec<f32>> {
        let dtype = self.dtype(name)?;
        let bytes = self.bytes(name)?;
        let values = match dtype {
            MlxDType::F16 => bytes
                .chunks_exact(2)
                .map(|chunk| {
                    f16_word_to_f32(u16::from_le_bytes([chunk[0], chunk[1]]))
                })
                .collect(),
            MlxDType::BF16 => bytes
                .chunks_exact(2)
                .map(|chunk| {
                    f32::from_bits(u32::from(u16::from_le_bytes([chunk[0], chunk[1]])) << 16)
                })
                .collect(),
            MlxDType::F32 => bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect(),
            other => {
                return Err(DiffusionError::model(format!(
                    "sam3 tensor {name} has unsupported floating dtype {other:?}"
                )))
            }
        };
        Ok(values)
    }

    pub fn f32_shaped(&self, name: &str, expected: &[usize]) -> Result<Vec<f32>> {
        let shape = self.shape(name)?;
        if shape != expected {
            return Err(DiffusionError::model(format!(
                "sam3 tensor {name} shape {shape:?}, expected {expected:?}"
            )));
        }
        self.f32(name)
    }

    fn entry(&self, name: &str) -> Result<&makepad_ai_loader::MlxTensorEntry> {
        self.header.tensors.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "sam3 tensor {name} missing from {}",
                self.path.display()
            ))
        })
    }

    fn expect(&self, name: &str, dtype: MlxDType, shape: &[usize]) -> Result<()> {
        let actual_dtype = self.dtype(name)?;
        let actual_shape = self.shape(name)?;
        if actual_dtype != dtype || actual_shape != shape {
            return Err(DiffusionError::model(format!(
                "sam3 tensor {name}: {actual_dtype:?} {actual_shape:?}, expected {dtype:?} {shape:?}"
            )));
        }
        Ok(())
    }

    /// Fail closed if this is not the Comfy-Org SAM 3.1 multiplex header.
    fn validate_architecture(&self) -> Result<()> {
        if self.has("model.sam_mask_decoder.mask_tokens.weight")
            && !self.has("detector.backbone.vision_backbone.trunk.blocks.0.attn.qkv.weight")
        {
            return Err(DiffusionError::model(
                "sam3 refused a facebook/sam3* style header; only Comfy-Org multiplex is accepted",
            ));
        }
        self.expect(
            "detector.backbone.vision_backbone.trunk.patch_embed.proj.weight",
            MlxDType::F16,
            &[SAM3_VISION_DIM, 3, SAM3_PATCH, SAM3_PATCH],
        )?;
        self.expect(
            "detector.backbone.vision_backbone.trunk.pos_embed",
            MlxDType::F16,
            &[1, 1 + SAM3_POS_BASE * SAM3_POS_BASE, SAM3_VISION_DIM],
        )?;
        self.expect(
            "detector.backbone.vision_backbone.trunk.blocks.0.attn.qkv.weight",
            MlxDType::F16,
            &[3 * SAM3_VISION_DIM, SAM3_VISION_DIM],
        )?;
        self.expect(
            "detector.backbone.vision_backbone.trunk.blocks.31.mlp.fc1.weight",
            MlxDType::F16,
            &[SAM3_VISION_MLP, SAM3_VISION_DIM],
        )?;
        self.expect(
            "detector.backbone.language_backbone.encoder.token_embedding.weight",
            MlxDType::F16,
            &[SAM3_TEXT_VOCAB, SAM3_TEXT_DIM],
        )?;
        self.expect(
            "detector.backbone.language_backbone.encoder.positional_embedding",
            MlxDType::F16,
            &[SAM3_TEXT_CTX, SAM3_TEXT_DIM],
        )?;
        self.expect(
            "detector.backbone.language_backbone.encoder.transformer.resblocks.23.mlp.c_fc.weight",
            MlxDType::F16,
            &[SAM3_TEXT_MLP, SAM3_TEXT_DIM],
        )?;
        self.expect(
            "detector.backbone.language_backbone.resizer.weight",
            MlxDType::F16,
            &[SAM3_DETECTOR_DIM, SAM3_TEXT_DIM],
        )?;
        self.expect(
            "detector.transformer.decoder.query_embed.weight",
            MlxDType::F16,
            &[SAM3_NUM_QUERIES, SAM3_DETECTOR_DIM],
        )?;
        self.expect(
            "detector.transformer.decoder.presence_token.weight",
            MlxDType::F16,
            &[1, SAM3_DETECTOR_DIM],
        )?;
        self.expect(
            "detector.transformer.decoder.layers.5.self_attn.in_proj_weight",
            MlxDType::F16,
            &[3 * SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM],
        )?;
        self.expect(
            "detector.transformer.encoder.layers.5.cross_attn_image.in_proj_weight",
            MlxDType::F16,
            &[3 * SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM],
        )?;
        self.expect(
            "detector.backbone.vision_backbone.propagation_convs.0.conv_1x1.weight",
            MlxDType::F16,
            &[SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM, 1, 1],
        )?;
        self.expect(
            "tracker.model.sam_mask_decoder.mask_tokens.weight",
            MlxDType::F16,
            &[SAM3_MULTIPLEX_SLOTS * 3, SAM3_DETECTOR_DIM],
        )?;
        self.expect(
            "detector.segmentation_head.semantic_seg_head.weight",
            MlxDType::F16,
            &[1, SAM3_DETECTOR_DIM, 1, 1],
        )?;
        for stage in 0..SAM3_VISION_DEPTH {
            let name = format!(
                "detector.backbone.vision_backbone.trunk.blocks.{stage}.attn.qkv.weight"
            );
            if !self.has(&name) {
                return Err(DiffusionError::model(format!(
                    "sam3 checkpoint is missing vision block {name}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Sam3Preprocessed {
    /// Planar RGB in [0, 1], bilinear-stretched to 1008×1008 (Comfy SAM3_Detect).
    pub pixels: Vec<f32>,
    pub width: usize,
    pub height: usize,
    pub src_width: usize,
    pub src_height: usize,
    /// Original image HWC RGB in [0, 1] for 05-refine crops.
    pub src_rgb: Vec<f32>,
}

/// Bilinear stretch HWC `[0,1]` RGB to planar NCHW `dst×dst` (align_corners=False).
pub fn stretch_hwc_to_planar(src_hwc: &[f32], src_w: usize, src_h: usize, dst: usize) -> Vec<f32> {
    let plane = dst * dst;
    let mut pixels = vec![0.0f32; 3 * plane];
    if src_w == 0 || src_h == 0 || dst == 0 {
        return pixels;
    }
    let scale_x = src_w as f32 / dst as f32;
    let scale_y = src_h as f32 / dst as f32;
    for y in 0..dst {
        let sy = (y as f32 + 0.5) * scale_y - 0.5;
        let y0 = sy.floor().clamp(0.0, (src_h - 1) as f32) as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let fy = (sy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..dst {
            let sx = (x as f32 + 0.5) * scale_x - 0.5;
            let x0 = sx.floor().clamp(0.0, (src_w - 1) as f32) as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let fx = (sx - x0 as f32).clamp(0.0, 1.0);
            let dst_i = y * dst + x;
            for c in 0..3 {
                let sample = |xx: usize, yy: usize| -> f32 { src_hwc[(yy * src_w + xx) * 3 + c] };
                let v00 = sample(x0, y0);
                let v10 = sample(x1, y0);
                let v01 = sample(x0, y1);
                let v11 = sample(x1, y1);
                pixels[c * plane + dst_i] = v00 * (1.0 - fx) * (1.0 - fy)
                    + v10 * fx * (1.0 - fy)
                    + v01 * (1.0 - fx) * fy
                    + v11 * fx * fy;
            }
        }
    }
    pixels
}

/// Bilinear stretch to 1008×1008, align_corners=False, no letterbox, no norm.
/// Matches the locked oracle tap `01_preprocessed_rgb_01_nchw.npy`.
///
/// NOTE "no norm": the tensor handed to the trunk is plain `[0, 1]` RGB. The
/// HF `Sam3ImageProcessor` and Meta's own transform both follow the resize
/// with `Normalize(mean=0.5, std=0.5)`; the target checkpoint pipeline — which
/// is what our oracle dumps come from — does not, and this port reproduces
/// the oracle. Anyone re-basing this on the HF processor must add the
/// normalization here and re-pin every downstream tap.
pub fn preprocess_image(image: Sam3Image<'_>) -> Result<Sam3Preprocessed> {
    let src_w = image.width;
    let src_h = image.height;
    let mut src_rgb = vec![0.0f32; src_w * src_h * 3];
    for y in 0..src_h {
        for x in 0..src_w {
            let src = (y * src_w + x) * image.channels;
            let dst = (y * src_w + x) * 3;
            src_rgb[dst] = image.pixels[src] as f32 / 255.0;
            src_rgb[dst + 1] = image.pixels[src + 1] as f32 / 255.0;
            src_rgb[dst + 2] = image.pixels[src + 2] as f32 / 255.0;
        }
    }
    let pixels = stretch_hwc_to_planar(&src_rgb, src_w, src_h, SAM3_INPUT_SIZE);
    Ok(Sam3Preprocessed {
        pixels,
        width: SAM3_INPUT_SIZE,
        height: SAM3_INPUT_SIZE,
        src_width: src_w,
        src_height: src_h,
        src_rgb,
    })
}

/// Long-lived native runtime. Preparing uploads/caches model weights.
pub struct Sam3 {
    model: crate::sam3_model::Sam3Model,
}

impl Sam3 {
    pub fn prepare(weights: &Sam3Weights) -> Result<Self> {
        Self::prepare_controlled(weights, None, None)
    }

    pub fn prepare_with_progress(
        weights: &Sam3Weights,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        Self::prepare_controlled(weights, None, progress)
    }

    pub fn prepare_controlled(
        weights: &Sam3Weights,
        cancel: Option<Sam3Cancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        Ok(Self {
            model: crate::sam3_model::Sam3Model::prepare(weights, cancel, progress)?,
        })
    }

    pub fn execution_info(&self) -> Sam3ExecutionInfo {
        SAM3_EXECUTION
    }

    pub fn segment(
        &self,
        image: Sam3Image<'_>,
        prompt: &str,
        progress: Option<ProgressHook>,
    ) -> Result<Sam3Mask> {
        self.segment_controlled(image, prompt, None, progress)
    }

    pub fn segment_controlled(
        &self,
        image: Sam3Image<'_>,
        prompt: &str,
        cancel: Option<Sam3Cancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<Sam3Mask> {
        check_cancel(cancel)?;
        let parsed = Sam3Prompt::parse(prompt)?;
        let preprocessed = preprocess_image(image)?;
        self.model
            .forward(&preprocessed, &parsed, cancel, progress)
    }

    pub fn segment_preprocessed(
        &self,
        preprocessed: &Sam3Preprocessed,
        prompt: &Sam3Prompt,
        cancel: Option<Sam3Cancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<Sam3Mask> {
        check_cancel(cancel)?;
        self.model
            .forward(preprocessed, prompt, cancel, progress)
    }
}

/// Evict every device-cached SAM3 tensor and release idle activation-pool
/// buffers. Invoke this on the same worker thread that ran SAM3.
pub fn unload_sam3() -> Result<usize> {
    let evicted = crate::backend::gpu_weight_cache_evict_prefix(&format!(
        "{SAM3_CACHE_NAMESPACE}::"
    ))
    .map_err(DiffusionError::model)?;
    crate::backend::gpu_pool_clear();
    Ok(evicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_contract_rejects_bad_lengths_and_channels() {
        assert!(Sam3Image::new(&[0; 11], 2, 2, 3).is_err());
        assert!(Sam3Image::new(&[0; 8], 2, 2, 2).is_err());
        assert!(Sam3Image::rgb8(&[0; 12], 2, 2).is_ok());
        assert!(Sam3Image::rgba8(&[0; 16], 2, 2).is_ok());
    }

    #[test]
    fn multiplex_prompt_parser() {
        let parsed = Sam3Prompt::parse("cat:1").unwrap();
        assert_eq!(parsed.phrases.len(), 1);
        assert_eq!(parsed.phrases[0].text, "cat");
        assert_eq!(parsed.phrases[0].max_instances, Some(1));
        let parsed = Sam3Prompt::parse("eye:2, window panels:4").unwrap();
        assert_eq!(parsed.phrases.len(), 2);
        assert_eq!(parsed.phrases[0].text, "eye");
        assert_eq!(parsed.phrases[0].max_instances, Some(2));
        assert_eq!(parsed.phrases[1].text, "window panels");
        assert_eq!(parsed.phrases[1].max_instances, Some(4));
        let parsed = Sam3Prompt::parse("person").unwrap();
        assert_eq!(parsed.phrases[0].max_instances, None);
        assert!(Sam3Prompt::parse("").is_err());
        assert!(Sam3Prompt::parse("cat:0").is_err());
    }

    #[test]
    fn pinned_manifest_is_explicit() {
        assert_eq!(SAM3_MODEL_SIZE, 1_745_546_848);
        assert_eq!(SAM3_MODEL_SHA256.len(), 64);
        assert_eq!(SAM3_REVISION.len(), 40);
        assert_eq!(SAM3_REPO, "Comfy-Org/sam3.1");
        assert!(!SAM3_REPO.starts_with("facebook/"));
    }

    #[test]
    fn preprocess_stretches_to_1008_unit_range() {
        let rgb = vec![128u8; 32 * 16 * 3];
        let image = Sam3Image::rgb8(&rgb, 32, 16).unwrap();
        let pre = preprocess_image(image).unwrap();
        assert_eq!(pre.width, 1008);
        assert_eq!(pre.height, 1008);
        assert_eq!(pre.src_width, 32);
        assert_eq!(pre.src_height, 16);
        assert_eq!(pre.pixels.len(), 3 * 1008 * 1008);
        let mid = pre.pixels[1008 * 1008 + 1008 * 504 + 504];
        assert!((mid - 128.0 / 255.0).abs() < 1e-5, "{mid}");
    }
}
