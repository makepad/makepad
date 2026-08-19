//! Native Practical-RIFE v4.26 frame interpolation (IFNet_HDv3).
//!
//! The production checkpoint is Comfy-Org's safetensors repack of hzwer's
//! Practical-RIFE `v4.26` `flownet.pkl` (MIT).  It is consumed directly — no
//! Python, no pickle, no conversion step at runtime.
//!
//! Architecture (transcribed from `IFNet_HDv3.py` of the 4.26 release, cross
//! checked against HolyWu/vs-rife `IFNet_HDv3_v4_26.py` and
//! Fannovel16/ComfyUI-Frame-Interpolation `rife_arch.py` `arch_ver="4.26"`):
//!
//! - `encode` (`Head`): `Conv2d(3,16,3,2,1)` → LeakyReLU(0.2) →
//!   `Conv2d(16,16,3,1,1)` → LeakyReLU → `Conv2d(16,16,3,1,1)` → LeakyReLU →
//!   `ConvTranspose2d(16,4,4,2,1)`.  Full-resolution 4-channel context.
//! - five `IFBlock`s with `c = [192,128,96,64,32]` run coarse-to-fine at
//!   `scale_list = [16,8,4,2,1]`.  Each is two stride-2 convs, eight
//!   `ResConv` (`LeakyReLU(conv(x) * beta + x)`), and
//!   `ConvTranspose2d(c, 4*13, 4, 2, 1) + PixelShuffle(2)`; the 13 output
//!   planes split into flow(4) · mask(1) · feat(8).
//! - block 0 sees `cat(img0, img1, f0, f1, timestep)` (15 planes); blocks 1..4
//!   see `cat(warped0, warped1, warp(f0), warp(f1), timestep, mask, feat)`
//!   plus the downscaled running flow (28 planes) and emit a flow residual.
//! - the result is `warp(img0, flow[:2]) * sigmoid(mask) + warp(img1,
//!   flow[2:4]) * (1 - sigmoid(mask))`.
//!
//! `ensemble` is not supported by 4.26 upstream and is not implemented here.
//! Images are padded (zero, right/bottom) to a multiple of 64 before the
//! forward and cropped after, exactly like the reference inference scripts.
//!
//! The forwards live in [`crate::rife_cpu`] (portable reference, unit-tested)
//! and [`crate::rife_model`] (device path); this module owns the public API
//! and all checkpoint validation.

