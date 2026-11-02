//! Approximation of any surface by a B-spline surface within a tolerance
//! (`ApproximateSurface`): piecewise cubic Bezier patches from the surface's corner
//! data, split until every patch is close enough, then knots removed again.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`,
//! `source/SMLib/src/SmBSplineSurfaceNLib.cpp`, `source/SMLib/inc/SmBSplineSurface.h`,
//! `source/SMLib/inc/SmCubicBezierSurface.h`, `source/SMLib/inc/SmOffsetSurface.h`,
//! `source/SMLib/inc/SmCurveBoundedSurface.h`, `source/SMLib/inc/SmSTEPSurface.h`,
//! `source/SMLib/inc/SmSrfInVolume.h`.
//!
//! | Rust                                    | Upstream                                     |
//! |-----------------------------------------|----------------------------------------------|
//! | [`Surface::approximate_surface`], [`ApproximatedSurface`] | `SmSurface::ApproximateSurface` |
//! | [`Surface::approximate_surface_ignoring_status`] | the same, for a caller that ignores its status |
//! | [`Surface::compute_cubic_bezier_approx`] | `SmSurface::ComputeCubicBezierApprox`       |
//! | [`CubicBezierSurface`]                  | `SmCubicBezierSurface` (its points only)     |
//! | `root_surface` (private)                | `GetRootSurface` (virtual)                   |
//! | [`BSplineSurface::degree_elevate`]      | `SmBSplineSurface::DegreeElevate`            |
//! | [`BSplineSurface::insert_knots`]        | `SmBSplineSurface::InsertKnots` (NLib form)  |
//! | [`BSplineSurface::insert_one_knot`]     | `SmBSplineSurface::InsertOneKnot`            |
//! | [`BSplineSurface::remove_knots`]        | `SmBSplineSurface::RemoveKnots`              |
//!
//! None of these is in `harness/callgraph/units.json`: they are listed in
//! `harness/callgraph/unused_functions.txt` (no upstream test or `SM_API` entry point
//! reaches them; `SmBrepData`'s `bWriteAsBSplines` writer and the graphics output call
//! `ApproximateSurface`).
//!
//! Deviations:
//! - **Outputs.** `rpNewBSplineSurface` and `dAchievedTol` are the fields of
//!   [`ApproximatedSurface`]. The new surface is a [`Surface`]: with
//!   `bJustCopyBSplines` the copy of a plane, cone, sphere, torus or swept surface is
//!   that kind (upstream returns it through an `SmBSplineSurface*`); otherwise it is
//!   always a B-spline. [`Surface::approximate_surface`] returns the status as `Err`
//!   (the achieved tolerance is then lost); a caller that ignores the status, as every
//!   upstream caller does, uses [`Surface::approximate_surface_ignoring_status`].
//! - **Patch index check.** When a starter surface's knots are not all of full
//!   multiplicity, a patch's control points lie outside the net; upstream signals
//!   `SM_ERR` but returns `SM_SUCCESS` without surface, as the port does (`Ok` with
//!   `surface: None`).
//! - **Root surface.** `GetRootSurface` of an offset, curve bounded surface or surface
//!   in a volume follows its base; the chain is walked in a loop bounded by
//!   [`MAX_GEOMETRY_NESTING`] (`InvalidInput` beyond). The root of a plane, cone,
//!   sphere, torus or swept surface is its `SmBSplineSurface` part, its NURB made as
//!   `GetDegree` and the copy constructor make it (`GetOrCreateGwNurbPointer`).
//! - **Degree elevation below the degree.** [`BSplineSurface::degree_elevate`] returns
//!   `InvalidInput` for a new degree below the current one, where upstream passes the
//!   negative increment on to `N_SrfElevateDegree` (`ApproximateSurface` only elevates
//!   degrees below 3, so it never asks for that).
//! - **Subdivision rounds.** `2 * lSubdivisionLevel + 1` wraps around in upstream's
//!   `ULONG`; the port returns `InvalidInput` when it overflows `usize`.
//! - **Starter construction.** A failed `CreateCanonical` of the single-patch starter
//!   (a degenerate natural domain) is returned; upstream dereferences the `NULL`
//!   surface it leaves.
//! - **Ignored statuses.** As upstream: `Copy`, `GetKnots`, `GetControlPoint`,
//!   `SetControlPoint`, `DegreeElevate` and `RemoveKnots` (a failed elevation or
//!   removal leaves the surface unchanged); an unported dependency still stops the
//!   port.
//! - **Memory and contexts.** `SmContext`, `SmNLibStackHandler` and `NL_STACKS` are
//!   dropped; the debug dumps and drawing (`SM_DEBUG_CODE`) are omitted.

