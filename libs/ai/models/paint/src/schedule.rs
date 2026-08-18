//! The exact noise schedule used by Hunyuan3D-Paint-2.1
//! (`scheduler/scheduler_config.json` @ 0b946776):
//! DDIM, v-prediction, scaled_linear betas 0.00085 -> 0.012 over 1000 train
//! steps, `timestep_spacing: "trailing"`, `rescale_betas_zero_snr: true`,
//! `set_alpha_to_one: true`, `clip_sample: false`, eta 0.
//!
//! Numerics follow diffusers' `DDIMScheduler` bit-for-bit in structure:
//! * betas are squared-space linear (`linspace(sqrt(b0), sqrt(b1), N)^2`);
//! * zero-terminal-SNR rescaling operates on sqrt cumulative alphas
//!   (Lin et al., "Common Diffusion Noise Schedules and Sample Steps are
//!   Flawed"), leaving the first cumprod unchanged and forcing the last to 0;
//! * trailing spacing is `round(arange(N, 0, -N/steps)) - 1` with numpy's
//!   round-half-to-even;
//! * the step's previous timestep is `t - N // steps` (integer division),
//!   independent of the iterated timestep list — a diffusers quirk that must
//!   be replicated for oracle parity (e.g. t=999, 15 steps -> prev 933 while
//!   the next list entry is 932);
//! * when the previous timestep is negative, `set_alpha_to_one` uses
//!   final alpha_cumprod = 1.0.

pub const TRAIN_TIMESTEPS: usize = 1000;
pub const BETA_START: f64 = 0.00085;
pub const BETA_END: f64 = 0.012;

pub struct DdimVpredZsnr {
    /// Rescaled cumulative alphas; `alphas_cumprod[999] == 0` exactly.
    pub alphas_cumprod: Vec<f64>,
    pub final_alpha_cumprod: f64,
}

/// numpy-compatible round-half-to-even.
pub fn round_half_even(v: f64) -> f64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff > 0.5 {
        floor + 1.0
    } else if diff < 0.5 {
        floor
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    }
}

fn scaled_linear_betas() -> Vec<f64> {
    let s0 = BETA_START.sqrt();
    let s1 = BETA_END.sqrt();
    let n = TRAIN_TIMESTEPS;
    (0..n)
        .map(|i| {
            let t = s0 + (s1 - s0) * (i as f64) / ((n - 1) as f64);
            t * t
        })
        .collect()
}

impl DdimVpredZsnr {
    pub fn hunyuan_paint() -> Self {
        let betas = scaled_linear_betas();
        let mut sqrt_cumprod = Vec::with_capacity(betas.len());
        let mut acc = 1.0f64;
        for beta in &betas {
            acc *= 1.0 - beta;
            sqrt_cumprod.push(acc.sqrt());
        }
        // Zero-terminal-SNR rescale on sqrt(alphas_cumprod).
        let s0 = sqrt_cumprod[0];
        let s_t = *sqrt_cumprod.last().unwrap();
        let scale = s0 / (s0 - s_t);
        let alphas_cumprod: Vec<f64> = sqrt_cumprod
            .iter()
            .map(|s| {
                let shifted = (s - s_t) * scale;
                shifted * shifted
            })
            .collect();
        Self {
            alphas_cumprod,
            final_alpha_cumprod: 1.0, // set_alpha_to_one
        }
    }

    /// Trailing timestep spacing for `steps` inference steps, descending.
    pub fn timesteps_trailing(&self, steps: usize) -> Vec<usize> {
        let ratio = TRAIN_TIMESTEPS as f64 / steps as f64;
        (0..steps)
            .map(|i| {
                let v = TRAIN_TIMESTEPS as f64 - i as f64 * ratio;
                (round_half_even(v) as i64 - 1) as usize
            })
            .collect()
    }

    /// diffusers `previous_timestep` for DDIM: `t - N // steps`, or None below zero.
    pub fn prev_timestep(&self, t: usize, steps: usize) -> Option<usize> {
        let delta = (TRAIN_TIMESTEPS / steps) as i64;
        let prev = t as i64 - delta;
        if prev >= 0 {
            Some(prev as usize)
        } else {
            None
        }
    }

    /// (sqrt(alpha_bar), sqrt(1 - alpha_bar)) at train timestep t.
    pub fn alpha_sigma(&self, t: usize) -> (f64, f64) {
        let ac = self.alphas_cumprod[t];
        (ac.sqrt(), (1.0 - ac).sqrt())
    }

    /// v-prediction to predicted clean sample: x0 = a*x - s*v.
    pub fn v_to_x0(&self, t: usize, x: &[f32], v: &[f32]) -> Vec<f32> {
        let (a, s) = self.alpha_sigma(t);
        x.iter()
            .zip(v.iter())
            .map(|(x, v)| (a * *x as f64 - s * *v as f64) as f32)
            .collect()
    }

    /// v-prediction to predicted noise: eps = s*x + a*v.
    pub fn v_to_eps(&self, t: usize, x: &[f32], v: &[f32]) -> Vec<f32> {
        let (a, s) = self.alpha_sigma(t);
        x.iter()
            .zip(v.iter())
            .map(|(x, v)| (s * *x as f64 + a * *v as f64) as f32)
            .collect()
    }

    /// Linear DDIM rewrite: `x_prev = c1 * x + c2 * v`.
    pub fn ddim_linear_coeffs(&self, t: usize, steps: usize) -> (f32, f32) {
        let (a, s) = self.alpha_sigma(t);
        let (a_prev, s_prev) = match self.prev_timestep(t, steps) {
            Some(prev) => self.alpha_sigma_with_final(prev),
            None => (
                self.final_alpha_cumprod.sqrt(),
                (1.0 - self.final_alpha_cumprod).sqrt(),
            ),
        };
        // x0 = a x - s v, eps = s x + a v, x_prev = a_prev x0 + s_prev eps
        let c1 = a_prev * a + s_prev * s;
        let c2 = s_prev * a - a_prev * s;
        (c1 as f32, c2 as f32)
    }

