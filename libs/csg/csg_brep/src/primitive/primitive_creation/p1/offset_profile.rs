//! The offset of a planar profile: the region between a set of closed planar curves and
//! their offsets, or the offset region itself.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`offset_profile`] | static `SmPrimitiveCreation::OffsetProfile` |
//!
//! Deviations: the curves are borrowed (upstream consumes them on success and reports
//! each transfer in `rInputOwnershipTransferred`, which has no Rust counterpart). The
//! result is the return value. The offset map attributes `CreateOffsetsOfManyCurves`
//! attaches to the offset curves are not carried onto the new edges (nothing in this
//! function reads them). The bounding box upstream computes of the profile B-rep and
//! never reads is not computed. Faces are built over an arena-only notification store.
//! No recursion.

use super::primitives::{new_brep, planar_faces};
use super::{Boolean2d, PrimitiveCreation};
use crate::curve::composite_curve::{CompositeCurve, OffsetCornerType, OffsetParams};
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::nurbs::knots::Side;
use crate::topology::Brep;

const OP: &str = "primitive::primitive_creation::p1::offset_profile";

/// `OffsetProfile`: the faces bounded by the closed loops of the coplanar `curves`
/// (`CreatePlanarFacesWith3DCurves` with `approx_tol_3d`), their loops offset by
/// `offset_distance` (`CreateOffsetsOfManyCurves`, `offset_type` 1 left, 2 right, 3 both
/// for every loop, corners rounded with `round_corners` or else extended, intersecting
/// and trimming the concave raw offsets, `approx_tol_3d` gap and `approx_tol_3d / 4`
/// approximation tolerances, in the plane of the last face's normal), and the faces those
/// offsets bound. With `shell_result` and a left offset the result is the profile minus
/// the offset faces, with a right offset the offset faces minus the profile (planar
/// [`PrimitiveCreation::boolean_2d`] differences); otherwise the offset faces alone.
///
/// # Errors
///
/// Those of the face construction, the composites, the offsets and the 2D difference;
/// `SM_ERR_NULL_POINTER` when the difference leaves no result (an operand without
/// faces).
pub fn offset_profile(
    k: &mut Kernel,
    curves: &[Curve],
    approx_tol_3d: f64,
    offset_distance: f64,
    offset_type: u64,
    round_corners: bool,
    shell_result: bool,
) -> Result<Brep> {
    // Make Brep to hold new faces - will become the output
    let mut brep = new_brep(k)?;
    let brep_editing = brep.editing_enabled();
    brep.set_editing_enabled(true);
    let doing_boolean = k.context.doing_boolean();
    k.context.set_doing_boolean(true);
    let result = offset_profile_in(
        k,
        &mut brep,
        curves,
        approx_tol_3d,
        offset_distance,
        offset_type,
        round_corners,
        shell_result,
    );
    k.context.set_doing_boolean(doing_boolean);
    // the editing flag of the B-rep returned is restored, as its
    // SmTemporaryChangeValue guard does on return
    result.map(|mut result| {
        result.set_editing_enabled(brep_editing);
        result
    })
}

#[allow(clippy::too_many_arguments)] // upstream's arguments
fn offset_profile_in(
    k: &mut Kernel,
    brep: &mut Brep,
    curves: &[Curve],
    approx_tol_3d: f64,
    offset_distance: f64,
    offset_type: u64,
    round_corners: bool,
    shell_result: bool,
) -> Result<Brep> {
    // Make Planar face in pBrep - cleanup curves, make Plane, check planarity
    let region = brep.infinite_region();
    let new_faces = planar_faces(k, brep, region, curves, approx_tol_3d)?;

    // for every face - build CompositeCurves for every loop
    let mut orients: Vec<u64> = Vec::new();
    let mut all_composites: Vec<CompositeCurve> = Vec::new();
    let mut plane_z = Vec3::UNDEF; // SmVector3d sPlaneZ (default constructor)
    for &face in &new_faces {
        let domain = crate::topology::face::p2::uv_domain(brep, face)?;
        plane_z = brep
            .face(face)?
            .surface
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .evaluate_normal(k, domain.evaluate(0.5, 0.5), Side::Left, Side::Left)?;
        // build a CompositeCurve for every loop
        let composites = crate::topology::face::p3::create_composites(k, brep, face)?;
        for _ in &composites {
            orients.push(offset_type);
        }
        all_composites.extend(composites);
    }

    // offset the set of CompositeCurves
    let corner = if round_corners {
        OffsetCornerType::FilletCorner
    } else {
        OffsetCornerType::LinearExtension
    };
    let params = OffsetParams {
        xsect_tol_3d: approx_tol_3d,
        approx_tol_3d: approx_tol_3d / 4.0,
        plane_normal: plane_z,
        corner,
        trim_results: true,
        offset_distance,
        match_parameterization: false,
    };
    let composites: Vec<&CompositeCurve> = all_composites.iter().collect();
    let offsets = CompositeCurve::create_offsets_of_many_curves(k, &composites, &orients, &params)?;
    let offset_curves: Vec<Curve> = offsets.offsets.into_iter().map(|p| p.curve).collect();

    // NewBrep to hold Planar faces made from the planar CompositeCurves
    let mut new_brep = new_brep(k)?;
    let new_brep_editing = new_brep.editing_enabled();
    new_brep.set_editing_enabled(true);

    // create a face in the infiniteRegion for every closed loop of curves (its
    // tolerance argument, SmTol::GetXSectTol3d(pNewBrep), is not used)
    let region = new_brep.infinite_region();
    planar_faces(k, &mut new_brep, region, &offset_curves, 0.0)?;

    if shell_result && offset_type == 1 {
        // when asked Shell result - Inset of faces branch: Result = pBrep - pNewBrep
        let operand = std::mem::take(brep);
        difference(k, operand, new_brep)
    } else if shell_result && offset_type == 2 {
        // when asked Shell result - Offset of faces branch: Result = pNewBrep - pBrep
        let operand = std::mem::take(brep);
        difference(k, new_brep, operand)
    } else {
        // no shelling branch
        new_brep.set_editing_enabled(new_brep_editing);
        Ok(new_brep)
    }
}

/// `Boolean2D(a, b, SM_2D_DIFFERENCE, pResult)` then `NER(pResult)`.
fn difference(k: &mut Kernel, mut a: Brep, mut b: Brep) -> Result<Brep> {
    use crate::topology::topology_traverser::Marks;
    let mut primitive = PrimitiveCreation::new(None, 0, false);
    let mut a_marks = Marks::default();
    let mut b_marks = Marks::default();
    let mut result = None;
    let mut state = crate::topology::brep::p2::ArenaNotifyState::new(&a);
    state.seed(&b);
    primitive.boolean_2d(
        k,
        &mut a,
        &mut b,
        Boolean2d::Difference,
        &mut result,
        (&mut a_marks, &mut b_marks),
        &mut state.env(None),
    )?;
    result.ok_or_else(|| SolidError::null_pointer(OP))
}
