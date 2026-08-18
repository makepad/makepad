//! 15-step v-pred ZSNR DDIM + 3-branch CFG orchestration.
//!
//! Pure CPU, executor-agnostic: the CUDA UNet (when it exists) is a
//! `predict_v` callback that returns the stacked `[negative, ref-only,
//! ref+dino]` v-predictions. This module owns noise init, 12-ch packing,
//! guidance combine, and the DDIM transport. It does **not** claim a complete
//! Hunyuan job — the real graph is still the missing callback.
//!
//! The checkpoint `scheduler_config.json` is DDIM / v-pred / ZSNR / trailing.
//! The official demo wrapper (`hy3dpaint/utils/multiview_utils.py`) then
//! *replaces* that scheduler with UniPC 15 steps at load time. Native stays
//! on the checkpoint DDIM 15-step path until a full-job oracle says otherwise.

use crate::cond_assembly::{
    guidance_combine, pack_view_latent, view_scales_for_batch, CFG_BRANCHES, LATENT_CHANNELS,
    PACKED_INPUT_CHANNELS, PBR_MATERIALS,
};
use crate::hunyuan;
use crate::schedule::DdimVpredZsnr;
use crate::test_backend::PbrError;

/// Official inference step count (`Hunyuan3DPaintConfig`).
pub const DEFAULT_STEPS: usize = 15;
/// Official CFG scale.
pub const DEFAULT_GUIDANCE: f32 = 3.0;

/// SplitMix64. Native seed mapping only — official `torch.randn` (Philox)
/// parity is pinned later against a full-step oracle dump, not here.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn uniform_open01(state: &mut u64) -> f64 {
    // 53-bit mantissa in (0, 1) so Box–Muller never hits log(0).
    let u = ((splitmix64(state) >> 11) as f64) * (1.0 / ((1u64 << 53) as f64));
    u.clamp(f64::EPSILON, 1.0 - f64::EPSILON)
}

/// Deterministic N(0, 1) sample. Not torch-Philox; documented above.
pub fn gaussian_sample(seed: u64, n: usize) -> Vec<f32> {
    let mut state = seed ^ 0xA076_1D64_78BD_642F;
    if state == 0 {
        state = 1;
    }
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let u1 = uniform_open01(&mut state);
        let u2 = uniform_open01(&mut state);
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        out.push((r * theta.cos()) as f32);
        if out.len() < n {
            out.push((r * theta.sin()) as f32);
        }
    }
    out
}

/// Interleaved RGB8 → planar [0,1] (`3 * W * H`, channel-major).
pub fn rgb8_interleaved_to_planar01(rgb: &[u8], width: usize, height: usize) -> Result<Vec<f32>, PbrError> {
    let plane = width
        .checked_mul(height)
        .ok_or_else(|| PbrError::InvalidParams("rgb plane overflow".into()))?;
    if rgb.len() != plane.saturating_mul(3) {
        return Err(PbrError::InvalidParams(format!(
            "rgb8 length {} != {}x{}x3",
            rgb.len(),
            width,
            height
        )));
    }
    let mut out = vec![0.0f32; 3 * plane];
    for i in 0..plane {
        out[i] = rgb[i * 3] as f32 / 255.0;
        out[plane + i] = rgb[i * 3 + 1] as f32 / 255.0;
        out[2 * plane + i] = rgb[i * 3 + 2] as f32 / 255.0;
    }
    Ok(out)
}

