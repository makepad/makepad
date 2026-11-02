//! Topology construction, profile tagging and incremental sweep control.
//!
//! Upstream: `source/SMLib/src/SmTopologySweep.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`SweepTargets`] | the six optional `DoSweep` input arrays |
//! | [`TopologySweep::tag_profile`] | `SmTopologySweep::TagProfile` |
//! | [`TopologySweep::sweep_vertices_same`] | `DoSweepVerticesSame` |
//! | [`TopologySweep::sweep_vertices_higher`] | `DoSweepVerticesHigher` |
//! | [`TopologySweep::sweep_edges_same`] | `DoSweepEdgesSame` |
//! | [`TopologySweep::sweep_edges_higher`] | `DoSweepEdgesHigher` |
//! | [`TopologySweep::sweep_faces_same`] | `DoSweepFacesSame` / `DoSweepFacesHigher` |
//!
//! Deviations: typed qualified ids and owned curves replace raw pointers. Dependency
//! services are statically dispatched through `SweepAccess`; their defaults defer
//! only the unavailable call. Partial output records remain visible after errors.
//! Repetitions are capped at 4096 to bound input-controlled work. Back-face
//! collection must mark its starting face or return an error, avoiding an infinite
//! retry on a failed traversal. Owned wire B-reps are released on every return;
//! a successful consuming union clears the wire handle before the next repetition.
//! `SweepEdgeHigh` makes a swept face's missing extrusion NURB (a recognised tiny-turn
//! revolution) before its degenerate test, where upstream's lazy
//! `GetOrCreateGwNurbPointer` in `EdgeSweepHigher` already made it in place.

use super::access::{
    EdgeRef, EdgeuseRef, FaceBuild, FaceInput, FaceRef, RegionRef, SweepAccess, VertexRef,
};
use super::geometry::SweepGeometry;
use super::{SweepEntity as E, TopologySweep, OP};
use crate::curve::curve::Curve;
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::attribute::{tag_ids, TagData};
use crate::geom::tol::ZONE_TOL_3D;
use crate::math::Point3;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::SurfaceKind;
use crate::topology::{
    edge, edgeuse::p1 as eu, face::p2 as face, loopuse, vertex, vertexuse, LoopuseStart,
};

fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| SolidError::null_pointer(OP))
}
fn v(v: VertexRef) -> E {
    E::Vertex(v.0, v.1)
}
fn e(e: EdgeRef) -> E {
    E::Edge(e.0, e.1)
}
fn f(f: FaceRef) -> E {
    E::Face(f.0, f.1)
}
fn mapped_vertex(value: Option<E>) -> Result<Option<VertexRef>> {
    match value {
        None => Ok(None),
        Some(E::Vertex(b, i)) => Ok(Some((b, i))),
        _ => Err(SolidError::sm_err(OP)),
    }
}
fn mapped_edge(value: Option<E>) -> Result<Option<EdgeRef>> {
    match value {
        None => Ok(None),
        Some(E::Edge(b, i)) => Ok(Some((b, i))),
        _ => Err(SolidError::sm_err(OP)),
    }
}
fn endpoints(a: &impl SweepAccess, edge: EdgeRef) -> Result<(VertexRef, VertexRef)> {
    let first = required(edge::start_vertex(a.brep(edge.0)?, edge.1)?)?;
    let second = required(edge::other_vertex(a.brep(edge.0)?, edge.1, Some(first))?)?;
    Ok(((edge.0, first), (edge.0, second)))
}
fn set_edge_tolerance(a: &mut impl SweepAccess, edge: EdgeRef, tolerance: f64) -> Result<()> {
    let tolerance = crate::geom::tol::zone_tol3d(a.kernel(edge.0)?, tolerance);
    edge::set_tolerance(a.brep_mut(edge.0)?, edge.1, tolerance, true, true)
}
fn set_vertex_tolerance(a: &mut impl SweepAccess, vertex: VertexRef, tolerance: f64) -> Result<()> {
    let tolerance = crate::geom::tol::zone_tol3d(a.kernel(vertex.0)?, tolerance);
    vertex::set_tolerance(a.brep_mut(vertex.0)?, vertex.1, tolerance, true, true)
}
fn add_unique<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// The six optional target arrays; `None` and an empty array are distinct.
#[derive(Debug, Clone, Default)]
pub struct SweepTargets {
    /// Faces to sweep into solids.
    pub faces_higher: Option<Vec<Option<FaceRef>>>,
    /// Edges to sweep into faces.
    pub edges_higher: Option<Vec<Option<EdgeRef>>>,
    /// Vertices to sweep into edges.
    pub vertices_higher: Option<Vec<Option<VertexRef>>>,
    /// Faces to copy to the final position.
    pub faces_same: Option<Vec<Option<FaceRef>>>,
    /// Edges to copy to the final position.
    pub edges_same: Option<Vec<Option<EdgeRef>>>,
    /// Vertices to copy to the final position.
    pub vertices_same: Option<Vec<Option<VertexRef>>>,
}

impl SweepTargets {
    /// All six pointers are null: the whole-B-rep envelope sweep.
    #[must_use]
    pub fn is_whole_brep(&self) -> bool {
        self.faces_higher.is_none()
            && self.edges_higher.is_none()
            && self.vertices_higher.is_none()
            && self.faces_same.is_none()
            && self.edges_same.is_none()
            && self.vertices_same.is_none()
    }
    /// `sm_UpdateArgumentArrays`: replace each target with its same-dimensional
    /// image when one exists, otherwise retain the previous target.
    pub fn update(&mut self, sweep: &TopologySweep) -> Result<()> {
        fn update<T: Copy>(
            list: &mut Option<Vec<Option<T>>>,
            sweep: &TopologySweep,
            wrap: impl Fn(T) -> E,
            unwrap: impl Fn(E) -> Result<T>,
        ) -> Result<()> {
            if let Some(list) = list {
                for item in list {
                    if let Some(image) = sweep.get_to_same(item.map(&wrap)) {
                        *item = Some(unwrap(image)?);
                    }
                }
            }
            Ok(())
        }
        let face = |image| {
            if let E::Face(b, i) = image {
                Ok((b, i))
            } else {
                Err(SolidError::sm_err(OP))
            }
        };
        let edge = |image| {
            if let E::Edge(b, i) = image {
                Ok((b, i))
            } else {
                Err(SolidError::sm_err(OP))
            }
        };
        let vertex = |image| {
            if let E::Vertex(b, i) = image {
                Ok((b, i))
            } else {
                Err(SolidError::sm_err(OP))
            }
        };
        update(&mut self.faces_higher, sweep, f, face)?;
        update(&mut self.edges_higher, sweep, e, edge)?;
        update(&mut self.vertices_higher, sweep, v, vertex)?;
        update(&mut self.faces_same, sweep, f, face)?;
        update(&mut self.edges_same, sweep, e, edge)?;
        update(&mut self.vertices_same, sweep, v, vertex)
    }
}

/// `SweepEdgeHigh`'s outputs, initialised before topology construction.
#[derive(Debug, Clone, Default)]
pub struct SweptEdge {
    /// Created face.
    pub face: Option<FaceRef>,
    /// Upstream never assigns this output (it stays null).
    pub copied_edge: Option<EdgeRef>,
    /// Edge swept from the start vertex.
    pub start_edge: Option<EdgeRef>,
    /// Edge swept from the end vertex.
    pub end_edge: Option<EdgeRef>,
    /// Original edge at the final position.
    pub far_edge: Option<EdgeRef>,
    /// Start vertex at the final position.
    pub start_vertex: Option<VertexRef>,
    /// End vertex at the final position (upstream leaves null for a closed end edge).
    pub end_vertex: Option<VertexRef>,
}