use std::borrow::Cow;

use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve::{ControlPointForm, KnotType};
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BSplineSurfaceForm, CanonicalSurfaceData, SurfParamType,
};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::nurbs::srf_basic::Direction;
use crate::nurbs::srf_tool::p1::{insert_knot, insert_knots};
use crate::nurbs::srf_tool::p2::elevate_degree;
use crate::nurbs::srf_tool::p3::remove_all_knots;
use crate::nurbs::srf_tool::p4::remove_knots_constrained;

use super::p2::{Surface, SurfaceKind, B_SPLINE_SURFACE_TYPE, MAX_GEOMETRY_NESTING};

const OP_APPROX: &str = "surface::surface::approximate::approximate_surface";
const OP_ROOT: &str = "surface::surface::approximate::root_surface";
const OP_ELEVATE: &str = "surface::surface::approximate::degree_elevate";
const OP_INSERT: &str = "surface::surface::approximate::insert_knots";
const OP_INSERT_ONE: &str = "surface::surface::approximate::insert_one_knot";
const OP_REMOVE: &str = "surface::surface::approximate::remove_knots";

/// A bicubic Bezier patch (`SmCubicBezierSurface`): its 4x4 control points,
/// `points[v][u]` (`m_vP[v][u]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezierSurface {
    /// `m_vP`: row `v`, column `u`.
    pub points: [[Point3; 4]; 4],
}

impl Default for CubicBezierSurface {
    fn default() -> Self {
        Self {
            points: [[Point3::ZERO; 4]; 4],
        }
    }
}

/// The result of [`Surface::approximate_surface`].
#[derive(Debug, Clone)]
pub struct ApproximatedSurface {
    /// `rpNewBSplineSurface`: the approximating surface with the fewest subdivisions
    /// within the tolerance; `None` when the finest allowed one is still out of it
    /// (and in the cases the module deviations list).
    pub surface: Option<Surface>,
    /// `dAchievedTol`: the largest sampled distance of the last test (`0` for a copied
    /// B-spline, `-1` when nothing was tested).
    pub achieved_tol: f64,
    /// The status upstream returns and its caller ignores
    /// ([`Surface::approximate_surface_ignoring_status`]); always `None` from
    /// [`Surface::approximate_surface`], which returns it as `Err`.
    pub failure: Option<SolidError>,
}

impl Surface {
    /// A B-spline surface within `approx_tol_3d` (an `SmApproxTol3d` value:
    /// [`crate::geom::tol::approx_tol3d`] converts a caller's `double`) of this one
    /// (`ApproximateSurface`),
    /// splitting the spans up to `subdivision_level` times in each direction. With
    /// `just_copy_b_splines` a surface that is an `SmBSplineSurface` (a B-spline,
    /// plane, cone, sphere, torus or swept surface) is copied instead.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when no subdivision level is within the tolerance, or the starter
    /// surface's degree is above 3; those of the evaluations, of the starter surface's
    /// construction and of knot insertion; `InvalidInput` for a root surface chain
    /// deeper than [`MAX_GEOMETRY_NESTING`].
    pub fn approximate_surface(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        subdivision_level: usize,
        just_copy_b_splines: bool,
    ) -> Result<ApproximatedSurface> {
        let mut achieved_tol = -1.0;
        let surface = self.approximate_surface_impl(
            k,
            approx_tol_3d,
            &mut achieved_tol,
            subdivision_level,
            just_copy_b_splines,
        )?;
        Ok(ApproximatedSurface {
            surface,
            achieved_tol,
            failure: None,
        })
    }