/// Bilinear resize of interleaved RGB8. Identity when sizes match.
pub fn resize_rgb8_bilinear(
    src: &[u8],
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> Result<Vec<u8>, PbrError> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Err(PbrError::InvalidParams("resize dim is zero".into()));
    }
    if src.len() != sw * sh * 3 {
        return Err(PbrError::InvalidParams("resize src length".into()));
    }
    if sw == dw && sh == dh {
        return Ok(src.to_vec());
    }
    let mut out = vec![0u8; dw * dh * 3];
    let x_scale = (sw as f32) / (dw as f32);
    let y_scale = (sh as f32) / (dh as f32);
    for y in 0..dh {
        let fy = (y as f32 + 0.5) * y_scale - 0.5;
        let y0 = fy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..dw {
            let fx = (x as f32 + 0.5) * x_scale - 0.5;
            let x0 = fx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = (fx - x0 as f32).clamp(0.0, 1.0);
            for c in 0..3 {
                let s00 = src[(y0 * sw + x0) * 3 + c] as f32;
                let s10 = src[(y0 * sw + x1) * 3 + c] as f32;
                let s01 = src[(y1 * sw + x0) * 3 + c] as f32;
                let s11 = src[(y1 * sw + x1) * 3 + c] as f32;
                let top = s00 + (s10 - s00) * tx;
                let bot = s01 + (s11 - s01) * tx;
                out[(y * dw + x) * 3 + c] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(out)
}

/// Working state for one dual-material, N-view DDIM trajectory.
pub struct DenoiseBatch {
    /// Flattened `[n_pbr * n_views]` rows of `4 * lat_w * lat_h` planar noise.
    pub sample: Vec<f32>,
    pub n_views: usize,
    pub lat_w: usize,
    pub lat_h: usize,
    pub steps: usize,
    pub guidance: f32,
    pub timesteps: Vec<usize>,
    /// Per-row view scale, length `n_pbr * n_views`.
    pub view_scales: Vec<f32>,
}

impl DenoiseBatch {
    pub fn init(
        seed: u64,
        azims: &[f32],
        lat_w: usize,
        lat_h: usize,
        steps: usize,
        guidance: f32,
        sched: &DdimVpredZsnr,
    ) -> Result<Self, PbrError> {
        if azims.is_empty() {
            return Err(PbrError::InvalidParams("denoise needs at least one view".into()));
        }
        if lat_w == 0 || lat_h == 0 {
            return Err(PbrError::InvalidParams("latent spatial size is zero".into()));
        }
        if steps == 0 {
            return Err(PbrError::InvalidParams("denoise step count is zero".into()));
        }
        let n_views = azims.len();
        let row = LATENT_CHANNELS
            .checked_mul(lat_w)
            .and_then(|v| v.checked_mul(lat_h))
            .ok_or_else(|| PbrError::InvalidParams("latent row overflow".into()))?;
        let n_rows = PBR_MATERIALS
            .checked_mul(n_views)
            .ok_or_else(|| PbrError::InvalidParams("denoise row overflow".into()))?;
        let n = n_rows
            .checked_mul(row)
            .ok_or_else(|| PbrError::InvalidParams("denoise sample overflow".into()))?;
        Ok(Self {
            sample: gaussian_sample(seed, n),
            n_views,
            lat_w,
            lat_h,
            steps,
            guidance,
            timesteps: sched.timesteps_trailing(steps),
            view_scales: view_scales_for_batch(azims),
        })
    }

    pub fn from_defaults(
        seed: u64,
        azims: &[f32],
        lat_w: usize,
        lat_h: usize,
        sched: &DdimVpredZsnr,
    ) -> Result<Self, PbrError> {
        let d = hunyuan::defaults();
        Self::init(
            seed,
            azims,
            lat_w,
            lat_h,
            d.num_inference_steps as usize,
            d.guidance_scale,
            sched,
        )
    }

    pub fn row_len(&self) -> usize {
        LATENT_CHANNELS * self.lat_w * self.lat_h
    }

    pub fn n_rows(&self) -> usize {
        PBR_MATERIALS * self.n_views
    }

    /// 12-ch packed UNet input, 3-way CFG-stacked.
    /// Layout: `[branch][material][view][12 * hw]` planar, matching
    /// `cond_assembly` flattening.
    pub fn pack_cfg_inputs(&self, normal: &[Vec<f32>], position: &[Vec<f32>]) -> Result<Vec<f32>, PbrError> {
        let hw = self.lat_w * self.lat_h;
        let row = self.row_len();
        if normal.len() != self.n_views || position.len() != self.n_views {
            return Err(PbrError::InvalidParams(format!(
                "pack expects {} view latents, got {} normal / {} position",
                self.n_views,
                normal.len(),
                position.len()
            )));
        }
        for (i, (n, p)) in normal.iter().zip(position.iter()).enumerate() {
            if n.len() != row || p.len() != row {
                return Err(PbrError::InvalidParams(format!(
                    "view {i} latent length {}/{} != {row}",
                    n.len(),
                    p.len()
                )));
            }
        }
        if self.sample.len() != self.n_rows() * row {
            return Err(PbrError::Internal("sample length drifted".into()));
        }
        let mut out = Vec::with_capacity(CFG_BRANCHES * self.n_rows() * PACKED_INPUT_CHANNELS * hw);
        for _branch in 0..CFG_BRANCHES {
            for mat in 0..PBR_MATERIALS {
                for view in 0..self.n_views {
                    let row_i = mat * self.n_views + view;
                    let noise = &self.sample[row_i * row..(row_i + 1) * row];
                    out.extend(pack_view_latent(noise, &normal[view], &position[view], hw));
                }
            }
        }
        Ok(out)
    }

    /// Apply one DDIM step from a 3-branch stacked v-prediction.
    pub fn apply_cfg_step(
        &mut self,
        sched: &DdimVpredZsnr,
        v_three_branches: &[f32],
        t: usize,
    ) -> Result<(), PbrError> {
        let row = self.row_len();
        let branch = self.sample.len();
        if v_three_branches.len() != CFG_BRANCHES * branch {
            return Err(PbrError::Internal(format!(
                "v-pred length {} != 3 * {}",
                v_three_branches.len(),
                branch
            )));
        }
        let uncond = &v_three_branches[..branch];
        let ref_only = &v_three_branches[branch..2 * branch];
        let full = &v_three_branches[2 * branch..];
        if std::env::var("MAKEPAD_PBR_CFG_DEBUG").as_deref() == Ok("1") {
            // Conditioning liveness probe: if the three branches are nearly
            // identical, the reference/DINO tokens never reached attention.
            let l2 = |v: &[f32]| (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>()).sqrt();
            let d = |a: &[f32], b: &[f32]| {
                (a.iter()
                    .zip(b)
                    .map(|(x, y)| ((*x - *y) as f64).powi(2))
                    .sum::<f64>())
                .sqrt()
            };
            eprintln!(
                "[pbr-cfg] t={t} |uncond|={:.3} |ref|={:.3} |full|={:.3} d(u,r)={:.4} d(r,f)={:.4}",
                l2(uncond),
                l2(ref_only),
                l2(full),
                d(uncond, ref_only),
                d(ref_only, full),
            );
        }
        let guided = guidance_combine(uncond, ref_only, full, self.guidance, &self.view_scales, row);
        self.sample = sched.ddim_step(&self.sample, &guided, t, self.steps);
        Ok(())
    }

    /// Split the current sample into per-view albedo / MR latents.
    pub fn split_materials(&self) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let row = self.row_len();
        let mut albedo = Vec::with_capacity(self.n_views);
        let mut mr = Vec::with_capacity(self.n_views);
        for view in 0..self.n_views {
            let a = view * row;
            albedo.push(self.sample[a..a + row].to_vec());
            let m = (self.n_views + view) * row;
            mr.push(self.sample[m..m + row].to_vec());
        }
        (albedo, mr)
    }
}

/// Drive the official 15-step (or `batch.steps`) loop. `predict_v` receives
/// the current sample and train timestep and must return a 3-branch stacked
/// v-prediction of length `3 * sample.len()`.
pub fn run_ddim_loop<F>(
    batch: &mut DenoiseBatch,
    sched: &DdimVpredZsnr,
    mut predict_v: F,
    progress: &mut dyn FnMut(u32, u32) -> bool,
) -> Result<(), PbrError>
where
    F: FnMut(&[f32], usize) -> Result<Vec<f32>, PbrError>,
{
    let total = batch.timesteps.len() as u32;
    for (i, &t) in batch.timesteps.clone().iter().enumerate() {
        if !progress(i as u32 + 1, total) {
            return Err(PbrError::Cancelled);
        }
        let v = predict_v(&batch.sample, t)?;
        batch.apply_cfg_step(sched, &v, t)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_is_deterministic_and_unitish() {
        let a = gaussian_sample(7, 1024);
        let b = gaussian_sample(7, 1024);
        assert_eq!(a, b);
        let mean = a.iter().map(|v| *v as f64).sum::<f64>() / a.len() as f64;
        let var = a.iter().map(|v| {
            let d = *v as f64 - mean;
            d * d
        }).sum::<f64>() / a.len() as f64;
        assert!(mean.abs() < 0.1, "mean {mean}");
        assert!((var - 1.0).abs() < 0.15, "var {var}");
        assert_ne!(gaussian_sample(8, 8), gaussian_sample(9, 8));
    }

    #[test]
    fn rgb8_planar_round_channels() {
        let rgb = [255u8, 128, 0, 0, 255, 128];
        let p = rgb8_interleaved_to_planar01(&rgb, 2, 1).unwrap();
        assert!((p[0] - 1.0).abs() < 1e-6);
        assert!((p[1] - 0.0).abs() < 1e-6);
        assert!((p[2] - 128.0 / 255.0).abs() < 1e-6);
        assert!((p[3] - 1.0).abs() < 1e-6);
        assert!((p[4] - 0.0).abs() < 1e-6);
        assert!((p[5] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn resize_identity_and_half() {
        let src = vec![10u8, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
        assert_eq!(resize_rgb8_bilinear(&src, 2, 2, 2, 2).unwrap(), src);
        let half = resize_rgb8_bilinear(&src, 2, 2, 1, 1).unwrap();
        assert_eq!(half.len(), 3);
        // Center sample of a 2x2 is the bilinear midpoint of all four pixels.
        assert_eq!(half, vec![55, 65, 75]);
    }

    #[test]
    fn pack_and_cfg_step_keep_shapes() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        let mut batch = DenoiseBatch::from_defaults(1, &[0.0, 90.0], 2, 2, &sched).unwrap();
        assert_eq!(batch.timesteps, sched.timesteps_trailing(DEFAULT_STEPS));
        assert_eq!(batch.n_rows(), 4);
        assert_eq!(batch.sample.len(), 4 * 4 * 2 * 2);
        let row = batch.row_len();
        let normal = vec![vec![0.1f32; row], vec![0.2; row]];
        let position = vec![vec![0.3f32; row], vec![0.4; row]];
        let packed = batch.pack_cfg_inputs(&normal, &position).unwrap();
        assert_eq!(packed.len(), CFG_BRANCHES * batch.n_rows() * PACKED_INPUT_CHANNELS * 4);
        // First view's noise occupies the first 4*hw of the packed 12-ch.
        assert_eq!(&packed[..row], &batch.sample[..row]);
        let v = vec![0.0f32; CFG_BRANCHES * batch.sample.len()];
        let t = batch.timesteps[0];
        batch.apply_cfg_step(&sched, &v, t).unwrap();
        assert_eq!(batch.sample.len(), 4 * row);
        assert!(batch.sample.iter().all(|x| x.is_finite()));
        let (albedo, mr) = batch.split_materials();
        assert_eq!(albedo.len(), 2);
        assert_eq!(mr.len(), 2);
        assert_eq!(albedo[0].len(), row);
    }

    #[test]
    fn ddim_loop_honors_cancel_and_runs_15() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        let mut batch = DenoiseBatch::from_defaults(2, &[0.0], 1, 1, &sched).unwrap();
        let mut seen = 0u32;
        let err = run_ddim_loop(
            &mut batch,
            &sched,
            |sample, _t| Ok(vec![0.0f32; CFG_BRANCHES * sample.len()]),
            &mut |step, total| {
                seen = step;
                assert_eq!(total, 15);
                step < 3
            },
        )
        .unwrap_err();
        assert_eq!(err, PbrError::Cancelled);
        assert_eq!(seen, 3);

        let mut batch = DenoiseBatch::from_defaults(3, &[180.0], 1, 1, &sched).unwrap();
        let mut last = 0;
        run_ddim_loop(
            &mut batch,
            &sched,
            |sample, _t| Ok(vec![0.0f32; CFG_BRANCHES * sample.len()]),
            &mut |step, _| {
                last = step;
                true
            },
        )
        .unwrap();
        assert_eq!(last, 15);
        assert!(batch.sample.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn defaults_match_hunyuan_pins() {
        assert_eq!(DEFAULT_STEPS, hunyuan::defaults().num_inference_steps as usize);
        assert_eq!(DEFAULT_GUIDANCE, hunyuan::defaults().guidance_scale);
    }
}
