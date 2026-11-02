//! The NLib-only members of the B-spline volume `SmBSplineVolume`: the parameter-plane
//! continuities of its knot vectors, whether it has internal discontinuities, and
//! whether a parameter-space curve stays inside its natural parameter domain.
//!
//! Upstream: `source/SMLib/src/SmBSplineVolumeNLib.cpp`, `source/SMLib/inc/SmBSplineVolume.h`.
//!
//! | Rust                                                  | Upstream                                         |
//! |-------------------------------------------------------|--------------------------------------------------|
//! | [`BSplineVolume::calculate_continuities_simple`]      | `SmBSplineVolume::CalculateContinuitiesSimple`   |
//! | [`BSplineVolume::has_discontinuities_simple`]         | `SmBSplineVolume::HasDiscontinuitiesSimple`      |
//! | [`BSplineVolume::is_curve_in_param_domain_simple`]    | `SmBSplineVolume::IsCurveInParamDomainSimple`    |
//!
//! Deviations:
//! - **Out-params.** `CalculateContinuitiesSimple` fills its outputs before it can
//!   fail, and `HasDiscontinuitiesSimple` ignores its status and reads them anyway, so
//!   the outputs stay an out-param ([`DirectionDiscontinuities`]: `min_cont`, `params`,
//!   `conts`; `unaligned` is not touched) that holds upstream's values on failure too.
//!   `HasDiscontinuitiesSimple` writes the `U`, `V` and `W` members of
//!   [`Discontinuities3d`] only (as upstream it sets neither `has_discontinuities` nor
//!   `min_continuity` beyond `Init`).
//! - **Representational continuities** (`bCalcGeometric == FALSE`): upstream sizes the
//!   continuity array to the knot count but fills only the first `count - 1` entries;
//!   the zero-filled last entry is [`ContinuityType::Undefined`] (`SmTArray::SetSize`
//!   clears new elements), and so it is here.
//! - **Geometric continuities** (`bCalcGeometric == TRUE`) build iso-parametric
//!   curves with `EvaluateIsoParametricCurveSimple`, which returns `SM_ERR` in
//!   upstream's build (no `USE_VOLUMES`, see [`BSplineVolume`]), so this path always
//!   fails at the first sample after setting `params` (to the knots), `conts` (empty)
//!   and `min_cont` ([`ContinuityType::CInfinity`]). The rest of the loop is ported
//!   for when that changes; where upstream indexes `sKnots2` (and the continuity
//!   array) past its size (it samples `sKnots2[ii]`, not `sKnots2[jj]`), the port
//!   returns [`SolidError::InvalidInput`] instead of reading stale memory.
//! - **Ignored statuses.** `IsCurveInParamDomainSimple` ignores the statuses of
//!   `CalculateBoundingBox` and `CalculateTightBoundingBox` (a failure leaves the
//!   default boxes); [`SolidError::Unported`] still propagates.
//! - **Contexts.** `IsCurveInParamDomainSimple` takes the [`Kernel`] its curve's
//!   bounding box needs (upstream reads the curve's context).
//! - **Curve callees.** `SmCurve::CalculateTightBoundingBox` (status ignored) and
//!   `SmCurve::IsOnPositiveSideOfPlanes` are `curve.curve_p9`'s
//!   ([`Curve::calculate_tight_bounding_box`], [`Curve::is_on_positive_side_of_planes`]).

use crate::curve::curve::p2::{Curve, CurveKind};
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tol::ANG_TOL_DEG;
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::b_spline_curve::BoundingBoxRequest;
use crate::surface::b_spline_volume::{BSplineVolume, VolumeParam, VolumeParams};
use crate::surface::volume::p1::{DirectionDiscontinuities, Discontinuities3d};

const OP_CONTS: &str =
    "surface::b_spline_volume_n_lib::BSplineVolume::calculate_continuities_simple";

/// `SM_CONTINUITY_ANGLE` (degrees): the default angle tolerance of
/// `SmCurve::CalculateContinuities`.
const CONTINUITY_ANGLE: f64 = ANG_TOL_DEG;

/// `a < b` in upstream's `SmContinuityType` order.
const fn lower(a: ContinuityType, b: ContinuityType) -> bool {
    (a as i32) < (b as i32)
}

