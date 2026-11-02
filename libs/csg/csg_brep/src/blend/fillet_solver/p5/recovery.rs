//! Segment walk and topology updates of fillet rollover recovery.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp` (5999–6064, 6342–6504, 7161–7899).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `Recovery::segments` | multi-interval `SmFilletSolver::ProcessRollOver` |
//! | `Recovery::split_rail` | calls [`super::super::p1::split_rail`] |
//! | `Recovery::support_edge` | calls [`super::super::p1::compute_support_edge`] |
//!
//! Deviations: missing callees have typed call boundaries in `RecoveryOps`;
//! their callers, mutations, cleanup and continuation are fully represented.
//! The retained solver supplies its actual virtuals; base data is borrowed only at
//! its original read/write positions. Geometry registration uses the shared pseudo-B-rep id in the solver list. Offset
//! identities are monotonically allocated keys, never addresses. The segment
//! walk has a checked input-derived progress budget (4 times the total interval
//! count times (target edge-use count + 1), plus 8); a broken callee cannot
//! leave a corner walk looping forever.
//! No recursion.

pub(super) use super::super::p1::rails::SupportIntersection;
use super::{
    edge, eu, ignored, p2, Brep, CurveClassification, EdgeId, EdgeuseId, EntityRef, FilletBrep,
    FilletCornerId, FilletCornerType, FilletGeomId, FilletGeomType, Kernel, PointClassType, Result,
    RolloverOwnership, SolidError, SurfOfRevolution, OP,
};
use crate::blend::fillet_classification::FilletClassObject;
use crate::blend::fillet_custom_solver::{OffsetSurface, SurfaceId};
use crate::blend::fillet_geom::p1::{
    CliffHandle, FilStatus, FilletEdgeType, FilletVertex, FilletVertexType,
};
use crate::blend::fillet_geom::p3::SplitCorners;
use crate::blend::fillet_solver::p1::{FilletSolverOffsetsData, OffsetSurfaceSlot};
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::Curve;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::math::{Point3, Vec3, EFF_ZERO_SQRT};
use crate::surface::surface::Surface;
use crate::topology::{FaceId, Vertex, VertexId};

pub(super) struct Recovery<'a> {
    pub solver: &'a mut dyn super::super::p1::FilletSolverOps,
    pub k: &'a Kernel,
    pub target: &'a mut Brep,
    pub fillets: &'a mut FilletBrep,
    /// The executive's corners (`m_vFilletCorners`, `GetFilletCornerOfVertex`).
    pub corners: &'a mut dyn SplitCorners,
    pub ownership: &'a mut dyn RolloverOwnership,
}

struct Rail {
    edge: EdgeId,
    class_edge: EdgeId,
    curve: Curve,
    uv_curve: Curve,
    count: usize,
    index: usize,
    split_last_time: bool,
}

#[derive(Clone, Copy, Default)]
pub(super) struct SideHit {
    pub edge: Option<EdgeId>,
    pub tangent: bool,
    pub trim: bool,
    pub opposite_normals: bool,
}

pub(super) struct SideSearch<'a> {
    pub classification: &'a CurveClassification<FilletClassObject>,
    pub index: &'a mut usize,
    pub rail: EdgeId,
    pub original_curve: &'a Curve,
    pub original_uv_curve: &'a Curve,
    pub original_surface: &'a Surface,
    pub convex: bool,
    pub corner_edgeuse: &'a mut Option<EdgeuseId>,
    pub corner_vertex: &'a mut Option<VertexId>,
    pub tangency_tolerance: f64,
}

