#[cfg(doc)]
use super::RigidBodyActivation;
use crate::math::Real;
use simba::simd::SimdRealField;

// TODO: enabling the block solver in 3d introduces a lot of jitters in
//       the 3D domino demo. So for now we dont enable it in 3D.
pub(crate) static BLOCK_SOLVER_ENABLED: bool = cfg!(feature = "dim2");

/// Friction models used for all contact constraints between two rigid-bodies.
///
/// This selection does not apply to multibodies that always rely on the [`FrictionModel::Coulomb`].
#[cfg(feature = "dim3")]
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub enum FrictionModel {
    /// A simplified friction model significantly faster to solve than [`Self::Coulomb`]
    /// but less accurate.
    ///
    /// Instead of solving one Coulomb friction constraint per contact in a contact manifold,
    /// this approximation only solves one Coulomb friction constraint per group of 4 contacts
    /// in a contact manifold, plus one "twist" constraint. The "twist" constraint is purely
    /// rotational and aims to eliminate angular movement in the manifold’s tangent plane.
    #[default]
    Simplified,
    /// The coulomb friction model.
    ///
    /// This results in one Coulomb friction constraint per contact point.
    Coulomb,
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
// TODO: we should be able to combine this with MotorModel.
/// Coefficients for a spring, typically used for configuring constraint softness for contacts and
/// joints.
pub struct SpringCoefficients<N> {
    /// Sets the natural frequency (Hz) of the spring-like constraint.
    ///
    /// Higher values make the constraint stiffer and resolve constraint violations more quickly.
    pub natural_frequency: N,
    /// Sets the damping ratio for the spring-like constraint.
    ///
    /// Larger values make the joint more compliant (allowing more drift before stabilization).
    pub damping_ratio: N,
}

impl<N: SimdRealField<Element = Real> + Copy> SpringCoefficients<N> {
    /// Initializes spring coefficients from the spring natural frequency and damping ratio.
    pub fn new(natural_frequency: N, damping_ratio: N) -> Self {
        Self {
            natural_frequency,
            damping_ratio,
        }
    }

    /// Default softness coefficients for contacts.
    pub fn contact_defaults() -> Self {
        Self {
            natural_frequency: N::splat(30.0),
            damping_ratio: N::splat(5.0),
        }
    }

    /// Default softness coefficients for joints.
    pub fn joint_defaults() -> Self {
        Self {
            natural_frequency: N::splat(1.0e6),
            damping_ratio: N::splat(1.0),
        }
    }

    /// The contact’s spring angular frequency for constraints regularization.
    pub fn angular_frequency(&self) -> N {
        self.natural_frequency * N::simd_two_pi()
    }

    /// The [`Self::erp`] coefficient, multiplied by the inverse timestep length.
    pub fn erp_inv_dt(&self, dt: N) -> N {
        let ang_freq = self.angular_frequency();
        ang_freq / (dt * ang_freq + N::splat(2.0) * self.damping_ratio)
    }

    /// The effective Error Reduction Parameter applied for calculating regularization forces.
    ///
    /// This parameter is computed automatically from [`Self::natural_frequency`],
    /// [`Self::damping_ratio`] and the substep length.
    pub fn erp(&self, dt: N) -> N {
        dt * self.erp_inv_dt(dt)
    }

    /// Compute CFM assuming a critically damped spring multiplied by the damping ratio.
    ///
    /// This coefficient softens the impulse applied at each solver iteration.
    pub fn cfm_coeff(&self, dt: N) -> N {
        let one = N::one();
        let erp = self.erp(dt);
        let erp_is_not_zero = erp.simd_ne(N::zero());
        let inv_erp_minus_one = one / erp - one;

        // let stiffness = 4.0 * damping_ratio * damping_ratio * projected_mass
        //     / (dt * dt * inv_erp_minus_one * inv_erp_minus_one);
        // let damping = 4.0 * damping_ratio * damping_ratio * projected_mass
        //     / (dt * inv_erp_minus_one);
        // let cfm = 1.0 / (dt * dt * stiffness + dt * damping);
        // NOTE: This simplifies to cfm = cfm_coeff / projected_mass:
        let result = inv_erp_minus_one * inv_erp_minus_one
            / ((one + inv_erp_minus_one) * N::splat(4.0) * self.damping_ratio * self.damping_ratio);
        result.select(erp_is_not_zero, N::zero())
    }

    /// The CFM factor to be used in the constraint resolution.
    ///
    /// This parameter is computed automatically from [`Self::natural_frequency`],
    /// [`Self::damping_ratio`] and the substep length.
    pub fn cfm_factor(&self, dt: N) -> N {
        let one = N::one();
        let cfm_coeff = self.cfm_coeff(dt);

        // We use this coefficient inside the impulse resolution.
        // Surprisingly, several simplifications happen there.
        // Let `m` the projected mass of the constraint.
        // Let `m’` the projected mass that includes CFM: `m’ = 1 / (1 / m + cfm_coeff / m) = m / (1 + cfm_coeff)`
        // We have:
        // new_impulse = old_impulse - m’ (delta_vel - cfm * old_impulse)
        //             = old_impulse - m / (1 + cfm_coeff) * (delta_vel - cfm_coeff / m * old_impulse)
        //             = old_impulse * (1 - cfm_coeff / (1 + cfm_coeff)) - m / (1 + cfm_coeff) * delta_vel
        //             = old_impulse / (1 + cfm_coeff) - m * delta_vel / (1 + cfm_coeff)
        //             = 1 / (1 + cfm_coeff) * (old_impulse - m * delta_vel)
        // So, setting cfm_factor = 1 / (1 + cfm_coeff).
        // We obtain:
        // new_impulse = cfm_factor * (old_impulse - m * delta_vel)
        //
        // The value returned by this function is this cfm_factor that can be used directly
        // in the constraint solver.
        one / (one + cfm_coeff)
    }
}

/// Configuration parameters that control the physics simulation quality and behavior.
///
/// These parameters affect how the physics engine advances time, resolves collisions, and
/// maintains stability. The defaults work well for most games, but you may want to adjust
/// them based on your specific needs.
///
/// # Key parameters for beginners
///
/// - **`dt`**: Timestep duration (default: 1/60 second). Most games run physics at 60Hz.
/// - **`num_solver_iterations`**: More iterations = more accurate but slower (default: 4)
/// - **`length_unit`**: Scale factor if your world units aren't meters (e.g., 100 for pixel-based games)
///
/// # Example
///
/// ```
/// # use rapier3d::prelude::*;
/// // Standard 60 FPS physics with default settings
/// let mut integration_params = IntegrationParameters::default();
///
/// // For a more accurate (but slower) simulation:
/// integration_params.num_solver_iterations = 8;
///
/// // For pixel-based 2D games where 100 pixels = 1 meter:
/// integration_params.length_unit = 100.0;
/// ```
///
/// Most other parameters are advanced settings for fine-tuning stability and performance.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct IntegrationParameters {
    /// The timestep length - how much simulated time passes per physics step (default: `1.0 / 60.0`).
    ///
    /// Set this to `1.0 / your_target_fps`. For example:
    /// - 60 FPS: `1.0 / 60.0` ≈ 0.0167 seconds
    /// - 120 FPS: `1.0 / 120.0` ≈ 0.0083 seconds
    ///
    /// Smaller timesteps are more accurate but require more CPU time per second of simulated time.
    pub dt: Real,
    /// Minimum timestep size when using CCD with multiple substeps (default: `1.0 / 60.0 / 100.0`).
    ///
    /// When CCD with multiple substeps is enabled, the timestep is subdivided
    /// into smaller pieces. This timestep subdivision won't generate timestep
    /// lengths smaller than `min_ccd_dt`.
    ///
    /// Setting this to a large value will reduce the opportunity to performing
    /// CCD substepping, resulting in potentially more time dropped by the
    /// motion-clamping mechanism. Setting this to an very small value may lead
    /// to numerical instabilities.
    pub min_ccd_dt: Real,

    /// Softness coefficients for contact constraints.
    pub contact_softness: SpringCoefficients<Real>,

    /// The coefficient in `[0, 1]` applied to warmstart impulses, i.e., impulses that are used as the
    /// initial solution (instead of 0) at the next simulation step.
    ///
    /// This should generally be set to 1.
    ///
    /// (default `1.0`).
    pub warmstart_coefficient: Real,

    /// The scale factor for your world if you're not using meters (default: `1.0`).
    ///
    /// Rapier is tuned for human-scale objects measured in meters. If your game uses different
    /// units, set this to how many of your units equal 1 meter in the real world.
    ///
    /// **Examples:**
    /// - Your game uses meters: `length_unit = 1.0` (default)
    /// - Your game uses centimeters: `length_unit = 100.0` (100 cm = 1 m)
    /// - Pixel-based 2D game where typical objects are 100 pixels tall: `length_unit = 100.0`
    /// - Your game uses feet: `length_unit = 3.28` (approximately)
    ///
    /// This automatically scales various internal tolerances and thresholds to work correctly
    /// with your chosen units.
    pub length_unit: Real,

    /// Amount of penetration the engine won’t attempt to correct (default: `0.001m`).
    ///
    /// This value is implicitly scaled by [`IntegrationParameters::length_unit`].
    pub normalized_allowed_linear_error: Real,
    /// Maximum amount of penetration the solver will attempt to resolve in one timestep (default: `10.0`).
    ///
    /// This value is implicitly scaled by [`IntegrationParameters::length_unit`].
    pub normalized_max_corrective_velocity: Real,
    /// The maximal distance separating two objects that will generate predictive contacts (default: `0.002m`).
    ///
    /// This value is implicitly scaled by [`IntegrationParameters::length_unit`].
    pub normalized_prediction_distance: Real,
    /// The number of solver iterations run by the constraints solver for calculating forces (default: `4`).
    ///
    /// Higher values produce more accurate and stable simulations at the cost of performance.
    /// - `4` (default): Good balance for most games
    /// - `8-12`: Use for demanding scenarios (stacks of objects, complex machinery)
    /// - `1-2`: Use if performance is critical and accuracy can be sacrificed
    pub num_solver_iterations: usize,
    /// Number of internal Project Gauss Seidel (PGS) iterations run at each solver iteration (default: `1`).
    pub num_internal_pgs_iterations: usize,
    /// The number of stabilization iterations run at each solver iterations (default: `1`).
    pub num_internal_stabilization_iterations: usize,
    /// Minimum number of dynamic bodies on each active island (default: `128`).
    pub min_island_size: usize,
    /// Maximum number of substeps performed by the  solver (default: `1`).
    pub max_ccd_substeps: usize,
    /// The type of friction constraints used in the simulation.
    #[cfg(feature = "dim3")]
    pub friction_model: FrictionModel,
}

