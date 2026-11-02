//! The surface/surface fillet solver's virtual methods and the part of its
//! `CalcFilletGeom` that builds the fillet geometry in a live executive's pseudo-B-rep.
//!
//! Upstream: `source/SMLib/src/SmFilletStandardSolver.cpp`
//! (`SmSurfaceSurfaceFS::CalcFilletGeom` 4592-4812),
//! `source/SMLib/inc/SmFilletStandardSolver.h` (`SmConstantRadiusFS`,
//! `SmSurfaceSurfaceFS`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `impl FilletSolverOps for SurfaceSurfaceFs` | the `SmSurfaceSurfaceFS` virtuals: those it inherits from `SmConstantRadiusFS` (`GetJacobianSize`, `SetupOffsetValues`, `OffsetRadiiCanChange`, `LoadJacobian`, `LoadInitialValues`) and from `SmFilletSolver` (`PointOnPlaneSolve`, `CanMakeAnalytic`, `SetExtensionFlags`), and its own `SetupOffsetExtension`, `GetType`, `IsKindOf`, `GetSolverType` |
//! | [`SurfaceSurfaceFs::calc_fillet_geom_with_context`] | `SmSurfaceSurfaceFS::CalcFilletGeom` |
//! | [`SurfaceSurfaceContext`] | the executive's pseudo-B-rep and the rails' original faces it reaches |
//! | [`BaseBodies`] | the rails' original faces' B-reps (`GetOriginalFace()->GetBrep()`) |
//!
//! Deviations: the part before `GetPseudoBrep` is `blend.fillet_standard_solver_p2`'s
//! [`SurfaceSurfaceFs::calc_fillet_start`]. The rails' classifications and
//! `SplitAndTrimFilletSurface` resolve their objects through the caller's
//! [`SurfaceSurfaceContext`] (the live session's base B-reps and pseudo-B-rep); each
//! rail is classified on its original face in that face's own base B-rep
//! ([`BaseBodies`], upstream `pBaseFace->GetBrep()`).
//! `SM_ASSERT(m_vFilletGeoms.GetSize() == 1)` (debug only) is not checked; the first
//! geometry is used. A rail, edge use, curve or original face upstream dereferences
//! unchecked is [`SolidError::InvalidInput`]. No recursion.

use super::p1::{FilletSolverType, SurfaceSurfaceFs};
use super::p2::{ConstantRadiusOps, SurfaceSurfaceStart};
use crate::blend::fillet_classification::FilletClassObject;
use crate::blend::fillet_custom_solver::{InitialValues, JacobianLoad, OffsetSurface, SurfaceId};
use crate::blend::fillet_geom::p1::FilletBrep;
use crate::blend::fillet_solver::p1::{FilletSolver, FilletSolverOps};
use crate::blend::fillet_solver::p5::RailClassificationContext;
use crate::blend::fillet_standard_solver::p1::BoundaryTrimmingType;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::matrix::Matrix;
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::surface_intersector::p1::TsectPnt;
use crate::kernel::Kernel;
use crate::math::Point3;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::surface::surface::Surface;
use crate::topology::{edge, edgeuse::p1 as eu, Brep, FaceId};

const OP: &str = "blend::fillet_standard_solver::surface_surface";

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// The B-reps of the two base faces (`pBaseFace->GetBrep()` of each rail): the
/// target, and the second surface's B-rep when its face is not in the target.
#[derive(Debug)]
pub struct BaseBodies<'a> {
    /// The first surface's B-rep (and the second's when `second` is `None`).
    pub target: &'a mut Brep,
    /// The second surface's B-rep when it is not `target`.
    pub second: Option<&'a mut Brep>,
}

