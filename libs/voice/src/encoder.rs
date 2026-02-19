use crate::model::WhisperModel;
use crate::tensor::{parallel_for, RawTensor, Tensor};

const EPS: f32 = 1e-5;

/// Multi-head self-attention with parallel head computation.
/// x: [seq_len, n_state]
/// Returns: [seq_len, n_state]
fn multi_head_attention(
    x: &Tensor,
    q_w: &RawTensor,
    q_b: &Tensor,
    k_w: &RawTensor,
    v_w: &RawTensor,
    v_b: &Tensor,
    out_w: &RawTensor,
    out_b: &Tensor,
    n_head: usize,
) -> Tensor {
    let seq_len = x.shape[0];
    let n_state = x.shape[1];
    let n_state_head = n_state / n_head;
    let scale = 1.0 / (n_state_head as f32).sqrt();

    let xq = if Tensor::activation_q8_enabled() && seq_len > 2 && n_state % 32 == 0 {
        Some(Tensor::prequantize_rows_q8(x, n_state))
    } else {
        None
    };

    // Q, K, V projections: [seq_len, n_state]
    let q = Tensor::linear_raw_with_prequant(x, q_w, xq.as_deref(), q_b);
    let k = Tensor::matmul_raw_with_prequant(x, k_w, xq.as_deref());
    let v = Tensor::linear_raw_with_prequant(x, v_w, xq.as_deref(), v_b);

    let metal_attn_enabled = std::env::var("MAKEPAD_VOICE_METAL_ATTN")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
        })
        .unwrap_or(true);

    if crate::metal_backend::is_requested() && metal_attn_enabled {
        if let Some(out) = crate::metal_backend::try_flash_attn_f32_packed(
            &q.data,
            &k.data,
            &v.data,
            seq_len,
            seq_len,
            n_head,
            n_state_head,
            scale,
        ) {
            let attn_out = Tensor {
                data: out,
                shape: vec![seq_len, n_state],
            };

            return Tensor::linear_raw(&attn_out, out_w, out_b);
        }
    }

    let mut out = vec![0.0f32; seq_len * n_state];
    let out_ptr = crate::tensor::SendPtr::new(out.as_mut_ptr());
    let q_data = &q.data;
    let k_data = &k.data;
    let v_data = &v.data;

    parallel_for(n_head, |h| {
        let h_off = h * n_state_head;
        let out_data = unsafe { std::slice::from_raw_parts_mut(out_ptr.ptr(), seq_len * n_state) };

        // Extract contiguous per-head Q, K, V for SIMD-friendly access
        let mut qh = vec![0.0f32; seq_len * n_state_head];
        let mut kh = vec![0.0f32; seq_len * n_state_head];
        let mut vh = vec![0.0f32; seq_len * n_state_head];
        for i in 0..seq_len {
            qh[i * n_state_head..(i + 1) * n_state_head]
                .copy_from_slice(&q_data[i * n_state + h_off..i * n_state + h_off + n_state_head]);
            kh[i * n_state_head..(i + 1) * n_state_head]
                .copy_from_slice(&k_data[i * n_state + h_off..i * n_state + h_off + n_state_head]);
            vh[i * n_state_head..(i + 1) * n_state_head]
                .copy_from_slice(&v_data[i * n_state + h_off..i * n_state + h_off + n_state_head]);
        }

        // Transpose V head to [n_state_head, seq_len] for SIMD dot
        let mut vh_t = vec![0.0f32; n_state_head * seq_len];
        for i in 0..seq_len {
            for d in 0..n_state_head {
                vh_t[d * seq_len + i] = vh[i * n_state_head + d];
            }
        }

        // Row-streamed attention: avoids allocating a full [seq_len, seq_len] score matrix.
        let mut scores_row = vec![0.0f32; seq_len];
        for i in 0..seq_len {
            let q_row = &qh[i * n_state_head..(i + 1) * n_state_head];

            let mut max_val = f32::NEG_INFINITY;
            for j in 0..seq_len {
                let k_row = &kh[j * n_state_head..(j + 1) * n_state_head];
                let s = Tensor::dot_f32(q_row, k_row) * scale;
                scores_row[j] = s;
                if s > max_val {
                    max_val = s;
                }
            }

            let mut sum = 0.0f32;
            for v in scores_row.iter_mut() {
                *v = (*v - max_val).exp();
                sum += *v;
            }
            let inv = 1.0 / sum;
            for v in scores_row.iter_mut() {
                *v *= inv;
            }

            for d in 0..n_state_head {
                let v_col = &vh_t[d * seq_len..(d + 1) * seq_len];
                out_data[i * n_state + h_off + d] = Tensor::dot_f32(&scores_row, v_col);
            }
        }
    });

    let attn_out = Tensor {
        data: out,
        shape: vec![seq_len, n_state],
    };

    // Output projection
    Tensor::linear_raw(&attn_out, out_w, out_b)
}

