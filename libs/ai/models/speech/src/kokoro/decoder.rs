//! The decoder: phoneme features plus pitch and energy in, waveform out.
//!
//! `F0_conv` and `N_conv` are stride-2, so the two curves arrive at half the
//! frame rate of the alignment and can be concatenated onto `asr`. Every one of
//! the four `decode` blocks re-concatenates them along with a 64-wide projection
//! of `asr`, which is how the pitch keeps its grip on a stack that would
//! otherwise drift. The last block upsamples back to the curves' original rate,
//! and the generator takes the *raw* F0 from there.

use super::adain::AdainResBlk1d;
use super::generator::{Conv, Generator, GeneratorTrace};
use super::ops::{concat_rows, Mat};
use super::weights::Weights;
use crate::TtsError;

const BLOCKS: usize = 4;

pub struct Decoder {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(super) vulkan_id: std::sync::Arc<()>,
    pub(super) f0_conv: Conv,
    pub(super) n_conv: Conv,
    pub(super) asr_res: Conv,
    pub(super) encode: AdainResBlk1d,
    pub(super) decode: Vec<AdainResBlk1d>,
    pub generator: Generator,
}

pub struct DecoderTrace {
    /// The stride-2 curves, `[1, frames]` each.
    pub f0: Mat,
    pub noise: Mat,
    pub asr_res: Mat,
    pub encode: Mat,
    /// Output of each `decode` block; the last is `[512, 2 * frames]`.
    pub decode: Vec<Mat>,
    pub generator: GeneratorTrace,
}

impl Decoder {
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
    pub(crate) fn drop_host_copies(&mut self) {
        self.f0_conv.drop_host_copies();
        self.n_conv.drop_host_copies();
        self.asr_res.drop_host_copies();
        self.encode.drop_host_copies();
        for block in &mut self.decode { block.drop_host_copies(); }
        self.generator.drop_host_copies();
    }

    pub fn load(weights: &Weights) -> Result<Self, TtsError> {
        let root = "decoder.module";

        // (dim_in, dim_out, upsample) — only the last block doubles the rate.
        let decode_shape = [
            (1090, 1024, false),
            (1090, 1024, false),
            (1090, 1024, false),
            (1090, 512, true),
        ];
        let decode = decode_shape
            .iter()
            .enumerate()
            .map(|(index, (dim_in, dim_out, upsample))| {
                AdainResBlk1d::load(
                    weights,
                    &format!("{root}.decode.{index}"),
                    *dim_in,
                    *dim_out,
                    *upsample,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        debug_assert_eq!(decode.len(), BLOCKS);

        Ok(Self {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            vulkan_id: std::sync::Arc::new(()),
            f0_conv: Conv::normed(weights, &format!("{root}.F0_conv"), 1, 1, 2, 1)?,
            n_conv: Conv::normed(weights, &format!("{root}.N_conv"), 1, 1, 2, 1)?,
            asr_res: Conv::normed(weights, &format!("{root}.asr_res.0"), 64, 0, 1, 1)?,
            // 512 (asr) + 1 (F0) + 1 (N).
            encode: AdainResBlk1d::load(weights, &format!("{root}.encode"), 514, 1024, false)?,
            decode,
            generator: Generator::load(weights, &format!("{root}.generator"))?,
        })
    }

    /// `asr` is `[512, frames]`, the alignment-expanded text encoding. `f0_curve`
    /// and `noise_curve` are `[1, 2 * frames]`. `style` is the *first* 128-wide
    /// half of the voice vector.
    pub fn run(&self, asr: &Mat, f0_curve: &Mat, noise_curve: &Mat, style: &[f32]) -> DecoderTrace {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(out) = super::vulkan::decode(self, asr, f0_curve, noise_curve, style, true) { return out; }
        let f0 = self.f0_conv.forward(f0_curve);
        let noise = self.n_conv.forward(noise_curve);
        let asr_res = self.asr_res.forward(asr);

        let mut x = self.encode.forward(&concat_rows(&[asr, &f0, &noise]), style);
        let encode = x.clone();

        let mut decode = Vec::with_capacity(BLOCKS);
        for block in &self.decode {
            x = block.forward(&concat_rows(&[&x, &asr_res, &f0, &noise]), style);
            decode.push(x.clone());
        }

        let generator = self.generator.run(&x, style, f0_curve);
        DecoderTrace {
            f0,
            noise,
            asr_res,
            encode,
            decode,
            generator,
        }
    }

    /// Normal synthesis reads back only the final spectrum. Diagnostic traces
    /// still use `run` and retain every documented checkpoint.
    pub fn waveform(&self, asr: &Mat, f0: &Mat, noise: &Mat, style: &[f32]) -> Vec<f32> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(out) = super::vulkan::decode(self, asr, f0, noise, style, false) { return out.generator.waveform; }
        self.run(asr, f0, noise, style).generator.waveform
    }
}