impl TopologySweep {
    /// `TagProfile`: tags faces and edges, including curve-origin metadata.
    /// Vertex targets are deliberately unused upstream.
    pub fn tag_profile(&self, a: &mut impl SweepAccess, targets: &SweepTargets) -> Result<()> {
        let mut edges = Vec::new();
        if let Some(list) = &targets.edges_higher {
            edges.extend(list.iter().copied());
        }
        if let Some(list) = &targets.edges_same {
            edges.extend(list.iter().copied());
        }
        let faces = targets
            .faces_higher
            .iter()
            .flatten()
            .chain(targets.faces_same.iter().flatten());
        for (i, id) in faces.copied().enumerate() {
            let id = required(id)?;
            for edge in a.face_edges(id)? {
                add_unique(&mut edges, Some((id.0, edge)));
            }
            if a.tag(f(id))?.is_none() {
                a.add_tag(
                    f(id),
                    TagData {
                        primary_id: self.tag_id,
                        primary_ordinal: tag_ids::START_OF_SWEEP
                            .wrapping_sub(i as i64)
                            .wrapping_sub(1),
                        ..TagData::default()
                    },
                )?;
            }
        }
        for (i, id) in edges.into_iter().enumerate() {
            let id = required(id)?;
            if a.tag(e(id))?.is_none() {
                let tag = if let Some(curve) = a.curve_tag(id)? {
                    TagData {
                        primary_id: self.tag_id,
                        primary_ordinal: 0,
                        secondary_id_type: tag_ids::GENERATOR_CURVE,
                        secondary_id: curve.primary_id,
                        secondary_ordinal: curve.primary_ordinal,
                        ..TagData::default()
                    }
                } else {
                    TagData {
                        primary_id: self.tag_id,
                        primary_ordinal: (i as i64).wrapping_add(1),
                        ..TagData::default()
                    }
                };
                a.add_tag(e(id), tag)?;
            }
        }
        Ok(())
    }

    /// `SweepVertexSame` (the caller records the relationship).
    pub fn sweep_vertex_same(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        original: VertexRef,
    ) -> Result<VertexRef> {
        let (point, tolerance) = g.vertex_sweep_same(a.brep(original.0)?, original.1)?;
        let (_, new) = a.make_shell_vertex(region, point)?;
        set_vertex_tolerance(a, new, tolerance)?;
        Ok(new)
    }

    /// `DoSweepVerticesSame`: skip null and previously mapped vertices.
    pub fn sweep_vertices_same(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        vertices: &[Option<VertexRef>],
    ) -> Result<()> {
        for original in vertices.iter().flatten().copied() {
            if self.get_to_same(Some(v(original))).is_some() {
                continue;
            }
            let new = self.sweep_vertex_same(a, g, region, original)?;
            self.set_same(Some(v(original)), Some(v(new)));
        }
        Ok(())
    }

    /// `DoSweepVerticesHigher`: preserve axis vertices or construct a new wire.
    pub fn sweep_vertices_higher(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        vertices: &[Option<VertexRef>],
    ) -> Result<()> {
        for original in vertices.iter().flatten().copied() {
            if self.get_to_higher(Some(v(original))).is_some() {
                continue;
            }
            let Some((curve, tolerance)) =
                g.vertex_sweep_higher(a.brep(original.0)?, original.1)?
            else {
                self.set_same(Some(v(original)), Some(v(original)));
                continue;
            };
            let interval = curve.natural_interval();
            let point = curve.evaluate_point(a.kernel(original.0)?, interval.max())?;
            // bNewVtxNeeded is unconditionally TRUE in the active upstream code.
            let point0 = vertex::point(a.brep(original.0)?, original.1)?;
            let (_, start) = a.make_shell_vertex(region, point0)?;
            let (new_edge, new_vertex) =
                a.make_wire_edge_vertex(region, start, curve, interval, OrientType::Same, point)?;
            set_edge_tolerance(a, new_edge, tolerance)?;
            set_vertex_tolerance(a, new_vertex, tolerance)?;
            self.set_same(Some(v(original)), Some(v(new_vertex)));
            self.set_higher(Some(v(original)), Some(e(new_edge)));
        }
        Ok(())
    }

    /// `SweepEdgeSame`. The existing image is returned separately: upstream still
    /// creates another wire, and `SetSame` keeps the first image.
    pub fn sweep_edge_same(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        original: EdgeuseRef,
        existing: &mut Option<EdgeRef>,
    ) -> Result<EdgeuseRef> {
        let edge_id = required(eu::edge(a.brep(original.0)?, original.1)?)?;
        let old = (original.0, edge_id);
        *existing = mapped_edge(self.get_to_same(Some(e(old))))?;
        let (curve, tolerance) = g.edge_sweep_same(a.kernel(region.0)?, a.brep(old.0)?, old.1)?;
        let vu = required(eu::vertexuse(a.brep(original.0)?, original.1)?)?;
        let first = required(a.brep(original.0)?.vertexuse(vu)?.vertex)?;
        let second = required(edge::other_vertex(a.brep(old.0)?, old.1, Some(first))?)?;
        let orientation = eu::orientation(a.brep(original.0)?, original.1)?;
        let (first, second) = if orientation == OrientType::Opposite {
            (second, first)
        } else {
            (first, second)
        };
        let first = (old.0, first);
        let second = (old.0, second);
        let mut new_first = mapped_vertex(self.get_to_same(Some(v(first))))?;
        let mut new_second = mapped_vertex(self.get_to_same(Some(v(second))))?;
        if new_first.is_none() {
            let new = self.sweep_vertex_same(a, g, region, first)?;
            self.set_same(Some(v(first)), Some(v(new)));
            new_first = Some(new);
        }
        if edge::is_closed(a.brep(old.0)?, old.1)? {
            new_second = new_first;
        } else if new_second.is_none() {
            let new = self.sweep_vertex_same(a, g, region, second)?;
            self.set_same(Some(v(second)), Some(v(new)));
            new_second = Some(new);
        }
        let (Some(new_first), Some(new_second)) = (new_first, new_second) else {
            return Err(SolidError::sm_err(OP));
        };
        let interval = curve.natural_interval();
        let new = a.make_wire_edge(
            region,
            new_first,
            new_second,
            curve,
            interval,
            OrientType::Same,
        )?;
        let peu = required(eu::primary_edgeuse(a.brep(new.0)?, new.1)?)?;
        let result = if orientation == OrientType::Same {
            peu
        } else {
            required(eu::mate(a.brep(new.0)?, peu)?)?
        };
        self.set_same(Some(e(old)), Some(e(new)));
        set_edge_tolerance(a, new, tolerance)?;
        Ok((new.0, result))
    }

    /// `DoSweepEdgesSame`.
    pub fn sweep_edges_same(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        edges: &[Option<EdgeRef>],
    ) -> Result<()> {
        a.brep(region.0)?.region(region.1)?;
        for original in edges.iter().flatten().copied() {
            if self.get_to_same(Some(e(original))).is_some() {
                continue;
            }
            let peu = required(eu::primary_edgeuse(a.brep(original.0)?, original.1)?)?;
            let mut existing = None;
            self.sweep_edge_same(a, g, region, (original.0, peu), &mut existing)?;
            if existing.is_some() {
                return Err(SolidError::sm_err(OP));
            }
        }
        Ok(())
    }

