//! Surface/surface filleting in a live fillet session: the executive's
//! `SurfaceSurfaceFillet` and `DoSurfaceFilleting` on the session's objects.
//!
//! Upstream: `source/SMLib/src/SmFilletExecutive.cpp` (`SurfaceSurfaceFillet`
//! 1306-1561, `DoSurfaceFilleting` 3750-3969).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`FilletSession::surface_surface_fillet`] | `SmFilletExecutive::SurfaceSurfaceFillet` |
//! | [`FilletSession::do_surface_filleting`] | `SmFilletExecutive::DoSurfaceFilleting` |
//! | `insert_surface_intersection_topology` (private) | `DoSurfaceFilleting`'s `InsertIntersectionTopology` loop |
//! | `allocate_surface_surface_solver` (private) | `new SmSurfaceSurfaceFS(..)`, `SetFilletTrimType`, `SetReverseTrim` |
//!
//! Deviations: the two surfaces are those of two faces (`pSur->GetFace()`, which
//! `SmSurfaceSurfaceFS::CalcFilletGeom` reads): the first in the session's target, the
//! second in the session's second base B-rep when it has one (the two may come from
//! different B-reps upstream), otherwise in the target; the caller supplies them
//! (`blend::fillet_executive::api::surface_surface_fillet`). Each rail is classified on
//! and merged into its base face's B-rep (`pBaseFace->GetBrep()`); a second base
//! B-rep's topology is named [`FilletBody::SecondBase`] in classifications, and its
//! rail merges run with it in the target's place, on their own copy of the
//! executive's intersector (taken before the first merge: the two arenas' typed ids
//! may coincide where upstream pointers differ), and a split of one base B-rep's face
//! leaves the other side's rails alone. A side edge with an original face in a
//! two-B-rep session is an invalid-input error: upstream's surface/surface side
//! edges are `CreateEndCurve` cross sections without one, so its side loop skips
//! them. `SM_BT_BLEND` end curves on a second base B-rep are `Unported`; no
//! surface/surface case reaches them. The executive is the session's
//! (upstream a fresh `SmFilletExecutive` on the stack), which owns the cross-section
//! generator (on the stack upstream). The fillet B-rep is the session's intermediate
//! body. The trimming of the original surfaces (only with `lBaseTrimType != 0`, which
//! `SmApiSurfaceSurfaceFillet` never passes) runs under the session's original-trim
//! mark scope (one context: both bodies' identities are this kernel's) and deletes the
//! selected faces as target faces. As upstream, the topology
//! insertion leaves the intersector's primary and other B-reps unset and inserts only
//! geometries with a fillet surface. No recursion.

use super::{classification, invalid, FilletObject, FilletSession, SessionObjects, SolverObject};
use crate::blend::fillet_classification::FilletBody;
use crate::blend::fillet_custom_solver::SurfaceId;
use crate::blend::fillet_executive::p2::SurfaceFilletOptions;
use crate::blend::fillet_executive::p3::{ErrorInfo, FaceConstructionAccess, FaceStitchAccess};
use crate::blend::fillet_geom::p1::{FilletBrep, FilletSolverId};
use crate::blend::fillet_solver::p1::{FilletSolver, FilletSurfaceGenerator};
use crate::blend::fillet_solver::p5::RailClassificationContext;
use crate::blend::fillet_standard_solver::p1::{BoundaryTrimmingType, SurfaceSurfaceFs};
use crate::blend::fillet_standard_solver::surface_surface::{BaseBodies, SurfaceSurfaceContext};
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::vector2d::Vec2;
use crate::kernel::Kernel;
use crate::math::PI;
use crate::surface::offset_surface::{OffsetSurface, SS_UNKNOWN};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::topology::{Brep, FaceId};

const OP: &str = "blend::fillet_executive::session::surface";

/// `SM_DEG2RAD(adeg)`.
fn deg_to_rad(deg: f64) -> f64 {
    deg * (PI / 180.0)
}