// Service boundaries let tests exercise the real caller continuation. Available
// callees delegate to their owning units; only unavailable callees remain stubs.
pub(super) trait RecoveryOps {
    fn full(&mut self, c: &mut Recovery<'_>, geom: FilletGeomId, rail: usize) -> Result<()> {
        c.solver.solver_mut().process_full_roll_over(
            c.k,
            c.target,
            c.fillets,
            geom,
            rail,
            c.ownership,
        )
    }
    fn convex(
        &mut self,
        c: &mut Recovery<'_>,
        edgeuse: EdgeuseId,
        samples: u64,
        step_off: bool,
        tolerance: f64,
    ) -> Result<bool> {
        p2::is_convex_radial_sector(c.k, c.target, edgeuse, samples, step_off, tolerance)
    }
    fn find_side(&mut self, c: &mut Recovery<'_>, search: SideSearch<'_>) -> Result<SideHit> {
        let SideSearch {
            classification,
            index,
            rail,
            original_curve,
            original_uv_curve,
            original_surface,
            convex,
            corner_edgeuse,
            corner_vertex,
            tangency_tolerance,
        } = search;
        // pOrigRailCurve is unused by sm_FindRolloverSideEdge
        let _ = original_curve;
        // the side edge and the three flags are fresh for this pass; the index and the
        // rollover corner are the caller's in/out values
        let mut io = crate::blend::fillet_solver::p2::RolloverSideEdge {
            curr_index: *index,
            rollover_corner_eu: *corner_edgeuse,
            rollover_corner_v: *corner_vertex,
            ..Default::default()
        };
        let edge_of = |o: FilletClassObject| match o.target_entity().ok() {
            Some(EntityRef::Edge(e)) => Some(e),
            _ => None,
        };
        let r = crate::blend::fillet_solver::p2::find_rollover_side_edge(
            c.k,
            c.fillets,
            c.target,
            classification,
            rail,
            original_uv_curve,
            original_surface,
            convex,
            tangency_tolerance,
            &edge_of,
            &mut io,
        );
        // the reference arguments are written in place, also on a failure
        *index = io.curr_index;
        *corner_edgeuse = io.rollover_corner_eu;
        *corner_vertex = io.rollover_corner_v;
        r?;
        Ok(SideHit {
            edge: io.side_edge,
            tangent: io.is_tangential_side_edge,
            trim: io.trim_side_faces,
            opposite_normals: io.opposite_normals,
        })
    }
    fn split_rail(
        &mut self,
        c: &mut Recovery<'_>,
        geom: FilletGeomId,
        rail: EdgeId,
    ) -> Result<(VertexId, EdgeId)>
    where
        Self: Sized,
    {
        c.split_rail(geom, rail, self)
    }
    fn topology_split(&mut self, c: &mut Recovery<'_>, geom: FilletGeomId) -> Result<FilletGeomId> {
        let solver_id = required(c.fillets.geom(geom)?.fillet_solver())?;
        c.fillets.topology_split(
            c.k,
            geom,
            FilletGeomType::Default,
            solver_id,
            c.solver.solver_mut(),
            c.target,
            c.corners,
        )
    }
    fn rail_x_edge(
        &mut self,
        c: &mut Recovery<'_>,
        vertex: VertexId,
        geom: FilletGeomId,
        which: usize,
        rail: EdgeId,
        side: EdgeId,
        guess: Option<Point3>,
    ) -> Result<()> {
        super::super::p3::calc_rail_x_edge_with_ops(
            c.solver, c.k, c.target, c.fillets, vertex, geom, which, rail, side, guess,
        )
    }
    fn tangent(&mut self, c: &mut Recovery<'_>, side: EdgeId) -> Result<bool> {
        super::super::p1::test_manifold_tangent_rollover(
            c.k,
            c.target,
            side,
            c.solver.solver().tangency_tolerance,
        )
        .map(|r| r.0)
    }
    fn make_edge(
        &mut self,
        k: &Kernel,
        fb: &mut FilletBrep,
        start: VertexId,
        end: VertexId,
    ) -> Result<EdgeId> {
        use super::super::p1::rails::{Missing, RailOps};
        Missing.make_edge(k, fb, start, end)
    }
    #[allow(clippy::too_many_arguments)]
    fn cliff(
        &mut self,
        c: &mut Recovery<'_>,
        geom: FilletGeomId,
        rail: EdgeId,
        side: EdgeId,
        adjacent_face: Option<FaceId>,
        trim: bool,
        extension: f64,
    ) -> Result<Option<FilletGeomId>> {
        // `this` is the solver of the geometry being processed
        let solver_id = required(c.fillets.geom(geom)?.fillet_solver())?;
        // upstream passes a NULL pAdjFace through; the side face is dereferenced there
        let side_face = adjacent_face.ok_or_else(|| {
            SolidError::invalid_input(OP, "CreateCliffRollOverSideFG: no adjacent face")
        })?;
        c.solver.solver_mut().create_cliff_roll_over_side_fg(
            c.k, c.target, c.fillets, solver_id, rail, side, side_face, trim, extension,
        )
    }
    fn corner_vertex(
        &mut self,
        c: &mut Recovery<'_>,
        vertex: VertexId,
        geom: FilletGeomId,
    ) -> Result<()> {
        // ProcessRollOver visits this solver's own geometries, so c.solver
        // is the solver identified by geom.fillet_solver().
        let solver_id = required(c.fillets.geom(geom)?.fillet_solver())?;
        // CalcRailIntEdgeuse reads the vertex's corner in the executive's table
        c.fillets.calc_rail_int_edgeuse_with_solver_ops(
            c.k,
            vertex,
            Some(geom),
            c.corners.lookup(),
            solver_id,
            &mut *c.solver,
            c.target,
        )
    }
    fn recalc(
        &mut self,
        c: &mut Recovery<'_>,
        geom: FilletGeomId,
        analytic: bool,
        direction: Option<Vec3>,
    ) -> Result<()> {
        c.fillets
            .re_calc_fillet_geom(c.k, geom, c.solver, c.target, analytic, direction, None)
    }
    fn surface_intersection(
        &mut self,
        k: &Kernel,
        fb: &mut FilletBrep,
        request: SupportIntersection<'_>,
    ) -> Result<Option<Curve>> {
        use super::super::p1::rails::{Missing, RailOps};
        Missing.surface_intersection(k, fb, request)
    }
    fn support(
        &mut self,
        c: &mut Recovery<'_>,
        edge: EdgeId,
        geom: FilletGeomId,
        approx_tol: f64,
        angle_tol: f64,
    ) -> Result<()>
    where
        Self: Sized,
    {
        c.support_edge(edge, geom, approx_tol, angle_tol, self)
    }
    fn trim_rails(&mut self, c: &mut Recovery<'_>, geom: FilletGeomId) -> Result<()> {
        let solver_id = required(c.fillets.geom(geom)?.fillet_solver())?;
        c.fillets
            .trim_rail_curves_with_solver(c.k, geom, solver_id, c.solver.solver(), c.target)
    }
    fn copy_offset(
        &mut self,
        k: &Kernel,
        offset: &OffsetSurface,
    ) -> (Option<OffsetSurface>, Result<()>) {
        match offset.surface.copy(k) {
            Ok(surface) => (
                Some(OffsetSurface {
                    surface,
                    base_id: offset.base_id,
                }),
                Ok(()),
            ),
            Err(error) => (None, Err(error)),
        }
    }
}
pub(super) struct Missing;
impl RecoveryOps for Missing {}

