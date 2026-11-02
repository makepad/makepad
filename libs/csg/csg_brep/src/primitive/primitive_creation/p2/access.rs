//! External B-rep services of the p2 primitive entry points.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp` (callee boundaries).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreationAccess`] | B-rep, face, merge and stitching services |
//!
//! Deviations: extends p3's [`PrimitiveAccess`] as that trait extends
//! `SweepAccess`. Defaults call the ported services (`topology.brep_p2`,
//! `topology.face_p3`, `topology.brep_p5`, `boolean.merge_p3` through
//! `boolean::api_brep`) and defer only callees whose unit is not ported yet;
//! outputs no p2 caller reads (stitch counts and gaps) are not returned.

use crate::curve::composite_curve::OffsetMapAttribute;
use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::math::{Point3, Vec3};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::primitive::primitive_creation::p3::PrimitiveAccess;
use crate::surface::surface::Surface;
use crate::sweep::topology_sweep::access::FaceRef;
use crate::topology::brep::p10::TrimType;
use crate::topology::brep::p7::FaceCurvesInput;
use crate::topology::{BrepId, FaceId};

/// Topology adapters used by the p2 primitives, on top of [`PrimitiveAccess`].
/// Construction tails never replace upstream's topological edits with arena-only
/// approximations.
pub trait PrimitiveCreationAccess: PrimitiveAccess {
    /// Copy `faces` of `source` into `destination` face by face, without the merge
    /// (`SmBrep::CopyFaces(faces, destination, NULL, FALSE)`; its result is unused
    /// here). Implementations resolve both B-reps for `topology::brep::p7::copy_faces`.
    fn copy_faces_into(
        &mut self,
        source: BrepId,
        faces: &[FaceId],
        destination: BrepId,
    ) -> Result<()>;
    /// Attach an offset map to the surface of `face` (`SmAObject::AddAttribute` of an
    /// `SM_AI_OFFSETMAP` attribute), in the caller's attribute store.
    fn attach_offset_map(&mut self, face: FaceRef, map: OffsetMapAttribute) -> Result<()>;
    /// The offset map of the surface of `face`, if any
    /// (`FindAttribute(SM_AI_OFFSETMAP)` on its surface).
    fn offset_map(&self, face: FaceRef) -> Result<Option<OffsetMapAttribute>>;
    /// Project `curve` along `direction` onto the B-rep's faces and trim them
    /// (`SmBrep::ProjectAndTrim(curve, direction, reference, type, &tol, NULL, NULL)`).
    /// Implementations call `topology::brep::p10::project_and_trim` with the B-rep's
    /// `ProjectEnv` and `approximation_tol_3d: Some(approx_tol)`.
    fn project_and_trim(
        &mut self,
        brep: BrepId,
        curve: &BSplineCurve,
        direction: Vec3,
        reference: Point3,
        trim_type: TrimType,
        approx_tol: f64,
    ) -> Result<()>;

