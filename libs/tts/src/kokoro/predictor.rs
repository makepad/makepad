//! Prosody predictor: how long each phoneme lasts, and its pitch and energy.
//!
//! The duration half is a `DurationEncoder` — three (BiLSTM → AdaLayerNorm →
//! re-concat style) blocks — followed by one more BiLSTM and a 50-way head whose
//! sigmoid is summed to give a duration in frames per phoneme.

use super::adain::AdainResBlk1d;
use super::ops::{concat_cols, conv1d, layer_norm_plain, linear, sigmoid_slice, BiLstm, Mat};
use super::text_encoder::load_bilstm;
use super::weights::Weights;
use crate::TtsError;

const CHANNELS: usize = 512;
const STYLE: usize = 128;
/// `d_model + style`, the LSTM input width.
const WIDE: usize = CHANNELS + STYLE;
const MAX_DUR: usize = 50;
/// StyleTTS2's LayerNorm epsilon, not BERT's.
const LAYER_NORM_EPS: f32 = 1e-5;
const BLOCKS: usize = 3;

/// `(1 + gamma) * layer_norm(x) + beta`, where gamma and beta come from style.
struct AdaLayerNorm {
    fc_weight: Vec<f32>,
    fc_bias: Vec<f32>,
}

impl AdaLayerNorm {
    fn forward(&self, x: &Mat, style: &[f32]) -> Mat {
        // fc: [2 * channels, style]
        let style_row = Mat::from_vec(1, STYLE, style.to_vec());
        let h = linear(&style_row, &self.fc_weight, &self.fc_bias, 2 * CHANNELS);
        let (gamma, beta) = h.data.split_at(CHANNELS);

        let mut out = layer_norm_plain(x, LAYER_NORM_EPS);
        for row in 0..out.rows {
            let target = out.row_mut(row);
            for c in 0..CHANNELS {
                target[c] = (1.0 + gamma[c]) * target[c] + beta[c];
            }
        }
        out
    }
}

pub struct Predictor {
    lstms: Vec<BiLstm>,
    norms: Vec<AdaLayerNorm>,
    lstm: BiLstm,
    duration_proj: (Vec<f32>, Vec<f32>),
    /// Shared BiLSTM feeding both the pitch and noise branches.
    shared: BiLstm,
    f0_blocks: Vec<AdainResBlk1d>,
    noise_blocks: Vec<AdainResBlk1d>,
    f0_proj: (Vec<f32>, Vec<f32>),
    noise_proj: (Vec<f32>, Vec<f32>),
}

/// The two curves, each `[1, 2 * frames]` — block 1 of each branch upsamples,
/// so they come out at twice the alignment's frame rate.
pub struct ProsodyTrace {
    pub shared: Mat,
    pub f0_blocks: Vec<Mat>,
    pub noise_blocks: Vec<Mat>,
    pub f0: Mat,
    pub noise: Mat,
}

pub struct DurationTrace {
    /// After each block's LSTM, `[time, 512]`.
    pub lstm_out: Vec<Mat>,
    /// After each block's AdaLayerNorm, `[time, 512]`.
    pub norm_out: Vec<Mat>,
    /// `d`, the DurationEncoder output, `[time, 640]`.
    pub encoded: Mat,
    /// After `predictor.lstm`, `[time, 512]`.
    pub hidden: Mat,
    /// `[time, 50]` before and after the sigmoid.
    pub duration_logits: Mat,
    pub duration_gates: Mat,
    /// Frames per phoneme: the row-sum of the gates.
    pub durations: Vec<f32>,
}

fn need(weights: &Weights, name: &str) -> Result<Vec<f32>, TtsError> {
    weights
        .get(name)
        .map(<[f32]>::to_vec)
        .ok_or_else(|| TtsError::Backend(format!("missing tensor {name}")))
}

