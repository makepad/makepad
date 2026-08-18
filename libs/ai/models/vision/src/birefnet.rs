//! Native BiRefNet HR-matting foundation.
//!
//! The production checkpoint is the official
//! `ZhengPeng7/BiRefNet_HR-matting` safetensors file.  It is consumed
//! directly: no Python conversion or GGUF subprocess is part of the runtime.
//! BatchNorm inference coefficients are folded deterministically while the
//! Rust model is prepared (`scale = gamma / sqrt(var + eps)`,
//! `shift = beta - mean * scale`).  The forward implementation lives in
//! [`crate::birefnet_model`]; this module deliberately owns the stable public
//! image/API contract and all checkpoint validation.

use crate::{DiffusionError, ProgressHook, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

pub const BIREFNET_REPO: &str = "ZhengPeng7/BiRefNet_HR-matting";
pub const BIREFNET_REVISION: &str = "5d6b6f8adcb5b417c871b1d84ceaae9871355b7f";
pub const BIREFNET_MODEL_PATH: &str = "model.safetensors";
pub const BIREFNET_MODEL_SIZE: u64 = 444_473_596;
pub const BIREFNET_MODEL_SHA256: &str =
    "a5a4de698739ea5e0e8bbab28e1b293dde95092b87a442d566cbc585c53cef55";

pub const BIREFNET_INPUT_SIZE: usize = 1024;
pub const BIREFNET_WINDOW_SIZE: usize = 12;
pub const BIREFNET_EMBED_DIM: usize = 192;
pub const BIREFNET_DEPTHS: [usize; 4] = [2, 2, 18, 2];
pub const BIREFNET_HEADS: [usize; 4] = [6, 12, 24, 48];
pub const BIREFNET_BN_EPS: f32 = 1e-5;
pub const BIREFNET_LN_EPS: f32 = 1e-5;
pub const BIREFNET_CACHE_NAMESPACE: &str = "birefnet";

pub type BiRefNetCancel<'a> = &'a dyn Fn() -> bool;

fn check_cancel(cancel: Option<BiRefNetCancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

/// Input pixels for native matting. RGB and RGBA are both accepted.  Alpha is
/// intentionally ignored by the neural net; callers that already have a
/// meaningful matte should bypass the model instead of multiplying two
/// unrelated alpha estimates.
#[derive(Clone, Copy, Debug)]
pub struct BiRefNetImage<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

impl<'a> BiRefNetImage<'a> {
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
                "birefnet image must be non-empty RGB8 or RGBA8",
            ));
        }
        let expected = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(channels))
            .ok_or_else(|| DiffusionError::workflow("birefnet image size overflow"))?;
        if pixels.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "birefnet image byte length {} != {width}*{height}*{channels}",
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

/// Continuous foreground coverage at the original input dimensions.
/// Values are finite and clamped to 0..=1; no thresholding is performed.
#[derive(Clone, Debug)]
pub struct BiRefNetMatte {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<f32>,
}

impl BiRefNetMatte {
    pub fn new(width: usize, height: usize, alpha: Vec<f32>) -> Result<Self> {
        if alpha.len() != width.saturating_mul(height)
            || alpha.iter().any(|value| !value.is_finite())
        {
            return Err(DiffusionError::model(
                "birefnet returned an invalid soft-alpha tensor",
            ));
        }
        Ok(Self {
            width,
            height,
            alpha,
        })
    }

    /// Quantize only at the artifact boundary. The neural/runtime contract
    /// stays f32 soft alpha so downstream crop and compositing can choose
    /// their own precision.
    pub fn alpha_u8(&self) -> Vec<u8> {
        self.alpha
            .iter()
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }
}

/// Thin direct reader around the pinned safetensors checkpoint.
pub struct BiRefNetWeights {
    pub path: PathBuf,
    header: MlxSafetensorsHeader,
}

