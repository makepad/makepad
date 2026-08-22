//! Incremental UNet2p5D CUDA graph sections, built exclusively on the
//! verified public `makepad_ai_common::backend::cuda` ops. CUDA-only by decree:
//! no CPU/Metal path exists here. Each section lands together with a frozen
//! host reference tap (see [`crate::numerical_fixtures`]) exercised by the
//! `pbr-cuda-taps` canary on a supported CUDA host before the next section
//! builds on it.
//!
//! Layouts: planar activations are `rows = channels, cols = width * height`
//! (the conv/group-norm planar convention); token activations are
//! `rows = tokens, cols = hidden`.

use crate::numerical_fixtures::ResnetSectionInputs;
use makepad_ai_common::backend::cuda::{
    gpu_add, gpu_conv2d_planar_cached, gpu_download, gpu_group_norm_planar,
    gpu_linear_nt_cached, gpu_silu, gpu_upload, GpuLinearPart, GpuTensor,
};
use makepad_ai_common::quant::GGML_TYPE_F16;

pub const GROUP_NORM_EPS: f32 = 1e-5;

/// Planar activation: `t` has rows = channels, cols = width*height.
pub struct Planar {
    pub t: GpuTensor,
    pub width: usize,
    pub height: usize,
}

/// Pack f32 weights as little-endian f16 bytes for the cached linear parts.
pub fn f16_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| crate::cond_assembly::f32_to_f16_bits(*v).to_le_bytes())
        .collect()
}

/// Compatibility implementation for the timestep row-broadcast. It uses only
/// CUDA APIs present on clean HEAD, expanding the tiny projection on the host
/// before the device add. This is intentionally a correctness tap, not a
/// production-performance implementation; a complete executor must replace it
/// with a fused device kernel before claiming availability.
pub fn add_rows_broadcast_compat(
    x: &GpuTensor,
    row_bias: &GpuTensor,
) -> Result<GpuTensor, String> {
    let bias_len = row_bias
        .rows()
        .checked_mul(row_bias.cols())
        .ok_or_else(|| "row-broadcast bias shape overflow".to_string())?;
    if bias_len != x.rows() {
        return Err(format!(
            "row-broadcast bias has {bias_len} elements, expected {}",
            x.rows()
        ));
    }
    let bias = gpu_download(row_bias)?;
    let mut expanded = Vec::with_capacity(
        x.rows()
            .checked_mul(x.cols())
            .ok_or_else(|| "row-broadcast output shape overflow".to_string())?,
    );
    for value in bias {
        expanded.extend(std::iter::repeat_n(value, x.cols()));
    }
    let expanded = gpu_upload(&expanded, x.rows(), x.cols())?;
    gpu_add(x, &expanded)
}

/// One SD ResNet block on device. `ns` is the weight-cache namespace (one per
/// residency group), `key` the unique block prefix within it.
#[allow(clippy::too_many_arguments)]
pub fn resnet_block(
    ns: &str,
    key: &str,
    x: &Planar,
    temb: &GpuTensor,
    w: &ResnetSectionInputs,
    temb_w_bytes: &[u8],
) -> Result<Planar, String> {
    let (width, height) = (x.width, x.height);
    let h = gpu_group_norm_planar(
        &x.t,
        width,
        height,
        w.gn1_groups,
        ns,
        &format!("{key}.norm1"),
        &w.gn1_gamma,
        &w.gn1_beta,
        GROUP_NORM_EPS,
    )?;
    let h = gpu_silu(&h)?;
    let h = gpu_conv2d_planar_cached(
        &h,
        width,
        height,
        ns,
        &format!("{key}.conv1"),
        &w.conv1_w,
        &w.conv1_b,
        w.cout,
        3,
        3,
        1,
        1,
    )?;
    // Timestep projection: silu(temb) [1, D] -> f16 linear -> [1, cout],
    // broadcast-added per channel row.
    let temb_act = gpu_silu(temb)?;
    let temb_proj = gpu_linear_nt_cached(
        &temb_act,
        ns,
        &[GpuLinearPart {
            bt_ggml_type: GGML_TYPE_F16,
            n: w.cout,
            cache_key: &format!("{key}.time_emb_proj"),
            bytes: temb_w_bytes,
        }],
        &w.temb_b,
    )?;
    let h = add_rows_broadcast_compat(&h, &temb_proj)?;
    let h = gpu_group_norm_planar(
        &h,
        width,
        height,
        w.gn2_groups,
        ns,
        &format!("{key}.norm2"),
        &w.gn2_gamma,
        &w.gn2_beta,
        GROUP_NORM_EPS,
    )?;
    let h = gpu_silu(&h)?;
    let h = gpu_conv2d_planar_cached(
        &h,
        width,
        height,
        ns,
        &format!("{key}.conv2"),
        &w.conv2_w,
        &w.conv2_b,
        w.cout,
        3,
        3,
        1,
        1,
    )?;
    let shortcut = gpu_conv2d_planar_cached(
        &x.t,
        width,
        height,
        ns,
        &format!("{key}.conv_shortcut"),
        &w.short_w,
        &w.short_b,
        w.cout,
        1,
        1,
        0,
        0,
    )?;
    let out = gpu_add(&h, &shortcut)?;
    Ok(Planar {
        t: out,
        width,
        height,
    })
}