    /// `SweepEdgeHigh`, including mappings and tolerances after face construction.
    pub fn sweep_edge_high(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        original: EdgeRef,
        result: &mut SweptEdge,
    ) -> Result<()> {
        *result = SweptEdge::default();
        a.brep(region.0)?.region(region.1)?;
        let (start, end) = endpoints(a, original)?;
        let swept = g.edge_sweep_higher(a.kernel(region.0)?, a.brep(original.0)?, original.1)?;
        let Some(mut surface) = swept.surface else {
            self.set_same(Some(e(original)), Some(e(original)));
            self.set_same(Some(v(start)), Some(v(start)));
            self.set_same(Some(v(end)), Some(v(end)));
            return Ok(());
        };
        if swept.trim_curves.is_empty() {
            return Err(SolidError::sm_err(OP));
        }
        // `EdgeSweepHigher`'s `CreateNaturalUVTrimCurves` made the NURB of a recognised
        // extrusion in place (`GetOrCreateGwNurbPointer`); here it is still lazy
        if let SurfaceKind::SurfOfExtrusion(s) = surface.kind_mut() {
            if s.base().gw_nurb().is_none() {
                ignore_status(s.make_nurb(a.kernel(region.0)?))?;
            }
        }
        if surface.is_degenerate_curve(a.kernel(region.0)?, 1.0e-4, None, ZONE_TOL_3D / 10.0)?
            || surface.is_degenerate_point(a.kernel(region.0)?, ZONE_TOL_3D / 10.0, None)?
        {
            return Err(SolidError::invalid_input(
                OP,
                "swept surface degenerates to a curve or point",
            ));
        }
        let curve_count = swept.trim_curves.len();
        let domain = surface.natural_uv_domain_with_nurb(a.kernel(region.0)?)?;
        let built = a.make_face(FaceInput {
            region,
            loop_counts: vec![curve_count],
            curves: swept.trim_curves,
            uv_curves: if swept.uv_trim_curves.is_empty() {
                None
            } else {
                Some(swept.uv_trim_curves.into_iter().map(Curve::from).collect())
            },
            orientations: swept.orients,
            loop_points: Vec::new(),
            surface,
            domain,
            orientation: OrientType::Same,
        })?;
        // pStrutC is always NULL: its only assignment is in the upstream #if 0.
        self.set_higher(Some(e(original)), Some(f(built.face)));
        let owner = |index: usize| required(built.curve_edges.get(index).copied().flatten());
        // `s3DCurves.FindElement(curve)`: MakeFaceWithCurves deleted (and nulled the
        // slot of) a degenerate curve or one coincident with an earlier edge: stale
        let live = |index: usize| built.curve_edges.get(index).copied().flatten().is_some();
        if let Some(index) = swept.start_curve {
            set_edge_tolerance(a, owner(index)?, swept.start_curve_tol)?;
        }
        if let Some(index) = swept.far_end_curve.filter(|&i| i < curve_count && live(i)) {
            let new = owner(index)?;
            set_edge_tolerance(a, new, swept.edge_tol)?;
            self.set_same(Some(e(original)), Some(e(new)));
            result.far_edge = Some(new);
        } else {
            self.set_same(Some(e(original)), Some(e(original)));
        }
        if let Some(index) = swept
            .start_vert_curve
            .filter(|&i| i < curve_count && live(i))
        {
            let new = owner(index)?;
            set_edge_tolerance(a, new, swept.start_vert_edge_tol)?;
            let (s, t) = endpoints(a, new)?;
            let p = vertex::point(a.brep(start.0)?, start.1)?;
            let ds = (vertex::point(a.brep(s.0)?, s.1)? - p).length();
            let dt = (vertex::point(a.brep(t.0)?, t.1)? - p).length();
            let image = if s == t {
                s
            } else if ds < dt {
                t
            } else {
                s
            };
            self.set_same(Some(v(start)), Some(v(image)));
            result.start_vertex = Some(image);
            self.set_higher(Some(v(start)), Some(e(new)));
            result.start_edge = Some(new);
        } else {
            self.set_same(Some(v(start)), Some(v(start)));
        }
        if let Some(index) = swept.end_vert_curve.filter(|&i| i < curve_count && live(i)) {
            let new = owner(index)?;
            set_edge_tolerance(a, new, swept.end_vert_edge_tol)?;
            let (s, t) = endpoints(a, new)?;
            let p = vertex::point(a.brep(end.0)?, end.1)?;
            let ds = (vertex::point(a.brep(s.0)?, s.1)? - p).length();
            let dt = (vertex::point(a.brep(t.0)?, t.1)? - p).length();
            let image = if s == t {
                s
            } else if ds < dt {
                t
            } else {
                s
            };
            self.set_same(Some(v(end)), Some(v(image)));
            if s != t {
                result.end_vertex = Some(image);
            }
            self.set_higher(Some(v(end)), Some(e(new)));
            result.end_edge = Some(new);
        } else {
            self.set_same(Some(v(end)), Some(v(end)));
        }
        result.face = Some(built.face);
        Ok(())
    }

    /// `DoSweepEdgesHigher`, copying source tags to the far edge and swept face.
    pub fn sweep_edges_higher(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        edges: &[Option<EdgeRef>],
    ) -> Result<()> {
        for original in edges.iter().flatten().copied() {
            if self.get_to_higher(Some(e(original))).is_some() {
                continue;
            }
            let mut result = SweptEdge::default();
            self.sweep_edge_high(a, g, region, original, &mut result)?;
            if self.do_tagging {
                let tag = a.tag(e(original))?;
                for entity in result.far_edge.map(e).into_iter().chain(result.face.map(f)) {
                    let mut tag = required(tag)?;
                    tag.primary_ordinal =
                        tag.primary_ordinal.wrapping_add(tag_ids::SWEEP_STEP_DELTA);
                    a.add_tag(entity, tag)?;
                }
            }
        }
        Ok(())
    }

    /// `SweepFaceSame`: the complete loop/edge/vertex construction and mapping.
    pub fn sweep_face_same(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        original: Option<FaceRef>,
    ) -> Result<()> {
        let Some(original) = original else {
            return Ok(());
        };
        a.brep(region.0)?.region(region.1)?;
        if self.get_to_same(Some(f(original))).is_some() {
            return Err(SolidError::sm_err(OP));
        }
        let fu = required(face::upward_faceuse(a.brep(original.0)?, original.1)?)?;
        let loops = eu::faceuse_loopuses(a.brep(original.0)?, fu, false, None)?;
        let mut loop_counts = Vec::new();
        let mut loop_points = Vec::new();
        let mut originals = Vec::new();
        let mut orientations = Vec::new();
        let mut curves = Vec::new();
        let mut tolerances = Vec::new();
        for lu in loops {
            let is_vertex = matches!(
                a.brep(original.0)?.loopuse(lu)?.start,
                LoopuseStart::Vertexuse(_)
            );
            let edges = loopuse::edgeuses(a.brep(original.0)?, lu, None, None)?;
            if !is_vertex {
                loop_counts.push(edges.len());
            }
            for edgeuse in edges {
                let edge = required(eu::edge(a.brep(original.0)?, edgeuse)?)?;
                let (curve, tolerance) =
                    g.edge_sweep_same(a.kernel(region.0)?, a.brep(original.0)?, edge)?;
                originals.push((original.0, edge));
                curves.push(curve);
                tolerances.push(tolerance);
                orientations.push(eu::orientation(a.brep(original.0)?, edgeuse)?);
            }
            if is_vertex {
                let uses = loopuse::vertexuses(a.brep(original.0)?, lu, None)?;
                let vu = required(uses.first().copied().flatten())?;
                let vertex = required(a.brep(original.0)?.vertexuse(vu)?.vertex)?;
                let vertex = (original.0, vertex);
                let point = if let Some(new) = mapped_vertex(self.get_to_same(Some(v(vertex))))? {
                    vertex::point(a.brep(new.0)?, new.1)?
                } else {
                    g.vertex_sweep_same(a.brep(vertex.0)?, vertex.1)?.0
                };
                loop_points.push(point);
            }
        }
        let (surface, tolerance) =
            required(g.face_sweep_same(a.kernel(region.0)?, a.brep(original.0)?, original.1)?)?;
        let domain = surface.natural_uv_domain_with_nurb(a.kernel(region.0)?)?;
        let built = a.make_face(FaceInput {
            region,
            loop_counts,
            curves,
            uv_curves: None,
            orientations,
            loop_points,
            surface,
            domain,
            orientation: OrientType::Same,
        })?;
        a.set_face_tolerance(built.face, tolerance)?;
        for (i, old) in originals.into_iter().enumerate() {
            let Some(new) = required(built.curve_edges.get(i).copied())? else {
                continue;
            };
            set_edge_tolerance(a, new, required(tolerances.get(i).copied())?)?;
            self.set_same(Some(e(old)), Some(e(new)));
            let (s, t) = endpoints(a, old)?;
            let (ns, nt) = endpoints(a, new)?;
            self.set_same(Some(v(s)), Some(v(ns)));
            if s != t {
                self.set_same(Some(v(t)), Some(v(nt)));
            }
        }
        self.map_vertex_loops(a, original, &built)?;
        self.set_same(Some(f(original)), Some(f(built.face)));
        if self.do_tagging {
            let mut tag = required(a.tag(f(original))?)?;
            tag.primary_ordinal = tag.primary_ordinal.wrapping_sub(tag_ids::SWEEP_STEP_DELTA);
            a.add_tag(f(built.face), tag)?;
        }
        Ok(())
    }

