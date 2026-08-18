//! Kokoro's text encoder: phoneme embedding, three weight-normed Conv1d blocks
//! each followed by channel LayerNorm and LeakyReLU, then a bidirectional LSTM.

use super::ops::{conv1d, embed, layer_norm_channels, leaky_relu, BiLstm, LstmLayer, Mat};
use super::weights::Weights;
use crate::TtsError;

const EMBED_DIM: usize = 512;
const CHANNELS: usize = 512;
const KERNEL: usize = 5;
const PAD: usize = (KERNEL - 1) / 2;
const LEAKY_SLOPE: f32 = 0.2;
const LAYER_NORM_EPS: f32 = 1e-5;
const BLOCKS: usize = 3;

struct ConvBlock {
    weight: Vec<f32>,
    bias: Vec<f32>,
    gamma: Vec<f32>,
    beta: Vec<f32>,
}

pub struct TextEncoder {
    embedding: Vec<f32>,
    blocks: Vec<ConvBlock>,
    lstm: BiLstm,
}

/// Every intermediate, so each stage can be diffed against the ONNX dump.
pub struct Trace {
    pub steps: Vec<(String, Mat)>,
    /// `[time, 512]` — forward and reverse LSTM states concatenated.
    pub output: Mat,
}

fn need<'a>(weights: &'a Weights, name: &str) -> Result<&'a [f32], TtsError> {
    weights
        .get(name)
        .ok_or_else(|| TtsError::Backend(format!("missing tensor {name}")))
}

pub(crate) fn load_lstm(
    weights: &Weights,
    prefix: &str,
    input: usize,
    hidden: usize,
    reverse: bool,
) -> Result<LstmLayer, TtsError> {
    let suffix = if reverse { "_reverse" } else { "" };
    Ok(LstmLayer {
        hidden,
        input,
        weight_ih: need(weights, &format!("{prefix}.weight_ih_l0{suffix}"))?.to_vec(),
        weight_hh: need(weights, &format!("{prefix}.weight_hh_l0{suffix}"))?.to_vec(),
        bias_ih: need(weights, &format!("{prefix}.bias_ih_l0{suffix}"))?.to_vec(),
        bias_hh: need(weights, &format!("{prefix}.bias_hh_l0{suffix}"))?.to_vec(),
    })
}

pub(crate) fn load_bilstm(
    weights: &Weights,
    prefix: &str,
    input: usize,
    hidden: usize,
) -> Result<BiLstm, TtsError> {
    Ok(BiLstm {
        forward: load_lstm(weights, prefix, input, hidden, false)?,
        reverse: load_lstm(weights, prefix, input, hidden, true)?,
    })
}

impl TextEncoder {
    pub fn load(weights: &Weights) -> Result<Self, TtsError> {
        let root = "text_encoder.module";

        let mut blocks = Vec::with_capacity(BLOCKS);
        for index in 0..BLOCKS {
            blocks.push(ConvBlock {
                weight: weights.weight_norm(&format!("{root}.cnn.{index}.0"))?,
                bias: need(weights, &format!("{root}.cnn.{index}.0.bias"))?.to_vec(),
                gamma: need(weights, &format!("{root}.cnn.{index}.1.gamma"))?.to_vec(),
                beta: need(weights, &format!("{root}.cnn.{index}.1.beta"))?.to_vec(),
            });
        }

        Ok(Self {
            embedding: need(weights, &format!("{root}.embedding.weight"))?.to_vec(),
            blocks,
            // 512 in, 256 per direction: PyTorch's `hidden_size` is per direction.
            lstm: load_bilstm(weights, &format!("{root}.lstm"), CHANNELS, CHANNELS / 2)?,
        })
    }

    /// `tokens` are the zero-padded phoneme ids. Returns `[time, 512]`.
    pub fn forward(&self, tokens: &[u16]) -> Mat {
        self.trace(tokens).output
    }

    pub fn trace(&self, tokens: &[u16]) -> Trace {
        let mut steps = Vec::new();

        // [512, time] — channel-major, like PyTorch's [B, C, T].
        let mut x = embed(tokens, &self.embedding, EMBED_DIM);
        steps.push(("embed".to_string(), x.clone()));

        for (index, block) in self.blocks.iter().enumerate() {
            x = conv1d(&x, &block.weight, &block.bias, CHANNELS, PAD);
            steps.push((format!("conv{index}"), x.clone()));

            x = layer_norm_channels(&x, &block.gamma, &block.beta, LAYER_NORM_EPS);
            steps.push((format!("norm{index}"), x.clone()));

            leaky_relu(&mut x, LEAKY_SLOPE);
            steps.push((format!("act{index}"), x.clone()));
        }

        // The LSTM wants [time, channels].
        let output = self.lstm.run(&x.transpose());
        Trace {
            steps,
            output: output.clone(),
        }
    }
}