/// Entry `i` of an intersection's UV curve list (`SM_ERR_NULL_POINTER` for a null
/// curve, which upstream dereferences).
fn uv(list: &[Option<Curve>], i: usize) -> Result<&Curve> {
    list.get(i)
        .and_then(Option::as_ref)
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// The objects `SmSurfaceSurfaceFS::CalcFilletGeom` reaches in a live session.
pub(super) struct SurfaceSurfaceSession<'a> {
    pub corners: &'a [crate::blend::fillet_corner::p3::FilletCorner],
}

impl RailClassificationContext for SurfaceSurfaceSession<'_> {
    fn rail_curve(
        &self,
        edge: crate::topology::EdgeId,
    ) -> crate::blend::fillet_classification::FilletClassObject {
        classification::Context {
            corners: self.corners,
        }
        .rail_curve(edge)
    }
    fn rail_mate_uv_curve(
        &self,
        pseudo: &Brep,
        mate: crate::topology::EdgeuseId,
    ) -> Option<crate::blend::fillet_classification::FilletClassObject> {
        classification::Context {
            corners: self.corners,
        }
        .rail_mate_uv_curve(pseudo, mate)
    }
    fn classify(
        &mut self,
        k: &Kernel,
        target: &mut Brep,
        pseudo: &Brep,
        face: FaceId,
        class: &mut crate::intersect::curve_class::p1::CurveClassification<
            crate::blend::fillet_classification::FilletClassObject,
        >,
    ) -> Result<()> {
        classification::Context {
            corners: self.corners,
        }
        .classify(k, target, pseudo, face, class)
    }
    fn corner_type(
        &self,
        corner: crate::blend::fillet_geom::p1::FilletCornerId,
    ) -> Result<crate::blend::fillet_geom::p1::FilletCornerType> {
        classification::Context {
            corners: self.corners,
        }
        .corner_type(corner)
    }
    fn corners(&self) -> crate::blend::fillet_geom::p1::CornerLookup<'_> {
        crate::blend::fillet_geom::p1::CornerLookup::table(self.corners)
    }
}

impl SurfaceSurfaceContext for SurfaceSurfaceSession<'_> {
    fn classify_rail(
        &mut self,
        k: &Kernel,
        bases: &mut BaseBodies<'_>,
        pseudo: &Brep,
        side: usize,
        face: FaceId,
        cc: &mut crate::intersect::curve_class::p1::CurveClassification<
            crate::blend::fillet_classification::FilletClassObject,
        >,
    ) -> Result<()> {
        let BaseBodies { target, second } = bases;
        let Some(second) = second.as_deref_mut().filter(|_| side == 1) else {
            return self.classify(k, target, pseudo, face, cc);
        };
        // The second rail's original face lies in the second base B-rep: classify it
        // there, naming that body's topology as the second base's.
        let mut cache = crate::query::face::p2::CurveOnClassifyCache::default();
        let result = {
            let objects = classification::Objects {
                target,
                pseudo,
                second: Some(second),
                classified: FilletBody::SecondBase,
            };
            crate::query::face::p2::curve_on_classify_cached(
                k, second, face, &objects, true, cc, true, false, true, &mut cache,
            )
        };
        cache.apply_uv_trim_updates(second)?;
        result.map(|_| ())
    }
    fn split_and_trim(
        &mut self,
        k: &Kernel,
        solver: &mut FilletSolver,
        bases: &BaseBodies<'_>,
        pseudo: &mut FilletBrep,
        fillet_geom_index: usize,
        trim_type: BoundaryTrimmingType,
    ) -> Result<()> {
        // The classifications only read the rails' curves and the base topology,
        // which the split leaves as they are: resolve them in a copy of the pseudo body.
        let snapshot = pseudo.brep().clone();
        let objects = classification::Objects {
            target: bases.target,
            pseudo: &snapshot,
            second: bases.second.as_deref(),
            classified: FilletBody::Target,
        };
        solver.split_and_trim_fillet_surface_with(
            k,
            pseudo,
            &objects,
            fillet_geom_index,
            trim_type,
            // CreateEndCurve's SM_BT_BLEND tangents read the edge each rail's end
            // vertex is classified on: the second rail's lies in the second base
            [
                bases.target,
                bases.second.as_deref().unwrap_or(bases.target),
            ],
        )
    }
}

