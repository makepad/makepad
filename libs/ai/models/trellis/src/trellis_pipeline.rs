//! TRELLIS 2 stage drivers: the FlowEulerGuidanceInterval sampling loop over
//! our device DiT forward, and the sparse-structure stage (flow -> ss_dec ->
//! argwhere) that feeds the SLAT cascade. Token-major layout throughout
//! (tokens, channels); the reference's dense (1, C, r, r, r) SS tensors
//! transpose at the boundary.

use crate::backend::GpuTensor;
use crate::trellis::{
    t2_cfg_combine, t2_euler_step, t2_occupancy_coords, t2_rope_tables, t2_ss_grid_coords,
    t2_t_sequence, T2SamplerConfig, T2_SS_CHANNELS, T2_SS_TOKENS,
};
use crate::trellis_dit::{t2_kv_cache_enabled, t2_upload_rope, T2CrossKv, T2Dit};
use crate::trellis_vae::T2SsDec;
use crate::Result;

/// (channels, tokens) c-major (torch dense flatten) -> (tokens, channels).
pub fn t2_chw_to_tokens(x: &[f32], channels: usize, tokens: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; x.len()];
    for c in 0..channels {
        for t in 0..tokens {
            out[t * channels + c] = x[c * tokens + t];
        }
    }
    out
}

/// (tokens, channels) -> (channels, tokens).
pub fn t2_tokens_to_chw(x: &[f32], channels: usize, tokens: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; x.len()];
    for t in 0..tokens {
        for c in 0..channels {
            out[c * tokens + t] = x[t * channels + c];
        }
    }
    out
}

/// The forward plan of one sampling run: (step, t, t_prev, cfg_active).
pub fn t2_step_plan(cfg: &T2SamplerConfig) -> Vec<(usize, f64, f64, bool)> {
    let t_seq = t2_t_sequence(cfg.steps, cfg.rescale_t);
    (0..cfg.steps)
        .map(|i| {
            let t = t_seq[i];
            let active = cfg.guidance_strength != 1.0
                && cfg.guidance_interval.0 <= t
                && t <= cfg.guidance_interval.1;
            (i, t, t_seq[i + 1], active)
        })
        .collect()
}

