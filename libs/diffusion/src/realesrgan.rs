//! Native RealESRGAN x4plus general-image upscaler.
//!
//! The production checkpoint is the Comfy-Org repackaged safetensors of the
//! official `xinntao/Real-ESRGAN` x4plus release (BSD-3).  It is consumed
//! directly — no Python, GGUF, or conversion step is part of the runtime.
//! The architecture is RRDBNet: `conv_first`, 23 RRDB blocks (each three
//! residual dense blocks of five 3x3 convolutions with LeakyReLU 0.2 and
//! 0.2-scaled residuals), `conv_body` with a trunk residual, two nearest-2x
//! upsample + conv stages, `conv_hr`, and `conv_last`.  The forward lives in
//! [`crate::realesrgan_model`]; this module owns the stable public image API
//! and all checkpoint validation.

use crate::{DiffusionError, ProgressHook, Result};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

pub const REALESRGAN_REPO: &str = "Comfy-Org/Real-ESRGAN_repackaged";
pub const REALESRGAN_REVISION: &str = "ea19b4cd14f85a5b914eee8aa7ff77bc371039a0";
pub const REALESRGAN_MODEL_PATH: &str = "RealESRGAN_x4plus.safetensors";
pub const REALESRGAN_MODEL_SIZE: u64 = 66_857_836;
pub const REALESRGAN_MODEL_SHA256: &str =
    "37f9a931c215f040aa6d50f711f2cb115f713c46df1d0d6469a8bd7bfe9a60bb";

pub const REALESRGAN_SCALE: usize = 4;
pub const REALESRGAN_NUM_FEAT: usize = 64;
pub const REALESRGAN_NUM_BLOCK: usize = 23;
pub const REALESRGAN_NUM_GROW: usize = 32;
pub const REALESRGAN_RESIDUAL_SCALE: f32 = 0.2;
pub const REALESRGAN_LRELU_SLOPE: f32 = 0.2;
pub const REALESRGAN_CACHE_NAMESPACE: &str = "realesrgan";

pub type RealEsrganCancel<'a> = &'a dyn Fn() -> bool;

fn check_cancel(cancel: Option<RealEsrganCancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

/// Input pixels for native upscaling.  RGB and RGBA are both accepted; alpha
/// is ignored by the neural net (upscale the color planes, re-composite any
/// alpha downstream at the caller's discretion).
#[derive(Clone, Copy, Debug)]
pub struct RealEsrganImage<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

impl<'a> RealEsrganImage<'a> {
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
                "realesrgan image must be non-empty RGB8 or RGBA8",
            ));
        }
        let expected = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(channels))
            .ok_or_else(|| DiffusionError::workflow("realesrgan image size overflow"))?;
        if pixels.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "realesrgan image byte length {} != {width}*{height}*{channels}",
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

/// Upscaled output at 4x the input dimensions.  `planes` is planar CHW f32
/// exactly as the network produced it (pre-clamp), so parity metrics against
/// reference dumps stay honest; `rgb8` quantizes at the artifact boundary the
/// same way the official inference script does.
#[derive(Clone, Debug)]
pub struct RealEsrganUpscale {
    pub width: usize,
    pub height: usize,
    pub planes: Vec<f32>,
}

impl RealEsrganUpscale {
    pub fn new(width: usize, height: usize, planes: Vec<f32>) -> Result<Self> {
        if planes.len() != width.saturating_mul(height).saturating_mul(3)
            || planes.iter().any(|value| !value.is_finite())
        {
            return Err(DiffusionError::model(
                "realesrgan returned an invalid output tensor",
            ));
        }
        Ok(Self {
            width,
            height,
            planes,
        })
    }

    /// Interleaved RGB8, `clamp(0,1) * 255` rounded (official quantization).
    pub fn rgb8(&self) -> Vec<u8> {
        let plane = self.width * self.height;
        let mut rgb = vec![0u8; plane * 3];
        for pixel in 0..plane {
            for channel in 0..3 {
                rgb[pixel * 3 + channel] = (self.planes[channel * plane + pixel]
                    .clamp(0.0, 1.0)
                    * 255.0)
                    .round() as u8;
            }
        }
        rgb
    }
}

/// Thin direct reader around the pinned safetensors checkpoint.
pub struct RealEsrganWeights {
    pub path: PathBuf,
    header: MlxSafetensorsHeader,
}

