//! External topology services of primitive construction.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp` (callee boundaries).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveAccess`] | B-rep, face, classification and stitching services |
//!
//! Deviations: extends the existing statically dispatched sweep access interface.
//! Defaults defer only unavailable callees; partial lists remain caller-owned.

use crate::curve::composite_curve::CompositeCurve;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::object::ObjectRef;
use crate::heal::stitch::p2::MinGapRequest;
use crate::kernel::Kernel;
use crate::surface::surface::Surface;
use crate::sweep::topology_sweep::access::{FaceRef, RegionRef, SweepAccess, SweepStitchCallback};
use crate::sweep::topology_sweep::TopologySweep;
use crate::topology::brep::p3::DeleteFaceOptions;
use crate::topology::brep::p8::FaceBuildEnv;
use crate::topology::topology_traverser::Marks;
use crate::topology::{Brep, BrepId, EdgeId, FaceId};

/// Topology adapters used by primitive construction. Contexts, record ids and
/// resolver identities are shared with `SweepAccess`; construction tails never
/// replace upstream's topological edits with arena-only approximations.
pub trait PrimitiveAccess: SweepAccess {
    /// Resolve the caller's Kernel and mutable B-rep together, without copying
    /// either context or topology state.
    fn with_primitive_brep<T>(
        &mut self,
        brep: BrepId,
        body: impl FnOnce(&mut Kernel, &mut Brep) -> Result<T>,
    ) -> Result<T>
    where
        Self: Sized;

