//! PL-BERT: an ALBERT encoder over the same phoneme tokens, whose hidden states
//! condition the prosody predictor.
//!
//! ALBERT shares one layer's weights across all twelve iterations, which is why
//! 12 layers cost only 6.29M parameters.

use super::ops::{add_into, gelu_new, layer_norm_rows, linear, softmax, Mat};
use super::weights::{WeightData, Weights};
use crate::TtsError;

const HIDDEN: usize = 768;
const EMBED: usize = 128;
const HEADS: usize = 12;
const HEAD_DIM: usize = HIDDEN / HEADS;
const INTERMEDIATE: usize = 2048;
const LAYERS: usize = 12;
/// BERT's epsilon, distinct from StyleTTS2's 1e-5. Both appear in the export.
const LAYER_NORM_EPS: f32 = 1e-12;

pub(super) struct AlbertLayer {
    pub(super) query: (WeightData, WeightData),
    pub(super) key: (WeightData, WeightData),
    pub(super) value: (WeightData, WeightData),
    pub(super) dense: (WeightData, WeightData),
    pub(super) attention_norm: (WeightData, WeightData),
    pub(super) ffn: (WeightData, WeightData),
    pub(super) ffn_output: (WeightData, WeightData),
    pub(super) full_norm: (WeightData, WeightData),
}

pub struct Bert {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(super) vulkan_id: std::sync::Arc<()>,
    word_embeddings: WeightData,
    position_embeddings: WeightData,
    token_type_embeddings: WeightData,
    embedding_norm: (WeightData, WeightData),
    pub(super) mapping: (WeightData, WeightData),
    pub(super) layer: AlbertLayer,
    /// `bert_encoder`: 768 -> 512, feeding the prosody predictor.
    pub(super) encoder: (WeightData, WeightData),
}

pub struct Trace {
    /// One `[time, 768]` per ALBERT iteration.
    pub layers: Vec<Mat>,
    /// `[time, 512]` after `bert_encoder`.
    pub output: Mat,
}

fn need(weights: &Weights, name: &str) -> Result<WeightData, TtsError> {
    weights
        .take(name)
        .ok_or_else(|| TtsError::Backend(format!("missing tensor {name}")))
}

fn pair(weights: &Weights, prefix: &str) -> Result<(WeightData, WeightData), TtsError> {
    Ok((
        need(weights, &format!("{prefix}.weight"))?,
        need(weights, &format!("{prefix}.bias"))?,
    ))
}

impl Bert {
    pub fn load(weights: &Weights) -> Result<Self, TtsError> {
        let root = "bert.module";
        let layer = format!("{root}.encoder.albert_layer_groups.0.albert_layers.0");

        Ok(Self {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            vulkan_id: std::sync::Arc::new(()),
            word_embeddings: need(weights, &format!("{root}.embeddings.word_embeddings.weight"))?,
            position_embeddings: need(
                weights,
                &format!("{root}.embeddings.position_embeddings.weight"),
            )?,
            token_type_embeddings: need(
                weights,
                &format!("{root}.embeddings.token_type_embeddings.weight"),
            )?,
            embedding_norm: pair(weights, &format!("{root}.embeddings.LayerNorm"))?,
            mapping: pair(weights, &format!("{root}.encoder.embedding_hidden_mapping_in"))?,
            layer: AlbertLayer {
                query: pair(weights, &format!("{layer}.attention.query"))?,
                key: pair(weights, &format!("{layer}.attention.key"))?,
                value: pair(weights, &format!("{layer}.attention.value"))?,
                dense: pair(weights, &format!("{layer}.attention.dense"))?,
                attention_norm: pair(weights, &format!("{layer}.attention.LayerNorm"))?,
                ffn: pair(weights, &format!("{layer}.ffn"))?,
                ffn_output: pair(weights, &format!("{layer}.ffn_output"))?,
                full_norm: pair(weights, &format!("{layer}.full_layer_layer_norm"))?,
            },
            encoder: pair(weights, "bert_encoder.module")?,
        })
    }