use crate::{DiffusionError, ProgressHook, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

pub const RIFE_REPO: &str = "Comfy-Org/frame_interpolation";
pub const RIFE_REVISION: &str = "9bca6366a22473ccee25602fa82b224d78413960";
pub const RIFE_MODEL_PATH: &str = "frame_interpolation/rife_v4.26.safetensors";
pub const RIFE_MODEL_SIZE: u64 = 22_674_688;
pub const RIFE_MODEL_SHA256: &str =
    "151874592c877740e5db11522f4514df569eeafb0a0fcb2696f16e9e8d317c94";

/// Coarse-to-fine IFBlock feature widths (v4.26 has five blocks).
pub const RIFE_BLOCK_CHANNELS: [usize; 5] = [192, 128, 96, 64, 32];
pub const RIFE_NUM_BLOCKS: usize = 5;
/// Matching `scale_list`; the reference exposes a global divisor for speed
/// (vs-rife `scale`), which [`RifeScale`] mirrors.
pub const RIFE_SCALE_LIST: [f32; RIFE_NUM_BLOCKS] = [16.0, 8.0, 4.0, 2.0, 1.0];
/// `Head` output planes (`encode_channel` in the reference runner).
pub const RIFE_ENCODE_CHANNELS: usize = 4;
/// `lastconv` emits `4 * 13` planes; PixelShuffle(2) turns them into 13.
pub const RIFE_LASTCONV_PLANES: usize = 13;
pub const RIFE_LRELU_SLOPE: f32 = 0.2;
/// Reference `modulo` for v4.26: the coarsest block downsamples by 16 and
/// then twice by 2, so both padded extents must be multiples of 64.
pub const RIFE_PAD_MULTIPLE: usize = 64;
pub const RIFE_CACHE_NAMESPACE: &str = "rife";

/// Block 0 input planes: img0(3) + img1(3) + f0(4) + f1(4) + timestep(1).
const BLOCK0_IN_PLANES: usize = 15;
/// Blocks 1..4: warped0(3) + warped1(3) + wf0(4) + wf1(4) + timestep(1)
/// + mask(1) + feat(8) + the flow(4) concatenated inside the block.
const BLOCK_N_IN_PLANES: usize = 28;
const RESCONV_COUNT: usize = 8;

pub type RifeCancel<'a> = &'a dyn Fn() -> bool;

fn check_cancel(cancel: Option<RifeCancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

/// Optional global speed/quality divisor, mirroring vs-rife's `scale`: the
/// whole `scale_list` is divided by it, so `Half` runs the flow estimator at
/// half resolution (roughly 2x faster, softer on large motion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RifeScale {
    /// `scale_list = [32, 16, 8, 4, 2]` — for very large canvases.
    Half,
    #[default]
    /// `scale_list = [16, 8, 4, 2, 1]` — the released default.
    Full,
}

impl RifeScale {
    pub fn scale_list(self) -> [f32; RIFE_NUM_BLOCKS] {
        let divisor = match self {
            RifeScale::Half => 0.5,
            RifeScale::Full => 1.0,
        };
        let mut list = RIFE_SCALE_LIST;
        for value in list.iter_mut() {
            *value /= divisor;
        }
        list
    }

    /// Padding modulus: `max(64, 64 / scale)` in the reference runner.
    pub fn pad_multiple(self) -> usize {
        match self {
            RifeScale::Half => RIFE_PAD_MULTIPLE * 2,
            RifeScale::Full => RIFE_PAD_MULTIPLE,
        }
    }
}

/// Padded `(width, height)` for a canvas, rounding up to the modulus the
/// coarsest block needs.
pub fn padded_extent(width: usize, height: usize, scale: RifeScale) -> (usize, usize) {
    let modulo = scale.pad_multiple();
    (
        width.div_ceil(modulo) * modulo,
        height.div_ceil(modulo) * modulo,
    )
}

/// Intermediate timesteps for an integer frame-rate multiplier: `2` yields
/// `[0.5]`, `4` yields `[0.25, 0.5, 0.75]`.  Factor `1` interpolates nothing.
pub fn interpolation_timesteps(factor: u32) -> Vec<f32> {
    (1..factor.max(1))
        .map(|k| k as f32 / factor as f32)
        .collect()
}

// ---------------------------------------------------------------------------
// Host-side checkpoint
// ---------------------------------------------------------------------------

/// A `Conv2d`: PyTorch weight layout `[out, in, kh, kw]`.
#[derive(Clone, Debug)]
pub struct ConvWeight {
    pub out_channels: usize,
    pub in_channels: usize,
    pub kh: usize,
    pub kw: usize,
    pub stride: usize,
    pub pad: usize,
    pub weights: Vec<f32>,
    pub bias: Vec<f32>,
}

/// A `ConvTranspose2d`: PyTorch weight layout `[in, out, kh, kw]`.
#[derive(Clone, Debug)]
pub struct DeconvWeight {
    pub in_channels: usize,
    pub out_channels: usize,
    pub kh: usize,
    pub kw: usize,
    pub stride: usize,
    pub pad: usize,
    pub weights: Vec<f32>,
    pub bias: Vec<f32>,
}

/// `LeakyReLU(conv(x) * beta + x)` — `beta` is one scalar per channel.
#[derive(Clone, Debug)]
pub struct ResConvWeight {
    pub conv: ConvWeight,
    pub beta: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct IfBlockWeights {
    pub channels: usize,
    pub in_planes: usize,
    pub conv0_a: ConvWeight,
    pub conv0_b: ConvWeight,
    pub convblock: Vec<ResConvWeight>,
    pub lastconv: DeconvWeight,
}

#[derive(Clone, Debug)]
pub struct HeadWeights {
    pub cnn0: ConvWeight,
    pub cnn1: ConvWeight,
    pub cnn2: ConvWeight,
    pub cnn3: DeconvWeight,
}

/// The whole prepared IFNet checkpoint, host side.
#[derive(Clone, Debug)]
pub struct RifeModelWeights {
    pub encode: HeadWeights,
    pub blocks: Vec<IfBlockWeights>,
}

/// Every tensor the v4.26 IFNet state dict must carry, with its exact shape,
/// derived from the architecture constants.  This is the single source of
/// truth for both the loader and the checkpoint validator, so a mismatch
/// with the released file is a loud failure rather than a silent mis-map.
pub fn expected_tensors() -> Vec<(String, Vec<usize>)> {
    let mut out = Vec::new();
    let mut conv = |name: &str, o: usize, i: usize, k: usize| {
        out.push((format!("{name}.weight"), vec![o, i, k, k]));
        out.push((format!("{name}.bias"), vec![o]));
    };
    conv("encode.cnn0", 16, 3, 3);
    conv("encode.cnn1", 16, 16, 3);
    conv("encode.cnn2", 16, 16, 3);
    // ConvTranspose2d weight is [in, out, kh, kw].
    out.push((
        "encode.cnn3.weight".to_string(),
        vec![16, RIFE_ENCODE_CHANNELS, 4, 4],
    ));
    out.push((
        "encode.cnn3.bias".to_string(),
        vec![RIFE_ENCODE_CHANNELS],
    ));
    for (index, &c) in RIFE_BLOCK_CHANNELS.iter().enumerate() {
        let in_planes = block_in_planes(index);
        let prefix = format!("blocks.{index}");
        out.push((
            format!("{prefix}.conv0.0.0.weight"),
            vec![c / 2, in_planes, 3, 3],
        ));
        out.push((format!("{prefix}.conv0.0.0.bias"), vec![c / 2]));
        out.push((format!("{prefix}.conv0.1.0.weight"), vec![c, c / 2, 3, 3]));
        out.push((format!("{prefix}.conv0.1.0.bias"), vec![c]));
        for res in 0..RESCONV_COUNT {
            out.push((
                format!("{prefix}.convblock.{res}.conv.weight"),
                vec![c, c, 3, 3],
            ));
            out.push((format!("{prefix}.convblock.{res}.conv.bias"), vec![c]));
            out.push((format!("{prefix}.convblock.{res}.beta"), vec![1, c, 1, 1]));
        }
        out.push((
            format!("{prefix}.lastconv.0.weight"),
            vec![c, 4 * RIFE_LASTCONV_PLANES, 4, 4],
        ));
        out.push((
            format!("{prefix}.lastconv.0.bias"),
            vec![4 * RIFE_LASTCONV_PLANES],
        ));
    }
    out
}

pub fn block_in_planes(index: usize) -> usize {
    if index == 0 {
        BLOCK0_IN_PLANES
    } else {
        BLOCK_N_IN_PLANES
    }
}

/// Thin direct reader around the pinned safetensors checkpoint.
pub struct RifeWeights {
    pub path: PathBuf,
    header: MlxSafetensorsHeader,
}

impl RifeWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let header = MlxSafetensorsHeader::load(&path).map_err(|err| {
            DiffusionError::model(format!("rife weights {}: {err:?}", path.display()))
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
        Ok(self
            .entry(name)?
            .shape
            .iter()
            .map(|&value| value as usize)
            .collect())
    }

    pub fn f32(&self, name: &str) -> Result<Vec<f32>> {
        let entry = self.entry(name)?;
        if entry.dtype != MlxDType::F32 {
            return Err(DiffusionError::model(format!(
                "rife tensor {name} has dtype {:?}, expected F32",
                entry.dtype
            )));
        }
        Ok(self
            .header
            .read_tensor_bytes(name)
            .map_err(|err| DiffusionError::model(format!("rife read tensor {name}: {err:?}")))?
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }

    fn entry(&self, name: &str) -> Result<&makepad_ai_loader::MlxTensorEntry> {
        self.header.tensors.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "rife tensor {name} missing from {}",
                self.path.display()
            ))
        })
    }

    /// Fail closed on a different RIFE generation (4.6/4.17 key sets, the
    /// `lite`/`heavy` variants, or a 4-block checkpoint) before any GPU or
    /// forward work happens.
    fn validate_architecture(&self) -> Result<()> {
        for (name, shape) in expected_tensors() {
            let entry = self.entry(&name)?;
            let actual: Vec<usize> = entry.shape.iter().map(|&value| value as usize).collect();
            if entry.dtype != MlxDType::F32 || actual != shape {
                return Err(DiffusionError::model(format!(
                    "rife tensor {name}: {:?} {actual:?}, expected F32 {shape:?} \
                     (is this the v4.26 flownet?)",
                    entry.dtype
                )));
            }
        }
        if self
            .header
            .tensors
            .contains_key(&format!("blocks.{RIFE_NUM_BLOCKS}.conv0.0.0.weight"))
        {
            return Err(DiffusionError::model(
                "rife checkpoint has more than 5 IFBlocks",
            ));
        }
        Ok(())
    }

    /// Materializes every tensor into the host-side model.  ~22 MB.
    pub fn prepare_model(&self, mut progress: Option<ProgressHook>) -> Result<RifeModelWeights> {
        let conv = |name: &str, stride: usize, pad: usize| -> Result<ConvWeight> {
            let shape = self.shape(&format!("{name}.weight"))?;
            Ok(ConvWeight {
                out_channels: shape[0],
                in_channels: shape[1],
                kh: shape[2],
                kw: shape[3],
                stride,
                pad,
                weights: self.f32(&format!("{name}.weight"))?,
                bias: self.f32(&format!("{name}.bias"))?,
            })
        };
        let deconv = |name: &str| -> Result<DeconvWeight> {
            let shape = self.shape(&format!("{name}.weight"))?;
            Ok(DeconvWeight {
                in_channels: shape[0],
                out_channels: shape[1],
                kh: shape[2],
                kw: shape[3],
                stride: 2,
                pad: 1,
                weights: self.f32(&format!("{name}.weight"))?,
                bias: self.f32(&format!("{name}.bias"))?,
            })
        };
        crate::emit_progress(&mut progress, "load rife", 0.0)?;
        let encode = HeadWeights {
            cnn0: conv("encode.cnn0", 2, 1)?,
            cnn1: conv("encode.cnn1", 1, 1)?,
            cnn2: conv("encode.cnn2", 1, 1)?,
            cnn3: deconv("encode.cnn3")?,
        };
        let mut blocks = Vec::with_capacity(RIFE_NUM_BLOCKS);
        for (index, &channels) in RIFE_BLOCK_CHANNELS.iter().enumerate() {
            let prefix = format!("blocks.{index}");
            let mut convblock = Vec::with_capacity(RESCONV_COUNT);
            for res in 0..RESCONV_COUNT {
                convblock.push(ResConvWeight {
                    conv: conv(&format!("{prefix}.convblock.{res}.conv"), 1, 1)?,
                    beta: self.f32(&format!("{prefix}.convblock.{res}.beta"))?,
                });
            }
            blocks.push(IfBlockWeights {
                channels,
                in_planes: block_in_planes(index),
                conv0_a: conv(&format!("{prefix}.conv0.0.0"), 2, 1)?,
                conv0_b: conv(&format!("{prefix}.conv0.1.0"), 2, 1)?,
                convblock,
                lastconv: deconv(&format!("{prefix}.lastconv.0"))?,
            });
            crate::emit_progress(
                &mut progress,
                "load rife",
                (index + 1) as f64 / RIFE_NUM_BLOCKS as f64,
            )?;
        }
        Ok(RifeModelWeights { encode, blocks })
    }
}

