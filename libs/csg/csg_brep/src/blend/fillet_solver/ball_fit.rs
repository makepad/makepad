//! Whether a fillet solver's rolling ball fits the faces it rolls on.
//!
//! Upstream: not in SMLib at the pinned commit. source's fork adds
//! `SmFilletSolver::BallFitsItsFaces` (`SmFilletSolver.cpp`) and its call before the
//! corners in `SmFilletExecutive::CreateFilletBrep`, with the error type
//! `FilletErrorType::NoRoom` (not an upstream code); the source is the diff
//! `2c5baae..e5696ca` quoted in
//! source issue #1172.
//!
//! | Rust                 | Upstream (fork)                       |
//! |----------------------|---------------------------------------|
//! | [`ball_fits_its_faces`] | `SmFilletSolver::BallFitsItsFaces` |
//!
//! Deviations: a solver without an edge use judges nothing (the fork's `NER` fails the
//! fillet with a null pointer); the fork's evaluation order `[1] = Dv, [2] = Du` is
//! Rust's `(P, Du, Dv)`, which gives the same determinant and trace.

use super::p1::FilletSolverOps;
use crate::blend::fillet_custom_solver::offset_evaluate_1st;
use crate::error::{Result, SolidError};
use crate::geom::vector2d::Point2;
use crate::intersect::surface_intersector::p1::{SrfIndex, TsectPnt};
use crate::kernel::Kernel;
use crate::math::{Vec3, EFF_ZERO};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::p5::DropPointOptions;
use crate::topology::edgeuse::p1 as eu;
use crate::topology::{edge, Brep};

const OP: &str = "blend::fillet_solver::ball_fit::ball_fits_its_faces";

/// A stretch this far below zero is still a fit (a ball as wide as the face's curve).
const FIT_TOL: f64 = 1.0e-6;

/// Sections judged along the filleted edge.
const NUM_SAMPLES: usize = 9;