/// Cancel hook for sampling/decode loops: checked before every DiT forward
/// (each forward is the granularity floor); returning `true` aborts the run
/// with [`crate::DiffusionError::Cancelled`].
pub type T2CancelHook<'a> = &'a (dyn Fn() -> bool + 'a);

/// FlowEulerGuidanceIntervalSampler.sample over our DiT: `x` is the
/// token-major noise, consumed in place into the final samples. `on_forward`
/// sees every model output (validation instrumentation).
#[allow(clippy::too_many_arguments)]
pub fn t2_sample_flow(
    dit: &T2Dit,
    x: &mut [f32],
    tokens: usize,
    cond: &GpuTensor,
    neg_cond: &GpuTensor,
    rope: &(GpuTensor, GpuTensor),
    cfg: &T2SamplerConfig,
    dense_std: bool,
    on_forward: impl FnMut(usize, f64, &[f32], &[f32]),
) -> Result<()> {
    t2_sample_flow_cancel(
        dit, x, tokens, cond, neg_cond, rope, cfg, dense_std, &|| false, on_forward,
    )
}

/// [`t2_sample_flow`] with a cancel hook (service jobs).
#[allow(clippy::too_many_arguments)]
pub fn t2_sample_flow_cancel(
    dit: &T2Dit,
    x: &mut [f32],
    tokens: usize,
    cond: &GpuTensor,
    neg_cond: &GpuTensor,
    rope: &(GpuTensor, GpuTensor),
    cfg: &T2SamplerConfig,
    dense_std: bool,
    cancel: T2CancelHook,
    on_forward: impl FnMut(usize, f64, &[f32], &[f32]),
) -> Result<()> {
    t2_sample_flow_concat_ctl(
        dit, x, tokens, None, cond, neg_cond, rope, cfg, dense_std, cancel, on_forward,
    )
}

/// Sampler variant with a per-token concat condition (the tex flow: model
/// input = [noise 32 | shape slat 32] rebuilt every forward, euler updates
/// only the noise half).
#[allow(clippy::too_many_arguments)]
pub fn t2_sample_flow_concat(
    dit: &T2Dit,
    x: &mut [f32],
    tokens: usize,
    concat_cond: Option<&[f32]>,
    cond: &GpuTensor,
    neg_cond: &GpuTensor,
    rope: &(GpuTensor, GpuTensor),
    cfg: &T2SamplerConfig,
    dense_std: bool,
    on_forward: impl FnMut(usize, f64, &[f32], &[f32]),
) -> Result<()> {
    t2_sample_flow_concat_ctl(
        dit, x, tokens, concat_cond, cond, neg_cond, rope, cfg, dense_std, &|| false, on_forward,
    )
}

/// [`t2_sample_flow_concat`] with a cancel hook checked before every forward.
#[allow(clippy::too_many_arguments)]
pub fn t2_sample_flow_concat_ctl(
    dit: &T2Dit,
    x: &mut [f32],
    tokens: usize,
    concat_cond: Option<&[f32]>,
    cond: &GpuTensor,
    neg_cond: &GpuTensor,
    rope: &(GpuTensor, GpuTensor),
    cfg: &T2SamplerConfig,
    dense_std: bool,
    cancel: T2CancelHook,
    mut on_forward: impl FnMut(usize, f64, &[f32], &[f32]),
) -> Result<()> {
    let x_channels = x.len() / tokens;
    let build_input = |x: &[f32]| -> Vec<f32> {
        match concat_cond {
            None => x.to_vec(),
            Some(extra) => {
                let extra_channels = extra.len() / tokens;
                let mut full = vec![0.0f32; tokens * (x_channels + extra_channels)];
                for token in 0..tokens {
                    let dst = token * (x_channels + extra_channels);
                    full[dst..dst + x_channels]
                        .copy_from_slice(&x[token * x_channels..(token + 1) * x_channels]);
                    full[dst + x_channels..dst + x_channels + extra_channels].copy_from_slice(
                        &extra[token * extra_channels..(token + 1) * extra_channels],
                    );
                }
                full
            }
        }
    };
    // Per-stage cross-attn KV caches: cond / neg_cond are fixed across every
    // forward of this sampling run.
    let use_kv_cache = t2_kv_cache_enabled();
    let mut pos_kv = T2CrossKv::default();
    let mut neg_kv = T2CrossKv::default();
    let mut fwd_index = 0usize;
    for (_step, t, t_prev, cfg_active) in t2_step_plan(cfg) {
        if cancel() {
            return Err(crate::DiffusionError::Cancelled);
        }
        let t1000 = (1000.0 * t) as f32;
        let input = build_input(x);
        let pos_cache = use_kv_cache.then_some(&mut pos_kv);
        let (mut velocity, _) = dit.forward(&input, tokens, t1000, cond, rope, pos_cache, &[])?;
        on_forward(fwd_index, t, x, &velocity);
        fwd_index += 1;
        if cfg_active {
            if cancel() {
                return Err(crate::DiffusionError::Cancelled);
            }
            let neg_cache = use_kv_cache.then_some(&mut neg_kv);
            let (neg_velocity, _) =
                dit.forward(&input, tokens, t1000, neg_cond, rope, neg_cache, &[])?;
            on_forward(fwd_index, t, x, &neg_velocity);
            fwd_index += 1;
            t2_cfg_combine(
                x,
                &mut velocity,
                &neg_velocity,
                t as f32,
                cfg.guidance_strength,
                cfg.guidance_rescale,
                dense_std,
            )?;
        }
        t2_euler_step(x, &velocity, t, t_prev)?;
    }
    Ok(())
}

pub struct T2SsOutput {
    /// Final latent, token-major (4096, 8).
    pub z_tokens: Vec<f32>,
    /// 64^3 occupancy logits (x, y, z lex).
    pub logits: Vec<f32>,
    /// Active coords at the target resolution (lex order).
    pub coords: Vec<[i32; 3]>,
}

/// The full sparse-structure stage: flow sampling on the dense 16^3 grid,
/// conv3d decode to 64^3 logits, threshold + max-pool to `ss_res` coords.
#[allow(clippy::too_many_arguments)]
pub fn t2_run_ss(
    dit: &T2Dit,
    ss_dec: &T2SsDec,
    noise_tokens: &[f32],
    cond: &GpuTensor,
    neg_cond: &GpuTensor,
    cfg: &T2SamplerConfig,
    ss_res: usize,
    on_forward: impl FnMut(usize, f64, &[f32], &[f32]),
) -> Result<T2SsOutput> {
    t2_run_ss_cancel(
        dit, ss_dec, noise_tokens, cond, neg_cond, cfg, ss_res, &|| false, on_forward,
    )
}

/// [`t2_run_ss`] with a cancel hook (service jobs).
#[allow(clippy::too_many_arguments)]
pub fn t2_run_ss_cancel(
    dit: &T2Dit,
    ss_dec: &T2SsDec,
    noise_tokens: &[f32],
    cond: &GpuTensor,
    neg_cond: &GpuTensor,
    cfg: &T2SamplerConfig,
    ss_res: usize,
    cancel: T2CancelHook,
    on_forward: impl FnMut(usize, f64, &[f32], &[f32]),
) -> Result<T2SsOutput> {
    let rope = t2_upload_rope(&t2_rope_tables(&t2_ss_grid_coords()))?;
    let mut x = noise_tokens.to_vec();
    t2_sample_flow_concat_ctl(
        dit,
        &mut x,
        T2_SS_TOKENS,
        None,
        cond,
        neg_cond,
        &rope,
        cfg,
        true, // dense torch .std() (Bessel) in the CFG rescale
        cancel,
        on_forward,
    )?;
    if cancel() {
        return Err(crate::DiffusionError::Cancelled);
    }
    let logits = ss_dec.decode(&x)?;
    let coords = t2_occupancy_coords(&logits, 64, ss_res)?;
    let _ = T2_SS_CHANNELS;
    Ok(T2SsOutput {
        z_tokens: x,
        logits,
        coords,
    })
}