// ---------------------------------------------------------------------------
// Public runtime
// ---------------------------------------------------------------------------

/// Which forward runs.  The device path is the production one; the portable
/// path is the readable reference the unit tests pin (and a usable, slow
/// fallback for tiny canvases).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RifeBackendKind {
    Device,
    Reference,
}

/// A frame pair to interpolate between: tightly packed RGB8, `w * h * 3`.
#[derive(Clone, Copy, Debug)]
pub struct RifeFramePair<'a> {
    pub frame0: &'a [u8],
    pub frame1: &'a [u8],
    pub width: usize,
    pub height: usize,
}

impl<'a> RifeFramePair<'a> {
    pub fn new(frame0: &'a [u8], frame1: &'a [u8], width: usize, height: usize) -> Result<Self> {
        let expected = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(3))
            .ok_or_else(|| DiffusionError::workflow("rife frame size overflow"))?;
        if width == 0 || height == 0 || frame0.len() != expected || frame1.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "rife frame pair must be two {width}x{height} RGB8 buffers of {expected} bytes, \
                 got {} and {}",
                frame0.len(),
                frame1.len()
            )));
        }
        Ok(Self {
            frame0,
            frame1,
            width,
            height,
        })
    }
}

/// Long-lived native runtime.  Preparing materializes (and, on the device
/// path, uploads/caches) the checkpoint; callers retain this across jobs.
pub struct Rife {
    weights: RifeModelWeights,
    kind: RifeBackendKind,
    scale: RifeScale,
}

