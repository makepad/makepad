//! The face and face-use checks.
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `SmFaceuse.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `face` | `SmFace::AssertValid` |
//! | `faceuse` | `SmFaceuse::AssertValid` |
//! | `assert_test_face34` | `sm_AssertTestFace34` |
//! | `assert_test_face39` | `sm_AssertTestFace39` |
//!
//! Deviations: `sm_AssertTestFace39` finds a vertex shared by two loops through one
//! vertex-to-loop map instead of comparing every loop pair (same result). A face's
//! surface owner is resolved to a face through the run's identities. A face without
//! surface returns [`crate::SolidError::InvalidInput`] at test 27 (upstream
//! dereferences null). `sm_AssertTestFace34` (level 2 only) compares only the edge
//! pairs a box search over the edges' curve boxes leaves possible, in the all-pairs
//! order (#431, fork `ef9041d`): the pairs it skips are those whose boxes
//! `GlobalCoincidenceChecker` itself rejects, so the result is upstream's.

use std::collections::BTreeMap;

use super::{base, class, geometry_key, seams, Cx};
use crate::curve::curve::CurveKind;
use crate::error::{EntityRef, Result};
use crate::geom::extent2d::SS_NONE;
use crate::geom::solution_array::SolutionType;
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_DEG, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::BoundingBoxRequest;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::query::face::p1::{point_3d_classify, Point3dClassifyOptions};
use crate::query::face::p2::{
    face_vertices, point_classify_indexed, FaceVertexIndex, PointClassifyOptions,
};
use crate::topology::brep::p1::HealLoopHandle;
use crate::topology::brep::validation::spatial::SpatialIndex;
use crate::topology::face::p1::SeamProblem;
use crate::topology::face::p2 as fp2;
use crate::topology::face::p4::{has_seam_problem_in_brep_mut, HasSeamProblemOptions};
use crate::topology::{vertex as tv, vertexuse as tvu, Brep, FaceId, FaceuseId};

const C: i64 = class::FACE;

/// `SM_ZONE_TOL_3D`: the tolerance of a vertex use (`SmTopology::GetTolerance`, not
/// overridden by `SmVertexuse`), its `SmTol::GetZoneTol3d`.
const VERTEXUSE_ZONE_TOL_3D: f64 = crate::geom::tol::ZONE_TOL_3D;

/// The face whose identity is `owner` (`SmSurface::GetFace()`): this face when the
/// identities agree, else the first face of the B-rep list with that identity.
fn surface_face(b: &Brep, cx: &Cx<'_>, f: FaceId) -> Result<Option<FaceId>> {
    let Some(surface) = b.face(f)?.surface.as_ref() else {
        return Ok(None);
    };
    let Some(owner) = surface.owner() else {
        return Ok(None);
    };
    if owner.type_code != class::FACE {
        return Ok(None);
    }
    if owner == (cx.keys)(EntityRef::Face(f)) {
        return Ok(Some(f));
    }
    Ok(b.face_list()
        .iter()
        .copied()
        .find(|&x| (cx.keys)(EntityRef::Face(x)) == owner))
}

/// `SmFace::AssertValid(SM_NO_WALK)`.
#[allow(clippy::too_many_lines)]
pub(super) fn face(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>, f: FaceId) -> Result<bool> {
    let this = EntityRef::Face(f);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::topology(b, cx, this)?;
    }

    // locals
    let face_zone_tol = fp2::tolerance(b, f)?;
    let domain = fp2::uv_domain(b, f)?;
    let need = cx.rep.need(0);
    // SM_ASSERT_TEST_NEED(n, SM_LEVEL_0) of the gated blocks: false at SM_LEVEL_GIVEN
    // (no test requests)
    let gate = cx.rep.test_need(0);

    // Face and the objects it attaches to need to share common contexts
    rtn &= cx.rep.this_bool(b, C, this, 0, true)?;
    let has_surface = b.face(f)?.surface.is_some();
    if let Some(surface) = b.face(f)?.surface.as_ref() {
        let natural = surface.natural_uv_domain_with_nurb(k)?;
        rtn &= cx.rep.this_bool(b, C, this, 1, true)?;
        let ok = !need || domain.is_contained_by(&natural, EFF_ZERO);
        rtn &= cx
            .rep
            .this_value(b, C, this, 2, ok, EFF_ZERO, || UNDEF_DOUBLE)?;
        let ok = !need || (domain.x_length() > EFF_ZERO && domain.y_length() > EFF_ZERO);
        rtn &= cx
            .rep
            .this_value(b, C, this, 22, ok, EFF_ZERO, || UNDEF_DOUBLE)?;
    }

    // check 1: Face->Surface pointer is not NULL
    rtn &= cx.rep.this_bool(b, C, this, 3, !need || has_surface)?;

    // check 1: Face<->Surface back pointers
    if let (true, true, Some(surface_face)) = (has_surface, gate, surface_face(b, cx, f)?) {
        let surface_face_zone_tol = fp2::tolerance(b, surface_face)?;

        // Face->Surface->Face must be this Face
        rtn &= cx.rep.this_bool(b, C, this, 4, surface_face == f)?;

        // The face UVDomain must be contained within the Surface domain
        let surface = super::super::required(b.face(f)?.surface.as_ref())?;
        let natural = surface.natural_uv_domain_with_nurb(k)?;
        let ok = domain.is_contained_by(&natural, EFF_ZERO);
        rtn &= cx
            .rep
            .this_value(b, C, this, 23, ok, 0.0, || UNDEF_DOUBLE)?;

        // Get Face->Surface->Poles (if any)
        let singular = surface.get_singularities(k, true, false)?;

        // When the surface has singularities (poles) - they must be marked by vertices
        // or out of the model
        if singular.singular != SS_NONE {
            // for every possible pole:[UMinPole, VMinPole, UMaxPole, VMaxPole]
            for pole in singular
                .pole_points
                .unwrap_or([crate::math::Point3::UNDEF; 4])
            {
                // when point is a pole
                if pole.is_undef() {
                    continue;
                }
                // classify the point against the Face->Brep
                let mut pc = PointClassification::new(surface_face_zone_tol);
                crate::error::ignored(point_3d_classify(
                    k,
                    b,
                    f,
                    pole,
                    surface_face_zone_tol,
                    true,
                    &mut pc,
                    Point3dClassifyOptions::default(),
                ))?;
                let class = pc.point_class();
                let ok = class != PointClassType::Face && class != PointClassType::Edge;
                rtn &= cx.rep.this_bool(b, C, this, 24, ok)?;
            }
        }
    } else if has_surface && gate {
        // The surface used by this face must have an owner
        rtn &= cx.rep.this_bool(b, C, this, 35, false)?;
    }

    // vertices classify onto this face (tests 12-13)
    if gate {
        let vertices = face_vertices(b, f, false, None)?;
        // (#431) each classification below would list the face's vertices again
        let vertex_index = FaceVertexIndex::new(b, f);
        for v in vertices {
            let mut connections = 0;
            for vu in tv::vertexuses(b, v)? {
                // look for those vertexuses which connect this vertex to the face
                let Some(fu) = tvu::faceuse(b, vu)? else {
                    continue;
                };
                if b.faceuse(fu)?.face != Some(f) {
                    continue;
                }
                // count the number of times the vertex connects to this face
                connections += 1;
                let mut pc = PointClassification::new(face_zone_tol);
                // side effect: builds the UV trim curve
                match tvu::compute_uv_point(k, b, vu, true) {
                    Ok(uv) => {
                        crate::error::ignored(point_classify_indexed(
                            k,
                            b,
                            f,
                            uv,
                            &mut pc,
                            PointClassifyOptions {
                                src_zone_tol_3d: VERTEXUSE_ZONE_TOL_3D,
                                do_boundary_intersections: true,
                                use_uv_classification: true,
                                ..PointClassifyOptions::default()
                            },
                            vertex_index.as_ref(),
                        ))?;
                    }
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {
                        let point = b.vertex(v)?.point;
                        crate::error::ignored(point_3d_classify(
                            k,
                            b,
                            f,
                            point,
                            VERTEXUSE_ZONE_TOL_3D,
                            true,
                            &mut pc,
                            Point3dClassifyOptions::default(),
                        ))?;
                    }
                }
                // Every face->Vertex must classify back to the face
                let ok = pc.point_class() != PointClassType::Unknown;
                rtn &= cx.rep.this_bool(b, C, this, 12, ok)?;
                // need to check only one VU of each Vertex that connects to this face
                break;
            }
            // Every Face->Vertex must connect back to the face at least once
            rtn &= cx.rep.this_bool(b, C, this, 13, connections >= 1)?;
        }
    }

    // Check all connected Faceuses
    let (fu1, fu2) = fp2::faceuses(b, f)?;
    if let Some(fu) = fu1 {
        rtn &= faceuse(b, cx, fu)?;
    }
    if let Some(fu) = fu2 {
        rtn &= faceuse(b, cx, fu)?;
    }

    let face_tol = fp2::tolerance(b, f)?;

    // 27: Zero Area Face degenerates to a point location
    let ok = !need || {
        let surface = super::super::required(b.face(f)?.surface.as_ref())?;
        !surface.is_degenerate_point(k, face_tol / 4.0, Some(&domain))?
    };
    rtn &= cx
        .rep
        .this_value(b, C, this, 27, ok, face_tol / 4.0, || UNDEF_DOUBLE)?;

    // 28: Zero Area Face degenerates to a curve shape
    let ok = !need || {
        let surface = super::super::required(b.face(f)?.surface.as_ref())?;
        !surface.is_degenerate_curve(k, EFF_ZERO_DEG, Some(&domain), face_tol / 4.0)?
    };
    rtn &= cx
        .rep
        .this_value(b, C, this, 28, ok, face_tol / 4.0, || UNDEF_DOUBLE)?;

    // 32: Face has Coincident Vertices
    let ok = !cx.rep.need(2) || seams::assert_test_face32(k, b, f)?;
    rtn &= cx.rep.this_bool(b, C, this, 32, ok)?;
    // 33: Face has Coincident Edges
    let ok = !cx.rep.need(2) || seams::assert_test_face33(k, b, f)?;
    rtn &= cx.rep.this_bool(b, C, this, 33, ok)?;
    // 34: Face has Coincident Edges
    let ok = !cx.rep.need(2) || assert_test_face34(k, b, f)?;
    rtn &= cx.rep.this_bool(b, C, this, 34, ok)?;
    // 37: Face InnerLoops must nest inside the OuterLoop and outside each other
    let ok = !need || seams::assert_test_face37(k, b, f)?;
    rtn &= cx.rep.this_bool(b, C, this, 37, ok)?;
    // 38: Face Loops must be disjoint in the Face UVSpace; a Face with no Surface has
    // no UVSpace to check - AssertTest 3 reports it
    let ok = !cx.rep.need(2) || !has_surface || seams::assert_test_face38(k, b, f)?;
    rtn &= cx.rep.this_bool(b, C, this, 38, ok)?;
    // 39: no Vertex may be shared by two Face Loops
    let ok = !need || assert_test_face39(b, f)?;
    rtn &= cx.rep.this_bool(b, C, this, 39, ok)?;

    // Test = FALSE when pFace->Edges intersects pFace->PeriodicSurface->Seam (run at
    // every level; status ignored)
    let problem = crate::error::ignored(has_seam_problem_in_brep_mut::<HealLoopHandle>(
        k,
        b,
        f,
        HasSeamProblemOptions::default(),
    ))?
    .map_or(
        SeamProblem {
            crossed: SurfParamType::Neither,
            missing: SurfParamType::Neither,
            near_miss: SurfParamType::Neither,
        },
        |c| c.problem,
    );
    // 29: Face OuterLoop boundaries cannot cross a periodic surface seam
    let ok = !cx.rep.need(2) || problem.crossed == SurfParamType::Neither;
    rtn &= cx.rep.this_bool(b, C, this, 29, ok)?;
    // 31: FaceSeams must be represented by Edges
    let ok = !cx.rep.need(2) || problem.missing == SurfParamType::Neither;
    rtn &= cx.rep.this_bool(b, C, this, 31, ok)?;
    // 36: FaceSeams must not have near miss
    let ok = !cx.rep.need(2) || problem.near_miss == SurfParamType::Neither;
    rtn &= cx.rep.this_bool(b, C, this, 36, ok)?;

    // when the face has a surface: Check Face->Surface
    if has_surface && cx.bases() {
        let level = cx.rep.level;
        rtn &= seams::surface_assert_valid(
            k,
            b,
            f,
            cx.rep.array.as_deref_mut(),
            level,
            geometry_key(20002, f.index()),
        )?;
    }

    // all done
    Ok(rtn)
}

/// `SmFaceuse::AssertValid(SM_NO_WALK)`: the base classes and the (intrinsic) face and
/// mate contexts.
pub(super) fn faceuse(b: &Brep, cx: &mut Cx<'_>, fu: FaceuseId) -> Result<bool> {
    const FC: i64 = class::FACEUSE;
    let this = EntityRef::Faceuse(fu);
    let mut rtn = true;
    if cx.bases() {
        rtn &= base::owning(b, cx, this)?;
    }
    if b.faceuse(fu)?.face.is_some() {
        rtn &= cx.rep.this_bool(b, FC, this, 0, true)?;
    }
    if b.faceuse(fu)?.mate.is_some() {
        rtn &= cx.rep.this_bool(b, FC, this, 1, true)?;
    }
    Ok(rtn)
}

/// `sm_AssertTestFace34`: `false` when two edges of the face are coincident over a
/// range (`GlobalCoincidenceChecker` within the larger edge tolerance).
fn assert_test_face34(k: &Kernel, b: &Brep, f: FaceId) -> Result<bool> {
    let edges = crate::topology::face::p4::edges(b, f, None, false, None)?;

    // (#431) GlobalCoincidenceChecker finds nothing for two curves whose boxes over
    // their intervals are farther apart than the tolerance, after degeneracy tests
    // that cannot fail for these curve kinds: only the pairs a box search finds within
    // the largest edge tolerance are compared (a pair with another curve kind, a box
    // that cannot be computed or a tolerance that is not finite still is)
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    let mut boxes = Vec::new();
    let mut max_tol = 0.0;
    let mut finite_tols = true;
    for &e in &edges {
        let edge = b.edge(e)?;
        finite_tols &= edge.tolerance.is_finite();
        if edge.tolerance > max_tol {
            max_tol = edge.tolerance;
        }
        let bbox = edge.curve.as_ref().and_then(|c| match c.kind() {
            CurveKind::BSpline(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Parabola(_)
            | CurveKind::Line(_) => c
                .calculate_bounding_box(k, &edge.interval, req, true)
                .ok()
                .and_then(|found| found.normal),
            _ => None,
        });
        boxes.push(bbox);
    }
    let search = SpatialIndex::new(boxes.iter().copied());

    // Compare each edge with all others
    for (ii, &e1) in edges.iter().enumerate() {
        let c1 = super::super::required(b.edge(e1)?.curve.as_ref())?;
        let i1 = b.edge(e1)?.interval;
        let t1 = b.edge(e1)?.tolerance;
        let candidates = match boxes.get(ii) {
            Some(Some(bbox)) if finite_tols => search.boxes_in_box(bbox, max_tol),
            _ => (0..edges.len()).collect(),
        };
        for e2 in candidates
            .into_iter()
            .filter(|&jj| jj > ii)
            .filter_map(|jj| edges.get(jj).copied())
        {
            let c2 = super::super::required(b.edge(e2)?.curve.as_ref())?;
            let i2 = b.edge(e2)?.interval;
            let t2 = b.edge(e2)?.tolerance;
            let t = if t1 > t2 { t1 } else { t2 };
            // status ignored
            let Some(found) = crate::error::ignored(
                c1.global_coincidence_checker::<crate::topology::ObjectHandle>(
                    k,
                    &i1,
                    c2,
                    &i2,
                    t,
                    true,
                    [None, None],
                ),
            )?
            else {
                continue;
            };
            // Check solutions for range intersections
            if found
                .solutions
                .iter()
                .any(|s| s.solution_type == SolutionType::RangeOfValues)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// `sm_AssertTestFace39`: `false` when a vertex is used by two loops of the face.
fn assert_test_face39(b: &Brep, f: FaceId) -> Result<bool> {
    let loops = fp2::loops(b, f, false, None)?;

    // no work - a Face with fewer than 2 Loops has no Loop pair to share a Vertex
    if loops.len() < 2 {
        return Ok(true);
    }

    // any Vertex two Loops both use connects Loops that must be disjoint
    let mut owner: BTreeMap<crate::topology::VertexId, usize> = BTreeMap::new();
    for (i, &l) in loops.iter().enumerate() {
        for v in crate::topology::loopuse::loop_vertices(b, l)?
            .into_iter()
            .flatten()
        {
            match owner.get(&v) {
                Some(&j) if j != i => return Ok(false),
                Some(_) => {}
                None => {
                    owner.insert(v, i);
                }
            }
        }
    }

    // arrive here when no Vertex is shared by two Face Loops
    Ok(true)
}
