//! Woosh text conditioner: RoBERTa-large (HF RobertaModel), CPU f32.
//!
//! The conditioner returns `hidden_states[-2]` — the output of layer 23 of
//! 24 — so only the first 23 encoder layers are executed (the pooler and the
//! CLAP sentence_head in the checkpoint are unused for conditioning).
//!
//! Post-LN architecture, all LayerNorms eps 1e-5, exact-erf GELU, 16 heads x
//! 64, learned absolute positions with RoBERTa's offset-by-pad-index scheme,
//! and a key-padding mask (pad queries still produce rows — the reference
//! encodes the full 77-token batch and the DiT later replaces masked rows
//! with its learned description_pad, so all 77 output rows matter for
//! parity).
//!
//! Weights: `checkpoints/TextConditionerA/weights.safetensors`, prefix
//! `sentence_frontend.`.

use crate::error::{DiffusionError, Result};
use crate::sa3::{linear, par_rows, Sa3Tensors};
use crate::woosh::{
    gelu_erf, WOOSH_DESC_TOKENS, WOOSH_TE_FFN, WOOSH_TE_HEADS, WOOSH_TE_HEAD_DIM,
    WOOSH_TE_HIDDEN, WOOSH_TE_LAYERS_RUN, WOOSH_TE_LN_EPS, WOOSH_TE_PAD_ID,
};
use crate::{emit_progress, ProgressHook};

struct TeLayer {
    q_w: Vec<f32>,
    q_b: Vec<f32>,
    k_w: Vec<f32>,
    k_b: Vec<f32>,
    v_w: Vec<f32>,
    v_b: Vec<f32>,
    attn_out_w: Vec<f32>,
    attn_out_b: Vec<f32>,
    attn_ln_w: Vec<f32>,
    attn_ln_b: Vec<f32>,
    inter_w: Vec<f32>,
    inter_b: Vec<f32>,
    out_w: Vec<f32>,
    out_b: Vec<f32>,
    out_ln_w: Vec<f32>,
    out_ln_b: Vec<f32>,
}

pub struct WooshTextEncoder {
    word_embed: Vec<f32>,
    position_embed: Vec<f32>,
    token_type_embed: Vec<f32>,
    embed_ln_w: Vec<f32>,
    embed_ln_b: Vec<f32>,
    layers: Vec<TeLayer>,
}

impl WooshTextEncoder {
    pub fn load(weights: &Sa3Tensors) -> Result<Self> {
        let h = WOOSH_TE_HIDDEN;
        let get = |name: &str, len: usize| -> Result<Vec<f32>> {
            let full = format!("sentence_frontend.{name}");
            let v = weights.f32(&full)?;
            if v.len() != len {
                return Err(DiffusionError::model(format!(
                    "woosh te tensor {full}: {} values, expected {len}",
                    v.len()
                )));
            }
            Ok(v)
        };
        let mut layers = Vec::with_capacity(WOOSH_TE_LAYERS_RUN);
        for l in 0..WOOSH_TE_LAYERS_RUN {
            let p = format!("encoder.layer.{l}");
            layers.push(TeLayer {
                q_w: get(&format!("{p}.attention.self.query.weight"), h * h)?,
                q_b: get(&format!("{p}.attention.self.query.bias"), h)?,
                k_w: get(&format!("{p}.attention.self.key.weight"), h * h)?,
                k_b: get(&format!("{p}.attention.self.key.bias"), h)?,
                v_w: get(&format!("{p}.attention.self.value.weight"), h * h)?,
                v_b: get(&format!("{p}.attention.self.value.bias"), h)?,
                attn_out_w: get(&format!("{p}.attention.output.dense.weight"), h * h)?,
                attn_out_b: get(&format!("{p}.attention.output.dense.bias"), h)?,
                attn_ln_w: get(&format!("{p}.attention.output.LayerNorm.weight"), h)?,
                attn_ln_b: get(&format!("{p}.attention.output.LayerNorm.bias"), h)?,
                inter_w: get(&format!("{p}.intermediate.dense.weight"), WOOSH_TE_FFN * h)?,
                inter_b: get(&format!("{p}.intermediate.dense.bias"), WOOSH_TE_FFN)?,
                out_w: get(&format!("{p}.output.dense.weight"), h * WOOSH_TE_FFN)?,
                out_b: get(&format!("{p}.output.dense.bias"), h)?,
                out_ln_w: get(&format!("{p}.output.LayerNorm.weight"), h)?,
                out_ln_b: get(&format!("{p}.output.LayerNorm.bias"), h)?,
            });
        }
        Ok(Self {
            word_embed: weights.f32("sentence_frontend.embeddings.word_embeddings.weight")?,
            position_embed: get("embeddings.position_embeddings.weight", 514 * h)?,
            token_type_embed: get("embeddings.token_type_embeddings.weight", h)?,
            embed_ln_w: get("embeddings.LayerNorm.weight", h)?,
            embed_ln_b: get("embeddings.LayerNorm.bias", h)?,
            layers,
        })
    }

