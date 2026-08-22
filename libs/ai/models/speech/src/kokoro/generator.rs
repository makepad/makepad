//! ISTFTNet generator: turns the decoder's `[512, 2 * frames]` feature map into
//! a waveform, driven by a harmonic source built from the predicted F0.
//!
//! Two paths meet at each upsample stage. The learned path is the usual HiFi-GAN
//! ladder — `ConvTranspose1d` then three parallel residual blocks, averaged. The
//! source path takes the F0 curve, synthesizes 9 sinusoids, mixes them to one
//! excitation signal, and folds its STFT back in so the model never has to
//! invent pitch from noise.
//!
//! The export is a determinism-patched `SineGen`: upstream adds a random initial
//! phase to harmonics 2..9 and Gaussian noise to the excitation, and the graph
//! has neither. We restore the noise (see [`force_deterministic`]) because the
//! network needs it: the excitation's STFT phase channels feed `noise_convs`
//! as raw features, and with a noiseless source every bin away from a harmonic
//! is ~zero, leaving those phases to floating-point residue. Whatever structure
//! that residue has goes straight into the noise branch — in this
//! implementation it came out correlated across frames and rang at the iSTFT
//! frame rate (4.8/9.6 kHz tones) while losing the 10-12 kHz noise band.
//! Upstream's per-sample Gaussian noise is what the model was trained on and
//! gives those phases well-defined statistics on any implementation.
//!
//! The random *initial phase* is not restored: upstream adds it to
//! `rad_values[t = 0]` only, and the 1/300 linear downsample never samples
//! t = 0, so it is dead code there.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::adain::AdaIn1d;
use super::ops::{
    add_into, conv1d_general, conv_transpose1d, leaky_relu, nearest_repeat, reflect_pad,
    resize_linear, snake, Mat,
};
use super::stft::{istft, stft_mag_phase, BINS};
use super::weights::Weights;
use crate::TtsError;

const SAMPLE_RATE: f32 = 24_000.0;
/// `harmonic_num = 8`, plus the fundamental.
const HARMONICS: usize = 9;
/// Samples per F0 frame: the two upsamples (10 and 6) times the iSTFT hop (5).
const UPP: usize = 300;
const SINE_AMP: f32 = 0.1;
/// Upstream `SineGen(noise_std=0.003)`: the Gaussian floor under voiced frames.
/// Unvoiced frames get `sine_amp / 3` instead — the excitation there IS noise.
const NOISE_STD: f32 = 0.003;
const VOICED_THRESHOLD: f32 = 10.0;
const DILATIONS: [usize; 3] = [1, 3, 5];
const KERNELS: usize = 3;

static DETERMINISTIC: AtomicBool = AtomicBool::new(false);
/// Distinct noise per synthesis call, still reproducible within a process.
static NOISE_STREAM: AtomicU64 = AtomicU64::new(0);

/// Drop the excitation noise and match the traced graph exactly. Only the
/// parity harness wants this — it diffs against the deterministic ONNX dump.
pub fn force_deterministic(value: bool) {
    DETERMINISTIC.store(value, Ordering::Relaxed);
}

/// xorshift64* + Box-Muller: `randn_like` without a dependency. Statistical
/// whiteness is all the model needs; the exact generator is immaterial.
struct Gaussian {
    state: u64,
    spare: Option<f32>,
}

impl Gaussian {
    fn new(stream: u64) -> Self {
        // splitmix64 of the stream index, so consecutive streams decorrelate.
        let mut z = stream.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Self { state: (z ^ (z >> 31)) | 1, spare: None }
    }

    fn uniform(&mut self) -> f32 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let bits = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Top 24 bits into (0, 1]; never 0, so ln() below stays finite.
        ((bits >> 40) as f32 + 1.0) / 16_777_216.0
    }

    fn sample(&mut self) -> f32 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let radius = (-2.0 * self.uniform().ln()).sqrt();
        let angle = 2.0 * PI * self.uniform();
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }
}

fn need(weights: &Weights, name: &str) -> Result<Vec<f32>, TtsError> {
    weights
        .get(name)
        .map(<[f32]>::to_vec)
        .ok_or_else(|| TtsError::Backend(format!("missing tensor {name}")))
}