impl RealEsrganWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let header = MlxSafetensorsHeader::load(&path).map_err(|err| {
            DiffusionError::model(format!(
                "realesrgan weights {}: {err:?}",
                path.display()
            ))
        })?;
        let weights = Self { path, header };
        weights.validate_architecture()?;
        Ok(weights)
    }

    pub fn file_len(&self) -> u64 {
        self.header.file_len
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
                        "realesrgan tensor {name} dimension {value} exceeds usize"
                    ))
                })
            })
            .collect()
    }

    pub fn f32(&self, name: &str) -> Result<Vec<f32>> {
        let entry = self.entry(name)?;
        if entry.dtype != MlxDType::F32 {
            return Err(DiffusionError::model(format!(
                "realesrgan tensor {name} has dtype {:?}, expected F32",
                entry.dtype
            )));
        }
        Ok(self
            .header
            .read_tensor_bytes(name)
            .map_err(|err| {
                DiffusionError::model(format!("realesrgan read tensor {name}: {err:?}"))
            })?
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }

    fn entry(&self, name: &str) -> Result<&makepad_mlx::MlxTensorEntry> {
        self.header.tensors.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "realesrgan tensor {name} missing from {}",
                self.path.display()
            ))
        })
    }

    fn expect(&self, name: &str, shape: &[usize]) -> Result<()> {
        let entry = self.entry(name)?;
        let actual: Vec<usize> = entry
            .shape
            .iter()
            .map(|&value| value as usize)
            .collect();
        if entry.dtype != MlxDType::F32 || actual != shape {
            return Err(DiffusionError::model(format!(
                "realesrgan tensor {name}: {:?} {actual:?}, expected F32 {shape:?}",
                entry.dtype
            )));
        }
        Ok(())
    }

    /// Fail closed if the file is a different ESRGAN family member (anime-6B,
    /// x2plus, old-arch `model.0.*` keys) before any GPU work happens.
    fn validate_architecture(&self) -> Result<()> {
        let feat = REALESRGAN_NUM_FEAT;
        let grow = REALESRGAN_NUM_GROW;
        self.expect("conv_first.weight", &[feat, 3, 3, 3])?;
        self.expect("conv_body.weight", &[feat, feat, 3, 3])?;
        self.expect("conv_up1.weight", &[feat, feat, 3, 3])?;
        self.expect("conv_up2.weight", &[feat, feat, 3, 3])?;
        self.expect("conv_hr.weight", &[feat, feat, 3, 3])?;
        self.expect("conv_last.weight", &[3, feat, 3, 3])?;
        for block in [0, REALESRGAN_NUM_BLOCK - 1] {
            for rdb in 1..=3 {
                for conv in 1..=5 {
                    let (out_ch, in_ch) = if conv == 5 {
                        (feat, feat + 4 * grow)
                    } else {
                        (grow, feat + (conv - 1) * grow)
                    };
                    self.expect(
                        &format!("body.{block}.rdb{rdb}.conv{conv}.weight"),
                        &[out_ch, in_ch, 3, 3],
                    )?;
                    self.expect(
                        &format!("body.{block}.rdb{rdb}.conv{conv}.bias"),
                        &[out_ch],
                    )?;
                }
            }
        }
        let extra_block = format!("body.{}.rdb1.conv1.weight", REALESRGAN_NUM_BLOCK);
        if self.header.tensors.contains_key(&extra_block) {
            return Err(DiffusionError::model(
                "realesrgan checkpoint has more than 23 RRDB blocks",
            ));
        }
        Ok(())
    }
}

/// Long-lived native runtime.  Preparing uploads/caches model weights; callers
/// retain this value across jobs for warm performance.  Cancellation is
/// cooperative at RRDB block boundaries through `cancel`.
pub struct RealEsrgan {
    model: crate::realesrgan_model::RealEsrganModel,
}

impl RealEsrgan {
    pub fn prepare(weights: &RealEsrganWeights) -> Result<Self> {
        Self::prepare_controlled(weights, None, None)
    }

    pub fn prepare_controlled(
        weights: &RealEsrganWeights,
        cancel: Option<RealEsrganCancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        Ok(Self {
            model: crate::realesrgan_model::RealEsrganModel::prepare(
                weights, cancel, progress,
            )?,
        })
    }

    pub fn upscale(
        &self,
        image: RealEsrganImage<'_>,
        progress: Option<ProgressHook>,
    ) -> Result<RealEsrganUpscale> {
        self.upscale_controlled(image, None, progress)
    }

    pub fn upscale_controlled(
        &self,
        image: RealEsrganImage<'_>,
        cancel: Option<RealEsrganCancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<RealEsrganUpscale> {
        check_cancel(cancel)?;
        self.model.upscale(image, cancel, progress)
    }

    /// Interleaved RGB8 at 4x, quantized on device in fast mode so only the
    /// final bytes cross PCIe — the warm production artifact path.
    pub fn upscale_rgb8_controlled(
        &self,
        image: RealEsrganImage<'_>,
        cancel: Option<RealEsrganCancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<Vec<u8>> {
        check_cancel(cancel)?;
        self.model.upscale_rgb8(image, cancel, progress)
    }
}

/// Evict every device-cached RealESRGAN tensor and release idle activation
/// pool buffers.  Invoke on the same worker thread that ran the model.
pub fn unload_realesrgan() -> Result<usize> {
    let evicted = crate::backend::gpu_weight_cache_evict_prefix(&format!(
        "{REALESRGAN_CACHE_NAMESPACE}::"
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
        assert!(RealEsrganImage::new(&[0; 11], 2, 2, 3).is_err());
        assert!(RealEsrganImage::new(&[0; 8], 2, 2, 2).is_err());
        assert!(RealEsrganImage::rgb8(&[0; 12], 2, 2).is_ok());
        assert!(RealEsrganImage::rgba8(&[0; 16], 2, 2).is_ok());
    }

    #[test]
    fn upscale_quantizes_like_the_official_script() {
        let upscale = RealEsrganUpscale::new(
            1,
            1,
            vec![-0.1, 0.5019, 1.7],
        )
        .unwrap();
        assert_eq!(upscale.rgb8(), vec![0, 128, 255]);
    }

    #[test]
    fn pinned_manifest_is_explicit() {
        assert_eq!(REALESRGAN_MODEL_SIZE, 66_857_836);
        assert_eq!(REALESRGAN_MODEL_SHA256.len(), 64);
        assert_eq!(REALESRGAN_REVISION.len(), 40);
    }
}
