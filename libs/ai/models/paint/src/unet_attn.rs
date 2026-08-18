//! Transformer2D (linear projection) + BasicTransformerBlock without 2.5D
//! extras: LayerNorm, self-attn, cross-attn, GEGLU-erf feed-forward.

use crate::unet_first::{paint_fast, UnetFirst, ATTN_GN_EPS, GN_GROUPS, LN_EPS, NS};
use makepad_ggml::backend::cuda::{
    gpu_add, gpu_attention_packed, gpu_attention_packed_cross, gpu_attention_packed_flash_bf16,
    gpu_download, gpu_gelu_erf, gpu_group_norm_planar, gpu_layer_norm_mul_add_cached, gpu_mul,
    gpu_paint_attn_batched_self, gpu_paint_ref_attn_wide_v, gpu_paint_scale, gpu_sdpa_flash_f16,
    gpu_sdpa_flash_f16_wide_v, gpu_slice_cols, gpu_slice_rows, gpu_to_f16, gpu_to_f32, gpu_upload,
    GpuTensor,
};

pub(crate) fn as_f32(x: &GpuTensor) -> Result<GpuTensor, String> {
    if x.is_half() {
        gpu_to_f32(x)
    } else {
        gpu_slice_rows(x, 0, x.rows())
    }
}

pub(crate) fn match_half(out: GpuTensor, like: &GpuTensor) -> Result<GpuTensor, String> {
    if like.is_half() && !out.is_half() {
        gpu_to_f16(&out)
    } else if !like.is_half() && out.is_half() {
        gpu_to_f32(&out)
    } else {
        Ok(out)
    }
}

/// Official `out * scale` is an ATen pointwise. The paint kernel is f32-only.
pub(crate) fn paint_scale_like(x: &GpuTensor, scale: f32) -> Result<GpuTensor, String> {
    match_half(gpu_paint_scale(&as_f32(x)?, scale)?, x)
}

/// Official RA is one `F.sdpa` with `V` last-dim = 2C (`head_dim_v=128`).
/// Inference uses that host: cat then one FLASH. Tap canaries stay f32.
pub(crate) fn ref_attn_wide_like(
    q: &GpuTensor,
    k: &GpuTensor,
    v_alb: &GpuTensor,
    v_mr: &GpuTensor,
    heads: usize,
    scale: f32,
) -> Result<(GpuTensor, GpuTensor), String> {
    if attn_use_official_sdpa(q, heads) {
        let (o_alb, o_mr) = gpu_sdpa_flash_f16_wide_v(q, k, v_alb, v_mr, 1, heads, scale)?;
        return Ok((match_half(o_alb, q)?, match_half(o_mr, q)?));
    }
    let (o_alb, o_mr) = gpu_paint_ref_attn_wide_v(
        &as_f32(q)?,
        &as_f32(k)?,
        &as_f32(v_alb)?,
        &as_f32(v_mr)?,
        heads,
        scale,
    )?;
    Ok((match_half(o_alb, q)?, match_half(o_mr, q)?))
}

/// Official `F.sdpa` is FLASH for hd=64, no mask. Tap canaries stay fp32.
fn attn_use_official_sdpa(q: &GpuTensor, heads: usize) -> bool {
    paint_fast() && heads > 0 && q.cols() / heads == 64 && q.cols() % heads == 0
}

/// Flash only when the score matrix is large enough that materializing it
/// dominates (256² MA is 6k²). Small 128² packed attn stays on composite /
/// the paint batched kernel. Homemade flash on tiny S is slower than
/// official `F.sdpa`. Tap canaries stay fp32 (`TAP_PARITY=1`).
fn attn_use_flash(q: &GpuTensor, k: &GpuTensor, heads: usize) -> bool {
    paint_fast()
        && heads > 0
        && q.cols() / heads == 64
        && q.cols() % heads == 0
        && q.rows().saturating_mul(k.rows()) >= 2_000_000
}

pub(crate) fn attn_packed(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    heads: usize,
    scale: f32,
) -> Result<GpuTensor, String> {
    if attn_use_official_sdpa(q, heads) {
        return match_half(gpu_sdpa_flash_f16(q, k, v, 1, heads, scale)?, q);
    }
    let q32 = as_f32(q)?;
    let k32 = as_f32(k)?;
    let v32 = as_f32(v)?;
    let out = if attn_use_flash(&q32, &k32, heads) {
        gpu_attention_packed_flash_bf16(&q32, &k32, &v32, heads, scale)?
    } else {
        gpu_attention_packed(&q32, &k32, &v32, heads, scale)?
    };
    match_half(out, q)
}

pub(crate) fn attn_cross(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    heads: usize,
    scale: f32,
) -> Result<GpuTensor, String> {
    if attn_use_official_sdpa(q, heads) {
        return match_half(gpu_sdpa_flash_f16(q, k, v, 1, heads, scale)?, q);
    }
    let q32 = as_f32(q)?;
    let k32 = as_f32(k)?;
    let v32 = as_f32(v)?;
    let out = if attn_use_flash(&q32, &k32, heads) {
        gpu_attention_packed_flash_bf16(&q32, &k32, &v32, heads, scale)?
    } else {
        gpu_attention_packed_cross(&q32, &k32, &v32, heads, scale)?
    };
    match_half(out, q)
}

