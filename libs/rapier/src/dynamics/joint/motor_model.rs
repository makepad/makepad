use crate::math::Real;

/// How motor stiffness/damping values are interpreted (mass-dependent vs mass-independent).
///
/// This affects how motors behave when attached to bodies of different masses.
///
/// ## Acceleration-Based (default, recommended)
///
/// Spring constants are automatically scaled by mass, so:
/// - Heavy and light objects respond similarly to same spring values
/// - Easier to tune - values work across different mass ranges
/// - **Formula**: `acceleration = stiffness × error + damping × velocity_error`
///
/// ## Force-Based
///
/// Spring constants produce absolute forces, so:
/// - Same spring values → different behavior for different masses
/// - More physically accurate representation
/// - Requires re-tuning if masses change
/// - **Formula**: `force = stiffness × error + damping × velocity_error`
///
/// **Most users should use AccelerationBased (the default).**
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub enum MotorModel {
    /// Spring constants auto-scale with mass (easier to tune, recommended).
    #[default]
    AccelerationBased,
    /// Spring constants produce absolute forces (mass-dependent).
    ForceBased,
}

impl MotorModel {
    /// Combines the coefficients used for solving the spring equation.
    ///
    /// Returns the coefficients (erp_inv_dt, cfm_coeff, cfm_gain).
    pub fn combine_coefficients(
        self,
        dt: Real,
        stiffness: Real,
        damping: Real,
    ) -> (Real, Real, Real) {
        match self {
            MotorModel::AccelerationBased => {
                let erp_inv_dt = stiffness * crate::utils::inv(dt * stiffness + damping);
                let cfm_coeff = crate::utils::inv(dt * dt * stiffness + dt * damping);
                (erp_inv_dt, cfm_coeff, 0.0)
            }
            MotorModel::ForceBased => {
                let erp_inv_dt = stiffness * crate::utils::inv(dt * stiffness + damping);
                let cfm_gain = crate::utils::inv(dt * dt * stiffness + dt * damping);
                (erp_inv_dt, 0.0, cfm_gain)
            }
        }
    }
}