    /// `[time, 128]`: word + position + token-type, normalized.
    fn embeddings(&self, tokens: &[u16]) -> Mat {
        let time = tokens.len();
        let mut out = Mat::zeros(time, EMBED);
        for (t, token) in tokens.iter().enumerate() {
            let word = *token as usize * EMBED;
            let position = t * EMBED;
            let row = out.row_mut(t);
            for c in 0..EMBED {
                // Token type is always 0 here: one segment.
                row[c] = self.word_embeddings[word + c]
                    + self.position_embeddings[position + c]
                    + self.token_type_embeddings[c];
            }
        }
        layer_norm_rows(&out, &self.embedding_norm.0, &self.embedding_norm.1, LAYER_NORM_EPS)
    }

    fn attention(&self, x: &Mat) -> Mat {
        let time = x.rows;
        let query = linear(x, &self.layer.query.0, &self.layer.query.1, HIDDEN);
        let key = linear(x, &self.layer.key.0, &self.layer.key.1, HIDDEN);
        let value = linear(x, &self.layer.value.0, &self.layer.value.1, HIDDEN);

        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        let mut context = Mat::zeros(time, HIDDEN);
        let mut scores = vec![0.0f32; time];

        for head in 0..HEADS {
            let offset = head * HEAD_DIM;
            for i in 0..time {
                let q = &query.row(i)[offset..offset + HEAD_DIM];
                for (j, score) in scores.iter_mut().enumerate() {
                    let k = &key.row(j)[offset..offset + HEAD_DIM];
                    *score = q.iter().zip(k).map(|(a, b)| a * b).sum::<f32>() * scale;
                }
                // No attention mask: batch of one, no padding beyond the sequence.
                softmax(&mut scores);

                let target = &mut context.row_mut(i)[offset..offset + HEAD_DIM];
                for (j, probability) in scores.iter().enumerate() {
                    let v = &value.row(j)[offset..offset + HEAD_DIM];
                    for (acc, vi) in target.iter_mut().zip(v) {
                        *acc += probability * vi;
                    }
                }
            }
        }

        // Project, add the residual from the layer input, then normalize.
        let mut projected = linear(&context, &self.layer.dense.0, &self.layer.dense.1, HIDDEN);
        add_into(&mut projected, x);
        layer_norm_rows(
            &projected,
            &self.layer.attention_norm.0,
            &self.layer.attention_norm.1,
            LAYER_NORM_EPS,
        )
    }

    fn albert_layer(&self, x: &Mat) -> Mat {
        let attended = self.attention(x);

        let mut hidden = linear(&attended, &self.layer.ffn.0, &self.layer.ffn.1, INTERMEDIATE);
        for value in &mut hidden.data {
            *value = gelu_new(*value);
        }
        let mut out = linear(
            &hidden,
            &self.layer.ffn_output.0,
            &self.layer.ffn_output.1,
            HIDDEN,
        );
        add_into(&mut out, &attended);
        layer_norm_rows(&out, &self.layer.full_norm.0, &self.layer.full_norm.1, LAYER_NORM_EPS)
    }

    pub fn forward(&self, tokens: &[u16]) -> Mat {
        self.trace(tokens).output
    }

    pub fn trace(&self, tokens: &[u16]) -> Trace {
        let embedded = self.embeddings(tokens);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(out) = super::vulkan::bert(self, &embedded, LAYERS) { return out; }
        let mut hidden = linear(&embedded, &self.mapping.0, &self.mapping.1, HIDDEN);

        let mut layers = Vec::with_capacity(LAYERS);
        for _ in 0..LAYERS {
            hidden = self.albert_layer(&hidden);
            layers.push(hidden.clone());
        }

        let output = linear(&hidden, &self.encoder.0, &self.encoder.1, 512);
        Trace { layers, output }
    }
}
