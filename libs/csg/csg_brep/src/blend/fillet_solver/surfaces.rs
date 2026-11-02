//! Borrowed offset context for the actual constant-radius rail solver.

use crate::blend::fillet_custom_solver::{
    ConstantRadiusAssistedFs, InitialValues, JacobianLoad, OffsetSurface,
};
use crate::blend::fillet_geom::p1::{FilletGeom, FilletGeomId};
use crate::blend::fillet_standard_solver::p1::constant_radius_load_jacobian;
use crate::blend::fillet_standard_solver::p2::{
    constant_radius_load_initial_values, ConstantRadiusOps,
};
use crate::error::{Result, SolidError};
use crate::geom::extent1d::ExtentNd;
use crate::geom::extent2d::Extent2d;
use crate::geom::matrix::Matrix;
use crate::intersect::surface_intersector::p1::TsectPnt;
use crate::kernel::Kernel;
use crate::surface::surface::Surface;
use crate::topology::{Brep, EdgeuseId};

use super::p1::{FilletSolver, FilletSolverOps, OffsetSurfaceSlot, RolloverGuesses};
use crate::math::Point2;

struct GeometrySolver<'a> {
    actual: &'a mut ConstantRadiusAssistedFs,
    geometry: &'a mut FilletGeom,
}

impl ConstantRadiusOps for GeometrySolver<'_> {
    fn constant_radius(&self) -> &ConstantRadiusAssistedFs {
        self.actual
    }

    fn constant_radius_mut(&mut self) -> &mut ConstantRadiusAssistedFs {
        self.actual
    }

    fn offset_surface(&self, rail: usize) -> Result<&OffsetSurface> {
        self.geometry
            .offset_surface(rail, Some(&self.actual.solver))?
            .ok_or_else(|| SolidError::null_pointer("blend::fillet_solver::surfaces"))
    }

    fn offset_surface_mut(&mut self, rail: usize) -> Result<&mut OffsetSurface> {
        let own = self.geometry.off_surfs.get_mut(rail).ok_or_else(|| {
            SolidError::invalid_input("blend::fillet_solver::surfaces", "rail index past 1")
        })?;
        if let Some(surface) = own {
            return Ok(surface);
        }
        self.actual.surface_mut(rail)
    }

    fn set_offset_by_radius(&mut self, rail: usize) -> Result<()> {
        let distance = self.actual.fillet_radius(rail)? * self.actual.solver.orientation(rail)?;
        ConstantRadiusOps::offset_surface_mut(self, rail)?.set_offset_distance(distance);
        Ok(())
    }

    fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        FilletSolverOps::setup_offset_extension(self.actual, k, surface, domain)
    }
}

impl FilletSolverOps for GeometrySolver<'_> {
    fn solver(&self) -> &FilletSolver {
        &self.actual.solver
    }
    fn solver_mut(&mut self) -> &mut FilletSolver {
        &mut self.actual.solver
    }

    fn solver_surface(&self, rail: usize) -> Result<Option<&OffsetSurface>> {
        Ok(Some(ConstantRadiusOps::offset_surface(self, rail)?))
    }

    fn solver_surface_mut(&mut self, rail: usize) -> Result<Option<&mut OffsetSurface>> {
        Ok(Some(ConstantRadiusOps::offset_surface_mut(self, rail)?))
    }

    fn jacobian_size(&self) -> Result<usize> {
        FilletSolverOps::jacobian_size(self.actual)
    }

    fn setup_offset_extension(
        &self,
        k: &Kernel,
        surface: &Surface,
        domain: &mut Extent2d,
    ) -> Result<()> {
        FilletSolverOps::setup_offset_extension(self.actual, k, surface, domain)
    }

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
        constant_radius_load_initial_values(
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

    fn load_jacobian(
        &mut self,
        k: &Kernel,
        num_equations: &mut usize,
        num_parameters: &mut usize,
        load: &mut JacobianLoad<'_>,
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let mut offsets = [load.surf1_offset, load.surf2_offset];
        let result = constant_radius_load_jacobian(
            self,
            k,
            num_equations,
            num_parameters,
            load.rail_index,
            [load.surface1, load.surface2],
            &mut offsets,
            load.x,
            f,
            jacobian,
        );
        [load.surf1_offset, load.surf2_offset] = offsets;
        result
    }
}