    /// Set the B-rep's zone tolerance (`SmBrep::SetTolerance(tol)`, with its defaults:
    /// `bUpdateOnlyIfLarger` and `bCascadeToBndries` `TRUE`, `SmBrep.h:700-702`).
    fn set_brep_tolerance(&mut self, brep: BrepId, tolerance: f64) -> Result<()>
    where
        Self: Sized,
    {
        self.with_primitive_brep(brep, |k, b| {
            crate::topology::brep::p2::set_tolerance(k, b, tolerance, true, true)
        })
    }
    /// Shrink every face's surface to its UV trim curves and every edge's curve to
    /// the edge (`SmBrep::ShrinkGeometry`).
    fn shrink_geometry(&mut self, brep: BrepId) -> Result<()>
    where
        Self: Sized,
    {
        self.with_primitive_brep(brep, |k, b| {
            crate::topology::brep::p4::shrink_geometry(k, b)
        })
    }
    /// Whether two consecutive edges of the face's outer loop are lamina
    /// (`SmFace::HasConsecutiveLaminaEdges`).
    fn has_consecutive_lamina_edges(&mut self, face: FaceRef) -> Result<bool> {
        crate::topology::face::p3::has_consecutive_lamina_edges(self.brep(face.0)?, face.1)
    }
    /// Delete a face with the edges and vertices left unconnected, nesting regions
    /// (`SmBrep::DeleteFace(face, TRUE)`).
    ///
    /// The B-rep keeps no notification records of its own, so the deletion notifies
    /// through the kernel context's callbacks under the kernel's identities
    /// ([`crate::topology::brep::p3::with_kernel_callbacks`]).
    fn delete_face_and_free_topology(&mut self, face: FaceRef) -> Result<()>
    where
        Self: Sized,
    {
        use crate::topology::brep::p3 as brep_p3;
        let options = brep_p3::DeleteFaceOptions {
            delete_edges_vertices: true,
            ..brep_p3::DeleteFaceOptions::default()
        };
        self.with_primitive_brep(face.0, |k, b| {
            brep_p3::with_kernel_callbacks(k, b, |k, b, env| {
                let mut marks = crate::topology::topology_traverser::Marks::default();
                brep_p3::delete_face(k, b, env, &mut marks, face.1, options).map(drop)
            })
        })
    }
    /// Make a face on `surface` (over `domain`) from one or more loops of 3D curves
    /// with their UV curves (`SmBrep::MakeFaceWithCurves(NULL, loops, &3d, &uv,
    /// orientations, no points, surface, domain, orientation, ...)`), returning the new
    /// face. Implementations call `topology::brep::p7::make_face_with_curves` on the
    /// B-rep with its face identities and notification records.
    fn face_with_curves(
        &mut self,
        brep: BrepId,
        curves: FaceCurvesInput,
        surface: Surface,
        domain: Extent2d,
        orientation: OrientType,
    ) -> Result<Option<FaceId>>
    where
        Self: Sized,
    {
        self.with_brep_mut(brep, |k, b, this_face, env| {
            crate::topology::brep::p7::make_face_with_curves(
                k,
                b,
                None,
                curves,
                surface,
                &domain,
                orientation,
                None,
                this_face,
                env,
            )
            .map(|made| made.face)
        })
    }
    /// Glue coincident boundary topology of the B-rep's faces within `tolerance`
    /// (`SmBrep::StitchFaces`; its counts and gaps are unused here).
    fn stitch_faces(&mut self, brep: BrepId, tolerance: f64) -> Result<()>
    where
        Self: Sized,
    {
        // header defaults: pOptFaces = NULL, bAllowSpineEdges = FALSE
        self.with_primitive_brep(brep, |k, b| {
            crate::topology::brep::p4::stitch_faces(k, b, tolerance, None, false).map(drop)
        })
    }
    /// Remove edges and vertices that only split faces or edges
    /// (`SmBrep::RemoveTopologicalEdgesAndVertices`).
    fn remove_topological_edges_and_vertices(&mut self, brep: BrepId) -> Result<()>
    where
        Self: Sized,
    {
        self.with_primitive_brep(brep, |k, b| {
            crate::topology::brep::p5::remove_topological_edges_and_vertices(k, b, None, None)
        })
    }
    /// Whether the B-rep is a closed manifold solid (`SmBrep::IsManifoldSolid`).
    fn is_manifold_solid(&mut self, brep: BrepId) -> Result<bool> {
        crate::topology::brep::p2::is_manifold_solid(self.brep(brep)?)
    }
    /// Combine `breps` by the postfix tree `tree`, leaving the result first
    /// (`SmMerge::BooleanTrees`). The B-reps move out of their slots into
    /// `boolean::api_brep::boolean_trees` and come back in its order (one result per
    /// tree, then the unused B-reps, as `sOrderedBreps` returns) in the first slots of
    /// `breps`; the slots left over are released and dropped from `breps`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` for an empty list or a B-rep listed twice, before any
    /// B-rep moves; those of the merge, after which the B-reps it had not reached are
    /// back in their slots and the ones it had taken stay empty (upstream loses them
    /// too); `SM_ERR_NULL_POINTER` for a tree that leaves no B-rep, where upstream would
    /// go on with a null `pSweepBrep`.
    fn boolean_trees(&mut self, breps: &mut Vec<BrepId>, tree: &[i64]) -> Result<()> {
        const OP: &str = "SmMerge::BooleanTrees";
        let first = *breps
            .first()
            .ok_or_else(|| SolidError::invalid_input(OP, "no B-reps to combine"))?;
        for (i, id) in breps.iter().enumerate() {
            self.brep(*id)?;
            if breps.iter().take(i).any(|other| other == id) {
                return Err(SolidError::invalid_input(OP, "a B-rep listed twice"));
            }
        }
        let mut operands: Vec<Option<crate::topology::Brep>> = Vec::with_capacity(breps.len());
        for &id in breps.iter() {
            operands.push(Some(std::mem::replace(
                self.brep_mut(id)?,
                crate::topology::Brep::new(),
            )));
        }
        let status = match self.kernel_mut(first) {
            Ok(k) => crate::boolean::api_brep::boolean_trees(k, &mut operands, tree),
            Err(e) => Err(e),
        };
        if let Err(e) = status {
            for (&id, operand) in breps.iter().zip(operands) {
                if let Some(b) = operand {
                    *self.brep_mut(id)? = b;
                }
            }
            return Err(e);
        }
        let count = operands.len();
        if count > breps.len() {
            return Err(SolidError::sm_err(OP));
        }
        let mut missing = false;
        for (&id, operand) in breps.iter().zip(operands) {
            match operand {
                Some(b) => *self.brep_mut(id)? = b,
                None => missing = true,
            }
        }
        for &id in breps.iter().skip(count) {
            self.delete_brep(id);
        }
        breps.truncate(count);
        if missing {
            return Err(SolidError::null_pointer(OP));
        }
        Ok(())
    }
}