    /// One deterministic DDIM step (eta = 0) from train timestep `t` given the
    /// model's v output. Returns the sample at the previous timestep.
    pub fn ddim_step(&self, x: &[f32], v: &[f32], t: usize, steps: usize) -> Vec<f32> {
        let (a, s) = self.alpha_sigma(t);
        let (a_prev, s_prev) = match self.prev_timestep(t, steps) {
            Some(prev) => self.alpha_sigma_with_final(prev),
            None => (self.final_alpha_cumprod.sqrt(), (1.0 - self.final_alpha_cumprod).sqrt()),
        };
        x.iter()
            .zip(v.iter())
            .map(|(xv, vv)| {
                let xf = *xv as f64;
                let vf = *vv as f64;
                let x0 = a * xf - s * vf;
                let eps = s * xf + a * vf;
                (a_prev * x0 + s_prev * eps) as f32
            })
            .collect()
    }

    fn alpha_sigma_with_final(&self, t: usize) -> (f64, f64) {
        self.alpha_sigma(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_half_even_matches_numpy() {
        assert_eq!(round_half_even(937.5), 938.0);
        assert_eq!(round_half_even(812.5), 812.0);
        assert_eq!(round_half_even(866.6666666666666), 867.0);
        assert_eq!(round_half_even(733.3333333333333), 733.0);
        assert_eq!(round_half_even(-0.5), 0.0);
    }

    #[test]
    fn trailing_15_matches_reference_list() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        assert_eq!(
            sched.timesteps_trailing(15),
            vec![999, 932, 866, 799, 732, 666, 599, 532, 466, 399, 332, 266, 199, 132, 66]
        );
    }

    #[test]
    fn trailing_30_spot_values() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        let ts = sched.timesteps_trailing(30);
        assert_eq!(ts.len(), 30);
        assert_eq!(ts[0], 999);
        assert_eq!(ts[1], 966);
        assert_eq!(ts[2], 932);
        assert_eq!(ts[28], 66);
        assert_eq!(ts[29], 32);
    }

    #[test]
    fn zsnr_terminal_and_start() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        assert_eq!(sched.alphas_cumprod.len(), TRAIN_TIMESTEPS);
        // Terminal SNR is exactly zero.
        assert!(sched.alphas_cumprod[999].abs() < 1e-24, "terminal {}", sched.alphas_cumprod[999]);
        // First cumprod is preserved by the rescale: 1 - beta_0.
        let expect0 = 1.0 - BETA_START;
        assert!((sched.alphas_cumprod[0] - expect0).abs() < 1e-12);
        // Monotone decreasing.
        for w in sched.alphas_cumprod.windows(2) {
            assert!(w[1] < w[0] + 1e-15);
        }
    }

    #[test]
    fn prev_timestep_replicates_diffusers_quirk() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        // 1000 // 15 == 66, so prev(999) is 933 even though the next iterated
        // timestep in the trailing list is 932.
        assert_eq!(sched.prev_timestep(999, 15), Some(933));
        assert_eq!(sched.prev_timestep(66, 15), Some(0));
        assert_eq!(sched.prev_timestep(32, 30), None);
    }

    #[test]
    fn ddim_step_transports_exact_x0_eps_pair() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        let x0 = [0.25f32, -1.5, 0.75, 2.0];
        let eps = [1.0f32, 0.5, -0.25, -1.0];
        let t = 666;
        let steps = 15;
        let (a, s) = sched.alpha_sigma(t);
        let x_t: Vec<f32> = x0
            .iter()
            .zip(eps.iter())
            .map(|(x0, e)| (a * *x0 as f64 + s * *e as f64) as f32)
            .collect();
        let v: Vec<f32> = x0
            .iter()
            .zip(eps.iter())
            .map(|(x0, e)| (a * *e as f64 - s * *x0 as f64) as f32)
            .collect();
        let prev = sched.prev_timestep(t, steps).unwrap();
        assert_eq!(prev, 600);
        let (ap, sp) = sched.alpha_sigma(prev);
        let expect: Vec<f32> = x0
            .iter()
            .zip(eps.iter())
            .map(|(x0, e)| (ap * *x0 as f64 + sp * *e as f64) as f32)
            .collect();
        let got = sched.ddim_step(&x_t, &v, t, steps);
        for (g, e) in got.iter().zip(expect.iter()) {
            assert!((g - e).abs() < 2e-6, "got {g} expect {e}");
        }
    }

    #[test]
    fn ddim_final_step_uses_alpha_one() {
        let sched = DdimVpredZsnr::hunyuan_paint();
        // steps=30 -> last trailing t=32, prev is negative -> final alpha 1.0:
        // the step returns exactly the predicted x0.
        let x0 = [0.5f32, -0.5];
        let eps = [0.1f32, 0.2];
        let t = 32;
        let (a, s) = sched.alpha_sigma(t);
        let x_t: Vec<f32> = x0
            .iter()
            .zip(eps.iter())
            .map(|(x0, e)| (a * *x0 as f64 + s * *e as f64) as f32)
            .collect();
        let v: Vec<f32> = x0
            .iter()
            .zip(eps.iter())
            .map(|(x0, e)| (a * *e as f64 - s * *x0 as f64) as f32)
            .collect();
        let got = sched.ddim_step(&x_t, &v, t, 30);
        for (g, e) in got.iter().zip(x0.iter()) {
            assert!((g - e).abs() < 2e-6);
        }
    }
}