/// Run with one actual solver and its selected geometry; no offset object moves.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rail_edgeuse_intersect(
    actual: &mut ConstantRadiusAssistedFs,
    k: &Kernel,
    geometry: &mut FilletGeom,
    target: &mut Brep,
    rail_index: usize,
    edgeuse: EdgeuseId,
    guess: f64,
    tsect: &mut TsectPnt,
) -> Result<Option<f64>> {
    FilletSolver::rail_edgeuse_intersect(
        &mut GeometrySolver { actual, geometry },
        k,
        target,
        rail_index,
        edgeuse,
        guess,
        tsect,
    )
}

/// The transient keys of a lent geometry's offset surfaces while the solver holds
/// them; they never leave a lending scope.
const SELECTED_KEYS: [u64; 2] = [u64::MAX - 1, u64::MAX];

/// What [`FilletSolver::lend_geometry_surfaces`] replaced, for
/// [`FilletSolver::return_geometry_surfaces`].
#[must_use]
pub(crate) struct LentSurfaces {
    /// The geometry for the surfaces before the scope (`pSavedFG` / `pBackupFG`).
    saved_fg: Option<FilletGeomId>,
    /// That geometry's tangent-rollover guesses.
    saved_rollover: Option<(FilletGeomId, RolloverGuesses)>,
    /// The enclosing scope's lent geometry and surfaces, when this scope lent other
    /// surfaces; `None` when the geometry's surfaces were already lent.
    enclosing: Option<(Option<FilletGeomId>, [Option<OffsetSurfaceSlot>; 2])>,
}

impl FilletSolver {
    /// `SetFilletGeomForSurfaces(pFG)` for a scope, with `off_surfs`, the geometry's
    /// offset surfaces (`pFG->m_pOffSurfs`), moved into the solver so that
    /// `GetSurface` / `SetSurface` reach them as upstream's `GetOffsetSurface` /
    /// `SetOffsetSurface` do. Surfaces already lent by an enclosing scope for the same
    /// geometry stay where they are (`off_surfs` is then the emptied geometry's).
    /// `rollover` is the geometry's tangent-rollover guesses
    /// ([`FilletBrep::rollover_guesses`](crate::blend::fillet_geom::p1::FilletBrep::rollover_guesses)),
    /// which `SmConstantRadiusFS::LoadInitialValues` reads in the scope.
    pub(crate) fn lend_geometry_surfaces(
        &mut self,
        geometry: FilletGeomId,
        rollover: RolloverGuesses,
        off_surfs: &mut [Option<OffsetSurface>; 2],
    ) -> LentSurfaces {
        let saved_fg = self.fg_for_surfaces;
        self.fg_for_surfaces = Some(geometry);
        let saved_rollover = self.lent_rollover.replace((geometry, rollover));
        if self.lent_geometry == Some(geometry) {
            return LentSurfaces {
                saved_fg,
                saved_rollover,
                enclosing: None,
            };
        }
        let mut lent: [Option<OffsetSurfaceSlot>; 2] = [None, None];
        for ((slot, surface), key) in lent.iter_mut().zip(off_surfs.iter_mut()).zip(SELECTED_KEYS) {
            *slot = surface
                .take()
                .map(|surface| OffsetSurfaceSlot { key, surface });
        }
        let enclosing = (
            self.lent_geometry.replace(geometry),
            std::mem::replace(&mut self.lent_surfaces, lent),
        );
        LentSurfaces {
            saved_fg,
            saved_rollover,
            enclosing: Some(enclosing),
        }
    }

    /// Ends a [`FilletSolver::lend_geometry_surfaces`] scope: the geometry's surfaces
    /// (with any `SetSurface` made in the scope) go back to `off_surfs`, and the
    /// enclosing lent surfaces and geometry for the surfaces are restored.
    pub(crate) fn return_geometry_surfaces(
        &mut self,
        saved: LentSurfaces,
        off_surfs: &mut [Option<OffsetSurface>; 2],
    ) {
        if let Some((geometry, surfaces)) = saved.enclosing {
            let lent = std::mem::replace(&mut self.lent_surfaces, surfaces);
            self.lent_geometry = geometry;
            for (surface, slot) in off_surfs.iter_mut().zip(lent) {
                *surface = slot.map(|slot| slot.surface);
            }
        }
        self.fg_for_surfaces = saved.saved_fg;
        self.lent_rollover = saved.saved_rollover;
    }