/// What `SmSurfaceSurfaceFS::CalcFilletGeom` reaches through the executive besides the
/// solver: the rails' classification on their original faces and the geometry
/// `SplitAndTrimFilletSurface` resolves their classifications with.
pub trait SurfaceSurfaceContext: RailClassificationContext {
    /// `pRail->GetOriginalFace()->CurveOnClassify(TRUE, *pCurveClass, TRUE)` of rail
    /// `side`, whose original face `face` lies in that side's base B-rep of `bases`.
    ///
    /// # Errors
    /// Those of the classification.
    #[allow(clippy::too_many_arguments)]
    fn classify_rail(
        &mut self,
        k: &Kernel,
        bases: &mut BaseBodies<'_>,
        pseudo: &Brep,
        side: usize,
        face: FaceId,
        class: &mut CurveClassification<FilletClassObject>,
    ) -> Result<()>;
    /// `SplitAndTrimFilletSurface(lFilletGeomIndex, eTrimType)` of `solver`, whose
    /// geometries live in `pseudo` and whose rails were classified on the faces of
    /// `bases`.
    ///
    /// # Errors
    /// Those of [`FilletSolver::split_and_trim_fillet_surface_with`].
    fn split_and_trim(
        &mut self,
        k: &Kernel,
        solver: &mut FilletSolver,
        bases: &BaseBodies<'_>,
        pseudo: &mut FilletBrep,
        fillet_geom_index: usize,
        trim_type: BoundaryTrimmingType,
    ) -> Result<()>;
}