    fn map_vertex_loops(
        &mut self,
        a: &impl SweepAccess,
        original: FaceRef,
        built: &FaceBuild,
    ) -> Result<()> {
        let old_fu = required(face::upward_faceuse(a.brep(original.0)?, original.1)?)?;
        let new_fu = required(face::upward_faceuse(a.brep(built.face.0)?, built.face.1)?)?;
        let new_loops = eu::faceuse_loopuses(a.brep(built.face.0)?, new_fu, false, None)?;
        let mut first = None;
        for (index, &lu) in new_loops.iter().enumerate() {
            if matches!(
                a.brep(built.face.0)?.loopuse(lu)?.start,
                LoopuseStart::Vertexuse(_)
            ) {
                first = Some(index);
                break;
            }
        }
        let mut offset = 0;
        for old_lu in eu::faceuse_loopuses(a.brep(original.0)?, old_fu, false, None)? {
            if !matches!(
                a.brep(original.0)?.loopuse(old_lu)?.start,
                LoopuseStart::Vertexuse(_)
            ) {
                continue;
            }
            let index = required(first)?
                .checked_add(offset)
                .ok_or_else(|| SolidError::sm_err(OP))?;
            let new_lu = required(new_loops.get(index).copied())?;
            offset += 1;
            let old_uses = loopuse::vertexuses(a.brep(original.0)?, old_lu, None)?;
            let new_uses = loopuse::vertexuses(a.brep(built.face.0)?, new_lu, None)?;
            let old = required(
                a.brep(original.0)?
                    .vertexuse(required(old_uses.first().copied().flatten())?)?
                    .vertex,
            )?;
            let new = required(
                a.brep(built.face.0)?
                    .vertexuse(required(new_uses.first().copied().flatten())?)?
                    .vertex,
            )?;
            self.set_same(Some(v((original.0, old))), Some(v((built.face.0, new))));
        }
        Ok(())
    }

    /// Both `DoSweepFacesSame` and `DoSweepFacesHigher` use this loop upstream.
    pub fn sweep_faces_same(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        faces: &[Option<FaceRef>],
    ) -> Result<()> {
        for face in faces {
            self.sweep_face_same(a, g, region, *face)?;
        }
        Ok(())
    }

    /// `sm_CreateEdgeForBase`. Its two vertex parameters are passed by value upstream;
    /// changing the locals does not expose those new vertices to the caller.
    pub fn create_edge_for_base(
        &self,
        a: &mut impl SweepAccess,
        face: FaceRef,
        curve: Curve,
        uv_curve: Option<Curve>,
        _vertex1: Option<VertexRef>,
        _vertex2: Option<VertexRef>,
    ) -> Result<EdgeRef> {
        let tolerance = a.brep(face.0)?.tolerance();
        let interval = curve.natural_interval();
        let start = curve.evaluate_point(a.kernel(face.0)?, interval.min())?;
        let end = curve.evaluate_point(a.kernel(face.0)?, interval.max())?;
        let mut vs = None;
        let mut vt = None;
        for id in a.face_vertices(face)? {
            let point = vertex::point(a.brep(face.0)?, id)?;
            let ds = (point - start).length();
            let dt = (point - end).length();
            if ds < tolerance && dt < tolerance {
                return Err(SolidError::sm_err(OP));
            }
            if ds < tolerance {
                if vs.is_some() {
                    return Err(SolidError::sm_err(OP));
                }
                vs = Some((face.0, id));
            }
            if dt < tolerance {
                if vt.is_some() {
                    return Err(SolidError::sm_err(OP));
                }
                vt = Some((face.0, id));
            }
        }
        let start = match vs {
            Some(v) => v,
            None => a.make_vertex_loop(face, start)?,
        };
        let end = match vt {
            Some(v) => v,
            None => a.make_vertex_loop(face, end)?,
        };
        a.make_edge_in_face(
            face,
            start,
            end,
            curve,
            uv_curve,
            interval,
            OrientType::Same,
            tolerance,
        )
    }
}

impl TopologySweep {
    /// `DoSweep` / `DoAdvSweep`. Temporary B-reps and both boolean flags are restored
    /// on every return. A wire sub-sweep always has explicit target arrays, so
    /// whole-B-rep recursion can nest only once (independent of input size).
    /// At most 4096 repetitions may be executed per call.
    pub fn do_sweep(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        original: Option<crate::topology::BrepId>,
        destination: Option<RegionRef>,
        targets: &mut SweepTargets,
    ) -> Result<()> {
        let original = required(original)?;
        if self.repetitions > 4096 {
            return Err(SolidError::invalid_input(
                OP,
                "sweep repetitions exceed 4096",
            ));
        }
        let mut destination = match destination {
            Some(r) => r,
            None => (original, a.infinite_region(original)?),
        };
        a.brep(destination.0)?.region(destination.1)?;
        let destination_brep = destination.0;
        let into_infinite = destination.1 == a.infinite_region(destination_brep)?;
        let old_dest = a.kernel(destination_brep)?.context.doing_boolean();
        a.kernel_mut(destination_brep)?
            .context
            .set_doing_boolean(true);
        let old_source = match a.kernel(original) {
            Ok(k) => k.context.doing_boolean(),
            Err(err) => {
                a.kernel_mut(destination_brep)?
                    .context
                    .set_doing_boolean(old_dest);
                return Err(err);
            }
        };
        match a.kernel_mut(original) {
            Ok(kernel) => kernel.context.set_doing_boolean(true),
            Err(err) => {
                a.kernel_mut(destination_brep)?
                    .context
                    .set_doing_boolean(old_dest);
                return Err(err);
            }
        }
        let mut temporary = None;
        let mut wire_temporaries = Vec::new();
        let result = (|| {
            if self.do_merge {
                temporary = Some(a.new_brep(destination_brep)?);
            }
            let whole = targets.is_whole_brep();
            let mut wire = WireSweep::Absent;
            for repetition in 0..self.repetitions {
                if repetition > 0 {
                    targets.update(self)?;
                }
                if into_infinite {
                    destination.1 = a.infinite_region(destination_brep)?;
                }
                let mut region = match temporary {
                    Some(b) => (b, a.infinite_region(b)?),
                    None => destination,
                };
                let closed = g.is_sweep_closed(repetition + 1)?;
                if whole {
                    self.sweep_whole(
                        a,
                        g,
                        original,
                        &mut destination,
                        &mut region,
                        temporary,
                        closed,
                        &mut wire,
                        &mut wire_temporaries,
                    )?;
                    // The upstream early return for an empty silhouette also skips
                    // stitching, merging and all later repetitions.
                    if wire == WireSweep::Empty {
                        return Ok(());
                    }
                } else {
                    self.sweep_selected(a, g, region, closed, targets)?;
                }
                if self.do_merge {
                    let _ = a.set_editing(destination_brep, true)?;
                    let tolerance = a.brep(destination_brep)?.tolerance();
                    a.with_merge(
                        destination_brep,
                        destination_brep,
                        required(temporary)?,
                        tolerance,
                        20.0 * crate::math::PI / 180.0,
                        |a, merge| a.piecewise_merge(merge, true, true),
                    )?;
                } else if self.do_stitching {
                    let tolerance = a.brep(original)?.tolerance();
                    a.stitch(original, self, tolerance, false)?;
                    ignore_status(a.orient(original, true, true))?;
                }
                if let WireSweep::Brep(wire_brep) = wire {
                    let geometry_copy = required(g.copy_for_sweep())?;
                    let mut sweeper = Self::new(super::TopologySweepOptions {
                        stitching: self.do_stitching,
                        merge: self.do_merge,
                        tagging: self.do_tagging,
                        tag_id: self.tag_id,
                        self_intersection: self.do_self_intersection,
                    });
                    let edges = a
                        .wire_edges(wire_brep)?
                        .into_iter()
                        .map(|i| Some((wire_brep, i)))
                        .collect();
                    let vertices = a
                        .shell_vertices(wire_brep)?
                        .into_iter()
                        .map(|i| Some((wire_brep, i)))
                        .collect();
                    let mut wire_targets = SweepTargets {
                        edges_higher: Some(edges),
                        vertices_higher: Some(vertices),
                        ..SweepTargets::default()
                    };
                    let region = (wire_brep, a.infinite_region(wire_brep)?);
                    sweeper.do_sweep(
                        a,
                        &geometry_copy,
                        Some(wire_brep),
                        Some(region),
                        &mut wire_targets,
                    )?;
                    let tolerance = a.brep(original)?.tolerance();
                    a.with_merge(
                        destination_brep,
                        original,
                        wire_brep,
                        tolerance,
                        20.0 * (crate::math::PI / 180.0),
                        SweepAccess::union,
                    )?;
                    // Successful ManifoldBoolean deletes its other B-rep. Do not
                    // delete it again or reuse its stale id on the next repetition.
                    wire_temporaries.retain(|&id| id != wire_brep);
                    wire = WireSweep::Absent;
                }
            }
            Ok(())
        })();
        for id in wire_temporaries.into_iter().rev() {
            a.delete_brep(id);
        }
        if let Some(id) = temporary {
            a.delete_brep(id);
        }
        // The source guard was constructed second and is destroyed first, which
        // matters when original and destination share one context.
        let source_restore = a.kernel_mut(original).map(|kernel| {
            kernel.context.set_doing_boolean(old_source);
        });
        let destination_restore = a.kernel_mut(destination_brep).map(|kernel| {
            kernel.context.set_doing_boolean(old_dest);
        });
        result.and(source_restore).and(destination_restore)
    }

