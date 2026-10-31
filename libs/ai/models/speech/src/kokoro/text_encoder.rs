//! Kokoro's text encoder: phoneme embedding, three weight-normed Conv1d blocks
//! each followed by channel LayerNorm and LeakyReLU, then a bidirectional LSTM.

use super::ops::{conv1d, embed, layer_norm_channels, leaky_relu, BiLstm, LstmLayer, Mat};
use super::weights::{WeightData, Weights};
use crate::TtsError;

const EMBED_DIM: usize = 512;
const CHANNELS: usize = 512;
const KERNEL: usize = 5;
const PAD: usize = (KERNEL - 1) / 2;
const LEAKY_SLOPE: f32 = 0.2;
const LAYER_NORM_EPS: f32 = 1e-5;
const BLOCKS: usize = 3;

pub(super) struct ConvBlock {
    pub(super) weight: WeightData,
    pub(super) bias: WeightData,
    pub(super) gamma: WeightData,
    pub(super) beta: WeightData,
}

pub struct TextEncoder {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(super) vulkan_id: std::sync::Arc<()>,
    embedding: WeightData,
    pub(super) blocks: Vec<ConvBlock>,
    lstm: BiLstm,
}

/// Every intermediate, so each stage can be diffed against the ONNX dump.
pub struct Trace {
    pub steps: Vec<(String, Mat)>,
    /// `[time, 512]` — forward and reverse LSTM states concatenated.
    pub output: Mat,
}

fn need(weights: &Weights, name: &str) -> Result<WeightData, TtsError> {
    weights
        .take(name)
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
        weight_ih: need(weights, &format!("{prefix}.weight_ih_l0{suffix}"))?,
        weight_hh: need(weights, &format!("{prefix}.weight_hh_l0{suffix}"))?,
        bias_ih: need(weights, &format!("{prefix}.bias_ih_l0{suffix}"))?,
        bias_hh: need(weights, &format!("{prefix}.bias_hh_l0{suffix}"))?,
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
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
    pub(crate) fn drop_host_copies(&mut self) {
        for block in &mut self.blocks { block.weight.drop_host_copy(); }
    }

    fn run_lstm(&self, x: &Mat) -> Mat {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(out) = super::vulkan::bilstm(&self.vulkan_id, &self.lstm, x) { return out; }
        self.lstm.run(x)
    }
    pub fn load(weights: &Weights) -> Result<Self, TtsError> {
        let root = "text_encoder.module";

        let mut blocks = Vec::with_capacity(BLOCKS);
        for index in 0..BLOCKS {
            blocks.push(ConvBlock {
                weight: weights.weight_norm(&format!("{root}.cnn.{index}.0"))?,
                bias: need(weights, &format!("{root}.cnn.{index}.0.bias"))?,
                gamma: need(weights, &format!("{root}.cnn.{index}.1.gamma"))?,
                beta: need(weights, &format!("{root}.cnn.{index}.1.beta"))?,
            });
        }

        Ok(Self {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            vulkan_id: std::sync::Arc::new(()),
            embedding: need(weights, &format!("{root}.embedding.weight"))?,
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

        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(gpu_steps) = super::vulkan::text_convs(self, &x) {
            x = gpu_steps.last().unwrap().1.clone();
            steps.extend(gpu_steps);
            return Trace { steps, output: self.run_lstm(&x.transpose()) };
        }

        for (index, block) in self.blocks.iter().enumerate() {
            x = conv1d(&x, &block.weight, &block.bias, CHANNELS, PAD);
            steps.push((format!("conv{index}"), x.clone()));

            x = layer_norm_channels(&x, &block.gamma, &block.beta, LAYER_NORM_EPS);
            steps.push((format!("norm{index}"), x.clone()));

            leaky_relu(&mut x, LEAKY_SLOPE);
            steps.push((format!("act{index}"), x.clone()));
        }

        // The LSTM wants [time, channels].
        let output = self.run_lstm(&x.transpose());
        Trace {
            steps,
            output: output.clone(),
        }
    }
}