impl Rife {
    /// `MAKEPAD_RIFE_MODE=reference` forces the portable forward (parity
    /// oracle / no-GPU boxes).  Otherwise the device path is required and a
    /// missing CUDA device is a loud error, like every other native backend.
    pub fn prepare(weights: &RifeWeights) -> Result<Self> {
        Self::prepare_controlled(weights, RifeScale::Full, None, None)
    }

    pub fn prepare_controlled(
        weights: &RifeWeights,
        scale: RifeScale,
        cancel: Option<RifeCancel<'_>>,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        let reference = matches!(std::env::var("MAKEPAD_RIFE_MODE"), Ok(mode) if mode == "reference");
        let kind = if reference {
            RifeBackendKind::Reference
        } else {
            if !crate::backend::gpu_device_available() {
                return Err(DiffusionError::model(
                    "native RIFE requires the Makepad CUDA backend \
                     (set MAKEPAD_RIFE_MODE=reference for the portable forward)",
                ));
            }
            RifeBackendKind::Device
        };
        Ok(Self {
            weights: weights.prepare_model(progress)?,
            kind,
            scale,
        })
    }

    /// Portable-forward constructor for tests and tiny canvases; never
    /// touches the GPU.
    pub fn prepare_reference(weights: &RifeWeights) -> Result<Self> {
        Ok(Self {
            weights: weights.prepare_model(None)?,
            kind: RifeBackendKind::Reference,
            scale: RifeScale::Full,
        })
    }