impl IntegrationParameters {
    /// The inverse of the time-stepping length, i.e. the steps per seconds (Hz).
    ///
    /// This is zero if `self.dt` is zero.
    #[inline]
    pub fn inv_dt(&self) -> Real {
        if self.dt == 0.0 { 0.0 } else { 1.0 / self.dt }
    }

    /// Sets the time-stepping length.
    #[inline]
    #[deprecated = "You can just set the `IntegrationParams::dt` value directly"]
    pub fn set_dt(&mut self, dt: Real) {
        assert!(dt >= 0.0, "The time-stepping length cannot be negative.");
        self.dt = dt;
    }

    /// Sets the inverse time-stepping length (i.e. the frequency).
    ///
    /// This automatically recompute `self.dt`.
    #[inline]
    pub fn set_inv_dt(&mut self, inv_dt: Real) {
        if inv_dt == 0.0 {
            self.dt = 0.0
        } else {
            self.dt = 1.0 / inv_dt
        }
    }

    /// Amount of penetration the engine won't attempt to correct (default: `0.001` multiplied by
    /// [`Self::length_unit`]).
    pub fn allowed_linear_error(&self) -> Real {
        self.normalized_allowed_linear_error * self.length_unit
    }

    /// Maximum amount of penetration the solver will attempt to resolve in one timestep.
    ///
    /// This is equal to [`Self::normalized_max_corrective_velocity`] multiplied by
    /// [`Self::length_unit`].
    pub fn max_corrective_velocity(&self) -> Real {
        if self.normalized_max_corrective_velocity != Real::MAX {
            self.normalized_max_corrective_velocity * self.length_unit
        } else {
            Real::MAX
        }
    }

    /// The maximal distance separating two objects that will generate predictive contacts
    /// (default: `0.002m` multiped by [`Self::length_unit`]).
    pub fn prediction_distance(&self) -> Real {
        self.normalized_prediction_distance * self.length_unit
    }
}

impl Default for IntegrationParameters {
    fn default() -> Self {
        Self {
            dt: 1.0 / 60.0,
            min_ccd_dt: 1.0 / 60.0 / 100.0,
            contact_softness: SpringCoefficients::contact_defaults(),
            warmstart_coefficient: 1.0,
            num_internal_pgs_iterations: 1,
            num_internal_stabilization_iterations: 1,
            num_solver_iterations: 4,
            // TODO: what is the optimal value for min_island_size?
            // It should not be too big so that we don't end up with
            // huge islands that don't fit in cache.
            // However we don't want it to be too small and end up with
            // tons of islands, reducing SIMD parallelism opportunities.
            min_island_size: 128,
            normalized_allowed_linear_error: 0.001,
            normalized_max_corrective_velocity: 10.0,
            normalized_prediction_distance: 0.002,
            max_ccd_substeps: 1,
            length_unit: 1.0,
            #[cfg(feature = "dim3")]
            friction_model: FrictionModel::default(),
        }
    }
}