/// Run the whisper encoder on a mel spectrogram chunk.
/// mel_data: slice of mel spectrogram for this chunk, shape [n_mels, 2*n_ctx]
/// Returns encoder output: [n_ctx, n_state]
pub fn encode(model: &WhisperModel, mel_data: &[f32], n_ctx: usize) -> Tensor {
    let n_mels = model.hparams.n_mels as usize;
    let n_state = model.hparams.n_audio_state as usize;
    let n_head = model.hparams.n_audio_head as usize;

    // mel input: [n_mels, 2*n_ctx]
    let mel = Tensor {
        data: mel_data.to_vec(),
        shape: vec![n_mels, 2 * n_ctx],
    };

    // Conv1 + GELU: [n_mels, 2*n_ctx] -> [n_state, 2*n_ctx]
    let _tc = std::time::Instant::now();
    let mut cur = Tensor::conv1d(&mel, &model.e_conv_1_w, &model.e_conv_1_b, 1);
    cur = Tensor::gelu(&cur);

    // Conv2 + GELU: [n_state, 2*n_ctx] -> [n_state, n_ctx] (stride 2)
    cur = Tensor::conv1d(&cur, &model.e_conv_2_w, &model.e_conv_2_b, 2);
    cur = Tensor::gelu(&cur);
    crate::PROF_ENC_CONV.fetch_add(
        _tc.elapsed().as_nanos() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );

    // cur is [n_state, n_ctx], transpose to [n_ctx, n_state]
    cur = Tensor::transpose_2d(&cur);

    // Add positional embedding: [n_ctx, n_state]
    // e_pe is [n_audio_ctx, n_state], take first n_ctx rows
    let pe_data: Vec<f32> = model.e_pe.data[..n_ctx * n_state].to_vec();
    let pe = Tensor {
        data: pe_data,
        shape: vec![n_ctx, n_state],
    };
    cur = Tensor::add(&cur, &pe);

    if crate::metal_backend::is_requested() {
        let t_stack = std::time::Instant::now();
        if let Some(out) = crate::metal_backend::try_encoder_stack_f32(
            &cur.data,
            cur.shape[0],
            cur.shape[1],
            n_head,
            &model.encoder_layers,
            &model.e_ln_w.data,
            &model.e_ln_b.data,
        ) {
            let dt = t_stack.elapsed().as_nanos() as u64;
            crate::PROF_ENC_ATTN.fetch_add(dt / 2, std::sync::atomic::Ordering::Relaxed);
            crate::PROF_ENC_ELEM.fetch_add(dt - dt / 2, std::sync::atomic::Ordering::Relaxed);
            return Tensor {
                data: out,
                shape: cur.shape.clone(),
            };
        }
    }

    // Transformer encoder blocks
    for layer in &model.encoder_layers {
        if crate::metal_backend::is_requested() {
            let t_layer = std::time::Instant::now();
            if let Some(out) = crate::metal_backend::try_encoder_layer_f32(
                &cur.data,
                cur.shape[0],
                cur.shape[1],
                n_head,
                &layer.attn_ln_0_w.data,
                &layer.attn_ln_0_b.data,
                &layer.attn_q_w.data,
                layer.attn_q_w.ggml_type,
                &layer.attn_q_b.data,
                &layer.attn_k_w.data,
                layer.attn_k_w.ggml_type,
                &layer.attn_v_w.data,
                layer.attn_v_w.ggml_type,
                &layer.attn_v_b.data,
                &layer.attn_ln_1_w.data,
                layer.attn_ln_1_w.ggml_type,
                &layer.attn_ln_1_b.data,
                &layer.mlp_ln_w.data,
                &layer.mlp_ln_b.data,
                &layer.mlp_0_w.data,
                layer.mlp_0_w.ggml_type,
                &layer.mlp_0_b.data,
                &layer.mlp_1_w.data,
                layer.mlp_1_w.ggml_type,
                &layer.mlp_1_b.data,
            ) {
                cur = Tensor {
                    data: out,
                    shape: cur.shape.clone(),
                };
                let dt = t_layer.elapsed().as_nanos() as u64;
                crate::PROF_ENC_ATTN.fetch_add(dt / 2, std::sync::atomic::Ordering::Relaxed);
                crate::PROF_ENC_ELEM.fetch_add(dt - dt / 2, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
        }

        // Self-attention block
        let t_attn = std::time::Instant::now();
        let mut attn_done = false;
        if crate::metal_backend::is_requested() {
            if let Some(out) = crate::metal_backend::try_encoder_attn_block_f32(
                &cur.data,
                cur.shape[0],
                cur.shape[1],
                n_head,
                &layer.attn_ln_0_w.data,
                &layer.attn_ln_0_b.data,
                &layer.attn_q_w.data,
                layer.attn_q_w.ggml_type,
                &layer.attn_q_b.data,
                &layer.attn_k_w.data,
                layer.attn_k_w.ggml_type,
                &layer.attn_v_w.data,
                layer.attn_v_w.ggml_type,
                &layer.attn_v_b.data,
                &layer.attn_ln_1_w.data,
                layer.attn_ln_1_w.ggml_type,
                &layer.attn_ln_1_b.data,
            ) {
                cur = Tensor {
                    data: out,
                    shape: cur.shape.clone(),
                };
                attn_done = true;
            }
        }
        if !attn_done {
            let residual = cur.clone();

            // Layer norm
            let normed =
                Tensor::layer_norm_mul_add(&cur, &layer.attn_ln_0_w, &layer.attn_ln_0_b, EPS);

            // Multi-head self-attention
            let attn_out = multi_head_attention(
                &normed,
                &layer.attn_q_w,
                &layer.attn_q_b,
                &layer.attn_k_w,
                &layer.attn_v_w,
                &layer.attn_v_b,
                &layer.attn_ln_1_w,
                &layer.attn_ln_1_b,
                n_head,
            );

            cur = Tensor::add(&attn_out, &residual);
        }
        crate::PROF_ENC_ATTN.fetch_add(
            t_attn.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Feed-forward block
        let t_elem = std::time::Instant::now();
        let mut ffn_done = false;
        if crate::metal_backend::is_requested() {
            if let Some(out) = crate::metal_backend::try_encoder_ffn_block_f32(
                &cur.data,
                cur.shape[0],
                cur.shape[1],
                &layer.mlp_ln_w.data,
                &layer.mlp_ln_b.data,
                &layer.mlp_0_w.data,
                layer.mlp_0_w.ggml_type,
                &layer.mlp_0_b.data,
                &layer.mlp_1_w.data,
                layer.mlp_1_w.ggml_type,
                &layer.mlp_1_b.data,
            ) {
                cur = Tensor {
                    data: out,
                    shape: cur.shape.clone(),
                };
                ffn_done = true;
            }
        }
        if !ffn_done {
            let residual = cur.clone();

            let normed = Tensor::layer_norm_mul_add(&cur, &layer.mlp_ln_w, &layer.mlp_ln_b, EPS);

            // MLP: linear -> gelu -> linear
            let mut ff = Tensor::linear_raw(&normed, &layer.mlp_0_w, &layer.mlp_0_b);
            ff = Tensor::gelu(&ff);
            ff = Tensor::linear_raw(&ff, &layer.mlp_1_w, &layer.mlp_1_b);

            cur = Tensor::add(&ff, &residual);
        }
        crate::PROF_ENC_ELEM.fetch_add(
            t_elem.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    // Final layer norm
    Tensor::layer_norm_mul_add(&cur, &model.e_ln_w, &model.e_ln_b, EPS)
}

/// Pre-compute cross-attention K and V for all decoder layers.
/// encoder_out: [n_ctx, n_state]
/// Returns: Vec of (K, V) pairs, one per decoder layer.
/// K: [n_ctx, n_state], V: [n_ctx, n_state]
pub fn compute_cross_kv(model: &WhisperModel, encoder_out: &Tensor) -> Vec<(Tensor, Tensor)> {
    let mut cross_kv = Vec::new();
    let n_state = encoder_out.shape[1];
    let batch = encoder_out.shape[0];
    let xq = if Tensor::activation_q8_enabled() && batch > 2 && n_state % 32 == 0 {
        Some(Tensor::prequantize_rows_q8(encoder_out, n_state))
    } else {
        None
    };
    for layer in &model.decoder_layers {
        let k = Tensor::matmul_raw_with_prequant(encoder_out, &layer.cross_attn_k_w, xq.as_deref());
        let v = Tensor::linear_raw_with_prequant(
            encoder_out,
            &layer.cross_attn_v_w,
            xq.as_deref(),
            &layer.cross_attn_v_b,
        );
        cross_kv.push((k, v));
    }
    cross_kv
}