impl SessionObjects<'_> {
    /// The B-rep of base surface `side`'s face: the second base B-rep for the second
    /// surface when the session has one, otherwise the target.
    fn base_body(&self, side: usize) -> &Brep {
        match &self.second_base {
            Some(base) if side == 1 => base.body,
            _ => self.target,
        }
    }

    /// `new SmSurfaceSurfaceFS(crContext, dTolerance, dAngTol, dTanTol, |r1|, |r2|,
    /// *pSur1, *pSur2, bOrient1, bOrient2, sGuess1, sGuess2)`, then
    /// `SetFilletTrimType(eFilTrimType)` and `SetReverseTrim(bReverseTrim)`. The surfaces
    /// are those of target faces `faces`.
    #[allow(clippy::too_many_arguments)] // the native constructor's parameters
    fn allocate_surface_surface_solver(
        &mut self,
        k: &Kernel,
        tolerance: f64,
        ang_tol: f64,
        tan_tol: f64,
        radii: [f64; 2],
        faces: [FaceId; 2],
        orientations: [bool; 2],
        guesses: [Vec2; 2],
        trim_type: BoundaryTrimmingType,
        reverse_trim: bool,
    ) -> Result<FilletSolverId> {
        let id = FilletSolverId(
            u32::try_from(self.solvers.len()).map_err(|_| invalid("too many solvers"))?,
        );
        let mut surfaces = Vec::with_capacity(2);
        for (side, face) in faces.into_iter().enumerate() {
            let surface = self
                .base_body(side)
                .face(face)?
                .surface
                .clone()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            surfaces.push((surface, SurfaceId::Face(face)));
        }
        let [s1, s2]: [(Surface, SurfaceId); 2] = surfaces
            .try_into()
            .map_err(|_| invalid("two surfaces expected"))?;
        // Reserve the identity before construction writes a geometry pointing at it.
        self.solvers.push(None);
        let mut solver = SurfaceSurfaceFs::new(
            k,
            &mut self.pseudo,
            id,
            tolerance,
            ang_tol,
            tan_tol,
            radii,
            [s1, s2],
            orientations,
            guesses,
        )?;
        solver.set_fillet_trim_type(trim_type);
        solver.base.solver.set_reverse_trim(reverse_trim);
        *self
            .solvers
            .get_mut(id.0 as usize)
            .ok_or_else(|| invalid("lost solver slot"))? =
            Some(SolverObject::SurfaceSurface(Box::new(solver)));
        Ok(id)
    }
}