/// The representational continuity of a knot of multiplicity `mult` in degree
/// `degree`.
fn knot_continuity(degree: usize, mult: usize) -> ContinuityType {
    // save representational continuity at knot
    let cont = degree.saturating_sub(mult);
    match cont {
        0 => ContinuityType::C0,
        1 => ContinuityType::C1,
        2 => ContinuityType::C1C2,
        3 => ContinuityType::C1C2C3,
        _ => ContinuityType::CInfinity,
    }
}

/// `sKnots[i]`, or `InvalidInput` where upstream reads past the array.
fn knot(k: &[f64], i: usize) -> Result<f64> {
    k.get(i).copied().ok_or(SolidError::InvalidInput {
        operation: OP_CONTS,
        reason: "sample knot index past the knot list (upstream reads stale memory)",
    })
}

/// Pads a single-span sample list with its midpoint and reduces a long one to five
/// samples (`[k0, .655 k0 + .345 kn, .5 k0 + .5 kn, .155 k0 + .845 kn, kn]`).
fn sample_knots(k: &mut Vec<f64>) {
    // add midKnot to singleSpan KnotVectors to avoid degenerate computations
    if let [a, b] = k[..] {
        *k = vec![a, 0.5 * (a + b), b];
    }
    // reduce large knot counts to 5 sample points (don't need more)
    if k.len() > 5 {
        if let (Some(&k0), Some(&kn)) = (k.first(), k.last()) {
            k.truncate(5);
            if let [_, k1, k2, k3, k4] = &mut k[..] {
                *k4 = kn;
                *k3 = 0.155 * k0 + 0.845 * *k4;
                *k2 = 0.5 * k0 + 0.5 * *k4;
                *k1 = 0.655 * k0 + 0.345 * *k4;
            }
        }
    }
}

