use crate::dynamics::solver::SolverVel;
use crate::math::{DIM, TangentImpulse};
use crate::utils::{ComponentMul, DotProduct, ScalarType};
use na::Vector2;
use simba::simd::SimdValue;

#[cfg(feature = "dim3")]
#[derive(Copy, Clone, Debug)]
pub(crate) struct ContactConstraintTwistPart<N: ScalarType> {
    // pub twist_dir: N::AngVector, // NOTE: The torque direction equals the normal in 3D and 1.0 in 2D.
    pub ii_twist_dir1: N::AngVector,
    pub ii_twist_dir2: N::AngVector,
    pub rhs: N,
    pub impulse: N,
    pub impulse_accumulator: N,
    pub r: N,
}

#[cfg(feature = "dim3")]
impl<N: ScalarType> ContactConstraintTwistPart<N> {
    #[inline]
    pub fn warmstart(&mut self, solver_vel1: &mut SolverVel<N>, solver_vel2: &mut SolverVel<N>)
    where
        N::AngVector: DotProduct<N::AngVector, Result = N>,
    {
        solver_vel1.angular += self.ii_twist_dir1 * self.impulse;
        solver_vel2.angular += self.ii_twist_dir2 * self.impulse;
    }

    #[inline]
    pub fn solve(
        &mut self,
        twist_dir1: &N::AngVector,
        limit: N,
        solver_vel1: &mut SolverVel<N>,
        solver_vel2: &mut SolverVel<N>,
    ) where
        N::AngVector: DotProduct<N::AngVector, Result = N>,
    {
        let dvel = twist_dir1.gdot(solver_vel1.angular - solver_vel2.angular) + self.rhs;
        let new_impulse = (self.impulse - self.r * dvel).simd_clamp(-limit, limit);
        let dlambda = new_impulse - self.impulse;
        self.impulse = new_impulse;
        solver_vel1.angular += self.ii_twist_dir1 * dlambda;
        solver_vel2.angular += self.ii_twist_dir2 * dlambda;
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ContactConstraintTangentPart<N: ScalarType> {
    pub torque_dir1: [N::AngVector; DIM - 1],
    pub torque_dir2: [N::AngVector; DIM - 1],
    pub ii_torque_dir1: [N::AngVector; DIM - 1],
    pub ii_torque_dir2: [N::AngVector; DIM - 1],
    pub rhs: [N; DIM - 1],
    pub rhs_wo_bias: [N; DIM - 1],
    #[cfg(feature = "dim2")]
    pub impulse: na::Vector1<N>,
    #[cfg(feature = "dim3")]
    pub impulse: na::Vector2<N>,
    #[cfg(feature = "dim2")]
    pub impulse_accumulator: na::Vector1<N>,
    #[cfg(feature = "dim3")]
    pub impulse_accumulator: na::Vector2<N>,
    #[cfg(feature = "dim2")]
    pub r: [N; 1],
    #[cfg(feature = "dim3")]
    pub r: [N; DIM],
}

impl<N: ScalarType> ContactConstraintTangentPart<N> {
    pub fn zero() -> Self {
        Self {
            torque_dir1: [Default::default(); DIM - 1],
            torque_dir2: [Default::default(); DIM - 1],
            ii_torque_dir1: [Default::default(); DIM - 1],
            ii_torque_dir2: [Default::default(); DIM - 1],
            rhs: [N::zero(); DIM - 1],
            rhs_wo_bias: [N::zero(); DIM - 1],
            impulse: na::zero(),
            impulse_accumulator: na::zero(),
            #[cfg(feature = "dim2")]
            r: [N::zero(); 1],
            #[cfg(feature = "dim3")]
            r: [N::zero(); DIM],
        }
    }

    /// Total impulse applied across all the solver substeps.
    #[inline]
    pub fn total_impulse(&self) -> TangentImpulse<N> {
        self.impulse_accumulator + self.impulse
    }

    #[inline]
    pub fn warmstart(
        &mut self,
        tangents1: [&N::Vector; DIM - 1],
        im1: &N::Vector,
        im2: &N::Vector,
        solver_vel1: &mut SolverVel<N>,
        solver_vel2: &mut SolverVel<N>,
    ) where
        N::AngVector: DotProduct<N::AngVector, Result = N>,
    {
        #[cfg(feature = "dim2")]
        {
            solver_vel1.linear += tangents1[0].component_mul(im1) * self.impulse[0];
            solver_vel1.angular += self.ii_torque_dir1[0] * self.impulse[0];

            solver_vel2.linear += tangents1[0].component_mul(im2) * -self.impulse[0];
            solver_vel2.angular += self.ii_torque_dir2[0] * self.impulse[0];
        }

        #[cfg(feature = "dim3")]
        {
            solver_vel1.linear += (*tangents1[0] * self.impulse[0]
                + *tangents1[1] * self.impulse[1])
                .component_mul(im1);
            solver_vel1.angular +=
                self.ii_torque_dir1[0] * self.impulse[0] + self.ii_torque_dir1[1] * self.impulse[1];

            solver_vel2.linear += (*tangents1[0] * -self.impulse[0]
                + *tangents1[1] * -self.impulse[1])
                .component_mul(im2);
            solver_vel2.angular +=
                self.ii_torque_dir2[0] * self.impulse[0] + self.ii_torque_dir2[1] * self.impulse[1];
        }
    }

    #[inline]
    pub fn solve(
        &mut self,
        tangents1: [&N::Vector; DIM - 1],
        im1: &N::Vector,
        im2: &N::Vector,
        limit: N,
        solver_vel1: &mut SolverVel<N>,
        solver_vel2: &mut SolverVel<N>,
    ) where
        N::AngVector: DotProduct<N::AngVector, Result = N>,
    {
        #[cfg(feature = "dim2")]
        {
            let dvel = tangents1[0].gdot(solver_vel1.linear)
                + self.torque_dir1[0].gdot(solver_vel1.angular)
                - tangents1[0].gdot(solver_vel2.linear)
                + self.torque_dir2[0].gdot(solver_vel2.angular)
                + self.rhs[0];
            let new_impulse = (self.impulse[0] - self.r[0] * dvel).simd_clamp(-limit, limit);
            let dlambda = new_impulse - self.impulse[0];
            self.impulse[0] = new_impulse;

            solver_vel1.linear += tangents1[0].component_mul(im1) * dlambda;
            solver_vel1.angular += self.ii_torque_dir1[0] * dlambda;

            solver_vel2.linear += tangents1[0].component_mul(im2) * -dlambda;
            solver_vel2.angular += self.ii_torque_dir2[0] * dlambda;
        }

        #[cfg(feature = "dim3")]
        {
            let dvel_0 = tangents1[0].gdot(solver_vel1.linear)
                + self.torque_dir1[0].gdot(solver_vel1.angular)
                - tangents1[0].gdot(solver_vel2.linear)
                + self.torque_dir2[0].gdot(solver_vel2.angular)
                + self.rhs[0];
            let dvel_1 = tangents1[1].gdot(solver_vel1.linear)
                + self.torque_dir1[1].gdot(solver_vel1.angular)
                - tangents1[1].gdot(solver_vel2.linear)
                + self.torque_dir2[1].gdot(solver_vel2.angular)
                + self.rhs[1];

            let dvel_00 = dvel_0 * dvel_0;
            let dvel_11 = dvel_1 * dvel_1;
            let dvel_01 = dvel_0 * dvel_1;
            let inv_lhs = (dvel_00 + dvel_11)
                * crate::utils::simd_inv(
                    dvel_00 * self.r[0] + dvel_11 * self.r[1] + dvel_01 * self.r[2],
                );
            let delta_impulse = Vector2::new(inv_lhs * dvel_0, inv_lhs * dvel_1);
            let new_impulse = self.impulse - delta_impulse;
            let new_impulse = {
                let _disable_fe_except =
                        crate::utils::DisableFloatingPointExceptionsFlags::
                        disable_floating_point_exceptions();
                new_impulse.simd_cap_magnitude(limit)
            };

            let dlambda = new_impulse - self.impulse;
            self.impulse = new_impulse;

            solver_vel1.linear +=
                (*tangents1[0] * dlambda[0] + *tangents1[1] * dlambda[1]).component_mul(im1);
            solver_vel1.angular +=
                self.ii_torque_dir1[0] * dlambda[0] + self.ii_torque_dir1[1] * dlambda[1];

            solver_vel2.linear +=
                (*tangents1[0] * -dlambda[0] + *tangents1[1] * -dlambda[1]).component_mul(im2);
            solver_vel2.angular +=
                self.ii_torque_dir2[0] * dlambda[0] + self.ii_torque_dir2[1] * dlambda[1];
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct ContactConstraintNormalPart<N: ScalarType> {
    pub torque_dir1: N::AngVector,
    pub torque_dir2: N::AngVector,
    pub ii_torque_dir1: N::AngVector,
    pub ii_torque_dir2: N::AngVector,
    pub rhs: N,
    pub rhs_wo_bias: N,
    pub impulse: N,
    pub impulse_accumulator: N,
    pub r: N,
    // For coupled constraint pairs, even constraints store the
    // diagonal of the projected mass matrix. Odd constraints
    // store the off-diagonal element of the projected mass matrix,
    // as well as the off-diagonal element of the inverse projected mass matrix.
    pub r_mat_elts: [N; 2],
}

impl<N: ScalarType> ContactConstraintNormalPart<N> {
    pub fn zero() -> Self {
        Self {
            torque_dir1: Default::default(),
            torque_dir2: Default::default(),
            ii_torque_dir1: Default::default(),
            ii_torque_dir2: Default::default(),
            rhs: N::zero(),
            rhs_wo_bias: N::zero(),
            impulse: N::zero(),
            impulse_accumulator: N::zero(),
            r: N::zero(),
            r_mat_elts: [N::zero(); 2],
        }
    }

    /// Total impulse applied across all the solver substeps.
    #[inline]
    pub fn total_impulse(&self) -> N {
        self.impulse_accumulator + self.impulse
    }

    #[inline]
    pub fn warmstart(
        &mut self,
        dir1: &N::Vector,
        im1: &N::Vector,
        im2: &N::Vector,
        solver_vel1: &mut SolverVel<N>,
        solver_vel2: &mut SolverVel<N>,
    ) {
        solver_vel1.linear += dir1.component_mul(im1) * self.impulse;
        solver_vel1.angular += self.ii_torque_dir1 * self.impulse;

        solver_vel2.linear += dir1.component_mul(im2) * -self.impulse;
        solver_vel2.angular += self.ii_torque_dir2 * self.impulse;
    }

    #[inline]
    pub fn solve(
        &mut self,
        cfm_factor: N,
        dir1: &N::Vector,
        im1: &N::Vector,
        im2: &N::Vector,
        solver_vel1: &mut SolverVel<N>,
        solver_vel2: &mut SolverVel<N>,
    ) where
        N::AngVector: DotProduct<N::AngVector, Result = N>,
    {
        let dvel = dir1.gdot(solver_vel1.linear) + self.torque_dir1.gdot(solver_vel1.angular)
            - dir1.gdot(solver_vel2.linear)
            + self.torque_dir2.gdot(solver_vel2.angular)
            + self.rhs;
        let new_impulse = cfm_factor * (self.impulse - self.r * dvel).simd_max(N::zero());
        let dlambda = new_impulse - self.impulse;
        self.impulse = new_impulse;

        solver_vel1.linear += dir1.component_mul(im1) * dlambda;
        solver_vel1.angular += self.ii_torque_dir1 * dlambda;

        solver_vel2.linear += dir1.component_mul(im2) * -dlambda;
        solver_vel2.angular += self.ii_torque_dir2 * dlambda;
    }

    #[inline]
    pub(crate) fn solve_mlcp_two_constraints(
        dvel: Vector2<N>,
        prev_impulse: Vector2<N>,
        r_a: N,
        r_b: N,
        [r_mat11, r_mat22]: [N; 2],
        [r_mat12, r_mat_inv12]: [N; 2],
        cfm_factor: N,
    ) -> Vector2<N> {
        let r_dvel = Vector2::new(
            r_mat11 * dvel.x + r_mat12 * dvel.y,
            r_mat12 * dvel.x + r_mat22 * dvel.y,
        );
        let new_impulse0 = prev_impulse - r_dvel;
        let new_impulse1 = Vector2::new(prev_impulse.x - r_a * dvel.x, N::zero());
        let new_impulse2 = Vector2::new(N::zero(), prev_impulse.y - r_b * dvel.y);
        let new_impulse3 = Vector2::new(N::zero(), N::zero());

        let keep0 = new_impulse0.x.simd_ge(N::zero()) & new_impulse0.y.simd_ge(N::zero());
        let keep1 = new_impulse1.x.simd_ge(N::zero())
            & (dvel.y + r_mat_inv12 * new_impulse1.x).simd_ge(N::zero());
        let keep2 = new_impulse2.y.simd_ge(N::zero())
            & (dvel.x + r_mat_inv12 * new_impulse2.y).simd_ge(N::zero());
        let keep3 = dvel.x.simd_ge(N::zero()) & dvel.y.simd_ge(N::zero());

        let selected3 = (new_impulse3 * cfm_factor).select(keep3, prev_impulse);
        let selected2 = (new_impulse2 * cfm_factor).select(keep2, selected3);
        let selected1 = (new_impulse1 * cfm_factor).select(keep1, selected2);
        (new_impulse0 * cfm_factor).select(keep0, selected1)
    }

    #[inline]
    pub fn solve_pair(
        constraint_a: &mut Self,
        constraint_b: &mut Self,
        cfm_factor: N,
        dir1: &N::Vector,
        im1: &N::Vector,
        im2: &N::Vector,
        solver_vel1: &mut SolverVel<N>,
        solver_vel2: &mut SolverVel<N>,
    ) where
        N::AngVector: DotProduct<N::AngVector, Result = N>,
    {
        let dvel_lin = dir1.gdot(solver_vel1.linear) - dir1.gdot(solver_vel2.linear);
        let dvel_a = dvel_lin
            + constraint_a.torque_dir1.gdot(solver_vel1.angular)
            + constraint_a.torque_dir2.gdot(solver_vel2.angular)
            + constraint_a.rhs;
        let dvel_b = dvel_lin
            + constraint_b.torque_dir1.gdot(solver_vel1.angular)
            + constraint_b.torque_dir2.gdot(solver_vel2.angular)
            + constraint_b.rhs;

        let prev_impulse = Vector2::new(constraint_a.impulse, constraint_b.impulse);
        let new_impulse = Self::solve_mlcp_two_constraints(
            Vector2::new(dvel_a, dvel_b),
            prev_impulse,
            constraint_a.r,
            constraint_b.r,
            constraint_a.r_mat_elts,
            constraint_b.r_mat_elts,
            cfm_factor,
        );

        let dlambda = new_impulse - prev_impulse;

        constraint_a.impulse = new_impulse.x;
        constraint_b.impulse = new_impulse.y;

        solver_vel1.linear += dir1.component_mul(im1) * (dlambda.x + dlambda.y);
        solver_vel1.angular +=
            constraint_a.ii_torque_dir1 * dlambda.x + constraint_b.ii_torque_dir1 * dlambda.y;
        solver_vel2.linear += dir1.component_mul(im2) * (-dlambda.x - dlambda.y);
        solver_vel2.angular +=
            constraint_a.ii_torque_dir2 * dlambda.x + constraint_b.ii_torque_dir2 * dlambda.y;
    }
}