/// `SmFilletSolver::BallFitsItsFaces`: `false` where, at one of nine sections along the
/// solver's edge, a face curves tighter than the rolling ball.
///
/// The ball touches a face where the face's offset surface (moved along the normal by
/// the ball's radius) passes through the ball's centre. Along a direction in which the
/// face curves toward the ball by `k` the offset is stretched by `1 - r k`; past the
/// face's centre of curvature that is negative, the ball cuts through the face and
/// the blend passes through itself. The stretches are the eigenvalues of `M = H G^-1`
/// (`G = [Bi.Bj]` of the base surface, `H = [Oi.Bj]` of the offset against it); both are
/// at least zero when `det M` and `trace M` are. A stretch of zero fits (a pin's
/// hemispherical end as wide as the round), by [`FIT_TOL`]. A section the solve does
/// not find, or a pole of a surface, judges nothing: the check refuses only what it can
/// solve for, so a refusal is never a false positive but a miss is possible. The nine
/// sections sit at `(j + 0.5) / 9` and never test the ends of the edge.
///
/// `brep` is the target holding the solver's edge use.
///
/// # Errors
///
/// Those of the drops, the section solves and the evaluations; `SM_ERR_NULL_POINTER`
/// for an edge without a curve.
pub fn ball_fits_its_faces(
    ops: &mut dyn FilletSolverOps,
    k: &Kernel,
    brep: &mut Brep,
) -> Result<bool> {
    let (Some(first), Some(second)) = (ops.solver().surface(0)?, ops.solver().surface(1)?) else {
        return Ok(true);
    };
    let domains = [
        first
            .surface
            .base_surface()
            .natural_uv_domain_with_nurb(k)?,
        second
            .surface
            .base_surface()
            .natural_uv_domain_with_nurb(k)?,
    ];
    let Some(edgeuse) = ops.solver().edgeuse(0)? else {
        return Ok(true);
    };
    let Some(edge_id) = eu::edge(brep, edgeuse)? else {
        return Err(SolidError::null_pointer(OP));
    };
    let interval = edge::interval(brep, edge_id)?;
    let curve = edge::curve(brep, edge_id)?
        .ok_or_else(|| SolidError::null_pointer(OP))?
        .clone();

    for j in 0..NUM_SAMPLES {
        let t = interval.evaluate((j as f64 + 0.5) / NUM_SAMPLES as f64);
        let pv = curve.evaluate(k, t, 1, true, false)?;
        let (Some(&point), Some(&tangent)) = (pv.first(), pv.get(1)) else {
            continue;
        };
        let moves = tangent.length() > EFF_ZERO;
        if !moves {
            continue;
        }
        let normal = tangent.unitize()?;

        // the edge point on each face's surface: the section's guess
        let mut guess: Vec<Point2> = Vec::with_capacity(2);
        let mut dropped = true;
        for (rail, domain) in domains.iter().enumerate() {
            if !dropped {
                break;
            }
            let Some(slot) = ops.solver().surface(rail)? else {
                return Ok(true);
            };
            let drop = slot.surface.base_surface().drop_point::<NoObject>(
                k,
                point,
                domain,
                &DropPointOptions::default(),
            )?;
            dropped = drop.success;
            guess.push(drop.result_uv);
        }
        if !dropped {
            continue;
        }

        let [g0, g1] = *guess.as_slice() else {
            continue;
        };

        // the section through it
        let mut section = TsectPnt::new();
        if !ops.point_on_plane_solve(
            k,
            Some(&mut *brep),
            point,
            normal,
            &domains[0],
            &domains[1],
            g0,
            g1,
            &mut section,
        )? {
            continue;
        }
        let radii_change = ops.offset_radii_can_change()?;
        if radii_change {
            ops.setup_offset_values(&section, None)?;
        }

        // the derivatives of the offset (O) and base (B) surfaces there
        let derivs = (|| -> Result<Vec<([Vec3; 2], [Vec3; 2])>> {
            let mut derivs = Vec::with_capacity(2);
            for (rail, srf) in [SrfIndex::First, SrfIndex::Second].into_iter().enumerate() {
                let uv = section.uv_pos(srf);
                let slot = ops
                    .solver_mut()
                    .surface_mut(rail)?
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                let (_, ou, ov) = offset_evaluate_1st(k, &mut slot.surface, uv)?;
                let (_, bu, bv) = slot
                    .surface
                    .base_surface()
                    .evaluate_1st_derivatives(k, uv, true, true)?;
                derivs.push(([ou, ov], [bu, bv]));
            }
            Ok(derivs)
        })();
        // restored whether or not the evaluations failed
        if radii_change {
            ops.solver_mut().reload_surface_offsets()?;
        }
        let derivs = derivs?;

        for ([o1, o2], [b1, b2]) in derivs {
            if !stretches_fit(o1, o2, b1, b2) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Whether both stretches of the offset `(o1, o2)` against its base `(b1, b2)` are at
/// least zero, by [`FIT_TOL`]: `M = H G^-1`, `G = [Bi.Bj]`, `H = [Oi.Bj]`.
fn stretches_fit(o1: Vec3, o2: Vec3, b1: Vec3, b2: Vec3) -> bool {
    let (g11, g12, g22) = (b1.dot(b1), b1.dot(b2), b2.dot(b2));
    let det_g = g11 * g22 - g12 * g12;
    // a pole (a parameter direction that does not move) has no stretch to judge
    let moves = det_g > EFF_ZERO * g11 * g22;
    if !moves {
        return true;
    }
    let (h11, h12, h21, h22) = (o1.dot(b1), o1.dot(b2), o2.dot(b1), o2.dot(b2));
    let det_m = (h11 * h22 - h12 * h21) / det_g;
    let trace_m = (h11 * g22 - h12 * g12 - h21 * g12 + h22 * g11) / det_g;
    !(det_m < -FIT_TOL || trace_m < -FIT_TOL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    /// A cylinder of radius 5 about z, at (5, 0, 0) with unit parameter speeds: its
    /// inward offset by `r` stretches the angular direction by `1 - r/5`.
    fn stretch_of_inward_offset(r: f64) -> bool {
        let b1 = v(0.0, 5.0, 0.0);
        let b2 = v(0.0, 0.0, 1.0);
        let o1 = v(0.0, 5.0 - r, 0.0);
        stretches_fit(o1, b2, b1, b2)
    }

    #[test]
    fn a_ball_over_the_faces_radius_does_not_fit() {
        assert!(stretch_of_inward_offset(2.0));
        assert!(stretch_of_inward_offset(5.0), "a zero stretch fits");
        assert!(!stretch_of_inward_offset(7.0));
    }

    /// An offset that stretches the two parameter directions of an orthonormal base by
    /// `s1` and `s2`.
    fn stretches(s1: f64, s2: f64) -> bool {
        let (b1, b2) = (v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
        stretches_fit(b1 * s1, b2 * s2, b1, b2)
    }

    #[test]
    fn a_doubly_curved_face_is_judged_by_both_det_and_trace() {
        // a sphere of radius 5 offset inward by 7 stretches both ways by -0.4: det is
        // positive, only the trace refuses it
        assert!(!stretches(-0.4, -0.4));
        assert!(stretches(0.6, 0.6));
        // a saddle-like offset (one direction past its centre): the trace is positive,
        // only the determinant refuses it
        assert!(!stretches(2.0, -0.5));
        assert!(stretches(2.0, 0.5));
    }
}