impl FilletSession<'_> {
    /// `SmFilletExecutive::SurfaceSurfaceFillet`: fillets the surfaces of faces `faces`
    /// (the first in the target, the second in the second base B-rep when the session
    /// has one, otherwise in the target) with signed radii (the fillet centre lies on the intersection of the
    /// two offset surfaces). Each intersection curve of the offsets seeds one
    /// surface/surface solver, rolled from near its middle; the fillet surfaces are
    /// built into the session's intermediate body ([`Self::do_surface_filleting`]).
    /// `false` when the offsets do not intersect (no fillet B-rep).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the intersection curves intersect each other (self-intersecting
    /// fillets are not handled); `SM_ERR_NULL_POINTER` for a face without surface or a
    /// null UV curve; those of the offsets, the intersections, the evaluations, the
    /// solver set-up and [`Self::do_surface_filleting`].
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn surface_surface_fillet(
        &mut self,
        k: &mut Kernel,
        faces: [FaceId; 2],
        fillet_radius1: f64,
        fillet_radius2: f64,
        tolerance: f64,
        options: &SurfaceFilletOptions,
    ) -> Result<bool> {
        let surface_of = |session: &Self, side: usize, face: FaceId| -> Result<Surface> {
            session
                .objects
                .base_body(side)
                .face(face)?
                .surface
                .clone()
                .ok_or_else(|| SolidError::null_pointer(OP))
        };
        let [face1, face2] = faces;
        let sur1 = surface_of(self, 0, face1)?;
        let sur2 = surface_of(self, 1, face2)?;

        // Create temporary offset surfaces
        let off1 = Surface::from(OffsetSurface::new(
            k,
            fillet_radius1,
            sur1,
            false,
            SS_UNKNOWN,
        )?);
        let off2 = Surface::from(OffsetSurface::new(
            k,
            fillet_radius2,
            sur2,
            false,
            SS_UNKNOWN,
        )?);

        // Intersect the offset surfaces. We will use only the two uv curves.
        let use_surface_edges = [true, true];
        let tol = (fillet_radius1.abs() + fillet_radius2.abs()) / 1000.0;
        let sect = off1.global_surface_intersect(
            k,
            &off1.natural_uv_domain_with_nurb(k)?,
            &off2,
            &off2.natural_uv_domain_with_nurb(k)?,
            use_surface_edges,
            Some(tol),
            None,
        )?;
        let uv_curves1 = sect.surface1_uv_curves;
        let uv_curves2 = sect.surface2_uv_curves;

        // no work - no intersection
        if uv_curves1.is_empty() {
            return Ok(false);
        }

        // When more than one intersection curve was found, check for
        // self-intersections: we don't do those.
        if uv_curves1.len() > 1 {
            for ii in 0..uv_curves1.len() {
                let uv_curve1 = uv(&uv_curves1, ii)?;
                let ivl1 = uv_curve1.natural_interval();
                for jjj in ii + 1..uv_curves1.len() {
                    let uv_curve2 = uv(&uv_curves1, jjj)?;
                    let ivl2 = uv_curve2.natural_interval();
                    let solutions = uv_curve1.global_curve_intersect(
                        k,
                        &ivl1,
                        uv_curve2,
                        &ivl2,
                        tol,
                        &CurveIntersectOptions::<NoObject>::default(),
                    )?;
                    // Self-intersecting fillets are not yet handled
                    if !solutions.is_empty() {
                        return Err(SolidError::sm_err(OP));
                    }
                }
            }
        }

        // The fillet surface generator for the cross section: 0: linear, 1:
        // approximated circular, 2: exact (rational) circular. All of the user options
        // apply only to the circular one.
        let generator = if options.xsect_type == 0 {
            self.add_surface_generator(FilletSurfaceGenerator::Linear)?
        } else {
            self.add_surface_generator(FilletSurfaceGenerator::circular(
                options.xsect_type == 1,
                options.xsect_accuracy,
                options.mirror,
                options.complement,
            ))?
        };

        // Info for trimming the base surfaces:
        let reverse_trim = options.base_trim_type == 2;

        // Each curve produced during intersection may produce a separate fillet surface.
        for kk in 0..uv_curves1.len() {
            let uv_curve1 = uv(&uv_curves1, kk)?;
            let ivl1 = uv_curve1.natural_interval();
            let uv_curve2 = uv(&uv_curves2, kk)?;
            let ivl2 = uv_curve2.natural_interval();

            let pnt1 = uv_curve1.evaluate_point(k, ivl1.evaluate(0.4567))?;
            let pnt2 = uv_curve2.evaluate_point(k, ivl2.evaluate(0.4567))?;
            let guess1 = Vec2::new(pnt1.x, pnt1.y);
            let guess2 = Vec2::new(pnt2.x, pnt2.y);

            // Create the fillet solver for filleting two surfaces.
            let orient1 = fillet_radius1 >= 0.0;
            let orient2 = fillet_radius2 >= 0.0;

            // A couple of default tolerances.
            let ang_tol = deg_to_rad(30.0);
            let tan_tol = deg_to_rad(1.0);

            let fs = self.objects.allocate_surface_surface_solver(
                k,
                tolerance,
                ang_tol,
                tan_tol,
                [fillet_radius1.abs(), fillet_radius2.abs()],
                faces,
                [orient1, orient2],
                [guess1, guess2],
                options.trim_type,
                reverse_trim,
            )?;

            // Attach the fillet surface generator to the fillet solver
            self.set_solver_generator(fs, generator)?;

            // Load the fillet solver into the fillet executive
            self.load_solver(k, fs)?;
        }

        // The following allows us to turn off trimming of the original fillet geometry.
        if options.base_trim_type == 0 {
            self.executive.options.trimming = false; // SetDoTrimming(FALSE)
        }

        self.do_surface_filleting(k, None)?;
        Ok(true)
    }

    /// `SmFilletExecutive::DoSurfaceFilleting`: with the context's doing-Boolean flag
    /// set, a new fillet B-rep (the intermediate body, editing enabled meanwhile), every
    /// solver's fillet geometry (`CalcFilletGeom`; a failing solver is skipped), the
    /// B-rep's tolerance (the largest solver approximation tolerance), a face for each
    /// geometry with a fillet surface, the executive's edge attribute on every edge,
    /// `StitchFaces`, `SetFilletBrepEdgeMap` and (when enabled) every geometry's
    /// intersection topology. `error_out` receives the executive's error info.
    ///
    /// # Errors
    ///
    /// Those of the face construction, stitching, edge map and insertion,
    /// and those of the original-surface trimming.
    pub fn do_surface_filleting(
        &mut self,
        k: &mut Kernel,
        error_out: Option<&mut ErrorInfo<FilletObject>>,
    ) -> Result<()> {
        let doing_boolean = k.context.doing_boolean();
        k.context.set_doing_boolean(true);
        let result = self.do_surface_filleting_body(k, error_out);
        k.context.set_doing_boolean(doing_boolean);
        result
    }

    fn do_surface_filleting_body(
        &mut self,
        k: &mut Kernel,
        error_out: Option<&mut ErrorInfo<FilletObject>>,
    ) -> Result<()> {
        // create output Fillet Brep to hold all the new Fillet Faces
        let fillet = self.objects.faces_new_brep(k)?;
        self.executive.fillet = Some(fillet);
        let editing = self
            .objects
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?
            .editing_enabled();
        self.objects.faces_enable_editing(fillet)?;
        let result = self.surface_fillet_body(k, fillet, error_out);
        if let Some(body) = self.objects.intermediate.as_mut() {
            body.set_editing_enabled(editing);
        }
        result
    }

    fn surface_fillet_body(
        &mut self,
        k: &mut Kernel,
        fillet: FilletObject,
        error_out: Option<&mut ErrorInfo<FilletObject>>,
    ) -> Result<()> {
        // for every FilletSolver create a FilletGeom
        let mut tol = 0.0_f64;
        let mut index = 0;
        while let Some(solver) = self.executive.solvers.get(index).copied() {
            index += 1;
            let FilletObject::Solver(id) = solver else {
                return Err(invalid("object is not a solver"));
            };
            match self
                .objects
                .calc_solver_geometry(k, id, &mut self.executive)
            {
                Err(error) if error.is_unported() => return Err(error),
                Err(_) => continue,
                Ok(()) => {}
            }
            tol = tol.max(self.objects.solver(id)?.base().this_approx_tol_3d());
        }

        // SetTolerance(dTol): SmBrep.h:700 implicitly constructs SmZoneTol3d; the
        // status is not read (void upstream)
        let zone = crate::geom::tol::zone_tol3d(k, tol);
        match self.objects.faces_set_tolerance(k, fillet, zone) {
            Err(error) if error.is_unported() => return Err(error),
            _ => {}
        }

        // for every FilletSolver->FilletGeom->FilletSurface, make a Face in the fillet
        // B-rep
        let mut index = 0;
        while let Some(solver) = self.executive.solvers.get(index).copied() {
            index += 1;
            let FilletObject::Solver(id) = solver else {
                return Err(invalid("object is not a solver"));
            };
            let geoms = self.objects.solver(id)?.base().fillet_geoms();
            for geom in geoms {
                if !self.objects.pseudo.geom(geom)?.has_fillet_surface() {
                    continue;
                }
                self.objects.faces_make_face(
                    k,
                    FilletObject::Geometry(geom),
                    fillet,
                    &mut self.executive,
                )?;
            }
        }

        // For every Edge in the fillet B-rep - add attribute(m_pAttributeE)
        for edge in self.objects.stitch_edges(fillet)? {
            match self
                .objects
                .stitch_attribute(edge, self.executive.edge_attribute)
            {
                Err(error) if error.is_unported() => return Err(error),
                _ => {}
            }
        }

        // stitch all topology in the fillet B-rep
        self.objects.faces_enable_editing(fillet)?;
        let mut stitch = self.objects.stitch_body(k, fillet, tol)?;
        if let Some(error) = stitch.status.take() {
            return Err(error);
        }

        // Set up a map between the fillet curves in the FilletGeoms and corresponding
        // fillet brep edges; insert the geometries' intersection topology.
        self.set_fillet_brep_edge_map(k)?;
        if self.executive.options.topology_insertion {
            self.insert_surface_intersection_topology(k)?;
        }

        // done with fillet B-rep editing
        if let Some(body) = self.objects.intermediate.as_mut() {
            body.set_editing_enabled(false);
        }

        // when asked for Trimming of the original surfaces
        if self.executive.options.trimming {
            self.trim_original_surfaces_for_surface_filleting(k)?;
        }

        if let Some(output) = error_out {
            output.assign(&self.executive.error);
        }
        Ok(())
    }

    /// `DoSurfaceFilleting`'s topology insertion: every geometry with a fillet surface
    /// of every solver, in order, `InsertIntersectionTopology()` (its rails merged into
    /// their base faces' B-reps). The topology intersector's primary and other B-reps
    /// stay unset, and no B-rep's editing is enabled beyond each rail merge's own scope.
    /// A second base B-rep's rails merge through a copy of the executive's intersector,
    /// taken here before the first merge, so that its relationships stay apart from
    /// the target's (the two arenas' typed ids may coincide; upstream one intersector
    /// keyed by distinct pointers, and a rail merge reads only its own rail's
    /// relationships).
    ///
    /// # Errors
    /// Those of the geometries' insertions.
    fn insert_surface_intersection_topology(&mut self, k: &mut Kernel) -> Result<()> {
        if let Some(base) = self.objects.second_base.as_mut() {
            base.intersector = Some(self.executive.intersector.clone());
        }
        let mut index = 0;
        while let Some(solver) = self.executive.solvers.get(index).copied() {
            index += 1;
            let FilletObject::Solver(id) = solver else {
                return Err(invalid("object is not a solver"));
            };
            let geoms = self.objects.solver(id)?.base().fillet_geoms();
            for geom in geoms {
                // GetFilletSurface() == NULL: the geometry's surface slot is empty (its
                // surface is the fillet B-rep face's once the face is made).
                if self.objects.pseudo.geom(geom)?.fillet_surface.is_none() {
                    continue;
                }
                // Merge each rail into originating face and more. . .
                self.insert_geometry_topology(k, FilletObject::Geometry(geom))?;
            }
        }
        Ok(())
    }

    /// Take the fillet B-rep [`Self::surface_surface_fillet`] built (upstream
    /// `GetFilletBrep()`, whose ownership the caller takes).
    pub fn take_fillet_brep(&mut self) -> Option<Brep> {
        self.objects.intermediate.take()
    }
}