    pub fn from_model_weights(weights: RifeModelWeights, kind: RifeBackendKind) -> Self {
        Self {
            weights,
            kind,
            scale: RifeScale::Full,
        }
    }

    pub fn kind(&self) -> RifeBackendKind {
        self.kind
    }

    pub fn scale(&self) -> RifeScale {
        self.scale
    }

    pub fn model_weights(&self) -> &RifeModelWeights {
        &self.weights
    }

    /// One intermediate frame at `timestep` in `(0, 1)`, RGB8 at the input
    /// resolution.
    pub fn interpolate_rgb8(
        &self,
        pair: RifeFramePair<'_>,
        timestep: f32,
    ) -> Result<Vec<u8>> {
        self.interpolate_rgb8_controlled(pair, timestep, None)
    }

    pub fn interpolate_rgb8_controlled(
        &self,
        pair: RifeFramePair<'_>,
        timestep: f32,
        cancel: Option<RifeCancel<'_>>,
    ) -> Result<Vec<u8>> {
        check_cancel(cancel)?;
        if !timestep.is_finite() || timestep <= 0.0 || timestep >= 1.0 {
            return Err(DiffusionError::workflow(format!(
                "rife timestep must be strictly between 0 and 1, got {timestep}"
            )));
        }
        match self.kind {
            RifeBackendKind::Reference => {
                crate::rife_cpu::interpolate_rgb8(&self.weights, pair, timestep, self.scale)
            }
            RifeBackendKind::Device => {
                crate::rife_model::interpolate_rgb8(&self.weights, pair, timestep, self.scale)
            }
        }
    }
}