impl BiRefNetWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let header = MlxSafetensorsHeader::load(&path).map_err(|err| {
            DiffusionError::model(format!("birefnet weights {}: {err:?}", path.display()))
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
                        "birefnet tensor {name} dimension {value} exceeds usize"
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
            DiffusionError::model(format!("birefnet read tensor {name}: {err:?}"))
        })
    }

    pub fn f32(&self, name: &str) -> Result<Vec<f32>> {
        let dtype = self.dtype(name)?;
        let bytes = self.bytes(name)?;
        let values = match dtype {
            MlxDType::F16 => bytes
                .chunks_exact(2)
                .map(|chunk| {
                    crate::f16_word_to_f32(u16::from_le_bytes([chunk[0], chunk[1]]))
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
                    "birefnet tensor {name} has unsupported floating dtype {other:?}"
                )))
            }
        };
        Ok(values)
    }

    pub fn f32_shaped(&self, name: &str, expected: &[usize]) -> Result<Vec<f32>> {
        let shape = self.shape(name)?;
        if shape != expected {
            return Err(DiffusionError::model(format!(
                "birefnet tensor {name} shape {shape:?}, expected {expected:?}"
            )));
        }
        self.f32(name)
    }

    pub fn i64(&self, name: &str) -> Result<Vec<i64>> {
        if self.dtype(name)? != MlxDType::I64 {
            return Err(DiffusionError::model(format!(
                "birefnet tensor {name} is not I64"
            )));
        }
        Ok(self
            .bytes(name)?
            .chunks_exact(8)
            .map(|chunk| {
                i64::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6],
                    chunk[7],
                ])
            })
            .collect())
    }

    pub fn folded_batch_norm(&self, prefix: &str) -> Result<FoldedBatchNorm> {
        let gamma = self.f32(&format!("{prefix}.weight"))?;
        let beta = self.f32(&format!("{prefix}.bias"))?;
        let mean = self.f32(&format!("{prefix}.running_mean"))?;
        let var = self.f32(&format!("{prefix}.running_var"))?;
        if gamma.len() != beta.len() || gamma.len() != mean.len() || gamma.len() != var.len() {
            return Err(DiffusionError::model(format!(
                "birefnet BatchNorm {prefix} vector length mismatch"
            )));
        }
        let mut scale = Vec::with_capacity(gamma.len());
        let mut shift = Vec::with_capacity(gamma.len());
        for (((gamma, beta), mean), var) in gamma.iter().zip(&beta).zip(&mean).zip(&var) {
            let value = *gamma / (*var + BIREFNET_BN_EPS).sqrt();
            scale.push(value);
            shift.push(*beta - *mean * value);
        }
        Ok(FoldedBatchNorm { scale, shift })
    }

    fn entry(&self, name: &str) -> Result<&makepad_ai_loader::MlxTensorEntry> {
        self.header.tensors.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "birefnet tensor {name} missing from {}",
                self.path.display()
            ))
        })
    }

    fn expect(&self, name: &str, dtype: MlxDType, shape: &[usize]) -> Result<()> {
        let actual_dtype = self.dtype(name)?;
        let actual_shape = self.shape(name)?;
        if actual_dtype != dtype || actual_shape != shape {
            return Err(DiffusionError::model(format!(
                "birefnet tensor {name}: {actual_dtype:?} {actual_shape:?}, expected {dtype:?} {shape:?}"
            )));
        }
        Ok(())
    }

    /// Fail closed if the registry points at a similarly named but different
    /// BiRefNet family member. This catches backbone/decoder mismatches before
    /// a multi-hundred-megabyte GPU prepare.
    fn validate_architecture(&self) -> Result<()> {
        self.expect(
            "bb.patch_embed.proj.weight",
            MlxDType::F16,
            &[BIREFNET_EMBED_DIM, 3, 4, 4],
        )?;
        self.expect(
            "bb.layers.0.blocks.0.attn.relative_position_index",
            MlxDType::I64,
            &[BIREFNET_WINDOW_SIZE * BIREFNET_WINDOW_SIZE; 2],
        )?;
        self.expect(
            "bb.layers.3.blocks.1.attn.qkv.weight",
            MlxDType::F16,
            &[4608, 1536],
        )?;
        self.expect(
            "squeeze_module.0.conv_in.weight",
            MlxDType::F16,
            &[64, 5760, 3, 3],
        )?;
        self.expect(
            "decoder.decoder_block1.dec_att.aspp_deforms.2.atrous_conv.regular_conv.weight",
            MlxDType::F16,
            &[256, 64, 7, 7],
        )?;
        self.expect("decoder.conv_out1.0.weight", MlxDType::F16, &[1, 240, 1, 1])?;
        for stage in 0..4 {
            let last = BIREFNET_DEPTHS[stage] - 1;
            let name = format!("bb.layers.{stage}.blocks.{last}.attn.qkv.weight");
            if !self.has(&name) {
                return Err(DiffusionError::model(format!(
                    "birefnet checkpoint is missing expected Swin block {name}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct FoldedBatchNorm {
    pub scale: Vec<f32>,
    pub shift: Vec<f32>,
}

/// Long-lived native runtime. Preparing uploads/caches model weights; callers
/// retain this value across jobs for warm performance. Cancellation is
/// cooperative at backbone/decoder block boundaries through `progress`.
pub struct BiRefNet {
    model: crate::birefnet_model::BiRefNetModel,
}

impl BiRefNet {
    pub fn prepare(weights: &BiRefNetWeights) -> Result<Self> {
        Self::prepare_controlled(weights, None, None)
    }

    pub fn prepare_with_progress(
        weights: &BiRefNetWeights,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        Self::prepare_controlled(weights, None, progress)
    }

    pub fn prepare_controlled(
        weights: &BiRefNetWeights,
        cancel: Option<BiRefNetCancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        Ok(Self {
            model: crate::birefnet_model::BiRefNetModel::prepare(weights, cancel, progress)?,
        })
    }

    pub fn matte(
        &self,
        image: BiRefNetImage<'_>,
        progress: Option<ProgressHook>,
    ) -> Result<BiRefNetMatte> {
        self.matte_controlled(image, None, progress)
    }

    pub fn matte_controlled(
        &self,
        image: BiRefNetImage<'_>,
        cancel: Option<BiRefNetCancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<BiRefNetMatte> {
        check_cancel(cancel)?;
        self.model.matte(image, cancel, progress)
    }
}

/// Evict every device-cached BiRefNet tensor and release idle activation-pool
/// buffers before a memory-heavy downstream model (notably TRELLIS) loads.
/// Invoke this on the same worker thread that ran BiRefNet.
pub fn unload_birefnet() -> Result<usize> {
    let evicted = crate::backend::gpu_weight_cache_evict_prefix(&format!(
        "{BIREFNET_CACHE_NAMESPACE}::"
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
        assert!(BiRefNetImage::new(&[0; 11], 2, 2, 3).is_err());
        assert!(BiRefNetImage::new(&[0; 8], 2, 2, 2).is_err());
        assert!(BiRefNetImage::rgb8(&[0; 12], 2, 2).is_ok());
        assert!(BiRefNetImage::rgba8(&[0; 16], 2, 2).is_ok());
    }

    #[test]
    fn matte_quantization_preserves_intermediate_alpha() {
        let matte = BiRefNetMatte::new(3, 1, vec![0.0, 0.5, 1.0]).unwrap();
        assert_eq!(matte.alpha_u8(), vec![0, 128, 255]);
    }

    #[test]
    fn pinned_manifest_is_explicit() {
        assert_eq!(BIREFNET_MODEL_SIZE, 444_473_596);
        assert_eq!(BIREFNET_MODEL_SHA256.len(), 64);
        assert_eq!(BIREFNET_REVISION.len(), 40);
    }
}