fn get<T>(pair: &[T; 2], which: usize) -> Result<&T> {
    pair.get(which)
        .ok_or_else(|| SolidError::invalid_input(OP, "rail index past 1"))
}
fn get_mut<T>(pair: &mut [T; 2], which: usize) -> Result<&mut T> {
    pair.get_mut(which)
        .ok_or_else(|| SolidError::invalid_input(OP, "rail index past 1"))
}
fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(OP))
}

impl Recovery<'_> {
    fn split_rail(
        &mut self,
        geom: FilletGeomId,
        rail: EdgeId,
        ops: &mut impl RecoveryOps,
    ) -> Result<(VertexId, EdgeId)> {
        super::super::p1::rails::split_rail_with_ops(
            self.k,
            self.fillets,
            self.ownership,
            geom,
            rail,
            &mut RailAdapter(ops),
        )
    }
    fn support_edge(
        &mut self,
        support: EdgeId,
        geom: FilletGeomId,
        approx: f64,
        angle: f64,
        ops: &mut impl RecoveryOps,
    ) -> Result<()> {
        super::super::p1::rails::compute_support_edge_with_ops(
            self.k,
            self.fillets,
            self.ownership,
            support,
            geom,
            approx,
            angle,
            &mut RailAdapter(ops),
        )
    }
    fn rail_copy(&self, geom: FilletGeomId, which: usize) -> Result<Rail> {
        let rail = required(self.fillets.geom(geom)?.rail(which))?;
        let count = required(self.fillets.fillet_edge(rail)?.curve_class())?.size();
        let curve = required(edge::curve(self.fillets.brep(), rail)?)?.copy(self.k)?;
        let primary = required(eu::primary_edgeuse(self.fillets.brep(), rail)?)?;
        let (uv, _) = eu::uv_trim_curve(self.fillets.brep(), primary)?;
        let uv_curve = required(uv)?.copy(self.k)?;
        Ok(Rail {
            edge: rail,
            class_edge: rail,
            curve,
            uv_curve,
            count,
            index: 0,
            split_last_time: false,
        })
    }
    fn start(&self, rail: EdgeId) -> Result<VertexId> {
        required(edge::vertex(self.fillets.brep(), rail)?)
    }
    fn end(&self, rail: EdgeId) -> Result<VertexId> {
        let start = self.start(rail)?;
        required(edge::other_vertex(self.fillets.brep(), rail, Some(start))?)
    }
    fn offset_changed(&self, saved: &FilletSolverOffsetsData, which: usize) -> Result<bool> {
        Ok(self.solver.solver().surface(which)?.map(|s| s.key)
            != get(&saved.surfaces, which)?.as_ref().map(|s| s.key))
    }
    fn new_offset_key(&self) -> Result<u64> {
        self.solver
            .solver()
            .surfaces
            .iter()
            .flatten()
            .chain(self.solver.solver().detached.iter())
            .map(|s| s.key)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| SolidError::invalid_input(OP, "offset identity exhausted"))
    }

    pub(super) fn process(
        &mut self,
        id: FilletGeomId,
        adjusted: &mut Vec<FilletCornerId>,
        ops: &mut impl RecoveryOps,
    ) -> Result<()> {
        if self.fillets.geom(id)?.fillet_geom_type() == FilletGeomType::Blends {
            return Ok(());
        }
        // Preserve rail-0 NER before touching rail 1.
        let r0 = required(self.fillets.geom(id)?.rail(0))?;
        let cc0 = required(self.fillets.fillet_edge(r0)?.curve_class())?;
        let n0 = cc0.size();
        let r1 = required(self.fillets.geom(id)?.rail(1))?;
        let cc1 = required(self.fillets.fillet_edge(r1)?.curve_class())?;
        if n0 == 1 && cc1.size() == 1 {
            let unknown0 = cc0.at(0)?.mid.point_class() == PointClassType::Unknown;
            let unknown1 = cc1.at(0)?.mid.point_class() == PointClassType::Unknown;
            if !unknown0 && !unknown1 {
                return Ok(());
            }
            let corner = required(self.fillets.fillet_vertex(self.start(r0)?)?.fillet_corner())?;
            if !matches!(
                self.corners.corner_type(corner)?,
                FilletCornerType::NByOneClosed | FilletCornerType::OneByOne
            ) {
                return Err(SolidError::sm_err(OP));
            }
            if unknown0 {
                ops.full(self, id, 0)?;
            }
            // The second interval is read after the first call, as upstream's
            // reference can observe mutations performed by full recovery.
            if required(self.fillets.fillet_edge(r1)?.curve_class())?
                .at(0)?
                .mid
                .point_class()
                == PointClassType::Unknown
            {
                ops.full(self, id, 1)?;
            }
            return Ok(());
        }
        self.segments(id, adjusted, ops)
    }

    fn segments(
        &mut self,
        id: FilletGeomId,
        adjusted: &mut Vec<FilletCornerId>,
        ops: &mut impl RecoveryOps,
    ) -> Result<()> {
        let solver_eu = required(self.solver.solver().edgeuse(0)?)?;
        let convex = ops.convex(self, solver_eu, 10, false, 0.0)?;
        let surface = required(self.fillets.geom(id)?.fillet_surface(None)?)?;
        let analytic = if surface.is_kind_of(crate::surface::surface::p2::B_SPLINE_SURFACE_TYPE)? {
            SurfOfRevolution::is_nurb_surface_surf_of_revolution(self.k, surface, 1.0)?.is_some()
        } else {
            false
        };
        let mut rails = [self.rail_copy(id, 0)?, self.rail_copy(id, 1)?];
        let original_surface =
            required(self.fillets.geom(id)?.fillet_surface(None)?)?.copy(self.k)?;
        let mut processing_rollover = false;
        let mut corner_eu = None;
        let mut corner_vertex = None;
        let mut support_edges = Vec::new();
        let mut current_geom = id;
        let mut budget = rails.iter().try_fold(8usize, |b, r| {
            self.target
                .edgeuses()
                .len()
                .checked_add(1)
                .and_then(|fan| r.count.checked_mul(fan))
                .and_then(|n| n.checked_mul(4))
                .and_then(|n| b.checked_add(n))
                .ok_or_else(|| SolidError::invalid_input(OP, "rollover interval count overflow"))
        })?;
        while rails.iter().all(|r| r.index < r.count) {
            budget = budget.checked_sub(1).ok_or_else(|| {
                SolidError::invalid_input(OP, "rollover segment walk did not progress")
            })?;
            let first = rails.iter().all(|r| r.index == 0);
            let mut last = rails.iter().all(|r| r.index == r.count - 1);
            let saved = self.solver.solver().offsets_data()?;
            let mut geom_type = FilletGeomType::Default;
            let mut hits = [SideHit::default(); 2];
            let mut adjacent = [None; 2];
            for (which, rail) in rails.iter_mut().enumerate() {
                // Upstream resets side-edge slots each segment: pPrevSideEdge is
                // therefore NULL, including on a final unclassified segment.
                let previous_side = get(&hits, which)?.edge;
                if rail.count == 1 {
                    continue;
                }
                let cc =
                    required(self.fillets.fillet_edge(rail.class_edge)?.curve_class())?.clone();
                let hit = ops.find_side(
                    self,
                    SideSearch {
                        classification: &cc,
                        index: &mut rail.index,
                        rail: rail.edge,
                        original_curve: &rail.curve,
                        original_uv_curve: &rail.uv_curve,
                        original_surface: &original_surface,
                        convex,
                        corner_edgeuse: &mut corner_eu,
                        corner_vertex: &mut corner_vertex,
                        tangency_tolerance: self.solver.solver().tangency_tolerance,
                    },
                )?;
                *get_mut(&mut hits, which)? = hit;
                let Some(side) = hit.edge else {
                    if rail.index == rail.count - 1 {
                        get_mut(&mut hits, which)?.edge = previous_side;
                        last = true;
                    }
                    continue;
                };
                if cc.at(rail.index)?.mid.point_class() == PointClassType::Unknown {
                    let original_face =
                        required(self.fillets.fillet_edge(rail.edge)?.original_face())?;
                    let face = required(
                        edge::faces(self.target, side, None)?
                            .into_iter()
                            .find(|f| *f != original_face),
                    )?;
                    *get_mut(&mut adjacent, which)? = Some(face);
                    if !hit.tangent || hit.trim {
                        geom_type = FilletGeomType::CliffRollover;
                    } else {
                        geom_type = FilletGeomType::TangentRollover;
                        let mut distance = required(self.solver.solver().surface(which)?)?
                            .surface
                            .offset_distance();
                        if hit.opposite_normals {
                            distance *= -1.0;
                            *get_mut(&mut self.solver.solver_mut().orientations, which)? *= -1.0;
                        }
                        let base = required(self.target.face(face)?.surface.as_ref())?;
                        let mut offset = OffsetSurface::new(
                            self.k,
                            distance,
                            base.clone(),
                            SurfaceId::Face(face),
                            crate::surface::offset_surface::SS_UNKNOWN,
                        )?;
                        offset.set_allow_extension(true);
                        let key = self.new_offset_key()?;
                        self.solver.solver_mut().set_surface(
                            which,
                            Some(OffsetSurfaceSlot {
                                key,
                                surface: offset.clone(),
                            }),
                        )?;
                        self.fillets
                            .geom_mut(current_geom)?
                            .set_offset_surface(which, Some(offset))?;
                    }
                    self.fillets
                        .geom_mut(current_geom)?
                        .set_fillet_geom_type(geom_type);
                }
            }
            let mut params = [0.0; 2];
            for (param, rail) in params.iter_mut().zip(&rails) {
                *param = required(self.fillets.fillet_edge(rail.class_edge)?.curve_class())?
                    .at(rail.index)?
                    .max();
            }
            let mut guess = 0.0;
            if corner_eu.is_none() {
                let [p0, p1] = params;
                if (p0 - p1).abs() < 1.0e-3 {
                    guess = p0;
                    for r in &mut rails {
                        r.index += 1;
                    }
                } else if p0 < p1 {
                    guess = p0;
                    get_mut(&mut rails, 0)?.index += 1;
                } else {
                    guess = p1;
                    get_mut(&mut rails, 1)?.index += 1;
                }
            }
            if hits.iter().all(|h| h.edge.is_none()) {
                if !last {
                    continue;
                }
                if !processing_rollover {
                    return Ok(());
                }
            }
            processing_rollover = true;
            let mut new_geom = None;
            let mut new_rails = [None; 2];
            let mut new_vertices = [None; 2];
            if last {
                for (which, rail) in rails.iter().enumerate() {
                    if self.offset_changed(&saved, which)? {
                        self.fillets
                            .fillet_edge_mut(rail.edge)?
                            .set_original_face(*get(&adjacent, which)?);
                    }
                }
            } else {
                if matches!(
                    geom_type,
                    FilletGeomType::CliffRollover | FilletGeomType::Default
                ) && hits.iter().all(|h| !h.tangent)
                {
                    for (which, hit) in hits.iter().enumerate() {
                        if hit.edge.is_none() {
                            continue;
                        }
                        let (vertex, rail) =
                            ops.split_rail(self, current_geom, get(&rails, which)?.edge)?;
                        *get_mut(&mut new_vertices, which)? = Some(vertex);
                        *get_mut(&mut new_rails, which)? = Some(rail);
                        let geom = self.fillets.geom_mut(current_geom)?;
                        if which == 0 {
                            geom.other_rails1.push(rail);
                        } else {
                            geom.other_rails2.push(rail);
                        }
                    }
                } else {
                    let geom = ops.topology_split(self, current_geom)?;
                    new_geom = Some(geom);
                    self.solver.solver_mut().fillet_geoms.push(geom);
                    for (slot, rail) in new_vertices.iter_mut().zip(&rails) {
                        *slot = Some(self.end(rail.edge)?);
                    }
                    for which in 0..2 {
                        let vertex = required(*get(&new_vertices, which)?)?;
                        let mate = *get(&new_vertices, 1 - which)?;
                        self.fillets.fillet_vertex_mut(vertex)?.set_mate(0, mate);
                    }
                }
                for (which, rail) in rails.iter().enumerate() {
                    if self.offset_changed(&saved, which)? {
                        self.fillets
                            .fillet_edge_mut(rail.edge)?
                            .set_original_face(*get(&adjacent, which)?);
                        let orig = required(get(&saved.surfaces, which)?.as_ref())?;
                        // Bare Copy: upstream ignores its status but uses its output.
                        let (copy, status) = ops.copy_offset(self.k, &orig.surface);
                        ignored(status)?;
                        if let Some(copy) = copy {
                            self.fillets
                                .geom_mut(required(new_geom)?)?
                                .set_offset_surface(which, Some(copy))?;
                        }
                    }
                    let hit = get(&hits, which)?;
                    let Some(side) = hit.edge else {
                        continue;
                    };
                    let vertex = required(*get(&new_vertices, which)?)?;
                    if hit.tangent || corner_eu.is_none() {
                        self.fillets
                            .fillet_vertex_mut(vertex)?
                            .set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
                        if let Some(mate) = *get(&new_vertices, 1 - which)? {
                            self.fillets
                                .fillet_vertex_mut(mate)?
                                .set_fillet_vertex_type(FilletVertexType::Mate);
                        }
                        let point = if corner_eu.is_none() {
                            Some(rail.curve.evaluate_point(self.k, guess)?)
                        } else {
                            None
                        };
                        self.fillets
                            .fillet_vertex_mut(vertex)?
                            .set_point_class(PointClassType::Edge, Some(EntityRef::Edge(side)));
                        ops.rail_x_edge(self, vertex, current_geom, which, rail.edge, side, point)?;
                    } else {
                        self.fillets
                            .fillet_vertex_mut(vertex)?
                            .set_fillet_vertex_type(FilletVertexType::CliffRailXEdge);
                        self.fillets.fillet_vertex_mut(vertex)?.set_point_class(
                            PointClassType::Edgeuse,
                            corner_eu.map(EntityRef::Edgeuse),
                        );
                    }
                }
            }
            if !first {
                for (which, rail) in rails.iter().enumerate() {
                    if !rail.split_last_time {
                        continue;
                    }
                    let vertex = self.start(rail.edge)?;
                    let record = self.fillets.fillet_vertex(vertex)?;
                    if record.fillet_vertex_type() != FilletVertexType::RailXEdgeuse {
                        continue;
                    }
                    let Some(EntityRef::Edge(side)) = record.point_class_object() else {
                        return Err(SolidError::null_pointer(OP));
                    };
                    let guess = self.fillets.vertex_point(vertex)?;
                    if ops.tangent(self, side)? {
                        ops.rail_x_edge(
                            self,
                            vertex,
                            current_geom,
                            which,
                            rail.edge,
                            side,
                            Some(guess),
                        )?;
                    }
                }
            }
            if geom_type == FilletGeomType::CliffRollover {
                for (which, hit) in hits.iter().enumerate() {
                    let Some(side) = hit.edge else {
                        continue;
                    };
                    if corner_eu.is_some() && !hit.trim {
                        // This is target-B-rep geometry, copied into a new pseudo vertex.
                        let point = self.target.vertex(required(corner_vertex)?)?.point;
                        // SmFilletVertex() + SetPoint: retain SmVertex's zero
                        // tolerance rather than Brep::add_vertex's zone default.
                        let end = self.fillets.brep_mut().add_vertex_record(Vertex {
                            point,
                            ..Vertex::default()
                        })?;
                        let mut record = FilletVertex::new(self.k, None);
                        record.set_fillet_vertex_type(FilletVertexType::OnVertex);
                        record.set_status(FilStatus::Processed);
                        self.fillets.insert_fillet_vertex(end, record);
                        let support = ops.make_edge(
                            self.k,
                            self.fillets,
                            end,
                            required(*get(&new_vertices, which)?)?,
                        )?;
                        self.fillets
                            .fillet_edge_mut(support)?
                            .set_fillet_edge_type(FilletEdgeType::OnExtendEdge);
                        support_edges.push(support);
                    }
                    let rail = get(&rails, which)?.edge;
                    self.fillets
                        .fillet_edge_mut(rail)?
                        .set_fillet_edge_type(FilletEdgeType::CliffRail);
                    let extension = required(get(&saved.surfaces, which)?.as_ref())?
                        .surface
                        .offset_distance()
                        .abs();
                    ops.cliff(
                        self,
                        current_geom,
                        rail,
                        side,
                        *get(&adjacent, which)?,
                        hit.trim,
                        extension,
                    )?;
                    break;
                }
            }
            if first || last {
                for which in 0..2 {
                    let rail = required(self.fillets.geom(current_geom)?.rail(which))?;
                    let vertex = if last {
                        self.end(rail)?
                    } else {
                        self.start(rail)?
                    };
                    let record = self.fillets.fillet_vertex(vertex)?;
                    if record.fillet_vertex_type() != FilletVertexType::RailXEdgeuse {
                        continue;
                    }
                    let Some(corner) = record.fillet_corner() else {
                        continue;
                    };
                    if !adjusted.contains(&corner) {
                        adjusted.push(corner);
                    }
                    if geom_type == FilletGeomType::TangentRollover {
                        self.fillets
                            .fillet_vertex_mut(vertex)?
                            .set_status(FilStatus::Unprocessed);
                        if let Some(mate) = self.fillets.fillet_vertex(vertex)?.mate(0) {
                            let record = self.fillets.fillet_vertex_mut(mate)?;
                            record.set_status(FilStatus::Unprocessed);
                            record.set_point_class(
                                PointClassType::Face,
                                get(&adjacent, 1 - which)?.map(EntityRef::Face),
                            );
                        }
                        ops.corner_vertex(self, vertex, current_geom)?;
                        if self.fillets.fillet_vertex(vertex)?.status()
                            == FilStatus::SolverNotConverge
                        {
                            return Err(SolidError::sm_err(OP));
                        }
                    }
                    break;
                }
            }
            if hits.iter().any(|h| h.tangent) || last {
                self.fillets
                    .geom_mut(current_geom)?
                    .set_fillet_surface(None);
                let start = self.start(required(self.fillets.geom(current_geom)?.rail(0))?)?;
                let point = self.fillets.vertex_point(start)?;
                let original_curve = &get(&rails, 0)?.curve;
                let solutions = original_curve.global_point_solve(
                    self.k,
                    &original_curve.natural_interval(),
                    SolverOperation::Minimize,
                    point,
                    EFF_ZERO_SQRT,
                    SolutionRequested::Single,
                    &GlobalPointSolveOptions {
                        handle: Some(CliffHandle::Curve(original_curve)),
                        ..GlobalPointSolveOptions::default()
                    },
                )?;
                let mut direction = None;
                if let Some(solution) = solutions.get(0) {
                    let t = solution
                        .start
                        .parameters
                        .first()
                        .copied()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    if let Some(values) =
                        ignored(original_curve.evaluate(self.k, t, 1, true, true))?
                    {
                        direction = values.get(1).copied();
                    }
                }
                ops.recalc(self, current_geom, analytic, direction)?;
                for &support in &support_edges {
                    ops.support(
                        self,
                        support,
                        current_geom,
                        self.solver.solver().approx_tol_3d,
                        self.solver.solver().ang_tol_rad,
                    )?;
                }
                support_edges.clear();
                ops.trim_rails(self, current_geom)?;
                if geom_type == FilletGeomType::TangentRollover {
                    self.solver.solver_mut().set_offsets_data(&saved)?;
                }
            }
            if last {
                break;
            }
            for rail in &mut rails {
                rail.split_last_time = false;
            }
            if let Some(geom) = new_geom {
                current_geom = geom;
                for (which, rail) in rails.iter_mut().enumerate() {
                    rail.edge = required(self.fillets.geom(geom)?.rail(which))?;
                    rail.split_last_time = true;
                }
            } else {
                for (rail, new) in rails.iter_mut().zip(new_rails) {
                    if let Some(new) = new {
                        rail.edge = new;
                        rail.split_last_time = true;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

struct RailAdapter<'a, T>(&'a mut T);
impl<T: RecoveryOps> super::super::p1::rails::RailOps for RailAdapter<'_, T> {
    fn make_edge(
        &mut self,
        k: &Kernel,
        fb: &mut FilletBrep,
        start: VertexId,
        end: VertexId,
    ) -> Result<EdgeId> {
        self.0.make_edge(k, fb, start, end)
    }
    fn surface_intersection(
        &mut self,
        k: &Kernel,
        fb: &mut FilletBrep,
        request: SupportIntersection<'_>,
    ) -> Result<Option<Curve>> {
        self.0.surface_intersection(k, fb, request)
    }
}