pub(crate) struct Conv {
    weight: Vec<f32>,
    bias: Vec<f32>,
    out_channels: usize,
    pad: usize,
    stride: usize,
    dilation: usize,
}

impl Conv {
    /// A weight-normed convolution: the checkpoint stores `weight_g`/`weight_v`.
    pub(crate) fn normed(
        weights: &Weights,
        prefix: &str,
        out_channels: usize,
        pad: usize,
        stride: usize,
        dilation: usize,
    ) -> Result<Self, TtsError> {
        Ok(Self {
            weight: weights.weight_norm(prefix)?,
            bias: need(weights, &format!("{prefix}.bias"))?,
            out_channels,
            pad,
            stride,
            dilation,
        })
    }

    /// A plain convolution. Only `noise_convs` are stored this way.
    fn plain(
        weights: &Weights,
        prefix: &str,
        out_channels: usize,
        pad: usize,
        stride: usize,
    ) -> Result<Self, TtsError> {
        Ok(Self {
            weight: need(weights, &format!("{prefix}.weight"))?,
            bias: need(weights, &format!("{prefix}.bias"))?,
            out_channels,
            pad,
            stride,
            dilation: 1,
        })
    }

    pub(crate) fn forward(&self, x: &Mat) -> Mat {
        conv1d_general(
            x,
            &self.weight,
            &self.bias,
            self.out_channels,
            self.pad,
            self.stride,
            self.dilation,
        )
    }
}

/// HiFi-GAN's `ResBlock1` with AdaIN conditioning and Snake activations.
///
/// Unlike [`super::adain::AdainResBlk1d`] the residual is a plain add — there is
/// no `1/sqrt(2)`.
struct AdaResBlock1 {
    adain1: Vec<AdaIn1d>,
    adain2: Vec<AdaIn1d>,
    convs1: Vec<Conv>,
    convs2: Vec<Conv>,
    alpha1: Vec<Vec<f32>>,
    alpha2: Vec<Vec<f32>>,
}

impl AdaResBlock1 {
    fn load(
        weights: &Weights,
        prefix: &str,
        channels: usize,
        kernel: usize,
    ) -> Result<Self, TtsError> {
        let mut block = Self {
            adain1: Vec::with_capacity(KERNELS),
            adain2: Vec::with_capacity(KERNELS),
            convs1: Vec::with_capacity(KERNELS),
            convs2: Vec::with_capacity(KERNELS),
            alpha1: Vec::with_capacity(KERNELS),
            alpha2: Vec::with_capacity(KERNELS),
        };
        for (index, dilation) in DILATIONS.iter().enumerate() {
            // `padding = dilation * (kernel - 1) / 2` keeps the length; convs2 is
            // never dilated.
            block.convs1.push(Conv::normed(
                weights,
                &format!("{prefix}.convs1.{index}"),
                channels,
                dilation * (kernel - 1) / 2,
                1,
                *dilation,
            )?);
            block.convs2.push(Conv::normed(
                weights,
                &format!("{prefix}.convs2.{index}"),
                channels,
                (kernel - 1) / 2,
                1,
                1,
            )?);
            block
                .adain1
                .push(AdaIn1d::load(weights, &format!("{prefix}.adain1.{index}"), channels)?);
            block
                .adain2
                .push(AdaIn1d::load(weights, &format!("{prefix}.adain2.{index}"), channels)?);
            block.alpha1.push(need(weights, &format!("{prefix}.alpha1.{index}"))?);
            block.alpha2.push(need(weights, &format!("{prefix}.alpha2.{index}"))?);
        }
        Ok(block)
    }

    fn forward(&self, x: &Mat, style: &[f32]) -> Mat {
        let mut x = x.clone();
        for index in 0..KERNELS {
            let mut h = self.adain1[index].forward(&x, style);
            snake(&mut h, &self.alpha1[index]);
            let h = self.convs1[index].forward(&h);

            let mut h = self.adain2[index].forward(&h, style);
            snake(&mut h, &self.alpha2[index]);
            let mut h = self.convs2[index].forward(&h);

            add_into(&mut h, &x);
            x = h;
        }
        x
    }
}

/// A transposed convolution whose weight is `[in, out, kernel]`.
struct Upsample {
    weight: Vec<f32>,
    bias: Vec<f32>,
    out_channels: usize,
    stride: usize,
    pad: usize,
}