impl Predictor {
    pub fn load(weights: &Weights) -> Result<Self, TtsError> {
        let root = "predictor.module";

        let mut lstms = Vec::with_capacity(BLOCKS);
        let mut norms = Vec::with_capacity(BLOCKS);
        for block in 0..BLOCKS {
            // LSTMs sit at even indices, AdaLayerNorms at odd.
            let lstm_index = block * 2;
            let norm_index = block * 2 + 1;
            lstms.push(load_bilstm(
                weights,
                &format!("{root}.text_encoder.lstms.{lstm_index}"),
                WIDE,
                CHANNELS / 2,
            )?);
            norms.push(AdaLayerNorm {
                fc_weight: need(
                    weights,
                    &format!("{root}.text_encoder.lstms.{norm_index}.fc.weight"),
                )?,
                fc_bias: need(
                    weights,
                    &format!("{root}.text_encoder.lstms.{norm_index}.fc.bias"),
                )?,
            });
        }

        // (dim_in, dim_out, upsample). Block 1 halves the channels and doubles time.
        let branch_shape = [(512, 512, false), (512, 256, true), (256, 256, false)];
        let branch = |name: &str| -> Result<Vec<AdainResBlk1d>, TtsError> {
            branch_shape
                .iter()
                .enumerate()
                .map(|(index, (dim_in, dim_out, upsample))| {
                    AdainResBlk1d::load(
                        weights,
                        &format!("{root}.{name}.{index}"),
                        *dim_in,
                        *dim_out,
                        *upsample,
                    )
                })
                .collect()
        };

        Ok(Self {
            lstms,
            norms,
            lstm: load_bilstm(weights, &format!("{root}.lstm"), WIDE, CHANNELS / 2)?,
            duration_proj: (
                need(weights, &format!("{root}.duration_proj.linear_layer.weight"))?,
                need(weights, &format!("{root}.duration_proj.linear_layer.bias"))?,
            ),
            shared: load_bilstm(weights, &format!("{root}.shared"), WIDE, CHANNELS / 2)?,
            f0_blocks: branch("F0")?,
            noise_blocks: branch("N")?,
            f0_proj: (
                need(weights, &format!("{root}.F0_proj.weight"))?,
                need(weights, &format!("{root}.F0_proj.bias"))?,
            ),
            noise_proj: (
                need(weights, &format!("{root}.N_proj.weight"))?,
                need(weights, &format!("{root}.N_proj.bias"))?,
            ),
        })
    }

    /// Pitch and energy curves. `en` is `[640, frames]`, the alignment-expanded
    /// DurationEncoder output; `style` is the same second half of the voice vector.
    pub fn prosody(&self, en: &Mat, style: &[f32]) -> ProsodyTrace {
        // The shared LSTM wants [frames, 640] and gives [frames, 512].
        let shared = self.shared.run(&en.transpose());
        let start = shared.transpose();

        let run = |blocks: &Vec<AdainResBlk1d>, proj: &(Vec<f32>, Vec<f32>)| {
            let mut traces = Vec::with_capacity(blocks.len());
            let mut x = start.clone();
            for block in blocks {
                x = block.forward(&x, style);
                traces.push(x.clone());
            }
            // 1x1 conv down to a single curve.
            let curve = conv1d(&x, &proj.0, &proj.1, 1, 0);
            (traces, curve)
        };

        let (f0_blocks, f0) = run(&self.f0_blocks, &self.f0_proj);
        let (noise_blocks, noise) = run(&self.noise_blocks, &self.noise_proj);

        ProsodyTrace {
            shared,
            f0_blocks,
            noise_blocks,
            f0,
            noise,
        }
    }

    /// `bert_out` is `[time, 512]` from `bert_encoder`; `style` is the *second*
    /// 128-wide half of the voice vector (the decoder gets the first).
    pub fn durations(&self, bert_out: &Mat, style: &[f32]) -> DurationTrace {
        let mut x = concat_cols(bert_out, style);

        let mut lstm_out = Vec::with_capacity(BLOCKS);
        let mut norm_out = Vec::with_capacity(BLOCKS);
        for block in 0..BLOCKS {
            let hidden = self.lstms[block].run(&x);
            lstm_out.push(hidden.clone());

            let normed = self.norms[block].forward(&hidden, style);
            norm_out.push(normed.clone());

            x = concat_cols(&normed, style);
        }
        let encoded = x;

        let hidden = self.lstm.run(&encoded);
        let duration_logits = linear(
            &hidden,
            &self.duration_proj.0,
            &self.duration_proj.1,
            MAX_DUR,
        );

        // Duration is how many of the 50 slots are "on".
        let mut duration_gates = duration_logits.clone();
        sigmoid_slice(&mut duration_gates.data);
        let durations = (0..duration_gates.rows)
            .map(|t| duration_gates.row(t).iter().sum())
            .collect();

        DurationTrace {
            lstm_out,
            norm_out,
            encoded,
            hidden,
            duration_logits,
            duration_gates,
            durations,
        }
    }
}