    /// The tangent-rollover guess `SmConstantRadiusFS::LoadInitialValues` reads for rail
    /// `rail` through the geometry for the surfaces (`GetFilletGeomForSurfaces()` of
    /// type `SM_FG_TANGENT_ROLLOVER`): the other surface's UV of the rail's first
    /// `SM_FV_RAIL_X_EDGEUSE` vertex use with a set intersection point. `None` without
    /// a geometry for the surfaces, for another geometry type, or without such a vertex
    /// use.
    ///
    /// # Errors
    /// `SM_ERR_NULL_POINTER` when the geometry has no rail `rail` (upstream dereferences
    /// it); [`SolidError::InvalidInput`] when the geometry for the surfaces was set
    /// without a lending scope (the solver cannot reach the pseudo B-rep).
    pub(crate) fn tangent_rollover_guess(&self, rail: usize) -> Result<Option<Point2>> {
        const OP: &str = "blend::fillet_solver::surfaces";
        let Some(fg) = self.fg_for_surfaces else {
            return Ok(None);
        };
        let Some((_, guesses)) = self.lent_rollover.filter(|(g, _)| *g == fg) else {
            return Err(SolidError::invalid_input(
                OP,
                "the geometry for the surfaces has not lent its rollover guesses",
            ));
        };
        if !guesses.tangent_rollover {
            return Ok(None);
        }
        match guesses.rails.get(rail).copied().flatten() {
            Some(guess) => Ok(guess),
            // pFG->GetRail(lRailIndex) is NULL: upstream dereferences it
            None => Err(SolidError::null_pointer(OP)),
        }
    }
}