impl BSplineVolume {
    /// The continuity of the volume across each knot of direction `param` (`U`, `V`,
    /// else `W`) into `out` (`CalculateContinuitiesSimple`): `params` the distinct
    /// knots, `conts` the continuity at each, `min_cont` the lowest. With
    /// `calc_geometric == false` the continuities are representational (degree minus
    /// multiplicity); with `true` (upstream's default) they are sampled from
    /// iso-parametric curves, which fails in upstream's build (module docs).
    ///
    /// `out` is written before any failure, as upstream; `out.unaligned` is left
    /// alone.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a knot vector with fewer than two distinct knots, and (geometric)
    /// from `EvaluateIsoParametricCurveSimple`; [`SolidError::InvalidInput`] where
    /// upstream reads past its sample arrays.
    pub fn calculate_continuities_simple(
        &self,
        k: &Kernel,
        param: VolumeParam,
        out: &mut DirectionDiscontinuities,
        calc_geometric: bool,
    ) -> Result<()> {
        // init output
        out.min_cont = ContinuityType::CInfinity;
        out.params.clear();
        out.conts.clear();

        // Get unique knot values for eVolumeParam direction
        let (params, mults) = self.knots(param, None)?;
        out.params = params;

        // knot count
        let knot_cnt = out.params.len();

        // check state - nonDegenerate knot vectors must have 2 or more knots
        if knot_cnt <= 1 {
            return Err(SolidError::sm_err(OP_CONTS));
        }

        let degree = self.degree(param);

        // low work - no geometric tests
        if !calc_geometric {
            // size rConts array (new elements are zero: SM_CT_UNDEFINED)
            out.conts = vec![ContinuityType::Undefined; knot_cnt];
            // compute conts at each internal knot
            for (c, &mult) in out.conts.iter_mut().zip(&mults).take(knot_cnt - 1) {
                *c = knot_continuity(degree, mult);
                // save min continuity
                if lower(*c, out.min_cont) {
                    out.min_cont = *c;
                }
            }
            return Ok(());
        }

        // Get the knots going in the other (u/v/w) directions.
        let (constant_params, mut knots1, mut knots2) = match param {
            VolumeParam::U => (
                VolumeParams::Vw,
                self.knots(VolumeParam::V, None)?.0,
                self.knots(VolumeParam::W, None)?.0,
            ),
            VolumeParam::V => (
                VolumeParams::Uw,
                self.knots(VolumeParam::U, None)?.0,
                self.knots(VolumeParam::W, None)?.0,
            ),
            _ => (
                VolumeParams::Uv,
                self.knots(VolumeParam::U, None)?.0,
                self.knots(VolumeParam::V, None)?.0,
            ),
        };
        sample_knots(&mut knots1);
        sample_knots(&mut knots2);

        // mark working with the first IsoCurve to classify
        let mut first = true;

        // for every knot value to sample
        for ii in 0..knots1.len() {
            let mut param1 = knot(&knots1, ii)?;
            for jj in 0..knots2.len() {
                // upstream samples sKnots2[ii], not sKnots2[jj]
                let mut param2 = knot(&knots2, ii)?;

                // make temp isoParameter curve for this knot sample value
                let mut iso = self.evaluate_iso_parametric_curve_simple(
                    constant_params,
                    param1,
                    param2,
                    0.0,
                    None,
                )?;
                // (the knots are fetched and never used; status ignored)
                let _ = ignored(iso.knots(k, None))?;

                // When curve is degenerate - try another knot sample value
                if iso.is_degenerate(k, EFF_ZERO, None)? {
                    // past the first knot and nothing classified yet: try the middle
                    // of the previous span
                    if ii > 0 && jj > 0 && first {
                        param1 = (knot(&knots1, ii - 1)? + knot(&knots1, ii)?) / 2.0;
                        param2 = (knot(&knots2, ii - 1)? + knot(&knots2, ii)?) / 2.0;
                        iso = self.evaluate_iso_parametric_curve_simple(
                            constant_params,
                            param1,
                            param2,
                            0.0,
                            None,
                        )?;
                    }
                    if iso.is_degenerate(k, EFF_ZERO, None)? {
                        continue;
                    }
                }

                // calculate the iso-curve continuities
                let (tmp_min, tmp_conts) = iso.calculate_continuities(k, CONTINUITY_ANGLE)?;

                if first {
                    first = false;
                    // store computed values
                    out.conts = tmp_conts;
                    out.min_cont = tmp_min;
                } else {
                    // save minimum continuity values
                    if lower(tmp_min, out.min_cont) {
                        out.min_cont = tmp_min;
                    }
                    // for every knot - save min continuity seen
                    for (kk, &t) in tmp_conts.iter().enumerate() {
                        let c = out.conts.get_mut(kk).ok_or(SolidError::InvalidInput {
                            operation: OP_CONTS,
                            reason: "iso-curve knot past the continuity list",
                        })?;
                        if lower(t, *c) {
                            *c = t;
                        }
                    }
                }
            }
        }

        // when none of the curves classify - default to
        // [Discontinuous, G1, ... G1, Discontinuous] (upstream reports an error)
        if first {
            out.conts = vec![ContinuityType::G1; knot_cnt];
            if let Some(c) = out.conts.first_mut() {
                *c = ContinuityType::Discontinuous;
            }
            if let Some(c) = out.conts.last_mut() {
                *c = ContinuityType::Discontinuous;
            }
        }
        Ok(())
    }

    /// Whether some knot plane of the volume is less than G1
    /// (`HasDiscontinuitiesSimple`): [`Self::calculate_continuities_simple`] in `U`,
    /// `V` and `W` into `disconts` (after `Init`; statuses ignored).
    ///
    /// # Errors
    ///
    /// Only [`SolidError::Unported`] (from a callee of the geometric path).
    pub fn has_discontinuities_simple(
        &self,
        k: &Kernel,
        disconts: Option<&mut Discontinuities3d>,
        calc_geometric: bool,
    ) -> Result<bool> {
        // locals - and optional indirection
        let mut local = Discontinuities3d::new();
        let d = disconts.unwrap_or(&mut local);

        // init output
        d.init();

        // Calc the continuities in U, V and W (statuses ignored)
        ignored(self.calculate_continuities_simple(k, VolumeParam::U, &mut d.u, calc_geometric))?;
        ignored(self.calculate_continuities_simple(k, VolumeParam::V, &mut d.v, calc_geometric))?;
        ignored(self.calculate_continuities_simple(k, VolumeParam::W, &mut d.w, calc_geometric))?;

        // all done
        Ok(lower(d.u.min_cont, ContinuityType::G1)
            || lower(d.v.min_cont, ContinuityType::G1)
            || lower(d.w.min_cont, ContinuityType::G1))
    }