impl SurfaceSurfaceFs {
    /// `SmSurfaceSurfaceFS::CalcFilletGeom` in a live executive: from the stepped start
    /// point ([`SurfaceSurfaceFs::calc_fillet_start`]), two rail edges of the solver's
    /// fillet geometry in `pseudo` on the faces of the two base surfaces in `bases`,
    /// the offset-surface intersection through the point (extended at both ends,
    /// `FlushCurve` building the fillet surface and rail curves), each rail's curve
    /// classification on its original face and the primary edge uses' UV lines along
    /// the fillet surface's `v` boundaries, then `SplitAndTrimFilletSurface(0,
    /// m_eTrimType)`.
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceSurfaceFs::calc_fillet_start`]; `SM_ERR_NULL_POINTER` for a
    /// base surface without face or a geometry without fillet surface; those of the
    /// rail construction, the intersection, the classifications, the line segment and
    /// the trimming.
    pub fn calc_fillet_geom_with_context(
        &mut self,
        k: &Kernel,
        bases: &mut BaseBodies<'_>,
        pseudo: &mut FilletBrep,
        context: &mut impl SurfaceSurfaceContext,
    ) -> Result<()> {
        let Some(SurfaceSurfaceStart {
            mut tsect_pnts,
            domains,
        }) = self.calc_fillet_start(k)?
        else {
            return Ok(());
        };

        let fillet_geom = self
            .base
            .solver
            .fillet_geoms
            .first()
            .copied()
            .ok_or_else(|| invalid("no fillet geometry"))?;

        // Create the two rail curves
        let mut faces = [None; 2];
        for (ii, face) in faces.iter_mut().enumerate() {
            let SurfaceId::Face(id) = self.base.surface(ii)?.base_id else {
                return Err(SolidError::null_pointer(OP));
            };
            *face = Some(id);
        }
        for (ii, face) in faces.into_iter().enumerate() {
            let rail = pseudo.make_rail_edge(k, fillet_geom, ii)?;
            pseudo.fillet_edge_mut(rail)?.set_original_face(face);
        }

        // Do the intersection
        let surfaces = [
            Surface::from(self.base.surface(0)?.surface.clone()),
            Surface::from(self.base.surface(1)?.surface.clone()),
        ];
        let app_tol = self.base.solver.this_approx_tol_3d();
        let ang_tol = self.base.solver.this_ang_tol_rad();
        crate::blend::fillet_solver::p4::point_intersection_on_surfaces(
            self,
            k,
            bases.target,
            pseudo,
            &surfaces,
            domains,
            &mut tsect_pnts,
            [true, true],
            app_tol,
            ang_tol,
        )?;

        let uv_domain = pseudo
            .geom(fillet_geom)?
            .fillet_surface(None)?
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .natural_uv_domain_with_nurb(k)?;
        let uv_max = uv_domain.max();
        let uv_min = uv_domain.min();
        // 'V' parameter of each rail (iso-curve)
        let v = [uv_domain.min().y, uv_domain.max().y];

        for (ii, v) in v.into_iter().enumerate() {
            let rail = pseudo
                .geom(fillet_geom)?
                .rail(ii)
                .ok_or_else(|| invalid("fillet geometry without rail"))?;
            let prim = eu::primary_edgeuse(pseudo.brep(), rail)?
                .ok_or_else(|| invalid("rail without primary edge use"))?;
            let mate = eu::mate(pseudo.brep(), prim)?
                .ok_or_else(|| invalid("rail edge use without mate"))?;
            let interval = edge::curve(pseudo.brep(), rail)?
                .ok_or_else(|| invalid("rail without curve"))?
                .natural_interval();
            let mut cc = CurveClassification::new(k);
            cc.reset(
                k,
                &ResetOptions {
                    curve: Some(context.rail_curve(rail)),
                    interval: Some(interval),
                    uv_curve: context.rail_mate_uv_curve(pseudo.brep(), mate),
                    zone_tol_3d: self.base.solver.this_approx_tol_3d(),
                    ..ResetOptions::default()
                },
            )?;
            pseudo.fillet_edge_mut(rail)?.set_curve_class(Some(cc));
            let face = pseudo
                .fillet_edge(rail)?
                .original_face()
                .ok_or_else(|| invalid("rail without original face"))?;
            pseudo.with_rail_classification(rail, |pseudo, cc| {
                context.classify_rail(k, bases, pseudo, ii, face, cc)
            })?;

            let start_uv = Point3::new(uv_min.x, v, 0.0);
            let end_uv = Point3::new(uv_max.x, v, 0.0);
            let mut uv_curve = BSplineCurve::create_line_segment(2, start_uv, end_uv)?;
            uv_curve.edit_parameterization(&Extent1d::from_min_max(uv_min.x, uv_max.x))?;
            pseudo.set_edgeuse_uv_curve(prim, Some(Curve::from(uv_curve)), false, true)?;
        }

        let trim_type = self.trim_type;
        context.split_and_trim(k, &mut self.base.solver, bases, pseudo, 0, trim_type)
    }
}

impl FilletSolverOps for SurfaceSurfaceFs {
    fn solver(&self) -> &FilletSolver {
        &self.base.solver
    }

    fn solver_mut(&mut self) -> &mut FilletSolver {
        &mut self.base.solver
    }

    /// `SmConstantRadiusFS::GetJacobianSize` (inherited).
    fn jacobian_size(&self) -> Result<usize> {
        FilletSolverOps::jacobian_size(&self.base)
    }

    /// `SmConstantRadiusFS::SetupOffsetValues` (inherited).
    fn setup_offset_values(
        &mut self,
        tsect: &TsectPnt,
        offset_dist: Option<&mut f64>,
    ) -> Result<()> {
        FilletSolverOps::setup_offset_values(&mut self.base, tsect, offset_dist)
    }

    /// `SmConstantRadiusFS::OffsetRadiiCanChange` (inherited): `FALSE`.
    fn offset_radii_can_change(&self) -> Result<bool> {
        FilletSolverOps::offset_radii_can_change(&self.base)
    }

    /// `SmSurfaceSurfaceFS::SetupOffsetExtension`.
    fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        ConstantRadiusOps::setup_offset_extension(self, k, surface, domain)
    }

    /// `SmConstantRadiusFS::LoadJacobian` (inherited).
    fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        FilletSolverOps::load_jacobian(
            &mut self.base,
            k,
            num_equations,
            num_parameters,
            load,
            f,
            jacobian,
        )
    }

    /// `SmConstantRadiusFS::LoadInitialValues` (inherited) with this class's
    /// `SetupOffsetExtension` ([`SurfaceSurfaceFs::load_initial_values`]).
    fn load_initial_values(
        &mut self,
        k: &Kernel,
        brep: Option<&mut Brep>,
        do_surf2_calcs: bool,
        rail_index: usize,
        surface1: &mut OffsetSurface,
        surface1_index: usize,
        intervals: &mut ExtentNd,
        periodicities: &mut Vec<bool>,
        guess_t: &mut Vec<f64>,
    ) -> Result<Option<InitialValues>> {
        SurfaceSurfaceFs::load_initial_values(
            self,
            k,
            brep,
            do_surf2_calcs,
            rail_index,
            surface1,
            surface1_index,
            intervals,
            periodicities,
            guess_t,
        )
    }

    /// `SmSurfaceSurfaceFS::GetType`.
    fn type_code(&self) -> Result<i64> {
        Ok(SurfaceSurfaceFs::TYPE)
    }

    /// `SmSurfaceSurfaceFS::IsKindOf`.
    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(SurfaceSurfaceFs::is_kind_of(self, t))
    }

    /// `SmSurfaceSurfaceFS::GetSolverType`: `SM_FS_SURF_SURF`.
    fn solver_type(&self) -> Result<FilletSolverType> {
        Ok(SurfaceSurfaceFs::solver_type(self))
    }
}
