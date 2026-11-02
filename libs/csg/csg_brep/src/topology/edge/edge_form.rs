//! The edge "on face edge form" check: the edge curve within the cross-section
//! tolerance of every face it bounds (`SmEdge::AssertValid` test 13).
//!
//! Upstream: `source/SMLib/src/SmEdge.cpp`, `source/SMLib/inc/SmEdge.h`.
//!
//! | Rust                       | Upstream                    |
//! |----------------------------|-----------------------------|
//! | [`is_on_face_edge_form`]   | `SmEdge::IsOnFaceEdgeForm`  |
//! | [`assert_test_edge13`]     | `sm_AssertTestEdge13`       |
//!
//! Deviations: the optional `double*` max-gap output is the second tuple member,
//! always returned. `GetMaxEdgeFaceGap` fills the edge uses' gap cache, so the B-rep
//! is `&mut`. `SmTol::GetXSectTol3d(this, pFace)` is the sum of the two
//! `SmTopology::GetTolerance` zone tolerances (`SM_USE_OLDTOL`), the system zone
//! tolerance for a null face. `dMaxGap3d < *pMaxEdgeFaceGap` resolves to
//! `operator<(double, const SmGap&)` (`fabs(d) < gap`); `*pMaxEdgeFaceGap >
//! sXSectTol3d` to `operator>(double, const SmTol3d&)` on the gap's `m_dGap3d`. No
//! recursion; the face loop is bounded by the edge's edge uses.

use crate::error::Result;
use crate::geom::tol::{self, SmallTopoType};
use crate::kernel::Kernel;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p2 as face2;
use crate::topology::{Brep, EdgeId};

/// `SmEdge::IsOnFaceEdgeForm(&dMaxGap3d)`: `(ok, max_gap_3d)`. `ok` is false when the
/// cached max edge/face gap of an edge use on any of the edge's faces exceeds
/// `SmTol::GetXSectTol3d(edge, face)`; `max_gap_3d` is the largest such gap (`0.0`
/// when there is none).
///
/// # Errors
///
/// [`crate::SolidError::NotFound`] for a dangling id; those of
/// [`eu1::max_edge_face_gap`] (the gap calculation's own status is ignored, as
/// upstream).
pub fn is_on_face_edge_form(k: &Kernel, b: &mut Brep, e: EdgeId) -> Result<(bool, f64)> {
    // locals
    let mut ok = true;
    let mut max_gap_3d = 0.0_f64;
    let faces = super::faces(b, e, None)?;

    // for every face - check for Max Edge/Face gaps
    for f in faces {
        // When Edge is connected to Face through an Edgeuse
        let Some(eu) = super::edgeuse_of_face(b, e, Some(f))? else {
            continue;
        };
        let face = eu1::face(b, eu)?;

        // gap free Edge/Face XSectTol3d
        let edge_zone = tol::zone_tol3d(k, super::tolerance(b, e)?);
        let face_zone = match face {
            Some(face) => tol::zone_tol3d(k, face2::tolerance(b, face)?),
            None => tol::zone_tol_3d(k, SmallTopoType::NormalSize),
        };
        let xsect_tol_3d = tol::xsect_tol_3d_from_zones(k, edge_zone, face_zone);

        // Find the Max Edge/Face gap
        if let Some(gap) = eu1::max_edge_face_gap(k, b, eu, false)? {
            // Save the largest MaxEdgeFaceGap
            if max_gap_3d.abs() < gap.gap_3d {
                max_gap_3d = gap.gap_3d;
            }
            // update the output
            if gap.gap_3d > xsect_tol_3d {
                ok = false;
            }
        }
    }
    Ok((ok, max_gap_3d))
}

/// `sm_AssertTestEdge13(pEdge, &dMaxGap3d)`: `(ok, max_gap_3d)` of
/// [`is_on_face_edge_form`] (`SmEdge::AssertValid` test 13, level 2).
///
/// # Errors
///
/// Those of [`is_on_face_edge_form`].
pub fn assert_test_edge13(k: &Kernel, b: &mut Brep, e: EdgeId) -> Result<(bool, f64)> {
    // bRtn == FALSE when Vertex Point is more than ScaledZero from best corner pos defined by connected faces
    is_on_face_edge_form(k, b, e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::math::Vec3;
    use crate::primitive::api_primitives::create_box;
    use crate::topology::vertex;

    fn box_brep(k: &mut Kernel) -> Brep {
        create_box(k, Vec3::new(0.0, 0.0, 0.0), 10.0, 10.0, 10.0).unwrap()
    }

    // A box edge lies exactly on both of its planes: test 13 passes with zero gap.
    #[test]
    fn box_edges_are_on_their_face_edge_form() {
        let mut k = Kernel::default();
        let mut b = box_brep(&mut k);
        let edges: Vec<EdgeId> = b.edges().ids().collect();
        assert_eq!(edges.len(), 12);
        for e in edges {
            assert_eq!(assert_test_edge13(&k, &mut b, e).unwrap(), (true, 0.0));
        }
    }

    // Move one edge curve 0.01 off one of its planes (the box's zone tolerance is
    // far smaller): the gap exceeds XSectTol3d and test 13 fails, reporting the gap.
    #[test]
    fn offset_edge_curve_fails_test_13() {
        let mut k = Kernel::default();
        let mut b = box_brep(&mut k);
        let e = b.edges().ids().next().unwrap();
        let (v1, v2) = super::super::start_end_vertices(&b, e).unwrap();
        let p1 = vertex::point(&b, v1.unwrap()).unwrap();
        let p2 = vertex::point(&b, v2.unwrap()).unwrap();
        let dir = p2 - p1;
        // move 0.01 toward the box centre along the first axis across the edge: off
        // one face's plane, still inside the other face
        let toward = |c: f64| if c < 5.0 { 0.01 } else { -0.01 };
        let shift = if dir.x == 0.0 {
            Vec3::new(toward(p1.x), 0.0, 0.0)
        } else {
            Vec3::new(0.0, toward(p1.y), 0.0)
        };
        let mut place = Axis2Placement::default();
        place.translate(shift);
        let curve = b.edge_mut(e).unwrap().curve.as_mut().unwrap();
        curve.transform(&k, &place, None).unwrap();
        let tol = super::super::tolerance(&b, e).unwrap();
        assert!(0.01 > 2.0 * tol);
        let (ok, gap) = assert_test_edge13(&k, &mut b, e).unwrap();
        assert!(!ok);
        assert!((gap - 0.01).abs() <= 1e-15, "gap {gap}");
    }
}