    /// Create planar faces, retaining partial outputs and consumed curve slots
    /// (`SmBrep::CreatePlanarFacesWith3DCurves`; the curves are consumed on success).
    fn planar_faces(
        &mut self,
        region: RegionRef,
        curves: &mut Vec<Option<Curve>>,
        tolerance: f64,
        faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        let input: Vec<Curve> = curves.iter().flatten().cloned().collect();
        *faces = with_face_env(self, region.0, |kernel, model, fenv| {
            crate::topology::brep::p8::create_planar_faces_with_3d_curves(
                kernel,
                model,
                Some(region.1),
                &input,
                tolerance,
                fenv,
            )
        })?;
        // consumed on success: the B-rep owns every curve, each slot stays empty
        for slot in curves.iter_mut() {
            *slot = None;
        }
        Ok(())
    }
    /// Create one planar face from its boundary curves
    /// (`SmBrep::CreatePlanarFaceWith3DCurves`).
    fn planar_face(
        &mut self,
        region: RegionRef,
        curves: Vec<Curve>,
        tolerance: f64,
        face: &mut Option<FaceId>,
    ) -> Result<()> {
        *face = with_face_env(self, region.0, |kernel, model, fenv| {
            crate::topology::brep::p8::create_planar_face_with_3d_curves(
                kernel,
                model,
                Some(region.1),
                &curves,
                tolerance,
                fenv,
            )
        })?;
        Ok(())
    }
    /// Create a wire, consuming its curve on success.
    fn wire(&mut self, brep: BrepId, curve: &mut Option<Curve>) -> Result<EdgeId> {
        let input = curve
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer("primitive::primitive_creation::p3::wire"))?
            .clone();
        let interval = input.natural_interval();
        let edge = self.with_brep_mut(brep, |kernel, model, _, _| {
            crate::topology::brep::p5::create_wire_edge_from_curve(kernel, model, input, interval)
        })?;
        *curve = None;
        Ok(edge)
    }
    /// Split a surface at G1 discontinuities and return its faces in `faces`
    /// (`SmBrep::CreateFacesFromSurface` with `SM_CT_G1`, as at every call site).
    /// `faces` is reset first, as upstream's `rNewFaces.ReSet()`, so a caller reusing
    /// one list across calls gets only this surface's faces.
    fn surface_faces(
        &mut self,
        brep: BrepId,
        surface: Surface,
        domain: Extent2d,
        faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        faces.clear();
        let made = with_face_env(self, brep, |kernel, model, fenv| {
            crate::topology::brep::p8::create_faces_from_surface(
                kernel,
                model,
                Some(surface),
                &domain,
                ContinuityType::G1,
                fenv,
            )
        })?;
        faces.extend(made);
        Ok(())
    }
    /// Create a face, retaining the output slot on failure
    /// (`SmBrep::CreateFaceFromSurface`).
    fn surface_face(
        &mut self,
        brep: BrepId,
        surface: Surface,
        domain: Extent2d,
        face: &mut Option<FaceId>,
    ) -> Result<()> {
        *face = Some(with_face_env(self, brep, |kernel, model, fenv| {
            crate::topology::brep::p8::create_face_from_surface(
                kernel,
                model,
                Some(surface),
                &domain,
                fenv,
            )
        })?);
        Ok(())
    }
    /// Delete a face without its connected edges/vertices; do region nesting
    /// (`SmBrep::DeleteFace(face, FALSE, TRUE)`).
    fn delete_primitive_face(&mut self, face: FaceRef) -> Result<()> {
        self.with_brep_mut(face.0, |kernel, model, _, notifications| {
            let mut marks = Marks::default();
            crate::topology::brep::p3::delete_face(
                kernel,
                model,
                notifications,
                &mut marks,
                face.1,
                DeleteFaceOptions::default(),
            )
            .map(drop)
        })
    }
    /// Topology-intersector deletion, retaining the supplied face order.
    /// Implementations invoke the ported `TopologyIntersector::delete_faces`
    /// using a context-only scoped intersector and caller-resolved object handles,
    /// matching the constructor inside the upstream rotational sweep.
    fn delete_primitive_faces(&mut self, brep: BrepId, faces: &[FaceId]) -> Result<()>;
    /// Stitch and orient the primitive's resulting B-rep.
    fn stitch_and_orient(&mut self, brep: BrepId) -> Result<()> {
        self.with_brep_mut(brep, |kernel, model, _, _| {
            crate::topology::brep::p5::stitch_and_orient(
                kernel,
                model,
                &crate::topology::brep::p5::StitchAndOrientOptions::default(),
            )
        })
    }
    /// Transform the complete B-rep, including pre-existing topology.
    fn transform_primitive(&mut self, brep: BrepId, placement: &Axis2Placement) -> Result<()> {
        self.with_brep_mut(brep, |kernel, model, _, notifications| {
            crate::topology::brep::p5::transform_with_notifications(
                kernel,
                model,
                placement,
                None,
                true,
                Some(notifications),
            )
        })
    }
    /// Create composites for all loops of an input face.
    fn face_composites(&mut self, face: FaceRef) -> Result<Vec<CompositeCurve>> {
        self.with_brep_mut(face.0, |kernel, model, _, _| {
            crate::topology::face::p3::create_composites(kernel, model, face.1)
        })
    }
    /// Face's normal bounding box for the rotational axis check.
    fn primitive_face_box(&mut self, face: FaceRef) -> Result<Extent3d> {
        self.with_brep_mut(face.0, |kernel, model, _, _| {
            crate::query::face::p1::calculate_bounding_box(kernel, model, face.1, false)
        })
    }
    /// Run `query::face::p2::curve_on_classify_mut` on the face's B-rep with
    /// caller-resolved object handles; report whether any interval midpoint is
    /// `SM_PC_FACE`.
    fn axis_inside_face(&mut self, face: FaceRef, axis: &Curve) -> Result<bool> {
        self.with_brep_mut(face.0, |kernel, model, _, _| {
            crate::sweep::topology_sweep::store::classification::axis_inside_face(
                kernel, model, face.1, axis,
            )
        })
    }
    /// Linear cap stitch: callback, fast edge compare, no vertex-edge splits,
    /// non-manifold mode, region nesting, default targets and tolerance.
    fn stitch_linear_caps(
        &mut self,
        brep: BrepId,
        sweep: &mut TopologySweep,
        tolerance: f64,
    ) -> Result<()>
    where
        Self: Sized,
    {
        self.with_primitive_brep(brep, |kernel, model| {
            let mut callback = SweepStitchCallback { brep, sweep };
            let mut stitch =
                crate::heal::stitch::p1::Stitch::new(&mut callback, tolerance, true, false);
            stitch.fast_edge_compare = true;
            stitch.split_edges_with_vertices = false;
            stitch.making_manifold_solid = false;
            stitch
                .do_stitching(kernel, model, None, None, MinGapRequest::default())
                .map(|_| ())
        })
    }
}

/// Run `body` on `brep` with the face-construction environment of
/// `SmBrep::Create*Face*`: the caller's face identities and notification records,
/// with fresh topology marks.
fn with_face_env<A: SweepAccess + ?Sized, T>(
    access: &mut A,
    brep: BrepId,
    body: impl FnOnce(&mut Kernel, &mut Brep, &mut FaceBuildEnv<'_, '_>) -> Result<T>,
) -> Result<T> {
    access.with_brep_mut(
        brep,
        |kernel, model, this_face: &dyn Fn(FaceId) -> ObjectRef, env| {
            let mut marks = Marks::default();
            let mut fenv = FaceBuildEnv {
                env,
                marks: &mut marks,
                this_face,
            };
            body(kernel, model, &mut fenv)
        },
    )
}