pub struct Generator {
    ups: Vec<Upsample>,
    noise_convs: Vec<Conv>,
    noise_res: Vec<AdaResBlock1>,
    resblocks: Vec<AdaResBlock1>,
    conv_post: Conv,
    /// `m_source.l_linear`: mixes the 9 sinusoids down to one excitation.
    source_weight: Vec<f32>,
    source_bias: f32,
}

/// Checkpoints the parity harness diffs against the ONNX dump.
pub struct GeneratorTrace {
    pub har_source: Vec<f32>,
    pub har: Mat,
    pub noise_convs: Vec<Mat>,
    pub ups: Vec<Mat>,
    pub reflect: Mat,
    /// After averaging the three residual blocks at each stage.
    pub merged: Vec<Mat>,
    pub post: Mat,
    pub waveform: Vec<f32>,
}

impl Generator {
    pub fn load(weights: &Weights, root: &str) -> Result<Self, TtsError> {
        // (channels, kernel) for the six residual blocks: three per upsample
        // stage, one per kernel size.
        let resblock_shape = [
            (256, 3),
            (256, 7),
            (256, 11),
            (128, 3),
            (128, 7),
            (128, 11),
        ];
        let resblocks = resblock_shape
            .iter()
            .enumerate()
            .map(|(index, (channels, kernel))| {
                AdaResBlock1::load(weights, &format!("{root}.resblocks.{index}"), *channels, *kernel)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let noise_res = [(256, 7), (128, 11)]
            .iter()
            .enumerate()
            .map(|(index, (channels, kernel))| {
                AdaResBlock1::load(weights, &format!("{root}.noise_res.{index}"), *channels, *kernel)
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            ups: vec![
                Upsample {
                    weight: weights.weight_norm(&format!("{root}.ups.0"))?,
                    bias: need(weights, &format!("{root}.ups.0.bias"))?,
                    out_channels: 256,
                    stride: 10,
                    pad: 5,
                },
                Upsample {
                    weight: weights.weight_norm(&format!("{root}.ups.1"))?,
                    bias: need(weights, &format!("{root}.ups.1.bias"))?,
                    out_channels: 128,
                    stride: 6,
                    pad: 3,
                },
            ],
            // `stride_f0` is the product of the remaining upsample rates, so the
            // source lands at exactly the stage's frame rate.
            noise_convs: vec![
                Conv::plain(weights, &format!("{root}.noise_convs.0"), 256, 3, 6)?,
                Conv::plain(weights, &format!("{root}.noise_convs.1"), 128, 0, 1)?,
            ],
            noise_res,
            resblocks,
            conv_post: Conv::normed(weights, &format!("{root}.conv_post"), 2 * BINS, 3, 1, 1)?,
            source_weight: need(weights, &format!("{root}.m_source.l_linear.weight"))?,
            source_bias: need(weights, &format!("{root}.m_source.l_linear.bias"))?[0],
        })
    }

    /// `SineGen` + `SourceModuleHnNSF`: F0 in samples, excitation out.
    ///
    /// The phase is never wrapped, so by the end of a few seconds it has grown to
    /// ~1e5 radians. Everything here stays in f32 in the graph's exact order —
    /// accumulating in f64 would be *more* accurate and match the reference
    /// *less*.
    fn source(&self, f0: &Mat) -> Vec<f32> {
        let samples = f0.cols;
        let f0 = f0.row(0);

        // Harmonics multiply the frequency before it becomes a phase increment.
        let mut radians = Mat::zeros(HARMONICS, samples);
        for harmonic in 0..HARMONICS {
            let multiple = (harmonic + 1) as f32;
            let row = radians.row_mut(harmonic);
            for (t, cell) in row.iter_mut().enumerate() {
                let turns = f0[t] * multiple / SAMPLE_RATE;
                // Floor-mod, so unvoiced frames with a negative F0 still land in
                // [0, 1). They contribute to the cumulative sum; only the final
                // mix is gated by `uv`.
                *cell = turns - turns.floor();
            }
        }

        // Accumulate at frame rate, then stretch back out: 1/300 of the additions,
        // and a phase that stays smooth across the frame boundary.
        let mut phase = resize_linear(&radians, 1.0 / UPP as f32);
        for harmonic in 0..HARMONICS {
            let mut total = 0.0f32;
            for cell in phase.row_mut(harmonic) {
                total += *cell;
                *cell = total * 2.0 * PI * UPP as f32;
            }
        }
        let phase = resize_linear(&phase, UPP as f32);

        // Upstream: `noise_amp = uv * noise_std + (1 - uv) * sine_amp / 3`,
        // `sine_waves = sine_waves * uv + noise_amp * randn_like(sine_waves)`,
        // per sample and per harmonic channel, before the linear mix.
        let noise_gain = if DETERMINISTIC.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
        let mut noise = Gaussian::new(NOISE_STREAM.fetch_add(1, Ordering::Relaxed));

        let mut excitation = vec![0.0; samples];
        for (t, sample) in excitation.iter_mut().enumerate() {
            let voiced = if f0[t] > VOICED_THRESHOLD { 1.0 } else { 0.0 };
            let noise_amp =
                noise_gain * (voiced * NOISE_STD + (1.0 - voiced) * SINE_AMP / 3.0);
            let mut sum = self.source_bias;
            for harmonic in 0..HARMONICS {
                let sine = phase.at(harmonic, t).sin() * SINE_AMP * voiced;
                sum += (sine + noise_amp * noise.sample()) * self.source_weight[harmonic];
            }
            *sample = sum.tanh();
        }
        excitation
    }

    /// `x` is `[512, 2 * frames]`; `f0_curve` is the raw `[1, 2 * frames]` pitch,
    /// not the stride-2 version the decoder concatenates.
    pub fn run(&self, x: &Mat, style: &[f32], f0_curve: &Mat) -> GeneratorTrace {
        let har_source = self.source(&nearest_repeat(f0_curve, UPP));
        let har = stft_mag_phase(&har_source);
        self.run_from_har(x, style, har_source, har)
    }

    /// Everything after the harmonic source's STFT, with `har` supplied by the
    /// caller. The parity harness uses this to run the network on the ONNX
    /// reference's own `har`, separating STFT phase noise from network bugs.
    pub fn run_from_har(
        &self,
        x: &Mat,
        style: &[f32],
        har_source: Vec<f32>,
        har: Mat,
    ) -> GeneratorTrace {
        let mut trace = GeneratorTrace {
            har_source,
            har,
            noise_convs: Vec::with_capacity(2),
            ups: Vec::with_capacity(2),
            reflect: Mat::zeros(0, 0),
            merged: Vec::with_capacity(2),
            post: Mat::zeros(0, 0),
            waveform: Vec::new(),
        };

        let mut x = x.clone();
        for stage in 0..self.ups.len() {
            leaky_relu(&mut x, 0.1);

            let projected = self.noise_convs[stage].forward(&trace.har);
            trace.noise_convs.push(projected.clone());
            let source = self.noise_res[stage].forward(&projected, style);

            let up = &self.ups[stage];
            let mut y = conv_transpose1d(&x, &up.weight, &up.bias, up.out_channels, up.stride, up.pad);
            trace.ups.push(y.clone());

            // The last stage is one frame short of the source; reflecting a single
            // sample on the left fixes it up.
            if stage == self.ups.len() - 1 {
                y = reflect_pad(&y, 1, 0);
                trace.reflect = y.clone();
            }
            add_into(&mut y, &source);

            let mut merged = self.resblocks[stage * KERNELS].forward(&y, style);
            for kernel in 1..KERNELS {
                add_into(&mut merged, &self.resblocks[stage * KERNELS + kernel].forward(&y, style));
            }
            for value in merged.data.iter_mut() {
                *value /= KERNELS as f32;
            }
            trace.merged.push(merged.clone());
            x = merged;
        }

        leaky_relu(&mut x, 0.01);
        let post = self.conv_post.forward(&x);

        let frames = post.cols;
        let mut re_im = Mat::zeros(2 * BINS, frames);
        for bin in 0..BINS {
            for t in 0..frames {
                let magnitude = post.at(bin, t).exp();
                let phase = post.at(BINS + bin, t).sin();
                re_im.data[bin * frames + t] = magnitude * phase.cos();
                re_im.data[(BINS + bin) * frames + t] = magnitude * phase.sin();
            }
        }

        trace.waveform = istft(&re_im);
        trace.post = post;
        trace
    }
}