    /// [`Surface::approximate_surface`] for a caller that ignores its status, as
    /// `SmBrepData`'s B-spline writer does: the outputs as the call leaves them (no
    /// surface on failure; the achieved tolerance of the last test) and the failed
    /// status in [`ApproximatedSurface::failure`].
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] and [`SolidError::PreparationRequired`] only.
    pub fn approximate_surface_ignoring_status(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        subdivision_level: usize,
        just_copy_b_splines: bool,
    ) -> Result<ApproximatedSurface> {
        let mut achieved_tol = -1.0;
        let (surface, failure) = match self.approximate_surface_impl(
            k,
            approx_tol_3d,
            &mut achieved_tol,
            subdivision_level,
            just_copy_b_splines,
        ) {
            Ok(surface) => (surface, None),
            Err(e) if e.is_unported() || matches!(e, SolidError::PreparationRequired { .. }) => {
                return Err(e)
            }
            Err(e) => (None, Some(e)),
        };
        Ok(ApproximatedSurface {
            surface,
            achieved_tol,
            failure,
        })
    }

    /// The body of `ApproximateSurface`, writing `dAchievedTol` as it goes.
    fn approximate_surface_impl(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        achieved_tol: &mut f64,
        subdivision_level: usize,
        just_copy_b_splines: bool,
    ) -> Result<Option<Surface>> {
        const OP: &str = OP_APPROX;

        // init output
        *achieved_tol = -1.0;

        // low work - when asked, just copy BSplines
        if just_copy_b_splines && self.is_kind_of(B_SPLINE_SURFACE_TYPE)? {
            let surface = ignored(self.copy(k))?;
            *achieved_tol = 0.0;
            return Ok(surface);
        }

        // arrive here when working with any surface not derived from SmBSplineSurface

        // get RootSurface - not every surface type has one, and make the starter
        // output surface
        let mut approx_surf: Option<BSplineSurface> = None;
        if let Some(root) = self.root_surface(k)? {
            // let starter ApproxSurf equal a copy of the SmBSplineSurface part of the
            // root surface, unless it has degree > 3 in either direction: our
            // algorithm requires cubic.
            let u_deg = root.degree(SurfParamType::U);
            let v_deg = root.degree(SurfParamType::V);
            if u_deg <= 3 && v_deg <= 3 {
                approx_surf = Some(root.copy());
            }
        }

        // If not a BSplineSurface of degree 3 or less:
        let mut approx_surf = if let Some(s) = approx_surf {
            s
        } else {
            // let starter ApproxSurf equal a degree 3x3 single patch with the same
            // domain as the RootSurface
            let uv_domain = self.natural_uv_domain_with_nurb(k)?;
            let mut control_points = [Point3::ZERO; 16];
            for (ii, row) in control_points.chunks_exact_mut(4).enumerate() {
                for (jj, p) in row.iter_mut().enumerate() {
                    *p = Point3::new(ii as f64, jj as f64, 1.0);
                }
            }
            let mults = [4, 4];
            let u_starter_knots = [uv_domain.u_min(), uv_domain.u_max()];
            let v_starter_knots = [uv_domain.v_min(), uv_domain.v_max()];
            BSplineSurface::create_canonical(&CanonicalSurfaceData {
                u_degree: 3,
                v_degree: 3,
                control_points: &control_points,
                form: BSplineSurfaceForm::Unspecified,
                u_multiplicities: &mults,
                v_multiplicities: &mults,
                u_knots: &u_starter_knots,
                v_knots: &v_starter_knots,
                knot_type: KnotType::UniformKnots,
                weights: None,
                uv_domain: None,
            })?
        };

        // specify the control point form
        let form = if approx_surf.is_rational() {
            ControlPointForm::EuclideanRational
        } else {
            ControlPointForm::NonRational
        };

        // Nurb locals: upstream's arrays live across the iterations, so a failed
        // `GetKnots` (no NURB) leaves the previous round's knots
        let mut u_knots: Vec<f64> = Vec::new();
        let mut v_knots: Vec<f64> = Vec::new();
        let mut more_knots: Vec<f64> = Vec::new();

        let mut testing_u_dir = false; // We'll toggle this each big iteration.

        // Process u- and v-direction for each subD level, so iterate to 2x SubD level.
        let last_sub_d = subdivision_level
            .checked_mul(2)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| SolidError::invalid_input(OP, "subdivision level too large"))?;
        for _sub_d in 0..=last_sub_d {
            testing_u_dir = !testing_u_dir; // Toggle

            let mut u_deg = approx_surf.degree(SurfParamType::U);
            let mut v_deg = approx_surf.degree(SurfParamType::V);

            // no work - high degree surfaces
            if u_deg > 3 || v_deg > 3 {
                return Err(SolidError::sm_err(OP));
            }

            // elevate surface degrees to 3 (statuses ignored)
            if u_deg < 3 {
                ignored(approx_surf.degree_elevate(SurfParamType::U, 3))?;
            }
            if v_deg < 3 {
                ignored(approx_surf.degree_elevate(SurfParamType::V, 3))?;
            }

            u_deg = approx_surf.degree(SurfParamType::U);
            v_deg = approx_surf.degree(SurfParamType::V);

            // Update pApproxSurf using cubic Bezier approximations to each patch.

            // get knots of the subdivided surface (statuses ignored: without NURB the
            // arrays keep their values)
            if let Some((knots, _mult)) = ignored(approx_surf.knots(SurfParamType::U, None))? {
                u_knots = knots;
            }
            if let Some((knots, _mult)) = ignored(approx_surf.knots(SurfParamType::V, None))? {
                v_knots = knots;
            }

            // For each surface patch, compute the cubic Bezier approximation from the
            // actual surface and copy those control Points into new approx surface.
            for (iu, u_span) in u_knots.windows(2).enumerate() {
                for (iv, v_span) in v_knots.windows(2).enumerate() {
                    let (&[u0, u1], &[v0, v1]) = (u_span, v_span) else {
                        continue;
                    };
                    let dom = Extent2d::from_points(Point2::new(u0, v0), Point2::new(u1, v1));

                    // build cubic bezier approximation to this curve
                    let cb = self.compute_cubic_bezier_approx(k, &dom)?;
                    let u_min = iu * 3;
                    let v_min = iv * 3;
                    let num_ctrl_u = approx_surf.number_control_points(SurfParamType::U);
                    let num_ctrl_v = approx_surf.number_control_points(SurfParamType::V);

                    // copy span approximation control points into new approx surface
                    for iub in 0..=3 {
                        for ivb in 0..=3 {
                            if u_min + iub >= num_ctrl_u || v_min + ivb >= num_ctrl_v {
                                // SE(SM_ERR): some problem here with knots; upstream
                                // returns SM_SUCCESS without surface
                                return Ok(None);
                            }
                            // (the point read is overwritten; statuses ignored)
                            let _ =
                                ignored(approx_surf.control_point(form, u_min + iub, v_min + ivb))?;
                            let weight = 1.0;
                            let p = cb
                                .points
                                .get(ivb)
                                .and_then(|row| row.get(iub))
                                .copied()
                                .ok_or_else(|| SolidError::sm_err(OP))?;
                            ignored(approx_surf.set_control_point(
                                form,
                                u_min + iub,
                                v_min + ivb,
                                p,
                                weight,
                            ))?;
                        }
                    }
                } // end iter every V patch
            } // end iter every U patch

            // Test the accuracy of this control polygon approx surface.
            // If good enough then we can stop here.

            // We'll test points at 1/4, 2/4, 3/4:
            let num_between: usize = 3;
            let fraction = (num_between + 1) as f64;
            let mut failed = false;

            if let Some((knots, _)) = ignored(approx_surf.knots(SurfParamType::U, None))? {
                u_knots = knots;
            }
            if let Some((knots, _)) = ignored(approx_surf.knots(SurfParamType::V, None))? {
                v_knots = knots;
            }

            let mut ivl_inner = Extent1d::new();
            let mut ivl_outer = Extent1d::new();

            // init max value
            *achieved_tol = -1.0;

            more_knots.clear();

            let (outer_knots, inner_knots) = if testing_u_dir {
                (&u_knots, &v_knots)
            } else {
                (&v_knots, &u_knots)
            };

            // for every surface patch
            for outer_span in outer_knots.windows(2) {
                failed = false; // Starting over on a new row.
                let &[o0, o1] = outer_span else {
                    continue;
                };
                // (knots ascend: the status is ignored, as upstream)
                let _ = ivl_outer.set_min_max(o0, o1);

                // for every 'outer' dir sample point
                for jj in 1..=num_between {
                    let outer_param = ivl_outer.evaluate(jj as f64 / fraction);

                    // for every 'inner' dir patch
                    for inner_span in inner_knots.windows(2) {
                        let &[i0, i1] = inner_span else {
                            continue;
                        };
                        let _ = ivl_inner.set_min_max(i0, i1);

                        // for every 'inner' dir sample point
                        for kk in 1..=num_between {
                            let inner_param = ivl_inner.evaluate(kk as f64 / fraction);

                            let uv = if testing_u_dir {
                                Point2::new(outer_param, inner_param)
                            } else {
                                Point2::new(inner_param, outer_param)
                            };

                            // Eval points on both surfaces and check distance.
                            let pnt = self.evaluate_point(k, uv)?;
                            let approx_pnt = approx_surf.evaluate_point(uv)?;

                            let dist = pnt.distance(approx_pnt);

                            // save max achieved tol
                            if dist > *achieved_tol {
                                *achieved_tol = dist;
                            }

                            // check gap for failure
                            if dist > approx_tol_3d {
                                // mark the approx surface as no good and quit
                                failed = true;

                                // Add a U or V knot between existing knots, multiplicity
                                // lUDeg or lVDeg, into the 'outer' direction knots.
                                let deg = if testing_u_dir { u_deg } else { v_deg };
                                for _ in 0..deg {
                                    more_knots.push((o0 + o1) / 2.0);
                                }

                                break; // out of 'inner' dir samples into 'inner' dir patches
                            }
                        } // end iter every 'inner' dir sample point

                        if failed {
                            break; // out of 'inner' dir patches into 'outer' dir samples
                        }
                    } // end iter every 'inner' dir patch

                    if failed {
                        break; // out of 'outer' dir samples into 'outer' dir patches
                    }
                } // end iter every 'outer' patch sample point

                // ... continue on to the next 'outer' dir row/column of patches
            } // end iter every 'outer' dir patch

            // when the approx surface is good enough (upstream tests the flag of the
            // last row or column only)
            if !failed {
                // Remove excess knots: the result does not have to be piecewise Bezier.
                ignored(approx_surf.remove_knots(approx_tol_3d, true, true, None, None))?;

                // all done
                return Ok(Some(Surface::from(approx_surf)));
            }

            // arrive here when current subdivision level is not within tolerance -
            // subdivide again
            if more_knots.is_empty() {
                // Can't happen? Infinite loop if it did.
                return Err(SolidError::sm_err(OP));
            }
            let param = if testing_u_dir {
                SurfParamType::U
            } else {
                SurfParamType::V
            };
            approx_surf.insert_knots(param, &more_knots)?;
        } // end iter lSubD

        // arrive here when approximation failed to converge
        Err(SolidError::sm_err(OP))
    }

    /// The bicubic Bezier patch over `uv_domain` from this surface's positions, first
    /// derivatives and twists at the domain's corners, scaled to the domain
    /// (`ComputeCubicBezierApprox`).
    ///
    /// # Errors
    ///
    /// Those of the evaluations.
    pub fn compute_cubic_bezier_approx(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
    ) -> Result<CubicBezierSurface> {
        // exact tangent values
        let flags = EvaluateFlags {
            u_side: Side::Left,
            v_side: Side::Left,
            only_upper_half: false,
            non_zero_tangents: false,
        };
        let size = uv_domain.size();
        // `SmVector3d sMat[2][2]`: SM_UNDEF_DOUBLE at first (its members' default
        // initializers), then what each corner's evaluation writes; an entry an
        // evaluation does not write (the twist of an upper-half-only `EvaluateSTEP`)
        // keeps its value
        let mut s_mat = [Vec3::UNDEF; 4];
        let mut corner = |uv: Point2| -> Result<[[Vec3; 2]; 2]> {
            self.evaluate_into(k, uv, 1, 1, flags, &mut s_mat)?;
            let [d00, d01, d10, d11] = s_mat;
            Ok([[d00, d01], [d10, d11]])
        };
        let mut cb = CubicBezierSurface::default();
        let p = &mut cb.points;

        // Now load control polygon at corners using hermite information.
        {
            let mat = corner(uv_domain.min())?;
            let p00 = mat[0][0];
            let du = mat[1][0] * size.x;
            let dv = mat[0][1] * size.y;
            let duv = mat[1][1] * size.x * size.y;
            let p01 = p00 + du / 3.0;
            let p10 = p00 + dv / 3.0;
            let p11 = p10 + p01 - p00 + duv / 9.0;
            p[0][0] = p00;
            p[0][1] = p01;
            p[1][0] = p10;
            p[1][1] = p11;
        }

        {
            let mat = corner(uv_domain.max())?;
            let p00 = mat[0][0];
            let du = mat[1][0] * size.x;
            let dv = mat[0][1] * size.y;
            let duv = mat[1][1] * size.x * size.y;
            let p01 = p00 - du / 3.0;
            let p10 = p00 - dv / 3.0;
            let p11 = p10 + p01 - p00 + duv / 9.0;
            p[3][3] = p00;
            p[3][2] = p01;
            p[2][3] = p10;
            p[2][2] = p11;
        }

        {
            let mat = corner(uv_domain.evaluate(1.0, 0.0))?;
            let p00 = mat[0][0];
            let du = mat[1][0] * size.x;
            let dv = mat[0][1] * size.y;
            let duv = mat[1][1] * size.x * size.y;
            let p01 = p00 + dv / 3.0;
            let p10 = p00 - du / 3.0;
            let p11 = p10 + p01 - p00 - duv / 9.0;
            p[0][3] = p00;
            p[0][2] = p10;
            p[1][3] = p01;
            p[1][2] = p11;
        }

        {
            let mat = corner(uv_domain.evaluate(0.0, 1.0))?;
            let p00 = mat[0][0];
            let du = mat[1][0] * size.x;
            let dv = mat[0][1] * size.y;
            let duv = mat[1][1] * size.x * size.y;
            let p10 = p00 + du / 3.0;
            let p01 = p00 - dv / 3.0;
            let p11 = p10 + p01 - p00 - duv / 9.0;
            p[3][0] = p00;
            p[2][0] = p01;
            p[3][1] = p10;
            p[2][1] = p11;
        }

        Ok(cb)
    }

    /// `GetRootSurface`: the `SmBSplineSurface` part of this surface (a B-spline, plane,
    /// cone, sphere, torus or swept surface, its NURB made when missing), followed
    /// through offset, curve bounded and volume surfaces to their base; `None` for a
    /// STEP surface.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a chain deeper than [`MAX_GEOMETRY_NESTING`];
    /// [`SolidError::Unported`] from a NURB's make.
    fn root_surface(&self, k: &Kernel) -> Result<Option<Cow<'_, BSplineSurface>>> {
        let mut cur = self;
        for _ in 0..=MAX_GEOMETRY_NESTING {
            let next = match cur.kind() {
                SurfaceKind::OffsetSurface(s) => s.base_surface(),
                SurfaceKind::CurveBounded(s) => s.basis(),
                SurfaceKind::SrfInVolume(s) => s.surface(),
                SurfaceKind::Step(_) => return Ok(None),
                SurfaceKind::BSpline(s) => return Ok(Some(Cow::Borrowed(s))),
                SurfaceKind::Plane(s) => return Ok(Some(s.base_for_query())),
                SurfaceKind::SurfOfExtrusion(_) => {
                    // a failed make leaves the surface without NURB (status ignored)
                    return match cur.extrusion_base_with_nurb(k) {
                        Ok(base) => Ok(Some(base)),
                        Err(e) if e.is_unported() => Err(e),
                        Err(_) => match cur.kind() {
                            SurfaceKind::SurfOfExtrusion(s) => Ok(Some(Cow::Borrowed(s.base()))),
                            _ => Ok(None),
                        },
                    };
                }
                SurfaceKind::Cone(_)
                | SurfaceKind::Sphere(_)
                | SurfaceKind::Torus(_)
                | SurfaceKind::SurfOfRevolution(_) => {
                    let base = match cur.kind_with_nurb(k)? {
                        SurfaceKind::Cone(s) => s.rev().base(),
                        SurfaceKind::Sphere(s) => s.base().base(),
                        SurfaceKind::Torus(s) => s.base().base(),
                        SurfaceKind::SurfOfRevolution(s) => s.base(),
                        _ => return Err(SolidError::sm_err(OP_ROOT)),
                    };
                    return Ok(Some(Cow::Borrowed(base)));
                }
            };
            cur = next;
        }
        Err(SolidError::invalid_input(
            OP_ROOT,
            "root surface chain deeper than MAX_GEOMETRY_NESTING",
        ))
    }
}

