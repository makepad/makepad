use crate::model::WhisperModel;
use crate::tensor::{parallel_for, SendPtr, Tensor};

const EPS: f32 = 1e-5;

/// KV cache for the decoder self-attention.
/// Stores K and V for each layer, growing as tokens are generated.
pub struct KvCache {
    /// Per-layer K cache: each is [n_past, n_state]
    pub k: Vec<Vec<f32>>,
    /// Per-layer V cache: each is [n_past, n_state]
    pub v: Vec<Vec<f32>>,
    pub n_past: usize,
}

impl KvCache {
    pub fn new(n_layers: usize) -> Self {
        KvCache {
            k: vec![Vec::new(); n_layers],
            v: vec![Vec::new(); n_layers],
            n_past: 0,
        }
    }

    pub fn clear(&mut self) {
        for k in &mut self.k {
            k.clear();
        }
        for v in &mut self.v {
            v.clear();
        }
        self.n_past = 0;
        crate::metal_backend::clear_decoder_kv_cache();
    }

    /// Append K, V rows for a layer
    fn append(&mut self, layer: usize, k_row: &[f32], v_row: &[f32]) {
        self.k[layer].extend_from_slice(k_row);
        self.v[layer].extend_from_slice(v_row);
    }

    /// Get cached K for a layer as [n_past + n_new, n_state] shaped data
    fn k_data(&self, layer: usize) -> &[f32] {
        &self.k[layer]
    }
    fn v_data(&self, layer: usize) -> &[f32] {
        &self.v[layer]
    }
}