/// Official text/DINO cross: one `F.sdpa` over `batch` independent
/// `[Sq, H, 64] × [Sk, H, 64]` rows. Packed Q is `[batch * Sq, H*64]`,
/// K/V `[batch * Sk, H*64]` in the same CFG/PBR/view order.
pub(crate) fn attn_cross_batched(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    batch: usize,
    heads: usize,
    scale: f32,
) -> Result<GpuTensor, String> {
    if batch == 0 || q.rows() % batch != 0 {
        return Err(format!(
            "attn_cross_batched rows {} vs batch {batch}",
            q.rows()
        ));
    }
    if attn_use_official_sdpa(q, heads) && k.rows() % batch == 0 && v.rows() == k.rows() {
        return match_half(gpu_sdpa_flash_f16(q, k, v, batch, heads, scale)?, q);
    }
    attn_cross(q, k, v, heads, scale)
}

/// Independent self-attn over `batch` packed sequences `[batch * seq, hidden]`.
pub(crate) fn attn_packed_batched(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    batch: usize,
    heads: usize,
    scale: f32,
) -> Result<GpuTensor, String> {
    if attn_use_official_sdpa(q, heads) && q.rows() % batch == 0 {
        return match_half(gpu_sdpa_flash_f16(q, k, v, batch, heads, scale)?, q);
    }
    let q32 = as_f32(q)?;
    let k32 = as_f32(k)?;
    let v32 = as_f32(v)?;
    let out = gpu_paint_attn_batched_self(&q32, &k32, &v32, batch, heads, scale)?;
    match_half(out, q)
}

impl UnetFirst {
    pub fn transformer2d(
        &self,
        h: &[f32],
        width: usize,
        height: usize,
        encoder: &[f32],
        enc_tokens: usize,
        prefix: &str,
        channels: usize,
        heads: usize,
    ) -> Result<Vec<f32>, String> {
        let plane = width * height;
        if h.len() != channels * plane {
            return Err(format!(
                "{prefix} expects {channels}x{width}x{height}, got {}",
                h.len()
            ));
        }
        if encoder.len() != enc_tokens * 1024 {
            return Err(format!(
                "{prefix} encoder expects {enc_tokens}x1024, got {}",
                encoder.len()
            ));
        }
        let residual = gpu_upload(h, channels, plane)?;
        let nw = self.get(&format!("{prefix}.norm.weight"))?;
        let nb = self.get(&format!("{prefix}.norm.bias"))?;
        let gn = gpu_group_norm_planar(
            &residual,
            width,
            height,
            GN_GROUPS,
            NS,
            &format!("{prefix}.norm"),
            &nw.data,
            &nb.data,
            ATTN_GN_EPS,
        )?;
        let tokens = planar_to_tokens(&gpu_download(&gn)?, channels, plane);
        let tokens = gpu_upload(&tokens, plane, channels)?;
        let tokens = self.linear_tokens(&tokens, &format!("{prefix}.proj_in"), true)?;
        let inner = format!("{prefix}.transformer_blocks.0.transformer");
        let tokens = self.basic_transformer(&tokens, encoder, enc_tokens, &inner, heads)?;
        let tokens = self.linear_tokens(&tokens, &format!("{prefix}.proj_out"), true)?;
        let planar = tokens_to_planar(&gpu_download(&tokens)?, channels, plane);
        let out = gpu_upload(&planar, channels, plane)?;
        gpu_download(&gpu_add(&out, &residual)?)
    }

    /// Transformer2D extras-off plus the inner `norm1` tokens official write-mode stashes.
    pub fn transformer2d_norm1(
        &self,
        h: &[f32],
        width: usize,
        height: usize,
        encoder: &[f32],
        enc_tokens: usize,
        prefix: &str,
        channels: usize,
        heads: usize,
    ) -> Result<(Vec<f32>, Vec<f32>), String> {
        let plane = width * height;
        if h.len() != channels * plane {
            return Err(format!(
                "{prefix} expects {channels}x{width}x{height}, got {}",
                h.len()
            ));
        }
        if encoder.len() != enc_tokens * 1024 {
            return Err(format!(
                "{prefix} encoder expects {enc_tokens}x1024, got {}",
                encoder.len()
            ));
        }
        let residual = gpu_upload(h, channels, plane)?;
        let nw = self.get(&format!("{prefix}.norm.weight"))?;
        let nb = self.get(&format!("{prefix}.norm.bias"))?;
        let gn = gpu_group_norm_planar(
            &residual,
            width,
            height,
            GN_GROUPS,
            NS,
            &format!("{prefix}.norm"),
            &nw.data,
            &nb.data,
            ATTN_GN_EPS,
        )?;
        let tokens = planar_to_tokens(&gpu_download(&gn)?, channels, plane);
        let tokens = gpu_upload(&tokens, plane, channels)?;
        let tokens = self.linear_tokens(&tokens, &format!("{prefix}.proj_in"), true)?;
        let inner = format!("{prefix}.transformer_blocks.0.transformer");
        let n1 = gpu_download(&self.layer_norm(&tokens, &format!("{inner}.norm1"))?)?;
        let tokens = self.basic_transformer(&tokens, encoder, enc_tokens, &inner, heads)?;
        let tokens = self.linear_tokens(&tokens, &format!("{prefix}.proj_out"), true)?;
        let planar = tokens_to_planar(&gpu_download(&tokens)?, channels, plane);
        let out = gpu_upload(&planar, channels, plane)?;
        Ok((gpu_download(&gpu_add(&out, &residual)?)?, n1))
    }