/// Run `f` with `geometry` of `pseudo` as `ops`'s geometry for the surfaces
/// (`SetFilletGeomForSurfaces(pFG)` around a call, the previous one restored after it,
/// also on error): every `GetSurface(rail)` / `SetSurface(rail, ..)` of the call then
/// reads or writes the geometry's offset surface, lent to the solver for the call
/// ([`FilletSolver::lend_geometry_surfaces`]).
///
/// # Errors
/// [`SolidError::NotFound`] for a geometry not in `pseudo`; those of `f`.
pub fn with_geometry_surfaces<T>(
    ops: &mut dyn FilletSolverOps,
    pseudo: &mut crate::blend::fillet_geom::p1::FilletBrep,
    geometry: FilletGeomId,
    f: impl FnOnce(
        &mut dyn FilletSolverOps,
        &mut crate::blend::fillet_geom::p1::FilletBrep,
    ) -> Result<T>,
) -> Result<T> {
    let rollover = pseudo.rollover_guesses(geometry)?;
    let saved = ops.solver_mut().lend_geometry_surfaces(
        geometry,
        rollover,
        &mut pseudo.geom_mut(geometry)?.off_surfs,
    );
    let result = f(&mut *ops, &mut *pseudo);
    let back = pseudo.geom_mut(geometry).map(|g| {
        ops.solver_mut()
            .return_geometry_surfaces(saved, &mut g.off_surfs);
    });
    let value = result?;
    back?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_custom_solver::SurfaceId;
    use crate::blend::fillet_geom::p1::{FilletBrep, FilletGeomId, FilletGeomType, FilletSolverId};
    use crate::math::Point3;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::offset_surface::SS_UNKNOWN;

    fn offset(k: &Kernel, distance: f64) -> OffsetSurface {
        let net = vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, 0.0, NOW),
            ],
        ];
        let nurb = crate::nurbs::srf_basic::Surface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        OffsetSurface::new(
            k,
            distance,
            Surface::from(BSplineSurface::from_nurb(nurb, false)),
            SurfaceId::Free(8),
            SS_UNKNOWN,
        )
        .unwrap()
    }

    fn actual(k: &Kernel) -> ConstantRadiusAssistedFs {
        ConstantRadiusAssistedFs::from_parts(
            1e-5,
            1e-3,
            0.01,
            [0.5, 0.5],
            [Some(offset(k, 99.0)), Some(offset(k, 101.0))],
            [-1.0, -1.0],
            [None, None],
        )
    }

    fn geometry(k: &Kernel) -> FilletGeom {
        let mut geometry = FilletGeom::new(k, None, FilletGeomType::Default).unwrap();
        geometry.solver = Some(FilletSolverId(37));
        geometry.off_surfs = [Some(offset(k, 7.0)), Some(offset(k, 8.0))];
        geometry
    }

    #[test]
    fn selected_objects_and_nested_jacobian_writes_stay_in_actual_geometry() {
        let k = Kernel::default();
        let mut actual = actual(&k);
        actual
            .solver
            .set_fillet_geom_for_surfaces(Some(FilletGeomId(57)));
        let mut geometry = geometry(&k);
        let selected = std::ptr::from_ref(geometry.off_surfs[0].as_ref().unwrap());
        let mut view = GeometrySolver {
            actual: &mut actual,
            geometry: &mut geometry,
        };
        assert!(std::ptr::eq(
            view.solver_surface(0).unwrap().unwrap(),
            selected
        ));
        assert_eq!(
            view.solver_surface(0).unwrap().unwrap().offset_distance(),
            7.0
        );
        let mut load = JacobianLoad {
            rail_index: 0,
            surface1: None,
            surface2: None,
            surf1_offset: 0,
            surf2_offset: 0,
            x: &[0.25, 0.75, 0.25, 0.75],
        };
        let mut equations = 0;
        let mut parameters = 0;
        assert!(view
            .load_jacobian(
                &k,
                &mut equations,
                &mut parameters,
                &mut load,
                &mut [0.0; 3],
                None
            )
            .unwrap());
        assert_eq!(
            view.solver_surface(0).unwrap().unwrap().offset_distance(),
            -0.5
        );
        assert_eq!(
            view.solver_surface(1).unwrap().unwrap().offset_distance(),
            -0.5
        );
        assert_eq!(
            view.actual.solver.surfaces[0]
                .as_ref()
                .unwrap()
                .surface
                .offset_distance(),
            99.0
        );
        assert_eq!(
            view.actual.solver.surfaces[1]
                .as_ref()
                .unwrap()
                .surface
                .offset_distance(),
            101.0
        );
        assert_eq!(
            view.actual.solver.fillet_geom_for_surfaces(),
            Some(FilletGeomId(57))
        );
        assert_eq!(equations, 3);
        assert_eq!(parameters, 4);
    }

    #[test]
    fn actual_geometry_context_is_restored_on_target_lookup_failure() {
        let k = Kernel::default();
        let mut actual = actual(&k);
        let saved = Some(FilletGeomId(57));
        actual.solver.set_fillet_geom_for_surfaces(saved);
        let mut pseudo = FilletBrep::new();
        let g = pseudo.add_geom(geometry(&k));
        let error = pseudo
            .rail_edgeuse_intersect_with_geometry(
                &k,
                g,
                &mut actual,
                &mut Brep::new(),
                0,
                EdgeuseId::from_index(0),
                0.5,
                &mut TsectPnt::default(),
            )
            .unwrap_err();
        assert!(!error.is_unported(), "{error:?}");
        assert_eq!(actual.solver.fillet_geom_for_surfaces(), saved);
        assert_eq!(
            actual.solver.surfaces[0]
                .as_ref()
                .unwrap()
                .surface
                .offset_distance(),
            99.0
        );
        assert_eq!(
            pseudo.geom(g).unwrap().off_surfs[0]
                .as_ref()
                .unwrap()
                .offset_distance(),
            7.0
        );
        assert_eq!(
            pseudo.geom_mut(g).unwrap().off_surfs[0]
                .as_mut()
                .unwrap()
                .surface
                .evaluate_point_extending(&k, Point2::new(0.25, 0.75))
                .unwrap(),
            Point3::new(0.25, 0.75, 7.0)
        );
    }

    fn distance(slot: Option<&OffsetSurfaceSlot>) -> Option<f64> {
        slot.map(|slot| slot.surface.offset_distance())
    }

    #[test]
    fn get_and_set_surface_reach_the_lent_geometry_surfaces() {
        let k = Kernel::default();
        let mut solver = FilletSolver::new(1e-5, 1e-3, 0.01);
        solver.surfaces = [
            Some(OffsetSurfaceSlot {
                key: 1,
                surface: offset(&k, 99.0),
            }),
            None,
        ];
        let mut pseudo = FilletBrep::new();
        let g = pseudo.add_geom(geometry(&k));
        let other = pseudo.add_geom(geometry(&k));
        pseudo.geom_mut(other).unwrap().off_surfs = [Some(offset(&k, 3.0)), None];

        // SetFilletGeomForSurfaces without its surfaces in the solver: refused
        solver.set_fillet_geom_for_surfaces(Some(g));
        assert!(matches!(
            solver.surface(0),
            Err(SolidError::InvalidInput { .. })
        ));
        solver.set_fillet_geom_for_surfaces(None);

        let r = with_geometry_surfaces(&mut solver, &mut pseudo, g, |fs, pseudo| {
            let s = fs.solver_mut();
            assert_eq!(s.fillet_geom_for_surfaces(), Some(g));
            // GetSurface -> m_pFGForSurfaces->GetOffsetSurface
            assert_eq!(distance(s.surface(0)?), Some(7.0));
            assert_eq!(distance(s.surface(1)?), Some(8.0));
            assert!(pseudo.geom(g)?.off_surfs.iter().all(Option::is_none));
            // SetSurface -> m_pFGForSurfaces->SetOffsetSurface
            s.set_surface(
                1,
                Some(OffsetSurfaceSlot {
                    key: 5,
                    surface: offset(&k, 9.0),
                }),
            )?;
            assert_eq!(distance(s.surface(1)?), Some(9.0));
            assert!(s.set_surface(2, None).is_err());
            // the same geometry again (a nested ReCalcFilletGeom/RailEdgeuseIntersect)
            with_geometry_surfaces(fs, pseudo, g, |fs, _| {
                assert_eq!(distance(fs.solver().surface(0)?), Some(7.0));
                assert_eq!(distance(fs.solver().surface(1)?), Some(9.0));
                Ok(())
            })?;
            // another geometry, then back to this one
            with_geometry_surfaces(fs, pseudo, other, |fs, _| {
                assert_eq!(fs.solver().fillet_geom_for_surfaces(), Some(other));
                assert_eq!(distance(fs.solver().surface(0)?), Some(3.0));
                // no surface for the rail: GetOffsetSurface's fallback recurses (UB-204)
                assert!(matches!(
                    fs.solver().surface(1),
                    Err(SolidError::InvalidInput { .. })
                ));
                Ok(())
            })?;
            assert_eq!(fs.solver().fillet_geom_for_surfaces(), Some(g));
            assert_eq!(distance(fs.solver().surface(1)?), Some(9.0));
            Ok(())
        });
        r.unwrap();
        assert_eq!(solver.fillet_geom_for_surfaces(), None);
        assert_eq!(solver.lent_geometry, None);
        assert_eq!(distance(solver.surface(0).unwrap()), Some(99.0));
        assert!(solver.surface(1).unwrap().is_none());
        let back = &pseudo.geom(g).unwrap().off_surfs;
        assert_eq!(
            back[0].as_ref().map(OffsetSurface::offset_distance),
            Some(7.0)
        );
        assert_eq!(
            back[1].as_ref().map(OffsetSurface::offset_distance),
            Some(9.0)
        );
        let back = &pseudo.geom(other).unwrap().off_surfs;
        assert_eq!(
            back[0].as_ref().map(OffsetSurface::offset_distance),
            Some(3.0)
        );
    }

    #[test]
    fn base_solver_geometry_context_resolves_offsets_and_guards_ub_204() {
        let k = Kernel::default();
        let mut solver = FilletSolver::new(1e-5, 1e-3, 0.01);
        let saved = Some(FilletGeomId(91));
        solver.set_fillet_geom_for_surfaces(saved);

        let mut pseudo = FilletBrep::new();
        let g = pseudo.add_geom(geometry(&k));
        let err = pseudo
            .rail_edgeuse_intersect_with_geometry(
                &k,
                g,
                &mut solver,
                &mut Brep::new(),
                0,
                EdgeuseId::from_index(0),
                0.5,
                &mut TsectPnt::default(),
            )
            .unwrap_err();
        assert!(!err.is_unported(), "{err:?}");
        assert_eq!(solver.fillet_geom_for_surfaces(), saved);

        pseudo.geom_mut(g).unwrap().off_surfs[1] = None;
        let ub = pseudo
            .rail_edgeuse_intersect_with_geometry(
                &k,
                g,
                &mut solver,
                &mut Brep::new(),
                0,
                EdgeuseId::from_index(0),
                0.5,
                &mut TsectPnt::default(),
            )
            .unwrap_err();
        assert!(matches!(ub, SolidError::InvalidInput { .. }), "{ub:?}");
        assert_eq!(solver.fillet_geom_for_surfaces(), saved);
    }
}
