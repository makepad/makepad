pub(crate) use contact_constraint_element::*;
pub(crate) use contact_constraints_set::{ConstraintsCounts, ContactConstraintsSet};
pub(crate) use contact_with_coulomb_friction::*;
pub(crate) use generic_contact_constraint::*;
pub(crate) use generic_contact_constraint_element::*;

#[cfg(feature = "dim3")]
pub(crate) use contact_with_twist_friction::*;

mod contact_constraint_element;
mod contact_constraints_set;
mod contact_with_coulomb_friction;
mod generic_contact_constraint;
mod generic_contact_constraint_element;

mod any_contact_constraint;
#[cfg(feature = "dim3")]
mod contact_with_twist_friction;

#[cfg(feature = "dim3")]
use crate::utils::ScalarType;
#[cfg(feature = "dim3")]
use crate::{
    math::DIM,
    utils::{DisableFloatingPointExceptionsFlags, OrthonormalBasis},
};

#[inline]
#[cfg(feature = "dim3")]
pub(crate) fn compute_tangent_contact_directions<N: ScalarType>(
    force_dir1: &N::Vector,
    linvel1: &N::Vector,
    linvel2: &N::Vector,
) -> [N::Vector; DIM - 1] {
    use crate::utils::{CrossProduct, DotProduct, SimdLength, SimdSelect};
    use OrthonormalBasis;

    // Compute the tangent direction. Pick the direction of
    // the linear relative velocity, if it is not too small.
    // Otherwise use a fallback direction.
    let relative_linvel = *linvel1 - *linvel2;
    let mut tangent_relative_linvel =
        relative_linvel - *force_dir1 * (force_dir1.gdot(relative_linvel));

    let tangent_linvel_norm = {
        let _disable_fe_except =
            DisableFloatingPointExceptionsFlags::disable_floating_point_exceptions();
        let length = tangent_relative_linvel.simd_length();
        tangent_relative_linvel /= length;
        length
    };

    const THRESHOLD: f32 = 1.0e-4;
    let use_fallback = tangent_linvel_norm.simd_lt(na::convert(THRESHOLD));
    let tangent_fallback = force_dir1.orthonormal_vector();
    let tangent1 = tangent_fallback.select(use_fallback, tangent_relative_linvel);
    let bitangent1 = force_dir1.gcross(tangent1);

    [tangent1, bitangent1]
}