    pub(crate) fn basic_transformer(
        &self,
        tokens: &GpuTensor,
        encoder: &[f32],
        enc_tokens: usize,
        prefix: &str,
        heads: usize,
    ) -> Result<GpuTensor, String> {
        let hidden = tokens.cols();
        if hidden % heads != 0 {
            return Err(format!("{prefix} hidden {hidden} not divisible by {heads}"));
        }
        let head_dim = hidden / heads;
        let scale = 1.0 / (head_dim as f32).sqrt();

        let n1 = self.layer_norm(tokens, &format!("{prefix}.norm1"))?;
        let q = self.linear_tokens(&n1, &format!("{prefix}.attn1.to_q"), false)?;
        let k = self.linear_tokens(&n1, &format!("{prefix}.attn1.to_k"), false)?;
        let v = self.linear_tokens(&n1, &format!("{prefix}.attn1.to_v"), false)?;
        let attn = attn_packed(&q, &k, &v, heads, scale)?;
        let attn = self.linear_tokens(&attn, &format!("{prefix}.attn1.to_out.0"), true)?;
        let h = gpu_add(tokens, &attn)?;

        let n2 = self.layer_norm(&h, &format!("{prefix}.norm2"))?;
        let enc = gpu_upload(encoder, enc_tokens, 1024)?;
        let q = self.linear_tokens(&n2, &format!("{prefix}.attn2.to_q"), false)?;
        let k = self.linear_tokens(&enc, &format!("{prefix}.attn2.to_k"), false)?;
        let v = self.linear_tokens(&enc, &format!("{prefix}.attn2.to_v"), false)?;
        let cross = attn_cross(&q, &k, &v, heads, scale)?;
        let cross = self.linear_tokens(&cross, &format!("{prefix}.attn2.to_out.0"), true)?;
        let h = gpu_add(&h, &cross)?;

        let n3 = self.layer_norm(&h, &format!("{prefix}.norm3"))?;
        let ff = self.geglu_ff(&n3, prefix)?;
        gpu_add(&h, &ff)
    }

    pub(crate) fn layer_norm(&self, x: &GpuTensor, prefix: &str) -> Result<GpuTensor, String> {
        let w = self.get(&format!("{prefix}.weight"))?;
        let b = self.get(&format!("{prefix}.bias"))?;
        let x32 = as_f32(x)?;
        let out = gpu_layer_norm_mul_add_cached(&x32, NS, prefix, &w.data, &b.data, LN_EPS)?;
        match_half(out, x)
    }

    pub(crate) fn linear_tokens(
        &self,
        x: &GpuTensor,
        prefix: &str,
        bias: bool,
    ) -> Result<GpuTensor, String> {
        self.linear_cached(x, prefix, bias)
    }

    pub(crate) fn geglu_ff(&self, x: &GpuTensor, prefix: &str) -> Result<GpuTensor, String> {
        let packed = self.linear_tokens(x, &format!("{prefix}.ff.net.0.proj"), true)?;
        if packed.cols() % 2 != 0 {
            return Err(format!("{prefix} GEGLU packed cols {}", packed.cols()));
        }
        let inner = packed.cols() / 2;
        let value = as_f32(&gpu_slice_cols(&packed, 0, inner)?)?;
        let gate = gpu_gelu_erf(&as_f32(&gpu_slice_cols(&packed, inner, inner)?)?)?;
        let hidden = match_half(gpu_mul(&value, &gate)?, &packed)?;
        self.linear_tokens(&hidden, &format!("{prefix}.ff.net.2"), true)
    }
}

pub(crate) fn planar_to_tokens(planar: &[f32], channels: usize, plane: usize) -> Vec<f32> {
    let mut tokens = vec![0.0f32; plane * channels];
    for c in 0..channels {
        for i in 0..plane {
            tokens[i * channels + c] = planar[c * plane + i];
        }
    }
    tokens
}

pub(crate) fn tokens_to_planar(tokens: &[f32], channels: usize, plane: usize) -> Vec<f32> {
    let mut planar = vec![0.0f32; channels * plane];
    for c in 0..channels {
        for i in 0..plane {
            planar[c * plane + i] = tokens[i * channels + c];
        }
    }
    planar
}