    /// Whether the parameter-space `curve` stays inside the natural parameter domain
    /// (`IsCurveInParamDomainSimple`): decided by the curve's bounding and pseudo
    /// boxes when they are inside or disjoint from the domain (within `SM_EFF_ZERO`),
    /// then by its tight box, else by the curve against the domain's six planes.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the curve's boxes and plane classification (their
    /// other failures are ignored as upstream).
    pub fn is_curve_in_param_domain_simple(&self, k: &Kernel, curve: &Curve) -> Result<bool> {
        // locals
        let param_domain = self.natural_param_domain();
        // note: PseudoBox and BBox are equivalent because BBox is orthogonal and axis aligned
        let param_pseudo_box = PseudoBox::from_extent3d(&param_domain);
        let curve_interval = curve.natural_interval();

        // low work - check BBoxes for gross inside/outside classifications
        let req = BoundingBoxRequest {
            normal: true,
            pseudo: true,
            polar: false,
        };
        let boxes = ignored(curve.calculate_bounding_box(k, &curve_interval, req, true))?;
        let (normal, pseudo) = boxes.map_or((None, None), |b| (b.normal, b.pseudo));
        let mut curve_bbox = normal.unwrap_or_else(Extent3d::new);
        let curve_pseudo_box = pseudo.unwrap_or_else(PseudoBox::new);

        if curve_bbox.is_contained_by(&param_domain, EFF_ZERO) {
            return Ok(true);
        }
        if curve_bbox.are_disjoint(&param_domain, EFF_ZERO) {
            return Ok(false);
        }
        if curve_pseudo_box.is_contained_by(&param_pseudo_box, EFF_ZERO) {
            return Ok(true);
        }
        if curve_pseudo_box.are_disjoint(&param_pseudo_box, None) {
            return Ok(false);
        }

        // now check the tight bounding box - a little more expensive (status ignored)
        match curve.kind() {
            CurveKind::Line(l) => {
                if let Some(b) = l.calculate_tight_bounding_box(&curve_interval) {
                    curve_bbox = b;
                }
            }
            _ => {
                if let Some(b) = ignored(curve.calculate_tight_bounding_box(k, &curve_interval))? {
                    curve_bbox = b;
                }
            }
        }
        if curve_bbox.is_contained_by(&param_domain, EFF_ZERO) {
            return Ok(true);
        }
        if curve_bbox.are_disjoint(&param_domain, EFF_ZERO) {
            return Ok(false);
        }

        // arrive here when more expensive check is needed: the curve against the
        // ParamBBox bounding planes
        let (plane_points, plane_normals) = param_domain.planes();
        curve.is_on_positive_side_of_planes(k, &plane_points, &plane_normals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::geom::vector3d::Vec3;
    use crate::nurbs::math::CPoint;
    use crate::nurbs::spiral::allocate_nurb_volume;

    /// A volume with the given degrees and knot vectors (control points on a grid).
    fn volume(deg: [usize; 3], knots: [&[f64]; 3]) -> BSplineVolume {
        let hi = [0, 1, 2].map(|d| knots[d].len() - deg[d] - 2);
        let kn = [0, 1, 2].map(|d| knots[d].len() - 1);
        let mut n = allocate_nurb_volume(hi, deg, kn).unwrap();
        for (d, k) in knots.iter().enumerate() {
            n.knots_mut(d).copy_from_slice(k);
        }
        for (i, c) in n.cpts_mut().iter_mut().enumerate() {
            let f = i as f64;
            *c = CPoint::new(f, 2.0 * f, -f, 1.0);
        }
        BSplineVolume::new(n)
    }

    fn unit_cube() -> BSplineVolume {
        let k: &[f64] = &[0.0, 0.0, 1.0, 1.0];
        volume([1, 1, 1], [k, k, k])
    }

    #[test]
    fn representational_continuities() {
        // cubic in u with a double interior knot at 0.5 and a single one at 0.25
        let ku: &[f64] = &[0.0, 0.0, 0.0, 0.0, 0.25, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0];
        let k: &[f64] = &[0.0, 0.0, 1.0, 1.0];
        let v = volume([3, 1, 1], [ku, k, k]);
        let mut d = DirectionDiscontinuities {
            unaligned: true,
            min_cont: ContinuityType::G1,
            params: vec![9.0],
            conts: vec![],
        };
        v.calculate_continuities_simple(&Kernel::default(), VolumeParam::U, &mut d, false)
            .unwrap();
        assert!(d.unaligned);
        assert_eq!(d.params, vec![0.0, 0.25, 0.5, 1.0]);
        assert_eq!(
            d.conts,
            vec![
                ContinuityType::C0,
                ContinuityType::C1C2,
                ContinuityType::C1,
                ContinuityType::Undefined,
            ]
        );
        assert_eq!(d.min_cont, ContinuityType::C0);
    }

    #[test]
    fn geometric_path_fails_after_setting_params() {
        let k = &Kernel::default();
        let v = unit_cube();
        let mut d = DirectionDiscontinuities {
            unaligned: false,
            min_cont: ContinuityType::C0,
            params: vec![],
            conts: vec![ContinuityType::C0],
        };
        assert!(v
            .calculate_continuities_simple(k, VolumeParam::V, &mut d, true)
            .is_err());
        assert_eq!(d.params, vec![0.0, 1.0]);
        assert!(d.conts.is_empty());
        assert_eq!(d.min_cont, ContinuityType::CInfinity);
    }

    #[test]
    fn single_knot_fails() {
        let k: &[f64] = &[0.0, 0.0, 1.0, 1.0];
        let z: &[f64] = &[2.0, 2.0, 2.0, 2.0];
        let v = volume([1, 1, 1], [k, k, z]);
        let mut d = DirectionDiscontinuities {
            unaligned: false,
            min_cont: ContinuityType::C0,
            params: vec![],
            conts: vec![],
        };
        let r = v.calculate_continuities_simple(&Kernel::default(), VolumeParam::W, &mut d, false);
        assert!(r.is_err());
        assert_eq!(d.params.len(), 1);
        assert_eq!(d.min_cont, ContinuityType::CInfinity);
    }

    #[test]
    fn has_discontinuities() {
        let k = &Kernel::default();
        let v = unit_cube();
        let mut d = Discontinuities3d::new();
        // representational: the end knots are C0
        assert!(v
            .has_discontinuities_simple(k, Some(&mut d), false)
            .unwrap());
        assert_eq!(d.u.conts.len(), 2);
        // geometric: every direction fails, nothing below G1 is reported
        assert!(!v.has_discontinuities_simple(k, Some(&mut d), true).unwrap());
        assert_eq!(d.w.params, vec![0.0, 1.0]);
        assert!(!v.has_discontinuities_simple(k, None, true).unwrap());
    }

    #[test]
    fn sample_knot_lists() {
        let mut k = vec![0.0, 2.0];
        sample_knots(&mut k);
        assert_eq!(k, vec![0.0, 1.0, 2.0]);
        let mut k = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 10.0];
        sample_knots(&mut k);
        assert_eq!(k.len(), 5);
        assert_eq!(k[4], 10.0);
        assert_eq!(k[2], 5.0);
    }

    #[test]
    fn curve_in_domain() {
        let k = Kernel::default();
        let v = unit_cube();
        let seg = |a: [f64; 3], b: [f64; 3]| {
            Curve::from(Line::from_points(
                Vec3::from_array(a),
                Vec3::from_array(b),
                3,
            ))
        };
        assert!(v
            .is_curve_in_param_domain_simple(&k, &seg([0.1, 0.2, 0.3], [0.9, 0.8, 0.7]))
            .unwrap());
        assert!(!v
            .is_curve_in_param_domain_simple(&k, &seg([2.0, 2.0, 2.0], [3.0, 3.0, 3.0]))
            .unwrap());
        // crossing the boundary: the plane classification says outside
        assert!(!v
            .is_curve_in_param_domain_simple(&k, &seg([0.5, 0.5, 0.5], [1.5, 0.5, 0.5]))
            .unwrap());
    }
}
