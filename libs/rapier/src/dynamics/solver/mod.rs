// #[cfg(not(feature = "parallel"))]
pub(crate) use self::island_solver::IslandSolver;
// #[cfg(feature = "parallel")]
// pub(crate) use self::parallel_island_solver::{ParallelIslandSolver, ThreadContext};
// #[cfg(feature = "parallel")]
// pub(self) use self::parallel_solver_constraints::ParallelSolverConstraints;
// #[cfg(feature = "parallel")]
// pub(self) use self::parallel_velocity_solver::ParallelVelocitySolver;
// #[cfg(not(feature = "parallel"))]
use self::velocity_solver::VelocitySolver;

use contact_constraint::*;
pub(crate) use joint_constraint::MotorParameters;
pub use joint_constraint::*;
use solver_body::SolverVel;

mod categorization;
mod contact_constraint;
mod interaction_groups;
// #[cfg(not(feature = "parallel"))]
mod island_solver;
mod joint_constraint;
// #[cfg(feature = "parallel")]
// mod parallel_island_solver;
// #[cfg(feature = "parallel")]
// mod parallel_solver_constraints;
// #[cfg(feature = "parallel")]
// mod parallel_velocity_solver;
mod solver_body;
// #[cfg(not(feature = "parallel"))]
// #[cfg(not(feature = "parallel"))]
mod velocity_solver;

// TODO: SAFETY: restrict with bytemuck::Zeroable to make this safe.
pub unsafe fn reset_buffer<T>(buffer: &mut Vec<T>, len: usize) {
    buffer.clear();
    buffer.reserve(len);

    unsafe {
        // NOTE: writing zeros is faster than u8::MAX.
        buffer.as_mut_ptr().write_bytes(0, len);
        buffer.set_len(len);
    }
}