impl BSplineSurface {
    /// Raises the degree in `param`'s direction to `new_degree` (`DegreeElevate`).
    ///
    /// # Errors
    ///
    /// `NULL` pointer without NURB; `InvalidInput` for a new degree below the
    /// current one; those of the NLib elevation (the surface is then unchanged).
    pub fn degree_elevate(&mut self, param: SurfParamType, new_degree: usize) -> Result<()> {
        const OP: &str = OP_ELEVATE;
        let dir = if param == SurfParamType::V {
            Direction::V
        } else {
            Direction::U
        };
        let increm = new_degree
            .checked_sub(self.degree(param))
            .ok_or_else(|| SolidError::invalid_input(OP, "new degree below the degree"))?;
        let sur_p = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let sur = elevate_degree(sur_p, increm, dir).map_err(SolidError::nl_ser(OP))?;

        // place output sur in a temporary and swap the NURBs
        self.nurb = Self::from_gw_nurb(&sur).nurb;
        Ok(())
    }

    /// Refines the knot vector in `param`'s direction with `new_knots`, an ascending
    /// list (`InsertKnots`, the `SmTArray<double>&` form).
    ///
    /// # Errors
    ///
    /// `NULL` pointer without NURB; those of the NLib insertion (the surface is then
    /// unchanged).
    pub fn insert_knots(&mut self, param: SurfParamType, new_knots: &[f64]) -> Result<()> {
        const OP: &str = OP_INSERT;
        let dir = if param == SurfParamType::U {
            Direction::U
        } else {
            Direction::V
        };
        let sur_p = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;

        // Make sur with the union of surP target knot vector and the new knots
        let sur = insert_knots(sur_p, new_knots, dir).map_err(SolidError::nl_ser(OP))?;

        // build a surface from it and swap the NURBs
        self.nurb = Self::from_gw_nurb(&sur).nurb;
        Ok(())
    }