    fn sweep_selected(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        region: RegionRef,
        closed: bool,
        targets: &SweepTargets,
    ) -> Result<()> {
        // This call is unconditional upstream, even when m_bDoTagging is FALSE.
        self.tag_profile(a, targets)?;
        let mut edges = targets.edges_higher.clone().unwrap_or_default();
        let mut vertices = Vec::new();
        if let Some(faces) = &targets.faces_higher {
            for face in faces {
                let face = required(*face)?;
                for edge in a.face_edges(face)? {
                    add_unique(&mut edges, Some((face.0, edge)));
                }
                for vertex in a.face_vertices(face)? {
                    for vu in vertex::vertexuses(a.brep(face.0)?, vertex)? {
                        if vertexuse::is_loop_vertexuse(a.brep(face.0)?, vu)? {
                            add_unique(&mut vertices, Some((face.0, vertex)));
                        }
                    }
                }
            }
        }
        if !vertices.is_empty() {
            self.sweep_vertices_higher(a, g, region, &vertices)?;
        }
        if let Some(faces) = &targets.faces_higher {
            if closed {
                for face in faces {
                    let face = required(*face)?;
                    self.set_same(Some(f(face)), Some(f(face)));
                }
            } else {
                self.sweep_faces_same(a, g, region, faces)?;
            }
        }
        if let Some(faces) = &targets.faces_same {
            self.sweep_faces_same(a, g, region, faces)?;
        }
        if let Some(edges) = &targets.edges_same {
            self.sweep_edges_same(a, g, region, edges)?;
        }
        if let Some(vertices) = &targets.vertices_higher {
            self.sweep_vertices_higher(a, g, region, vertices)?;
        }
        if let Some(vertices) = &targets.vertices_same {
            self.sweep_vertices_same(a, g, region, vertices)?;
        }
        if !edges.is_empty() {
            self.sweep_edges_higher(a, g, region, &edges)?;
        }
        Ok(())
    }

    fn sweep_whole(
        &mut self,
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        original: crate::topology::BrepId,
        destination: &mut RegionRef,
        region: &mut RegionRef,
        temporary: Option<crate::topology::BrepId>,
        closed: bool,
        wire: &mut WireSweep,
        wire_temporaries: &mut Vec<crate::topology::BrepId>,
    ) -> Result<()> {
        let mut bbox = crate::geom::extent3d::Extent3d::new();
        ignore_status(a.bounding_box(original, &mut bbox))?;
        let start = g.start_coord_system(bbox.mid())?;
        let mut frame = crate::geom::axis2_placement::Axis2Placement::IDENTITY;
        ignore_status(frame.set_canonical(Point3::ZERO, start.x_axis(), start.y_axis()))?;
        let inverse = frame.invert();
        let infinite = (original, a.infinite_region(original)?);
        let shells = a.region_shells(infinite)?;
        let mark = a.lock_mark(original, crate::geom::context::ALL_MARKS)?;
        let remove = (|| {
            for shell in shells {
                for face in a.shell_faceuse_faces((original, shell))? {
                    let entity = f((original, face));
                    if a.is_marked(entity, mark)? {
                        continue;
                    }
                    a.mark(entity, mark)?;
                }
            }
            let mut remove = Vec::new();
            for face in a.faces(original)? {
                if !a.is_marked(f((original, face)), mark)? {
                    remove.push(face);
                }
            }
            Ok(remove)
        })();
        a.unlock_mark(original, mark)?;
        let remove = remove?;
        let old = a.set_editing(original, true)?;
        let removal = ignore_status(a.remove_faces(original, &remove));
        a.set_editing(original, old)?;
        removal?;
        // The HCR service retains its complete constructor configuration and
        // ignores ComputeGlobalVisibility's soft status before reading its scene.
        let scene = a.hcr(
            original,
            inverse,
            0.001,
            5.0 * (crate::math::PI / 180.0),
            false,
            false,
            true,
        )?;
        let result = (|| {
            let scene_faces = a.faces(scene.brep)?;
            let wires = a.wire_edges(original)?;
            let vertices = a.shell_vertices(original)?;
            if !wires.is_empty() || !vertices.is_empty() {
                let id = a.new_brep(destination.0)?;
                wire_temporaries.push(id);
                *wire = WireSweep::Brep(id);
                ignore_status(a.copy_edges(original, &wires, id))?;
                ignore_status(a.copy_vertices(original, &vertices, id))?;
            }
            let old = a.set_editing(original, true)?;
            let part = (|| {
                let refresh = *destination == (original, a.infinite_region(original)?);
                a.remove_all_topology(original)?;
                if refresh {
                    destination.1 = a.infinite_region(original)?;
                    if temporary.is_none() {
                        *region = *destination;
                    }
                }
                let tolerance = a.brep(original)?.tolerance();
                a.with_merge(
                    destination.0,
                    original,
                    scene.brep,
                    tolerance,
                    20.0 * crate::math::PI / 180.0,
                    |a, merge| {
                        let mut mates = super::access::BTreeEdgeMap::new();
                        ignore_status(a.copy_faces(
                            scene.brep,
                            &scene_faces,
                            original,
                            merge,
                            &mut mates,
                        ))?;
                        ignore_status(a.transform(original, frame))?;
                        // GetThisMate's null result is retained and skipped by the edge sweep.
                        let silhouette_edges = a.silhouette_edges(scene.brep)?;
                        let edges: Vec<_> = silhouette_edges
                            .iter()
                            .map(|id| mates.get(id).copied().map(|id| (original, id)))
                            .collect();
                        if edges.is_empty() && wires.is_empty() && vertices.is_empty() {
                            *wire = WireSweep::Empty;
                            return Ok(());
                        }
                        if self.do_tagging {
                            // Upstream appends the wire-edge list to itself here.
                            let mut all: Vec<_> =
                                wires.iter().map(|&id| Some((original, id))).collect();
                            all.extend(wires.iter().map(|&id| Some((original, id))));
                            let targets = SweepTargets {
                                edges_higher: Some(all),
                                vertices_higher: Some(
                                    vertices.iter().map(|&id| Some((original, id))).collect(),
                                ),
                                ..SweepTargets::default()
                            };
                            self.tag_profile(a, &targets)?;
                        }
                        self.sweep_edges_higher(a, g, *region, &edges)?;
                        if closed {
                            let faces = a.faces(original)?;
                            ignore_status(a.remove_faces(original, &faces))?;
                        } else if !edges.is_empty() {
                            Self::move_back_faces(a, g, original, destination.0, frame, &edges)?;
                        }
                        Ok(())
                    },
                )
            })();
            a.set_editing(original, old)?;
            part
        })();
        a.delete_brep(scene.brep);
        result
    }

