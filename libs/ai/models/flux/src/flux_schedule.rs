use crate::{DiffusionError, Result};

pub const FLUX_VAE_SCALING_FACTOR: f32 = 0.3611;
pub const FLUX_VAE_SHIFT_FACTOR: f32 = 0.1159;

const FLUX_TIMESTEPS: f32 = 1000.0;
const FLUX_T_MAX: f32 = FLUX_TIMESTEPS - 1.0;
const FLUX_SHIFT_NO_GUIDANCE: f32 = 1.0;
const FLUX_SHIFT_WITH_GUIDANCE: f32 = 1.15;

#[derive(Clone, Debug)]
pub struct FluxSchedule {
    pub sigmas: Vec<f32>,
}

impl FluxSchedule {
    pub fn for_flux1(num_inference_steps: usize, guidance_embed: bool) -> Result<Self> {
        if num_inference_steps == 0 {
            return Err(DiffusionError::workflow(
                "flux schedule requires at least one inference step",
            ));
        }

        let mut sigmas = Vec::with_capacity(num_inference_steps + 1);
        let shift = if guidance_embed {
            FLUX_SHIFT_WITH_GUIDANCE
        } else {
            FLUX_SHIFT_NO_GUIDANCE
        };

        if num_inference_steps == 1 {
            sigmas.push(1.0);
            sigmas.push(0.0);
            return Ok(Self { sigmas });
        }

        let timestep_step = FLUX_T_MAX / (num_inference_steps - 1) as f32;
        for step_index in 0..num_inference_steps {
            let timestep = FLUX_T_MAX - step_index as f32 * timestep_step;
            let t = (timestep + 1.0) / FLUX_TIMESTEPS;
            sigmas.push(flux_time_shift(shift, 1.0, t));
        }
        sigmas.push(0.0);
        Ok(Self { sigmas })
    }
}

pub fn euler_step(
    sample: &mut [f32],
    model_output: &[f32],
    sigma: f32,
    sigma_next: f32,
) -> Result<()> {
    if sample.len() != model_output.len() {
        return Err(DiffusionError::workflow(format!(
            "flux euler step length mismatch: sample {} vs model_output {}",
            sample.len(),
            model_output.len()
        )));
    }
    let dt = sigma_next - sigma;
    for (sample_value, model_value) in sample.iter_mut().zip(model_output.iter()) {
        *sample_value += dt * model_value;
    }
    Ok(())
}

fn flux_time_shift(mu: f32, sigma: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    mu.exp() / (mu.exp() + (1.0 / t - 1.0).powf(sigma))
}

#[cfg(test)]
mod tests {
    use super::{
        flux_time_shift, FluxSchedule, FLUX_SHIFT_NO_GUIDANCE, FLUX_SHIFT_WITH_GUIDANCE,
        FLUX_TIMESTEPS, FLUX_T_MAX,
    };

    #[test]
    fn matches_flux1_guided_discrete_schedule() {
        let schedule = FluxSchedule::for_flux1(20, true).unwrap();
        assert_eq!(schedule.sigmas.len(), 21);
        assert!((schedule.sigmas[0] - 1.0).abs() < 1.0e-6);

        let step = FLUX_T_MAX / 19.0;
        let expected = flux_time_shift(
            FLUX_SHIFT_WITH_GUIDANCE,
            1.0,
            (FLUX_T_MAX - step + 1.0) / FLUX_TIMESTEPS,
        );
        assert!((schedule.sigmas[1] - expected).abs() < 1.0e-6);
        assert_eq!(schedule.sigmas[20], 0.0);
    }

    #[test]
    fn matches_flux1_unguided_discrete_schedule() {
        let schedule = FluxSchedule::for_flux1(20, false).unwrap();
        let step = FLUX_T_MAX / 19.0;
        let expected = flux_time_shift(
            FLUX_SHIFT_NO_GUIDANCE,
            1.0,
            (FLUX_T_MAX - step + 1.0) / FLUX_TIMESTEPS,
        );
        assert!((schedule.sigmas[1] - expected).abs() < 1.0e-6);
    }

    #[test]
    fn supports_single_step() {
        let schedule = FluxSchedule::for_flux1(1, true).unwrap();
        assert_eq!(schedule.sigmas, vec![1.0, 0.0]);
    }
}