    /// Inserts the interior knot `new_knot` `count` times in `param`'s direction (v for
    /// [`SurfParamType::V`], else u), its old and new multiplicities summing to at most
    /// the degree (`InsertOneKnot`).
    ///
    /// # Errors
    ///
    /// `NULL` pointer without NURB; those of the NLib insertion (the surface is then
    /// unchanged).
    pub fn insert_one_knot(
        &mut self,
        new_knot: f64,
        count: usize,
        param: SurfParamType,
    ) -> Result<()> {
        const OP: &str = OP_INSERT_ONE;
        let dir = if param == SurfParamType::V {
            Direction::V
        } else {
            Direction::U
        };
        let sur_p = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let sur = insert_knot(sur_p, new_knot, count, dir).map_err(SolidError::nl_ser(OP))?;

        // build a surface from it and swap the NURBs
        self.nurb = Self::from_gw_nurb(&sur).nurb;
        Ok(())
    }

    /// Removes the knots whose removal keeps the surface within `approx_tol_3d`, in u
    /// and/or v (both when neither is asked for), except the knots in `kept_u_knots` /
    /// `kept_v_knots` (`RemoveKnots`).
    ///
    /// # Errors
    ///
    /// `NULL` pointer without NURB; those of the NLib removal (the surface is then
    /// unchanged).
    pub fn remove_knots(
        &mut self,
        approx_tol_3d: f64,
        remove_u_knots: bool,
        remove_v_knots: bool,
        kept_u_knots: Option<&[f64]>,
        kept_v_knots: Option<&[f64]>,
    ) -> Result<()> {
        const OP: &str = OP_REMOVE;
        let dir = if remove_u_knots && remove_v_knots {
            Direction::UV
        } else if remove_u_knots {
            Direction::U
        } else if remove_v_knots {
            Direction::V
        } else {
            Direction::UV
        };
        let sur_p = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let sur = if kept_u_knots.is_some() || kept_v_knots.is_some() {
            remove_knots_constrained(sur_p, kept_u_knots, kept_v_knots, approx_tol_3d, dir)
        } else {
            remove_all_knots(sur_p, approx_tol_3d, dir)
        }
        .map_err(SolidError::nl_ser(OP))?;

        self.nurb = Self::from_gw_nurb(&sur).nurb;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::cone::Cone;
    use crate::surface::offset_surface::OffsetSurface;

    fn kernel() -> Kernel {
        Kernel::default()
    }

    fn bilinear() -> BSplineSurface {
        let pts = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.2),
            Point3::new(1.0, 0.0, 0.1),
            Point3::new(1.0, 1.0, 0.0),
        ];
        BSplineSurface::create_canonical(&CanonicalSurfaceData {
            u_degree: 1,
            v_degree: 1,
            control_points: &pts,
            form: BSplineSurfaceForm::Unspecified,
            u_multiplicities: &[2, 2],
            v_multiplicities: &[2, 2],
            u_knots: &[0.0, 1.0],
            v_knots: &[0.0, 1.0],
            knot_type: KnotType::Unspecified,
            weights: None,
            uv_domain: None,
        })
        .unwrap()
    }

    #[test]
    fn copies_b_splines_when_asked() {
        let k = kernel();
        let s = Surface::from(bilinear());
        let r = s.approximate_surface(&k, 1e-3, 3, true).unwrap();
        assert_eq!(r.achieved_tol, 0.0);
        assert!(r.surface.is_some());
    }

    #[test]
    fn approximates_a_b_spline_exactly() {
        // a bilinear patch elevated to bicubic is reproduced by its Hermite data
        let k = kernel();
        let s = Surface::from(bilinear());
        let r = s.approximate_surface(&k, 1e-6, 3, false).unwrap();
        assert!(
            r.achieved_tol >= 0.0 && r.achieved_tol < 1e-12,
            "{}",
            r.achieved_tol
        );
        assert!(r.surface.is_some());
    }

    #[test]
    fn approximates_an_offset_quarter_cylinder() {
        // issue #1441: the offset of a quarter cylinder of radius 5
        let k = kernel();
        let cyl = Surface::from(Cone::new_cylinder_bounded(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            5.0,
            0.0,
            90.0,
            10.0,
            false,
            false,
            false,
        ));
        let off = Surface::from(OffsetSurface::new(&k, 2.0, cyl, true, 0).unwrap());
        let tol = 1e-3;
        let r = off.approximate_surface(&k, tol, 3, false).unwrap();
        assert!(r.failure.is_none());
        assert!(
            r.achieved_tol >= 0.0 && r.achieved_tol <= tol,
            "{}",
            r.achieved_tol
        );
        let s = r.surface.unwrap();
        let uv = off.natural_uv_domain_with_nurb(&k).unwrap().mid();
        let p = s.evaluate_point(&k, uv).unwrap();
        let q = off.evaluate_point(&k, uv).unwrap();
        assert!(p.distance(q) <= 2.0 * tol, "{}", p.distance(q));
    }

    #[test]
    fn degree_elevate_and_insert_keep_the_shape() {
        let mut s = bilinear();
        let before = s.evaluate_point(Point2::new(0.3, 0.7)).unwrap();
        s.degree_elevate(SurfParamType::U, 3).unwrap();
        s.degree_elevate(SurfParamType::V, 3).unwrap();
        assert_eq!(s.degree(SurfParamType::U), 3);
        s.insert_knots(SurfParamType::U, &[0.5, 0.5, 0.5]).unwrap();
        assert_eq!(s.number_control_points(SurfParamType::U), 7);
        let after = s.evaluate_point(Point2::new(0.3, 0.7)).unwrap();
        assert!(before.distance(after) < 1e-12);
        s.remove_knots(1e-9, true, true, None, None).unwrap();
        assert_eq!(s.number_control_points(SurfParamType::U), 4);
    }
}