    fn collect_back_faces(
        a: &mut impl SweepAccess,
        start: FaceRef,
        mark: crate::geom::context::MarkType,
        backs: &mut Vec<crate::topology::FaceId>,
    ) -> Result<()> {
        let mut faces = Vec::new();
        ignore_status(a.collect_faces(start, mark, &mut faces))?;
        // CollectFaces marks before walking. Without this invariant, upstream
        // retries the same face indefinitely after ignoring a soft failure.
        if !a.is_marked(f(start), mark)? {
            return Err(SolidError::invalid_input(
                OP,
                "back-face traversal made no progress",
            ));
        }
        backs.extend(faces);
        Ok(())
    }

    fn move_back_faces(
        a: &mut impl SweepAccess,
        g: &SweepGeometry,
        original: crate::topology::BrepId,
        context_of: crate::topology::BrepId,
        frame: crate::geom::axis2_placement::Axis2Placement,
        edges: &[Option<EdgeRef>],
    ) -> Result<()> {
        let first = required(edges.first().copied().flatten())?;
        let mark = a.lock_mark(first.0, crate::geom::context::ALL_MARKS)?;
        let result = (|| {
            for edge in edges {
                a.mark(e(required(*edge)?), mark)?;
            }
            let mut backs = Vec::new();
            loop {
                let mut start = None;
                for edge in edges {
                    let edge = required(*edge)?;
                    let peu = required(eu::primary_edgeuse(a.brep(edge.0)?, edge.1)?)?;
                    let interval = edge::interval(a.brep(edge.0)?, edge.1)?;
                    let mut point = Point3::ZERO;
                    let mut binormal = Point3::ZERO;
                    ignore_status(a.binormal(
                        (edge.0, peu),
                        interval.mid(),
                        false,
                        &mut point,
                        &mut binormal,
                    ))?;
                    let face = if binormal.dot(frame.z_axis()) >= 0.0 {
                        eu::face(a.brep(edge.0)?, peu)?
                    } else {
                        let radial = required(eu::radial(a.brep(edge.0)?, peu)?)?;
                        eu::face(a.brep(edge.0)?, radial)?
                    };
                    let face = (edge.0, required(face)?);
                    if !a.is_marked(f(face), mark)? {
                        start = Some(face);
                        break;
                    }
                }
                let Some(start) = start else {
                    break;
                };
                Self::collect_back_faces(a, start, mark, &mut backs)?;
            }
            let back = a.new_brep(original)?;
            let moved = (|| {
                let tolerance = a.brep(original)?.tolerance();
                a.with_merge(
                    context_of,
                    back,
                    original,
                    tolerance,
                    20.0 * crate::math::PI / 180.0,
                    |a, merge| {
                        a.set_editing(back, true)?;
                        let mut mates = super::access::BTreeEdgeMap::new();
                        ignore_status(a.copy_faces(original, &backs, back, merge, &mut mates))?;
                        let mut remove = Vec::new();
                        for face in backs {
                            if !a.has_lamina_edge((original, face))? {
                                remove.push(face);
                            }
                        }
                        ignore_status(a.remove_faces(original, &remove))?;
                        let mut transform = crate::geom::axis2_placement::Axis2Placement::IDENTITY;
                        match g.sweep_transform() {
                            Ok(frame) => transform = frame,
                            Err(err) => ignore_status::<()>(Err(err))?,
                        }
                        ignore_status(a.transform(back, transform))?;
                        ignore_status(a.merge_brep(original, back))?;
                        Ok(())
                    },
                )
            })();
            a.delete_brep(back);
            moved
        })();
        a.unlock_mark(first.0, mark)?;
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireSweep {
    Absent,
    Empty,
    Brep(crate::topology::BrepId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{attribute::TagData, context::MarkType, extent3d::Extent3d};
    use crate::kernel::Kernel;
    use crate::sweep::topology_sweep::access::{SweepMergeId, SweepScene};
    use crate::sweep::trans_sweep_geometry::TranslationalSweepGeometry;
    use crate::topology::{Brep, BrepId, EdgeId, FaceId, Region, RegionId, ShellId};

    #[derive(Default)]
    struct Access {
        records: crate::topology::topology::NotifyRecords,
        attributes: crate::geom::attribute::AttributeStore,
        kernel: Kernel,
        breps: Vec<Brep>,
        deleted: Vec<BrepId>,
        deleted_merges: usize,
        marked: bool,
        collect_marks: bool,
        mutable_calls: usize,
        fail_mutable_call: Option<usize>,
        complete_whole: bool,
        wire_queries: usize,
        merge_source: Option<BrepId>,
        silhouette_calls: usize,
    }
    impl Access {
        fn new() -> Self {
            let mut access = Self::default();
            access.new_brep(BrepId::from_index(0)).unwrap();
            access
        }
    }
    fn service_object(
        brep: BrepId,
        entity: crate::error::EntityRef,
    ) -> crate::geom::object::ObjectRef {
        use crate::error::EntityRef as R;
        use crate::geom::assert_array::labels;
        let (type_code, index) = match entity {
            R::Brep(id) => (labels::BREP_TYPE, id.index()),
            R::Region(id) => (labels::REGION_TYPE, id.index()),
            R::Shell(id) => (labels::SHELL_TYPE, id.index()),
            R::Face(id) => (labels::FACE_TYPE, id.index()),
            R::Loop(id) => (labels::LOOP_TYPE, id.index()),
            R::Edge(id) => (labels::EDGE_TYPE, id.index()),
            R::Vertex(id) => (labels::VERTEX_TYPE, id.index()),
            R::Faceuse(id) => (labels::FACEUSE_TYPE, id.index()),
            R::Loopuse(id) => (labels::LOOPUSE_TYPE, id.index()),
            R::Edgeuse(id) => (labels::EDGEUSE_TYPE, id.index()),
            R::Vertexuse(id) => (labels::VERTEXUSE_TYPE, id.index()),
        };
        crate::geom::object::ObjectRef {
            type_code,
            key: crate::geom::attribute::ObjectKey(
                1_000_000
                    + u64::from(brep.index()) * 100_000
                    + (type_code as u64) * 1000
                    + u64::from(index),
            ),
        }
    }
    impl SweepAccess for Access {
        fn brep(&self, id: BrepId) -> Result<&Brep> {
            self.breps
                .get(id.index() as usize)
                .ok_or(SolidError::sm_err(OP))
        }
        fn brep_mut(&mut self, id: BrepId) -> Result<&mut Brep> {
            self.breps
                .get_mut(id.index() as usize)
                .ok_or(SolidError::sm_err(OP))
        }
        fn kernel(&self, _: BrepId) -> Result<&Kernel> {
            Ok(&self.kernel)
        }
        fn kernel_mut(&mut self, _: BrepId) -> Result<&mut Kernel> {
            self.mutable_calls += 1;
            if Some(self.mutable_calls) == self.fail_mutable_call {
                return Err(SolidError::sm_err(OP));
            }
            Ok(&mut self.kernel)
        }
        fn with_brep_mut<T>(
            &mut self,
            id: BrepId,
            body: impl FnOnce(
                &mut Kernel,
                &mut Brep,
                &dyn Fn(FaceId) -> crate::geom::object::ObjectRef,
                &mut edge::NotifyEnv<'_>,
            ) -> Result<T>,
        ) -> Result<T> {
            let brep = self
                .breps
                .get_mut(id.index() as usize)
                .ok_or_else(|| SolidError::null_pointer("sweep test model"))?;
            let face_objects =
                |face: FaceId| service_object(id, crate::error::EntityRef::Face(face));
            let key_of = |entity| service_object(id, entity);
            let mut env = edge::NotifyEnv {
                records: &mut self.records,
                attributes: &mut self.attributes,
                context: None,
                key_of: &key_of,
            };
            body(&mut self.kernel, brep, &face_objects, &mut env)
        }
        fn with_breps_mut<T>(
            &mut self,
            source: BrepId,
            destination: BrepId,
            body: impl FnOnce(
                &mut Kernel,
                &mut Brep,
                &mut Brep,
                &dyn Fn(crate::EntityRef) -> crate::geom::object::ObjectRef,
                &dyn Fn(FaceId) -> crate::geom::object::ObjectRef,
                Option<&mut crate::geom::context::Context>,
                &mut edge::NotifyEnv<'_>,
            ) -> Result<T>,
        ) -> Result<T> {
            let si = source.index() as usize;
            let di = destination.index() as usize;
            if si == di {
                return Err(SolidError::invalid_input(
                    "sweep test model",
                    "aliased B-reps",
                ));
            }
            let (left, right) = self
                .breps
                .split_at_mut_checked(si.max(di))
                .ok_or_else(|| SolidError::null_pointer("sweep test model"))?;
            let (src, dst) = if si < di {
                (left.get_mut(si), right.first_mut())
            } else {
                (right.first_mut(), left.get_mut(di))
            };
            let src = src.ok_or_else(|| SolidError::null_pointer("sweep test model"))?;
            let dst = dst.ok_or_else(|| SolidError::null_pointer("sweep test model"))?;
            let face_objects =
                |face: FaceId| service_object(destination, crate::error::EntityRef::Face(face));
            let key_of = |entity| service_object(destination, entity);
            let mut env = edge::NotifyEnv {
                records: &mut self.records,
                attributes: &mut self.attributes,
                context: None,
                key_of: &key_of,
            };
            body(
                &mut self.kernel,
                src,
                dst,
                &|e| service_object(source, e),
                &face_objects,
                None,
                &mut env,
            )
        }
        fn with_brep_notifications<T>(
            &mut self,
            id: BrepId,
            body: impl FnOnce(&mut Brep, &mut edge::NotifyEnv<'_>) -> Result<T>,
        ) -> Result<T> {
            let brep = self
                .breps
                .get_mut(id.index() as usize)
                .ok_or_else(|| SolidError::null_pointer("sweep test model"))?;
            let key_of = |entity| service_object(id, entity);
            let mut env = edge::NotifyEnv {
                records: &mut self.records,
                attributes: &mut self.attributes,
                context: Some(&mut self.kernel.context),
                key_of: &key_of,
            };
            body(brep, &mut env)
        }
        fn retain_hcr(&mut self, _: BrepId, _: crate::blend::hcr::Hcr) -> Result<BrepId> {
            Err(SolidError::invalid_input(
                "sweep test model",
                "fixture has no retained HCR service",
            ))
        }
        fn hcr_session(&self, _: BrepId) -> Result<&crate::blend::hcr::Hcr> {
            Err(SolidError::invalid_input(
                "sweep test model",
                "fixture has no retained HCR service",
            ))
        }
        fn new_brep(&mut self, _: BrepId) -> Result<BrepId> {
            let id = BrepId::from_index(self.breps.len() as u32);
            let mut brep = Brep::new();
            let region = brep.add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })?;
            brep.set_infinite_region(Some(region));
            self.breps.push(brep);
            Ok(id)
        }
        fn delete_brep(&mut self, id: BrepId) {
            self.deleted.push(id);
        }
        fn tag(&self, _: E) -> Result<Option<TagData>> {
            Ok(None)
        }
        fn add_tag(&mut self, _: E, _: TagData) -> Result<()> {
            Ok(())
        }
        fn curve_tag(&self, _: EdgeRef) -> Result<Option<TagData>> {
            Ok(None)
        }
        fn set_editing(&mut self, _: BrepId, _: bool) -> Result<bool> {
            Ok(false)
        }
        fn delete_merge(&mut self, _: SweepMergeId) {
            self.deleted_merges += 1;
        }
        fn new_merge(
            &mut self,
            _: BrepId,
            _: BrepId,
            source: BrepId,
            _: f64,
            _: f64,
        ) -> Result<SweepMergeId> {
            self.merge_source = Some(source);
            Ok(SweepMergeId(0))
        }
        fn merge_operands(&self, _: SweepMergeId) -> Result<(BrepId, BrepId)> {
            Err(SolidError::invalid_input(
                OP,
                "fixture has no retained merge",
            ))
        }
        fn merge_relations(
            &mut self,
            _: SweepMergeId,
        ) -> Result<&mut crate::boolean::topology_intersector::p1::Relations> {
            Err(SolidError::invalid_input(
                OP,
                "fixture has no retained merge",
            ))
        }
        fn piecewise_merge(&mut self, _: SweepMergeId, _: bool, _: bool) -> Result<BrepId> {
            Err(SolidError::invalid_input(
                OP,
                "fixture has no piecewise service",
            ))
        }
        fn lock_mark(&mut self, _: BrepId, _: i32) -> Result<MarkType> {
            Ok(MarkType::Mark)
        }
        fn unlock_mark(&mut self, _: BrepId, _: MarkType) -> Result<()> {
            Ok(())
        }
        fn is_marked(&self, _: E, _: MarkType) -> Result<bool> {
            Ok(self.marked)
        }
        fn mark(&mut self, _: E, _: MarkType) -> Result<()> {
            self.marked = true;
            Ok(())
        }
        fn collect_faces(
            &mut self,
            face: FaceRef,
            _: MarkType,
            faces: &mut Vec<FaceId>,
        ) -> Result<()> {
            self.marked = self.collect_marks;
            faces.push(face.1);
            // Upstream ignores soft errors, but retains partial outputs/marks.
            Err(SolidError::sm_err(OP))
        }
        fn infinite_region(&mut self, _: BrepId) -> Result<RegionId> {
            Ok(RegionId::from_index(0))
        }
        fn bounding_box(&mut self, _: BrepId, output: &mut Extent3d) -> Result<()> {
            *output = Extent3d::from_point(Point3::X);
            Ok(())
        }
        fn region_shells(&mut self, _: RegionRef) -> Result<Vec<ShellId>> {
            Ok(Vec::new())
        }
        fn faces(&mut self, _: BrepId) -> Result<Vec<FaceId>> {
            Ok(Vec::new())
        }
        fn remove_faces(&mut self, _: BrepId, _: &[FaceId]) -> Result<()> {
            Ok(())
        }
        fn hcr(
            &mut self,
            brep: BrepId,
            _: crate::geom::axis2_placement::Axis2Placement,
            _: f64,
            _: f64,
            _: bool,
            _: bool,
            _: bool,
        ) -> Result<SweepScene> {
            Ok(SweepScene {
                brep: self.new_brep(brep)?,
            })
        }
        fn wire_edges(&mut self, brep: BrepId) -> Result<Vec<EdgeId>> {
            assert!(!self.deleted.contains(&brep), "access to deleted B-rep");
            let edges = if self.wire_queries == 0 {
                vec![EdgeId::from_index(0)]
            } else {
                Vec::new()
            };
            self.wire_queries += 1;
            Ok(edges)
        }
        fn copy_edges(&mut self, _: BrepId, _: &[EdgeId], _: BrepId) -> Result<Vec<EdgeId>> {
            if self.complete_whole {
                Ok(Vec::new())
            } else {
                Err(SolidError::Unported {
                    unit: "topology.brep_p5",
                    upstream: "SmBrep::CreateWireEdgeFromCurve",
                })
            }
        }
        fn copy_vertices(
            &mut self,
            _: BrepId,
            _: &[crate::topology::VertexId],
            _: BrepId,
        ) -> Result<Vec<crate::topology::VertexId>> {
            Ok(Vec::new())
        }
        fn remove_all_topology(&mut self, _: BrepId) -> Result<()> {
            Ok(())
        }
        fn copy_faces(
            &mut self,
            _: BrepId,
            _: &[FaceId],
            _: BrepId,
            _: SweepMergeId,
            mates: &mut crate::sweep::topology_sweep::access::BTreeEdgeMap,
        ) -> Result<()> {
            mates.insert(EdgeId::from_index(0), EdgeId::from_index(0));
            Ok(())
        }
        fn transform(
            &mut self,
            _: BrepId,
            _: crate::geom::axis2_placement::Axis2Placement,
        ) -> Result<()> {
            Ok(())
        }
        fn silhouette_edges(&mut self, _: BrepId) -> Result<Vec<EdgeId>> {
            self.silhouette_calls += 1;
            Ok(if self.silhouette_calls > 1 {
                vec![EdgeId::from_index(0)]
            } else {
                Vec::new()
            })
        }
        fn union(&mut self, _: SweepMergeId) -> Result<BrepId> {
            let source = self.merge_source.take().unwrap();
            assert!(
                !self.deleted.contains(&source),
                "source deleted before union"
            );
            self.delete_brep(source);
            Ok(BrepId::from_index(0))
        }
        fn shell_vertices(&mut self, _: BrepId) -> Result<Vec<crate::topology::VertexId>> {
            Ok(Vec::new())
        }
    }
    fn geometry() -> SweepGeometry {
        SweepGeometry::Translation(TranslationalSweepGeometry::new(Point3::Z))
    }
    #[test]
    fn error_releases_scene_and_wire_breps_and_restores_shared_context() {
        let mut access = Access::new();
        let mut sweep = TopologySweep::default();
        let result = sweep.do_sweep(
            &mut access,
            &geometry(),
            Some(BrepId::from_index(0)),
            None,
            &mut SweepTargets::default(),
        );
        assert!(matches!(
            result,
            Err(SolidError::Unported {
                unit: "topology.brep_p5",
                ..
            })
        ));
        assert_eq!(
            access.deleted,
            vec![BrepId::from_index(1), BrepId::from_index(2)]
        );
        assert!(!access.kernel.context.doing_boolean());
    }
    #[test]
    fn failed_second_context_guard_restores_first() {
        let mut access = Access::new();
        access.fail_mutable_call = Some(2);
        let result = TopologySweep::default().do_sweep(
            &mut access,
            &geometry(),
            Some(BrepId::from_index(0)),
            None,
            &mut SweepTargets::default(),
        );
        assert!(result.is_err());
        assert!(!access.kernel.context.doing_boolean());
    }
    #[test]
    fn extreme_repetitions_fail_before_allocating_or_mutating_context() {
        let mut access = Access::new();
        let mut sweep = TopologySweep {
            repetitions: u64::MAX,
            ..TopologySweep::default()
        };
        let result = sweep.do_sweep(
            &mut access,
            &geometry(),
            Some(BrepId::from_index(0)),
            None,
            &mut SweepTargets {
                vertices_same: Some(Vec::new()),
                ..SweepTargets::default()
            },
        );
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
        assert_eq!(access.breps.len(), 1);
        assert_eq!(access.mutable_calls, 0);
    }
    #[test]
    fn ignored_collect_error_cannot_retry_an_unmarked_face_forever() {
        let mut access = Access::new();
        let mut backs = Vec::new();
        let face = (BrepId::from_index(0), FaceId::from_index(0));
        assert!(
            TopologySweep::collect_back_faces(&mut access, face, MarkType::Mark, &mut backs)
                .is_err()
        );
        assert!(backs.is_empty());
        access.collect_marks = true;
        TopologySweep::collect_back_faces(&mut access, face, MarkType::Mark, &mut backs).unwrap();
        assert_eq!(backs, vec![face.1]);
    }
    #[test]
    fn consuming_union_is_not_deleted_twice_or_reused_on_next_repetition() {
        let mut access = Access::new();
        access.complete_whole = true;
        let mut sweep = TopologySweep {
            do_stitching: false,
            repetitions: 2,
            ..TopologySweep::default()
        };
        sweep.set_higher(
            Some(e((BrepId::from_index(0), EdgeId::from_index(0)))),
            Some(f((BrepId::from_index(0), FaceId::from_index(0)))),
        );
        let rotation = SweepGeometry::Rotation(
            crate::sweep::rotational_sweep_geometry::RotationalSweepGeometry::new(
                Point3::ZERO,
                Point3::Z,
                180.0,
                1e-5,
            ),
        );
        sweep
            .do_sweep(
                &mut access,
                &rotation,
                Some(BrepId::from_index(0)),
                None,
                &mut SweepTargets::default(),
            )
            .unwrap();
        assert_eq!(
            access.deleted,
            vec![
                BrepId::from_index(1),
                BrepId::from_index(2),
                BrepId::from_index(3)
            ]
        );
        assert!(!access.kernel.context.doing_boolean());
    }
    #[test]
    fn tolerance_helpers_apply_zone_conversion_in_the_entity_context() {
        use crate::topology::{Edge, Edgeuse, Vertex, Vertexuse};
        for (input, expected) in [
            (-0.25, 0.25),
            (crate::geom::tol::USE_DEFAULT, 0.125),
            (0.03125, 0.03125),
        ] {
            let mut access = Access::new();
            access.kernel.tolerances.zone_tol_3d = 0.125;
            let b = BrepId::from_index(0);
            let brep = access.brep_mut(b).unwrap();
            let v1 = brep.add_vertex_record(Vertex::default()).unwrap();
            let v2 = brep.add_vertex_record(Vertex::default()).unwrap();
            let v3 = brep.add_vertex_record(Vertex::default()).unwrap();
            let vu1 = brep
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(v1),
                    ..Vertexuse::default()
                })
                .unwrap();
            let vu2 = brep
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(v2),
                    ..Vertexuse::default()
                })
                .unwrap();
            let e = EdgeId::from_index(0);
            let eu1 = brep
                .add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    vertexuse: Some(vu1),
                    ..Edgeuse::default()
                })
                .unwrap();
            let eu2 = brep
                .add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    vertexuse: Some(vu2),
                    ..Edgeuse::default()
                })
                .unwrap();
            brep.add_edge_record(Edge {
                vertices: [v1, v2],
                edgeuses: vec![eu1, eu2],
                tolerance: 0.0,
                ..Edge::new([v1, v2])
            })
            .unwrap();
            set_edge_tolerance(&mut access, (b, e), input).unwrap();
            set_vertex_tolerance(&mut access, (b, v3), input).unwrap();
            let brep = access.brep(b).unwrap();
            assert_eq!(brep.edge(e).unwrap().tolerance, expected);
            for vertex in [v1, v2, v3] {
                assert_eq!(brep.vertex(vertex).unwrap().tolerance, expected);
            }
        }
    }
    #[test]
    fn merge_notifications_are_released_after_an_error() {
        let mut access = Access::new();
        let b = BrepId::from_index(0);
        let result: Result<()> =
            access.with_merge(b, b, b, 1e-5, 0.1, |_, _| Err(SolidError::sm_err(OP)));
        assert!(result.is_err());
        assert_eq!(access.deleted_merges, 1);
    }
    #[test]
    fn stitch_default_calls_port_and_restores_caller_state() {
        let mut access = Access::new();
        let owner = BrepId::from_index(0);
        let mut sweep = TopologySweep::new(super::super::TopologySweepOptions::default());
        for editing in [false, true] {
            for doing_boolean in [false, true] {
                access.brep_mut(owner).unwrap().set_editing_enabled(editing);
                access.kernel.context.set_doing_boolean(doing_boolean);
                let result = access.stitch(owner, &mut sweep, 0.125, false);
                // query.brep (CalculateBoundingBox) is ported: the stitch completes.
                assert!(result.is_ok(), "stitch result: {result:?}");
                assert_eq!(access.brep(owner).unwrap().editing_enabled(), editing);
                assert_eq!(access.kernel.context.doing_boolean(), doing_boolean);
            }
        }
    }
}