    /// Encodes the fixed 77-token batch. `ids`/`mask` come from
    /// [`crate::woosh_tokenizer::WooshTokenizer::encode_padded`]. Returns the
    /// full 77 x 1024 hidden_states[-2]. `progress` ticks "text-encode k/23".
    pub fn encode(
        &self,
        ids: &[u32],
        mask: &[f32],
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let h = WOOSH_TE_HIDDEN;
        let seq = WOOSH_DESC_TOKENS;
        if ids.len() != seq || mask.len() != seq {
            return Err(DiffusionError::model(format!(
                "woosh te: {}/{} inputs, expected {seq}",
                ids.len(),
                mask.len()
            )));
        }

        // Embeddings: word + learned position + token_type(0), then LN.
        // RoBERTa position ids: cumsum(not-pad) * not-pad + pad_index.
        let mut x = vec![0f32; seq * h];
        let mut running = 0u32;
        for (row, &id) in ids.iter().enumerate() {
            let not_pad = id != WOOSH_TE_PAD_ID;
            if not_pad {
                running += 1;
            }
            let pos_id = if not_pad { running + WOOSH_TE_PAD_ID } else { WOOSH_TE_PAD_ID } as usize;
            let id = id as usize;
            if (id + 1) * h > self.word_embed.len() {
                return Err(DiffusionError::model(format!(
                    "woosh te: token id {id} out of embedding range"
                )));
            }
            let out = &mut x[row * h..(row + 1) * h];
            let word = &self.word_embed[id * h..(id + 1) * h];
            let pos = &self.position_embed[pos_id * h..(pos_id + 1) * h];
            for i in 0..h {
                out[i] = word[i] + pos[i] + self.token_type_embed[i];
            }
        }
        layer_norm_rows(&mut x, h, &self.embed_ln_w, &self.embed_ln_b);

        // Additive key-padding mask (HF: (1 - mask) * finfo.min).
        let key_bias: Vec<f32> = mask
            .iter()
            .map(|&m| if m > 0.5 { 0.0 } else { f32::MIN })
            .collect();

        let heads = WOOSH_TE_HEADS;
        let hd = WOOSH_TE_HEAD_DIM;
        let scale = 1.0 / (hd as f32).sqrt();

        for (index, layer) in self.layers.iter().enumerate() {
            emit_progress(
                &mut progress,
                &format!("text-encode {}/{}", index + 1, WOOSH_TE_LAYERS_RUN),
                index as f64 / WOOSH_TE_LAYERS_RUN as f64,
            )?;
            // --- self attention (post-LN) ---
            let q = linear(&x, &layer.q_w, Some(&layer.q_b), seq, h, h);
            let k = linear(&x, &layer.k_w, Some(&layer.k_b), seq, h, h);
            let v = linear(&x, &layer.v_w, Some(&layer.v_b), seq, h, h);
            let mut attn = vec![0f32; seq * h];
            par_rows(&mut attn, h, &|row, out_row| {
                let mut scores = vec![0f32; seq];
                for head in 0..heads {
                    let q_vec = &q[row * h + head * hd..][..hd];
                    let mut max_s = f32::NEG_INFINITY;
                    for (kr, score) in scores.iter_mut().enumerate() {
                        let k_vec = &k[kr * h + head * hd..][..hd];
                        let mut dot = 0f32;
                        for i in 0..hd {
                            dot += q_vec[i] * k_vec[i];
                        }
                        *score = dot * scale + key_bias[kr];
                        max_s = max_s.max(*score);
                    }
                    let mut denom = 0f32;
                    for score in scores.iter_mut() {
                        *score = (*score - max_s).exp();
                        denom += *score;
                    }
                    let inv = 1.0 / denom;
                    let out_vec = &mut out_row[head * hd..(head + 1) * hd];
                    out_vec.fill(0.0);
                    for (kr, &score) in scores.iter().enumerate() {
                        let w = score * inv;
                        let v_vec = &v[kr * h + head * hd..][..hd];
                        for i in 0..hd {
                            out_vec[i] += w * v_vec[i];
                        }
                    }
                }
            });
            let proj = linear(&attn, &layer.attn_out_w, Some(&layer.attn_out_b), seq, h, h);
            for (xv, pv) in x.iter_mut().zip(proj.iter()) {
                *xv += *pv;
            }
            layer_norm_rows(&mut x, h, &layer.attn_ln_w, &layer.attn_ln_b);

            // --- feed forward (post-LN) ---
            let mut hidden = linear(&x, &layer.inter_w, Some(&layer.inter_b), seq, h, WOOSH_TE_FFN);
            par_rows(&mut hidden, WOOSH_TE_FFN, &|_row, slice| {
                for value in slice.iter_mut() {
                    *value = gelu_erf(*value);
                }
            });
            let out = linear(&hidden, &layer.out_w, Some(&layer.out_b), seq, WOOSH_TE_FFN, h);
            for (xv, ov) in x.iter_mut().zip(out.iter()) {
                *xv += *ov;
            }
            layer_norm_rows(&mut x, h, &layer.out_ln_w, &layer.out_ln_b);
        }
        Ok(x)
    }
}

/// Affine LayerNorm over rows, eps [`WOOSH_TE_LN_EPS`].
fn layer_norm_rows(x: &mut [f32], d: usize, weight: &[f32], bias: &[f32]) {
    par_rows(x, d, &|_row, row| {
        let mut mean = 0f32;
        for v in row.iter() {
            mean += *v;
        }
        mean /= d as f32;
        let mut var = 0f32;
        for v in row.iter() {
            let dv = *v - mean;
            var += dv * dv;
        }
        var /= d as f32;
        let inv = 1.0 / (var + WOOSH_TE_LN_EPS).sqrt();
        for (i, v) in row.iter_mut().enumerate() {
            *v = (*v - mean) * inv * weight[i] + bias[i];
        }
    });
}