/// Evict every device-cached RIFE tensor and release idle pool buffers.
/// Invoke on the same worker thread that ran the model.
pub fn unload_rife() -> Result<usize> {
    let evicted =
        crate::backend::gpu_weight_cache_evict_prefix(&format!("{RIFE_CACHE_NAMESPACE}::"))
            .map_err(DiffusionError::model)?;
    crate::backend::gpu_pool_clear();
    Ok(evicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_manifest_is_explicit() {
        assert_eq!(RIFE_MODEL_SIZE, 22_674_688);
        assert_eq!(RIFE_MODEL_SHA256.len(), 64);
        assert_eq!(RIFE_REVISION.len(), 40);
        assert!(RIFE_MODEL_PATH.ends_with("rife_v4.26.safetensors"));
    }

    #[test]
    fn timesteps_follow_the_frame_rate_multiplier() {
        assert!(interpolation_timesteps(1).is_empty());
        assert_eq!(interpolation_timesteps(2), vec![0.5]);
        assert_eq!(interpolation_timesteps(4), vec![0.25, 0.5, 0.75]);
    }

    #[test]
    fn padding_rounds_up_to_the_block_modulus() {
        assert_eq!(padded_extent(640, 352, RifeScale::Full), (640, 384));
        assert_eq!(padded_extent(64, 64, RifeScale::Full), (64, 64));
        assert_eq!(padded_extent(1, 1, RifeScale::Full), (64, 64));
        assert_eq!(padded_extent(640, 352, RifeScale::Half), (640, 384));
        assert_eq!(padded_extent(200, 100, RifeScale::Half), (256, 128));
    }

    #[test]
    fn scale_list_matches_the_reference_divisor() {
        assert_eq!(RifeScale::Full.scale_list(), [16.0, 8.0, 4.0, 2.0, 1.0]);
        assert_eq!(RifeScale::Half.scale_list(), [32.0, 16.0, 8.0, 4.0, 2.0]);
    }

    /// The state dict of the released `rife_v4.26.safetensors`: 158 tensors.
    /// The exact spot checks below are the shapes read out of the pinned
    /// file, so a wrong architecture constant fails here, not on a box.
    #[test]
    fn expected_state_dict_matches_the_released_checkpoint() {
        let tensors = expected_tensors();
        assert_eq!(tensors.len(), 158);
        let find = |name: &str| {
            tensors
                .iter()
                .find(|(key, _)| key == name)
                .unwrap_or_else(|| panic!("{name} not in the expected state dict"))
                .1
                .clone()
        };
        assert_eq!(find("encode.cnn0.weight"), vec![16, 3, 3, 3]);
        assert_eq!(find("encode.cnn3.weight"), vec![16, 4, 4, 4]);
        assert_eq!(find("encode.cnn3.bias"), vec![4]);
        assert_eq!(find("blocks.0.conv0.0.0.weight"), vec![96, 15, 3, 3]);
        assert_eq!(find("blocks.0.conv0.1.0.weight"), vec![192, 96, 3, 3]);
        assert_eq!(find("blocks.0.convblock.7.conv.weight"), vec![192, 192, 3, 3]);
        assert_eq!(find("blocks.0.convblock.0.beta"), vec![1, 192, 1, 1]);
        assert_eq!(find("blocks.0.lastconv.0.weight"), vec![192, 52, 4, 4]);
        assert_eq!(find("blocks.1.conv0.0.0.weight"), vec![64, 28, 3, 3]);
        assert_eq!(find("blocks.2.conv0.0.0.weight"), vec![48, 28, 3, 3]);
        assert_eq!(find("blocks.2.convblock.3.beta"), vec![1, 96, 1, 1]);
        assert_eq!(find("blocks.3.lastconv.0.weight"), vec![64, 52, 4, 4]);
        assert_eq!(find("blocks.4.conv0.0.0.weight"), vec![16, 28, 3, 3]);
        assert_eq!(find("blocks.4.lastconv.0.weight"), vec![32, 52, 4, 4]);
        assert_eq!(find("blocks.4.lastconv.0.bias"), vec![52]);
    }

    /// Opt-in end-to-end check against the real 22 MB checkpoint:
    /// `MAKEPAD_RIFE_WEIGHTS=/path/rife_v4.26.safetensors cargo test`.
    /// Asserts the state dict is EXACTLY the expected set — no missing key,
    /// no unknown extra.
    #[test]
    fn real_checkpoint_state_dict_is_exactly_the_expected_set() {
        let Ok(path) = std::env::var("MAKEPAD_RIFE_WEIGHTS") else {
            return;
        };
        let weights = RifeWeights::load(&path).expect("load pinned rife checkpoint");
        assert_eq!(weights.file_len(), RIFE_MODEL_SIZE);
        let mut actual: Vec<String> = weights.tensor_names().cloned().collect();
        actual.sort();
        let mut expected: Vec<String> =
            expected_tensors().into_iter().map(|(name, _)| name).collect();
        expected.sort();
        assert_eq!(actual, expected);
        let model = weights.prepare_model(None).expect("prepare rife model");
        assert_eq!(model.blocks.len(), RIFE_NUM_BLOCKS);
        assert_eq!(model.encode.cnn3.out_channels, RIFE_ENCODE_CHANNELS);
        for (index, block) in model.blocks.iter().enumerate() {
            assert_eq!(block.channels, RIFE_BLOCK_CHANNELS[index]);
            assert_eq!(block.conv0_a.in_channels, block_in_planes(index));
            assert_eq!(block.convblock.len(), RESCONV_COUNT);
            assert_eq!(block.lastconv.out_channels, 4 * RIFE_LASTCONV_PLANES);
        }
    }
}