/// Run one decoder step.
/// tokens: token IDs for this step (can be >1 for prompt processing)
/// kv_cache: self-attention KV cache (mutated)
/// cross_kv: pre-computed cross-attention K, V per layer
/// Returns logits: [n_tokens, n_vocab]
pub fn decode(
    model: &WhisperModel,
    tokens: &[i32],
    positions: &[i32],
    kv_cache: &mut KvCache,
    cross_kv: &[(Tensor, Tensor)],
) -> Tensor {
    let decoder_flash = std::env::var("MAKEPAD_VOICE_METAL_DECODER_FLASH")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
        })
        .unwrap_or(false);
    let decoder_flash_self = std::env::var("MAKEPAD_VOICE_METAL_DECODER_FLASH_SELF")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
        })
        .unwrap_or(decoder_flash);
    let decoder_flash_cross = std::env::var("MAKEPAD_VOICE_METAL_DECODER_FLASH_CROSS")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
        })
        .unwrap_or(true);

    let n_tokens = tokens.len();
    let n_state = model.hparams.n_text_state as usize;
    let n_head = model.hparams.n_text_head as usize;
    let n_state_head = n_state / n_head;

    // Token embedding + positional embedding
    let token_embd = Tensor::get_rows(&model.d_te, tokens);
    let pos_embd = Tensor::get_rows(&model.d_pe, positions);
    let mut cur = Tensor::add(&token_embd, &pos_embd);

    let n_past = kv_cache.n_past;
    let n_kv = n_past + n_tokens; // total KV length after this step

    for (il, layer) in model.decoder_layers.iter().enumerate() {
        // === Self-Attention ===
        let residual = cur.clone();

        // Layer norm
        let normed = Tensor::layer_norm_mul_add(&cur, &layer.attn_ln_0_w, &layer.attn_ln_0_b, EPS);

        // Q, K, V projections
        let q = Tensor::linear_raw(&normed, &layer.attn_q_w, &layer.attn_q_b);
        let k_new = Tensor::matmul_raw(&normed, &layer.attn_k_w);
        let v_new = Tensor::linear_raw(&normed, &layer.attn_v_w, &layer.attn_v_b);

        // Append new K, V to cache
        kv_cache.append(il, &k_new.data, &v_new.data);

        // Self-attention with causal mask
        let k_all = kv_cache.k_data(il);
        let v_all = kv_cache.v_data(il);

        let scale = 1.0 / (n_state_head as f32).sqrt();
        let attn_out =
            if decoder_flash_self && crate::metal_backend::is_requested() && n_tokens == 1 {
                crate::metal_backend::try_flash_attn_f32_self_kv_cache(
                    il,
                    &q.data,
                    k_all,
                    v_all,
                    n_kv,
                    n_head,
                    n_state_head,
                    scale,
                )
                .or_else(|| {
                    crate::metal_backend::try_flash_attn_f32_packed(
                        &q.data,
                        k_all,
                        v_all,
                        n_tokens,
                        n_kv,
                        n_head,
                        n_state_head,
                        scale,
                    )
                })
                .unwrap_or_else(|| {
                    let mut attn_out = vec![0.0f32; n_tokens * n_state];
                    let out_ptr = SendPtr::new(attn_out.as_mut_ptr());
                    let q_data = &q.data;
                    parallel_for(n_head, |h| {
                        let h_off = h * n_state_head;
                        let mut qh = vec![0.0f32; n_tokens * n_state_head];
                        let mut kh = vec![0.0f32; n_kv * n_state_head];
                        let mut vh = vec![0.0f32; n_kv * n_state_head];
                        for i in 0..n_tokens {
                            qh[i * n_state_head..(i + 1) * n_state_head].copy_from_slice(
                                &q_data[i * n_state + h_off..i * n_state + h_off + n_state_head],
                            );
                        }
                        for j in 0..n_kv {
                            kh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                                &k_all[j * n_state + h_off..j * n_state + h_off + n_state_head],
                            );
                            vh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                                &v_all[j * n_state + h_off..j * n_state + h_off + n_state_head],
                            );
                        }

                        // Q @ K^T with causal mask
                        let mut scores = vec![f32::NEG_INFINITY; n_tokens * n_kv];
                        for i in 0..n_tokens {
                            let q_row = &qh[i * n_state_head..(i + 1) * n_state_head];
                            let q_pos = n_past + i;
                            for j in 0..n_kv.min(q_pos + 1) {
                                let k_row = &kh[j * n_state_head..(j + 1) * n_state_head];
                                scores[i * n_kv + j] = Tensor::dot_f32(q_row, k_row) * scale;
                            }
                        }

                        // Softmax per query
                        for i in 0..n_tokens {
                            let row = &mut scores[i * n_kv..(i + 1) * n_kv];
                            let max_val = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                            let mut sum = 0.0f32;
                            for v in row.iter_mut() {
                                if *v > f32::NEG_INFINITY {
                                    *v = (*v - max_val).exp();
                                } else {
                                    *v = 0.0;
                                }
                                sum += *v;
                            }
                            if sum > 0.0 {
                                let inv = 1.0 / sum;
                                for v in row.iter_mut() {
                                    *v *= inv;
                                }
                            }
                        }

                        // scores @ V with transposed V
                        let mut vh_t = vec![0.0f32; n_state_head * n_kv];
                        for j in 0..n_kv {
                            for d in 0..n_state_head {
                                vh_t[d * n_kv + j] = vh[j * n_state_head + d];
                            }
                        }
                        for i in 0..n_tokens {
                            let s_row = &scores[i * n_kv..(i + 1) * n_kv];
                            for d in 0..n_state_head {
                                let v_col = &vh_t[d * n_kv..(d + 1) * n_kv];
                                unsafe {
                                    *out_ptr.ptr().add(i * n_state + h_off + d) =
                                        Tensor::dot_f32(s_row, v_col);
                                }
                            }
                        }
                    });
                    attn_out
                })
            } else {
                let mut attn_out = vec![0.0f32; n_tokens * n_state];
                let out_ptr = SendPtr::new(attn_out.as_mut_ptr());
                let q_data = &q.data;
                parallel_for(n_head, |h| {
                    let h_off = h * n_state_head;
                    let mut qh = vec![0.0f32; n_tokens * n_state_head];
                    let mut kh = vec![0.0f32; n_kv * n_state_head];
                    let mut vh = vec![0.0f32; n_kv * n_state_head];
                    for i in 0..n_tokens {
                        qh[i * n_state_head..(i + 1) * n_state_head].copy_from_slice(
                            &q_data[i * n_state + h_off..i * n_state + h_off + n_state_head],
                        );
                    }
                    for j in 0..n_kv {
                        kh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                            &k_all[j * n_state + h_off..j * n_state + h_off + n_state_head],
                        );
                        vh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                            &v_all[j * n_state + h_off..j * n_state + h_off + n_state_head],
                        );
                    }

                    // Q @ K^T with causal mask
                    let mut scores = vec![f32::NEG_INFINITY; n_tokens * n_kv];
                    for i in 0..n_tokens {
                        let q_row = &qh[i * n_state_head..(i + 1) * n_state_head];
                        let q_pos = n_past + i;
                        for j in 0..n_kv.min(q_pos + 1) {
                            let k_row = &kh[j * n_state_head..(j + 1) * n_state_head];
                            scores[i * n_kv + j] = Tensor::dot_f32(q_row, k_row) * scale;
                        }
                    }

                    // Softmax per query
                    for i in 0..n_tokens {
                        let row = &mut scores[i * n_kv..(i + 1) * n_kv];
                        let max_val = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                        let mut sum = 0.0f32;
                        for v in row.iter_mut() {
                            if *v > f32::NEG_INFINITY {
                                *v = (*v - max_val).exp();
                            } else {
                                *v = 0.0;
                            }
                            sum += *v;
                        }
                        if sum > 0.0 {
                            let inv = 1.0 / sum;
                            for v in row.iter_mut() {
                                *v *= inv;
                            }
                        }
                    }

                    // scores @ V with transposed V
                    let mut vh_t = vec![0.0f32; n_state_head * n_kv];
                    for j in 0..n_kv {
                        for d in 0..n_state_head {
                            vh_t[d * n_kv + j] = vh[j * n_state_head + d];
                        }
                    }
                    for i in 0..n_tokens {
                        let s_row = &scores[i * n_kv..(i + 1) * n_kv];
                        for d in 0..n_state_head {
                            let v_col = &vh_t[d * n_kv..(d + 1) * n_kv];
                            unsafe {
                                *out_ptr.ptr().add(i * n_state + h_off + d) =
                                    Tensor::dot_f32(s_row, v_col);
                            }
                        }
                    }
                });
                attn_out
            };

        let attn_result = Tensor {
            data: attn_out,
            shape: vec![n_tokens, n_state],
        };
        let projected = Tensor::linear_raw(&attn_result, &layer.attn_ln_1_w, &layer.attn_ln_1_b);
        cur = Tensor::add(&projected, &residual);

        // === Cross-Attention ===
        let residual = cur.clone();

        let normed = Tensor::layer_norm_mul_add(
            &cur,
            &layer.cross_attn_ln_0_w,
            &layer.cross_attn_ln_0_b,
            EPS,
        );

        let q = Tensor::linear_raw(&normed, &layer.cross_attn_q_w, &layer.cross_attn_q_b);

        let (ref k_cross, ref v_cross) = cross_kv[il];
        let n_audio_ctx = k_cross.shape[0];

        let scale = 1.0 / (n_state_head as f32).sqrt();

        let attn_out = if decoder_flash_cross
            && crate::metal_backend::is_requested()
            && n_tokens == 1
        {
            crate::metal_backend::try_flash_attn_f32_cross_kv_cache(
                il,
                &q.data,
                &k_cross.data,
                &v_cross.data,
                n_tokens,
                n_audio_ctx,
                n_head,
                n_state_head,
                scale,
            )
            .or_else(|| {
                crate::metal_backend::try_flash_attn_f32_packed(
                    &q.data,
                    &k_cross.data,
                    &v_cross.data,
                    n_tokens,
                    n_audio_ctx,
                    n_head,
                    n_state_head,
                    scale,
                )
            })
            .unwrap_or_else(|| {
                let mut attn_out = vec![0.0f32; n_tokens * n_state];
                let out_ptr = SendPtr::new(attn_out.as_mut_ptr());
                let q_data = &q.data;
                let k_cross_data = &k_cross.data;
                let v_cross_data = &v_cross.data;
                parallel_for(n_head, |h| {
                    let h_off = h * n_state_head;
                    let mut qh = vec![0.0f32; n_tokens * n_state_head];
                    let mut kh = vec![0.0f32; n_audio_ctx * n_state_head];
                    let mut vh = vec![0.0f32; n_audio_ctx * n_state_head];
                    for i in 0..n_tokens {
                        qh[i * n_state_head..(i + 1) * n_state_head].copy_from_slice(
                            &q_data[i * n_state + h_off..i * n_state + h_off + n_state_head],
                        );
                    }
                    for j in 0..n_audio_ctx {
                        kh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                            &k_cross_data[j * n_state + h_off..j * n_state + h_off + n_state_head],
                        );
                        vh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                            &v_cross_data[j * n_state + h_off..j * n_state + h_off + n_state_head],
                        );
                    }

                    // Q @ K_cross^T
                    let mut scores = vec![0.0f32; n_tokens * n_audio_ctx];
                    for i in 0..n_tokens {
                        let q_row = &qh[i * n_state_head..(i + 1) * n_state_head];
                        for j in 0..n_audio_ctx {
                            let k_row = &kh[j * n_state_head..(j + 1) * n_state_head];
                            scores[i * n_audio_ctx + j] = Tensor::dot_f32(q_row, k_row) * scale;
                        }
                    }

                    // Softmax
                    for i in 0..n_tokens {
                        let row = &mut scores[i * n_audio_ctx..(i + 1) * n_audio_ctx];
                        let max_val = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                        let mut sum = 0.0f32;
                        for v in row.iter_mut() {
                            *v = (*v - max_val).exp();
                            sum += *v;
                        }
                        let inv = 1.0 / sum;
                        for v in row.iter_mut() {
                            *v *= inv;
                        }
                    }

                    // scores @ V_cross with transposed V
                    let mut vh_t = vec![0.0f32; n_state_head * n_audio_ctx];
                    for j in 0..n_audio_ctx {
                        for d in 0..n_state_head {
                            vh_t[d * n_audio_ctx + j] = vh[j * n_state_head + d];
                        }
                    }
                    for i in 0..n_tokens {
                        let s_row = &scores[i * n_audio_ctx..(i + 1) * n_audio_ctx];
                        for d in 0..n_state_head {
                            let v_col = &vh_t[d * n_audio_ctx..(d + 1) * n_audio_ctx];
                            unsafe {
                                *out_ptr.ptr().add(i * n_state + h_off + d) =
                                    Tensor::dot_f32(s_row, v_col);
                            }
                        }
                    }
                });
                attn_out
            })
        } else {
            let mut attn_out = vec![0.0f32; n_tokens * n_state];
            let out_ptr = SendPtr::new(attn_out.as_mut_ptr());
            let q_data = &q.data;
            let k_cross_data = &k_cross.data;
            let v_cross_data = &v_cross.data;
            parallel_for(n_head, |h| {
                let h_off = h * n_state_head;
                let mut qh = vec![0.0f32; n_tokens * n_state_head];
                let mut kh = vec![0.0f32; n_audio_ctx * n_state_head];
                let mut vh = vec![0.0f32; n_audio_ctx * n_state_head];
                for i in 0..n_tokens {
                    qh[i * n_state_head..(i + 1) * n_state_head].copy_from_slice(
                        &q_data[i * n_state + h_off..i * n_state + h_off + n_state_head],
                    );
                }
                for j in 0..n_audio_ctx {
                    kh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                        &k_cross_data[j * n_state + h_off..j * n_state + h_off + n_state_head],
                    );
                    vh[j * n_state_head..(j + 1) * n_state_head].copy_from_slice(
                        &v_cross_data[j * n_state + h_off..j * n_state + h_off + n_state_head],
                    );
                }

                // Q @ K_cross^T
                let mut scores = vec![0.0f32; n_tokens * n_audio_ctx];
                for i in 0..n_tokens {
                    let q_row = &qh[i * n_state_head..(i + 1) * n_state_head];
                    for j in 0..n_audio_ctx {
                        let k_row = &kh[j * n_state_head..(j + 1) * n_state_head];
                        scores[i * n_audio_ctx + j] = Tensor::dot_f32(q_row, k_row) * scale;
                    }
                }

                // Softmax
                for i in 0..n_tokens {
                    let row = &mut scores[i * n_audio_ctx..(i + 1) * n_audio_ctx];
                    let max_val = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    let mut sum = 0.0f32;
                    for v in row.iter_mut() {
                        *v = (*v - max_val).exp();
                        sum += *v;
                    }
                    let inv = 1.0 / sum;
                    for v in row.iter_mut() {
                        *v *= inv;
                    }
                }

                // scores @ V_cross with transposed V
                let mut vh_t = vec![0.0f32; n_state_head * n_audio_ctx];
                for j in 0..n_audio_ctx {
                    for d in 0..n_state_head {
                        vh_t[d * n_audio_ctx + j] = vh[j * n_state_head + d];
                    }
                }
                for i in 0..n_tokens {
                    let s_row = &scores[i * n_audio_ctx..(i + 1) * n_audio_ctx];
                    for d in 0..n_state_head {
                        let v_col = &vh_t[d * n_audio_ctx..(d + 1) * n_audio_ctx];
                        unsafe {
                            *out_ptr.ptr().add(i * n_state + h_off + d) =
                                Tensor::dot_f32(s_row, v_col);
                        }
                    }
                }
            });
            attn_out
        };

        let attn_result = Tensor {
            data: attn_out,
            shape: vec![n_tokens, n_state],
        };
        let projected = Tensor::linear_raw(
            &attn_result,
            &layer.cross_attn_ln_1_w,
            &layer.cross_attn_ln_1_b,
        );
        cur = Tensor::add(&projected, &residual);

        // === Feed-Forward ===
        let residual = cur.clone();

        let normed = Tensor::layer_norm_mul_add(&cur, &layer.mlp_ln_w, &layer.mlp_ln_b, EPS);

        let mut ff = Tensor::linear_raw(&normed, &layer.mlp_0_w, &layer.mlp_0_b);
        ff = Tensor::gelu(&ff);
        ff = Tensor::linear_raw(&ff, &layer.mlp_1_w, &layer.mlp_1_b);

        cur = Tensor::add(&ff, &residual);
    }

    // Final layer norm
    let cur = Tensor::layer_norm_mul_add(&cur, &model.d_ln_w, &model.d_ln_b, EPS);

    // Project to vocab logits: cur [n_tokens, n_state] @ d_te^T [n_state, n_vocab] -> [n_tokens, n_vocab]
    // d_te is [n_vocab, n_state], so we compute cur @ d_te^T
    Tensor::matmul_t(&cur, &model.d_te)
}

/// Increment KV cache position counter after a decode step.
pub fn advance_kv_cache(kv_cache: &mut KvCache, n_tokens: usize) {
    kv_cache.n_past += n_tokens;
}
