//! B-rep region/orientation queries and vertex/edge construction.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`find_infinite_region`] | `SmBrep::FindInfiniteRegion` |
//! | [`orient_sheet_surfaces`] | `SmBrep::OrientSheetSurfaces` |
//! | [`make_shell_vertex`], [`make_shell_vertex_with_kernel_context`] | `SmBrep::MakeShellVertex` |
//! | [`make_vertex_loop`] | `SmBrep::MakeVertexLoop` |
//! | [`make_vertex_split_edge`], [`make_vertex_split_edge_with_kernel_context`] | `SmBrep::MakeVertexSplitEdge` |
//! | [`make_edge_in_face_with_vu`], [`drop_edge_curve`] | `SmBrep::MakeEdgeInFaceWithVU` |
//!
//! Deviations: typed ids replace pointers; notification storage and geometry
//! identities are caller-owned. Kernel-context entrypoints bind those stores to
//! the actual kernel context at notification sites; legacy environments retain
//! their external context, including null. Editing state is part 1's `Brep::editing_enabled`.
//! Ordinary split failures retain the nearby vertex alongside their status. All six
//! owned bodies are present; individual dependency calls defer to their owners.
//! `FindInfiniteRegion` retries a failed `SmShell::RayFire` as upstream, but propagates
//! errors that have no upstream status (unported, stale id, pending preparation, B-rep
//! cache owner/handle resolution) instead of treating them as a missed ray.
//! Part 1's `add_*_record` appends a face, edge or vertex to its owning list at
//! allocation; native `PostInsert`s later, with no same-kind allocation in between,
//! so the list order matches, but a record allocated before a dependency error stays
//! listed (native leaks it unlisted). Deleted loops and loop uses are reset in place
//! (arena records cannot be freed), as part 1 deletes vertex uses. Topology record
//! construction uses the shared arena model (no allocator/debug notifications).
//! Owned geometry arguments are dropped on Rust errors. The shell-to-no-shell
//! retry is a two-pass loop (at most one retry), so there is no recursion. Inner
//! loop transfer errors must decrease the source list length, otherwise return an
//! error instead of looping forever. Incomplete edgeuse pairs and native null
//! dereferences return `InvalidInput`; nonfinite split parameters retain native
//! refusal/output behavior. `OrientSheetSurfaces` preserves the native final TRUE
//! status (1), distinct from its early `SM_SUCCESS` (0). Mutation success paths
//! remain uncertified while part 2 and face notification dependencies defer;
//! passing refusal/projection comparisons do not certify those paths.
//! Opt-in ([`UpstreamFixes::new_edge_vertex_tolerance_covers_gap`](crate::kernel::UpstreamFixes::new_edge_vertex_tolerance_covers_gap),
//! WR-469): `MakeEdgeInFaceWithVU` follows the source fork's fix: after
//! `RefreshTolerance`, which keeps a vertex's tolerance as it was, each end vertex's
//! tolerance grows to twice its gap to the new edge's end (only if larger), as
//! `UpdateVertexEdgeIntervals` sets it. By default it leaves gaps up to the full
//! tolerance, as the pinned C++ does, which `AssertValid` refuses above half of it.
//! See `plan/upstream-issues/WR-469--make-edge-in-face-vertex-tolerance-gap.md`.
//! Opt-in ([`UpstreamFixes::boolean_skips_faces_apart`](crate::kernel::UpstreamFixes::boolean_skips_faces_apart),
//! WR-527, #1527): on two planes, `MakeEdgeInFaceWithVU` leaves an inner loop whose
//! test point lies outside the new face's outer loop box (at the retry tolerance)
//! in place without classifying it, as the fork's source fork@60b1c6a.

use crate::curve::curve::Curve;
use crate::error::{ignore_status, Result, SolidError, SM_ERR};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::object::NotifyOperation;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO_PARAM};
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::BrepQuery;
use crate::query::brep::{self as qbrep, BoundingBoxOptions};
use crate::surface::surface::Surface;
use crate::topology::edge;
use crate::topology::vertex;
use crate::topology::vertexuse;
use crate::topology::{
    Brep, EdgeId, FaceId, Loop, LoopId, Loopuse, LoopuseStart, RegionId, ShellId, Vertex, VertexId,
    Vertexuse, VertexuseId, VertexuseOwner,
};
use crate::EntityRef;

const OP: &str = "topology::brep::p9";

/// Identity transport at the actual curve construction event. Legacy callbacks
/// keep their existing domain; Actual receives the original object and live stores.
#[derive(Clone, Copy)]
pub enum CurveIdentity<'a> {
    /// Existing infallible edge/type resolver.
    Legacy(&'a dyn Fn(EdgeId, i64) -> crate::geom::object::ObjectRef),
    /// Existing checked edge/type resolver.
    Checked(&'a dyn Fn(EdgeId, i64) -> Result<crate::geom::object::ObjectRef>),
    /// Checked issuer with the real curve, including a not-yet-installed copy.
    Actual(
        &'a dyn Fn(
            EdgeId,
            &mut Curve,
            &mut edge::NotifyEnv<'_>,
        ) -> Result<crate::geom::object::ObjectRef>,
    ),
}
impl CurveIdentity<'_> {
    pub(crate) fn resolve(
        self,
        edge: EdgeId,
        curve: &mut Curve,
        env: &mut edge::NotifyEnv<'_>,
    ) -> Result<crate::geom::object::ObjectRef> {
        match self {
            Self::Legacy(resolve) => Ok(resolve(edge, curve.type_code())),
            Self::Checked(resolve) => resolve(edge, curve.type_code()),
            Self::Actual(issue) => issue(edge, curve, env),
        }
    }
}

/// Issue the real Curve identity through the caller's retained notification context.
/// No topology key or temporary kernel context substitutes for the geometry slot.
/// # Errors
/// Missing notification context or a geometry identity owned by another context.
pub fn issue_curve_identity(
    _edge: EdgeId,
    curve: &mut Curve,
    env: &mut edge::NotifyEnv<'_>,
) -> Result<crate::geom::object::ObjectRef> {
    let type_code = curve.type_code();
    let context = env.context.as_deref_mut().ok_or_else(|| {
        SolidError::invalid_input(
            OP,
            "curve identity requires the actual notification context",
        )
    })?;
    let key = context.geometry_key(curve.notify_identity_slot())?;
    Ok(crate::geom::object::ObjectRef { type_code, key })
}

/// Outputs of `MakeVertexSplitEdge`, including the endpoint on ordinary failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SplitEdgeOutput {
    /// Whether the operation split the edge (`SM_SUCCESS`). False means `SM_ERR`.
    pub split: bool,
    /// Reused edge and new edge, assigned only on successful completion.
    pub edges: Option<[EdgeId; 2]>,
    /// New split vertex, or the nearby endpoint when the split was refused.
    pub vertex: Option<VertexId>,
}

impl SplitEdgeOutput {
    /// The upstream status; dependency errors use the enclosing `Result`.
    #[must_use]
    pub const fn status(self) -> i32 {
        if self.split {
            0
        } else {
            SM_ERR
        }
    }
}

/// `FindInfiniteRegion`, including its optional shell and bounding box arguments.
///
/// The first call, `GetRegions`, requires the B-rep's native owning-list order.
/// An arena's allocation order cannot substitute for it. Ray selection and its
/// single shell retry follow that individual dependency call. `query` (the B-rep's
/// identities and cache) serves both the shell's ray fires and the B-rep's.
///
/// # Errors
/// Returns the owning unit's `Unported` error, without selecting a guessed region.
pub fn find_infinite_region(
    k: &mut Kernel,
    b: &mut Brep,
    query: &BrepQuery<'_>,
    shell: Option<ShellId>,
    bbox: Option<&crate::geom::extent3d::Extent3d>,
) -> Result<Option<RegionId>> {
    use crate::intersect::point_class::PointClassification;
    use crate::topology::brep_cache::CacheObject;
    use crate::topology::shell::{ShellRayInput, ShellRayState};
    let brep_id = query.identity().brep_id;
    let mut shell_to_test = shell;
    let mut supplied_box = bbox.copied();
    // The native recursive retry drops the optional shell AND bounding box. It
    // can happen only once; this loop preserves its traversal/evaluation order.
    for _ in 0..2 {
        let regions = brep_regions(b)?;
        if let [region] = regions.as_slice() {
            return Ok(Some(*region));
        }
        let tolerance = b.tolerance();
        let mut state = ShellRayState {
            region: None,
            classification: PointClassification::<CacheObject>::new(tolerance),
        };
        let bounds = match supplied_box {
            Some(bounds) => bounds,
            None => crate::error::ignored(qbrep::calculate_bounding_box(
                k,
                &mut *b,
                BoundingBoxOptions::default(),
            ))?
            .unwrap_or_else(crate::geom::extent3d::Extent3d::new),
        };
        let ray_start = bounds.max() + (bounds.max() - bounds.min()) * 0.5;
        let mut direction = Vec3::new(1.0, 1.0, 1.0);
        let parameters = [0.12345, 0.34567, 0.56789, 0.78912, 0.86571];
        let face_shell = match shell_to_test {
            Some(shell) if b.shell(shell)?.is_faceuse_shell() => Some(shell),
            _ => None,
        };
        if let Some(shell) = face_shell {
            let uses = b.shell(shell)?.get_faceuses()?;
            let mut distance = crate::math::BIG_DOUBLE;
            let mut best_edge = None;
            for pass in 0..2 {
                if pass == 1 && distance != crate::math::BIG_DOUBLE {
                    continue;
                }
                for fu in &uses {
                    let use_ = b.faceuse(*fu)?;
                    let mate = required(use_.mate, "shell faceuse without mate")?;
                    if use_.shell == b.faceuse(mate)?.shell {
                        continue;
                    }
                    let face = required(use_.face, "shell faceuse without face")?;
                    let edges = crate::topology::topology_traverser::face_edges(b, face)?;
                    if edges.is_empty() {
                        return Err(SolidError::sm_err(OP));
                    }
                    for edge_id in edges {
                        required(
                            b.edge(edge_id)?.curve.as_ref(),
                            "ray target edge without curve",
                        )?;
                        // Preserve short-circuiting of both calls in each pass. IsSeam
                        // may create UV trim curves, so it runs before the curve borrow.
                        let dubious = edge::is_seam(k, b, edge_id)?
                            || required(
                                b.edge(edge_id)?.curve.as_ref(),
                                "ray target edge without curve",
                            )?
                            .has_repeated_end_control_points(k, crate::geom::tol::ZONE_TOL_3D)?;
                        if (pass == 0 && dubious) || (pass == 1 && !dubious) {
                            continue;
                        }
                        let record = b.edge(edge_id)?;
                        let curve =
                            required(record.curve.as_ref(), "ray target edge without curve")?;
                        for parameter in parameters {
                            let point =
                                curve.evaluate_point(k, record.interval.evaluate(parameter))?;
                            let ray = point - ray_start;
                            let squared = ray.length_squared();
                            if squared < distance {
                                best_edge = Some(edge_id);
                                direction = ray;
                                distance = squared;
                            }
                        }
                    }
                }
            }
            direction = ray_direction(direction)?;
            let radius = bounds.sphere_bound().radius;
            let attempt = crate::topology::shell::ray_fire(
                k,
                b,
                brep_id,
                shell,
                query,
                ShellRayInput {
                    start: ray_start,
                    direction,
                    zone_tolerance: tolerance,
                    sphere_bound: Some(radius),
                    state,
                },
            );
            let success = ray_success(&attempt.status)?;
            state = attempt.state;
            if !success {
                let edge_id = best_edge.ok_or_else(|| SolidError::null_pointer(OP))?;
                let record = b.edge(edge_id)?;
                // (a copy: the ray fires may add UV trim curves to the B-rep)
                let curve =
                    required(record.curve.as_ref(), "ray target edge without curve")?.clone();
                let interval = record.interval;
                let point = curve.evaluate_point(k, interval.evaluate(0.789))?;
                let attempt = crate::topology::shell::ray_fire(
                    k,
                    b,
                    brep_id,
                    shell,
                    query,
                    ShellRayInput {
                        start: ray_start,
                        direction: point - ray_start,
                        zone_tolerance: tolerance,
                        sphere_bound: None,
                        state,
                    },
                );
                let success = ray_success(&attempt.status)?;
                state = attempt.state;
                if !success {
                    let point = curve.evaluate_point(k, interval.evaluate(0.189))?;
                    let start = bounds.min() - (bounds.max() - bounds.min()) * 0.5;
                    let attempt = crate::topology::shell::ray_fire(
                        k,
                        b,
                        brep_id,
                        shell,
                        query,
                        ShellRayInput {
                            start,
                            direction: point - start,
                            zone_tolerance: tolerance,
                            sphere_bound: None,
                            state,
                        },
                    );
                    let success = ray_success(&attempt.status)?;
                    state = attempt.state;
                    if !success {
                        shell_to_test = None;
                        supplied_box = None;
                        continue;
                    }
                }
            }
        } else {
            let edges = brep_edges(b)?;
            if edges.is_empty() {
                return Err(SolidError::sm_err(OP));
            }
            let mut distance = crate::math::BIG_DOUBLE;
            for pass in 0..6 {
                if pass >= 1 && distance != crate::math::BIG_DOUBLE {
                    break;
                }
                for edge_id in &edges {
                    let record = b.edge(*edge_id)?;
                    let curve = required(record.curve.as_ref(), "ray target edge without curve")?;
                    let big_tolerance = record.tolerance > 1.01 * b.tolerance();
                    let repeated =
                        curve.has_repeated_end_control_points(k, crate::geom::tol::ZONE_TOL_3D)?;
                    // IsSeam may create UV trim curves; reborrow the edge afterwards.
                    let seam = edge::is_seam(k, b, *edge_id)?;
                    let record = b.edge(*edge_id)?;
                    let curve = required(record.curve.as_ref(), "ray target edge without curve")?;
                    let approximated = is_intersector_approx(curve)?;
                    let round = if !big_tolerance && !repeated && !seam && !approximated {
                        0
                    } else if !big_tolerance && !repeated && !seam && approximated {
                        1
                    } else if big_tolerance && !repeated && !seam && approximated {
                        2
                    } else if big_tolerance && !repeated && seam && approximated {
                        3
                    } else if repeated {
                        4
                    } else {
                        5
                    };
                    if pass != round {
                        continue;
                    }
                    for parameter in parameters {
                        let point = curve.evaluate_point(k, record.interval.evaluate(parameter))?;
                        let ray = point - ray_start;
                        let squared = ray.length_squared();
                        if squared < distance {
                            direction = ray;
                            distance = squared;
                        }
                    }
                }
            }
            direction = ray_direction(direction)?;
            // (the rays solve on one cache: nothing edits the B-rep in between)
            let mut fire = |b: &mut Brep, start: Point3, direction: Vec3| {
                brep_ray_fire(k, query, b, start, direction, tolerance, &mut state)
            };
            if !fire(b, ray_start, direction)? {
                // Native retries use edges[0], not the closest sampled edge.
                let edge_id = *required(edges.first(), "missing first ray edge")?;
                let record = b.edge(edge_id)?;
                // (a copy: the ray fires may add UV trim curves to the B-rep)
                let curve =
                    required(record.curve.as_ref(), "ray target edge without curve")?.clone();
                let interval = record.interval;
                let point = curve.evaluate_point(k, interval.evaluate(0.789))?;
                if !fire(b, ray_start, point - ray_start)? {
                    let point = curve.evaluate_point(k, interval.evaluate(0.189))?;
                    let start = bounds.min() - (bounds.max() - bounds.min()) * 0.5;
                    if !fire(b, start, point - start)? {
                        return Err(SolidError::sm_err(OP));
                    }
                }
            }
        }
        return Ok(state.region);
    }
    Err(SolidError::invalid_input(
        OP,
        "infinite region shell retry exceeded its single retry",
    ))
}

fn brep_regions(b: &Brep) -> Result<Vec<RegionId>> {
    crate::topology::brep::p1::regions(b, None)
}

fn brep_edges(b: &Brep) -> Result<Vec<EdgeId>> {
    crate::topology::brep::p1::edges(b, None)
}

/// `this->RayFire(start, direction, tolerance, pRegion, sPC)` (unit `topology.brep_p2`)
/// on `query`'s cache: `Ok(true)` with the region in `state` on success, `Ok(false)` on
/// an ordinary failure (the caller tries its next ray). The intersection is not read
/// after the ray fires, so `state.classification` stays as it was.
fn brep_ray_fire<O>(
    k: &Kernel,
    query: &BrepQuery<'_>,
    b: &mut Brep,
    start: Point3,
    direction: Vec3,
    tolerance: f64,
    state: &mut crate::topology::shell::ShellRayState<O>,
) -> Result<bool> {
    let fired = query.ray_fire_live(k, b, start, direction, tolerance);
    match crate::error::ignored(fired)? {
        Some(hit) => {
            state.region = hit.region;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn ray_direction(direction: Vec3) -> Result<Vec3> {
    direction.unitize().map_err(|_| SolidError::Upstream {
        operation: OP,
        code: 1007,
        entity: None,
    })
}

/// Whether a `SmShell::RayFire` call succeeded. Upstream retries on any failed status;
/// an error with no upstream status (an unported callee, a stale id, a pending
/// geometry preparation, or the B-rep cache failing to resolve an owner under the
/// query's identities) is propagated instead, so a naming mismatch cannot pass for a
/// missed ray and silently select another region.
fn ray_success(status: &Result<()>) -> Result<bool> {
    match status {
        Ok(()) => Ok(true),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::NotFound { .. }
            | SolidError::PreparationRequired { .. }),
        ) => Err(e.clone()),
        Err(e) if crate::topology::brep_cache::is_identity_error(e) => Err(e.clone()),
        Err(_) => Ok(false),
    }
}

fn is_intersector_approx(curve: &Curve) -> Result<bool> {
    use crate::curve::curve::CurveKind;
    match curve.kind() {
        CurveKind::BSpline(c) => c.is_intersector_approx(),
        CurveKind::Ellipse(c) => c.base().is_intersector_approx(),
        CurveKind::Hyperbola(c) => c.base().is_intersector_approx(),
        CurveKind::Parabola(c) => c.base().is_intersector_approx(),
        CurveKind::Line(c) => c.base_with_nurb()?.is_intersector_approx(),
        _ => Ok(false),
    }
}

/// `OrientSheetSurfaces`. `same=true` is upstream's default orientation policy.
///
/// # Errors
/// `SM_ERR` for a face not in the B-rep or a B-rep that is not a sheet; those of the
/// part-2 sheet/oriented-sheet predicates and of `SwapUV`. The UV swaps' own statuses
/// are ignored, as upstream.
pub fn orient_sheet_surfaces(
    k: &mut Kernel,
    b: &mut Brep,
    face: Option<FaceId>,
    same: bool,
) -> Result<()> {
    let faces = brep_faces(b)?;
    let Some(face) = face.filter(|face| faces.contains(face)) else {
        return Err(SolidError::sm_err(OP));
    };
    if !is_sheet(k, b)? {
        return Err(SolidError::sm_err(OP));
    }
    if is_oriented_sheet(k, b)? {
        if !same {
            // Status ignored upstream (SmBrep.cpp:11644).
            ignore_status(swap_brep_uv(k, b))?;
        }
        return Ok(());
    }
    crate::geom::context::with_mark_lock(k, crate::geom::context::ALL_MARKS, |k, mark| {
        use crate::topology::topology_traverser::{Entity, Marks};
        let mut marks = Marks::default();
        if !same {
            // Status ignored upstream (SmBrep.cpp:11659).
            ignore_status(swap_face_uv(k, b, &mut marks, face))?;
        }
        let upward = required(
            crate::topology::face::p2::upward_faceuse(b, face)?,
            "sheet face without upward faceuse",
        )?;
        let mut pending = vec![upward];
        while let Some(fu) = pending.pop() {
            if marks.is_marked(k, Entity::Faceuse(fu), mark) {
                continue;
            }
            marks.mark(k, Entity::Faceuse(fu), mark);
            for neighbor in crate::topology::edgeuse::p1::faceuse_neighbors(b, fu, true)? {
                if marks.is_marked(k, Entity::Faceuse(neighbor), mark) {
                    continue;
                }
                let f = required(b.faceuse(neighbor)?.face, "neighbor without face")?;
                if crate::topology::face::p2::upward_faceuse(b, f)? != Some(neighbor) {
                    // Status ignored upstream (SmBrep.cpp:11690).
                    ignore_status(swap_face_uv(k, b, &mut marks, f))?;
                }
                pending.push(neighbor);
            }
        }
        // Upstream's final return is TRUE (1), unlike the earlier SM_SUCCESS (0).
        Err(SolidError::Upstream {
            operation: OP,
            code: 1,
            entity: None,
        })
    })?
}

fn brep_faces(b: &Brep) -> Result<Vec<FaceId>> {
    crate::topology::brep::p1::faces(b, None)
}

/// `IsSheet()` ([`crate::topology::brep::p2::is_sheet`]).
fn is_sheet(k: &mut Kernel, b: &Brep) -> Result<bool> {
    crate::topology::brep::p2::is_sheet(k, b)
}

/// `IsOrientedSheet()` ([`crate::topology::brep::p2::is_oriented_sheet`]).
fn is_oriented_sheet(k: &mut Kernel, b: &Brep) -> Result<bool> {
    crate::topology::brep::p2::is_oriented_sheet(k, b)
}

/// `SwapUV()` ([`crate::topology::brep::p2::swap_uv`], which claims its own mark).
fn swap_brep_uv(k: &mut Kernel, b: &mut Brep) -> Result<()> {
    let mut marks = crate::topology::topology_traverser::Marks::default();
    crate::topology::brep::p2::swap_uv(k, b, &mut marks)
}

/// `sm_SwapFaceUV(pFace, SM_MT_NOMARK)`: swap without marking the face.
fn swap_face_uv(
    k: &Kernel,
    b: &mut Brep,
    marks: &mut crate::topology::topology_traverser::Marks,
    face: FaceId,
) -> Result<()> {
    crate::topology::brep::p1::swap_face_uv(
        k,
        b,
        marks,
        face,
        crate::geom::context::MarkType::NoMark,
    )
}

/// `MakeShellVertex`: `region=None` selects the infinite region.
///
/// # Errors
/// Those of part 2's `MakeShellVertexTopology` (allocation and linking), the vertex
/// point and tolerance setters and the two `ADD_TO_BREP` notifications.
pub fn make_shell_vertex(
    b: &mut Brep,
    region: Option<RegionId>,
    point: Point3,
    env: &mut edge::NotifyEnv<'_>,
) -> Result<(ShellId, VertexId)> {
    make_shell_vertex_with_context(
        None,
        b,
        region,
        point,
        &mut edge::NotifyState::from_env(env),
    )
}

/// [`make_shell_vertex`] using the actual kernel context and caller-owned stores.
/// The same incomplete topology barriers and allocation order apply.
pub fn make_shell_vertex_with_kernel_context(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    point: Point3,
    stores: &mut edge::NotificationStores<'_>,
) -> Result<(ShellId, VertexId)> {
    make_shell_vertex_with_state(
        k,
        b,
        region,
        point,
        &mut edge::NotifyState::from_kernel_stores(stores),
    )
}

pub(crate) fn make_shell_vertex_with_state(
    k: &mut Kernel,
    b: &mut Brep,
    region: Option<RegionId>,
    point: Point3,
    state: &mut edge::NotifyState<'_>,
) -> Result<(ShellId, VertexId)> {
    make_shell_vertex_with_context(Some(&mut k.context), b, region, point, state)
}

// No geometry call in this routine needs the whole kernel. The optional context
// lets the immutable-kernel legacy face constructor use this same algorithm.
pub(crate) fn make_shell_vertex_with_context(
    mut kernel_context: Option<&mut crate::geom::context::Context>,
    b: &mut Brep,
    region: Option<RegionId>,
    point: Point3,
    state: &mut edge::NotifyState<'_>,
) -> Result<(ShellId, VertexId)> {
    let region = match region {
        Some(region) => Some(region),
        // GetInfiniteRegion: the stored m_pInfiniteRegion.
        None => b.infinite_region(),
    };
    if let Some(region) = region {
        b.region(region)?;
    }
    let (shell, vertex) = state.with_context(kernel_context.as_deref_mut(), |env| {
        make_shell_vertex_topology(b, region, env)
    })?;
    register_state_entities(state, &[EntityRef::Shell(shell), EntityRef::Vertex(vertex)]);
    for vu in b.vertex(vertex)?.vertexuses.clone() {
        register_state_entities(state, &[EntityRef::Vertexuse(vu)]);
    }
    state.with_context(kernel_context.as_deref_mut(), |env| {
        ignore_status(set_vertex_point(b, vertex, point, env))
    })?;
    vertex::set_tolerance(b, vertex, b.tolerance(), true, true)?;
    state.with_context(kernel_context.as_deref_mut(), |env| {
        brep_notify(
            b,
            env,
            NotifyOperation::AddToBrep,
            EntityRef::Vertex(vertex),
            None,
            None,
        )
    })?;
    state.with_context(kernel_context, |env| {
        brep_notify(
            b,
            env,
            NotifyOperation::AddToBrep,
            EntityRef::Shell(shell),
            None,
            None,
        )
    })?;
    Ok((shell, vertex))
}

fn make_shell_vertex_topology(
    b: &mut Brep,
    region: Option<RegionId>,
    env: &mut edge::NotifyEnv<'_>,
) -> Result<(ShellId, VertexId)> {
    // MakeShellVertexTopology sends no notification; the caller registers the records
    let _ = env;
    crate::topology::brep::p2::make_shell_vertex_topology(b, region)
}

/// `MakeVertexLoop`: creates a vertex loop with the supplied orientation
/// (upstream's default is `OrientType::Opposite`).
///
/// # Errors
/// The face's `PRE_EDIT` notification is the first operation. It belongs to part 3,
/// and must execute before allocating/linking loop uses or emitting ADD notifications.
pub fn make_vertex_loop(
    b: &mut Brep,
    face: FaceId,
    point: Point3,
    orientation: OrientType,
    env: &mut edge::NotifyEnv<'_>,
) -> Result<(LoopId, VertexId)> {
    b.face(face)?;
    face_notify(b, face, env, NotifyOperation::PreEdit)?;
    let loop_ = b.add_loop_record(Loop {
        face,
        edges: Vec::new(),
        loopuse: None,
    })?;
    let lu1 = b.add_loopuse_record(Loopuse::default())?;
    let lu2 = b.add_loopuse_record(Loopuse::default())?;
    let vertex = b.add_vertex_record(Vertex::default())?;
    let vu1 = b.add_vertexuse_record(Vertexuse::default())?;
    let vu2 = b.add_vertexuse_record(Vertexuse::default())?;
    register_entities(
        env,
        &[
            EntityRef::Loop(loop_),
            EntityRef::Loopuse(lu1),
            EntityRef::Loopuse(lu2),
            EntityRef::Vertex(vertex),
            EntityRef::Vertexuse(vu1),
            EntityRef::Vertexuse(vu2),
        ],
    );
    // These are the same owning-list insertions as the existing face/edge ports:
    // append in native PostInsert order and set each child's owner.
    for vu in [vu1, vu2] {
        b.vertex_mut(vertex)?.vertexuses.push(vu);
        b.vertexuse_mut(vu)?.vertex = Some(vertex);
    }
    ignore_status(set_vertex_point(b, vertex, point, env))?;
    let fu1 = crate::topology::face::p2::upward_faceuse(b, face)?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let fu2 = b
        .faceuse(fu1)?
        .mate
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    b.loop_mut(loop_)?.loopuse = Some(lu1);
    for (lu, mate, fu, vu) in [(lu1, lu2, fu1, vu1), (lu2, lu1, fu2, vu2)] {
        *b.loopuse_mut(lu)? = Loopuse {
            faceuse: Some(fu),
            loop_: Some(loop_),
            mate: Some(mate),
            start: LoopuseStart::Vertexuse(Some(vu)),
            orientation,
        };
        b.faceuse_mut(fu)?.loopuses.push(lu);
        b.vertexuse_mut(vu)?.owner = VertexuseOwner::Loopuse(Some(lu));
    }
    insert_brep_entity(b, EntityRef::Vertex(vertex))?;
    vertex::set_tolerance(b, vertex, b.face(face)?.tolerance, true, true)?;
    brep_notify(
        b,
        env,
        NotifyOperation::AddToBrep,
        EntityRef::Vertex(vertex),
        None,
        None,
    )?;
    brep_notify(
        b,
        env,
        NotifyOperation::AddToBrep,
        EntityRef::Loop(loop_),
        None,
        None,
    )?;
    face_notify(b, face, env, NotifyOperation::PostEdit)?;
    sync_face_cache(b, face)?;
    Ok((loop_, vertex))
}

fn register_entities(env: &mut edge::NotifyEnv<'_>, entities: &[EntityRef]) {
    register_state_entities(&mut edge::NotifyState::from_env(env), entities);
}

fn register_state_entities(state: &mut edge::NotifyState<'_>, entities: &[EntityRef]) {
    for &entity in entities {
        state.records.or_default((state.key_of)(entity).key);
    }
}

fn set_vertex_point(
    b: &mut Brep,
    v: VertexId,
    point: Point3,
    env: &mut edge::NotifyEnv<'_>,
) -> Result<()> {
    vertex::set_point(
        b,
        v,
        point,
        env.records,
        env.attributes,
        env.context.as_deref_mut(),
        (env.key_of)(EntityRef::Vertex(v)),
        &|vu| (env.key_of)(EntityRef::Vertexuse(vu)),
    )
}

/// `m_p{Face,Edge,Vertex}ListHead->PostInsert`: appends to the owning list. Part 1's
/// `add_*_record` already appended the record at allocation, and no record of the
/// same kind is allocated in between, so the list order is native; this checks it.
fn insert_brep_entity(b: &mut Brep, entity: EntityRef) -> Result<()> {
    let listed = match entity {
        EntityRef::Face(f) => b.face_list().last() == Some(&f),
        EntityRef::Edge(e) => b.edge_list().last() == Some(&e),
        EntityRef::Vertex(v) => b.vertex_list().last() == Some(&v),
        _ => false,
    };
    if listed {
        Ok(())
    } else {
        Err(SolidError::invalid_input(
            OP,
            "new topology record is not last on its owning list",
        ))
    }
}

fn brep_notify(
    b: &mut Brep,
    env: &mut edge::NotifyEnv<'_>,
    operation: NotifyOperation,
    object: EntityRef,
    other: Option<EntityRef>,
    geometry: Option<crate::geom::object::ObjectRef>,
) -> Result<()> {
    let object = (env.key_of)(object);
    let other = other.map(env.key_of);
    let data = match operation {
        NotifyOperation::AddToBrep => [Some(object), Some(brep_ref(b)), geometry],
        NotifyOperation::SplitInBrep => [Some(object), Some(object), other],
        NotifyOperation::TrimNoSplitInBrep => [Some(object), other, None],
        _ => {
            return Err(SolidError::invalid_input(
                OP,
                "unsupported construction notification",
            ))
        }
    };
    ignore_status(brep_notify_call(b, env, operation, data))
}

fn brep_notify_call(
    b: &mut Brep,
    env: &mut edge::NotifyEnv<'_>,
    operation: NotifyOperation,
    data: [Option<crate::geom::object::ObjectRef>; 3],
) -> Result<()> {
    // no B-rep cache is held by the caller (`brep::p2::notify` with no cache slot)
    let data = data.map(|d| d.map(crate::topology::brep::p2::NotifyArg::Object));
    crate::topology::brep::p2::notify(b, env, None, operation, data)
}

fn face_notify(
    b: &mut Brep,
    face: FaceId,
    env: &mut edge::NotifyEnv<'_>,
    operation: NotifyOperation,
) -> Result<()> {
    let data = [
        Some((env.key_of)(EntityRef::Face(face))),
        Some(brep_ref(b)),
        None,
    ];
    ignore_status(face_notify_call(b, face, env, operation, data))
}

fn face_notify_call(
    b: &mut Brep,
    face: FaceId,
    env: &mut edge::NotifyEnv<'_>,
    operation: NotifyOperation,
    data: [Option<crate::geom::object::ObjectRef>; 3],
) -> Result<()> {
    crate::topology::face::p3::notify(b, face, env, None, operation, data)
}

fn required<T>(v: Option<T>, reason: &'static str) -> Result<T> {
    v.ok_or_else(|| SolidError::invalid_input(OP, reason))
}

fn brep_ref(b: &Brep) -> crate::geom::object::ObjectRef {
    crate::geom::object::ObjectRef {
        key: b.key(),
        type_code: 16011,
    }
}

fn set_curve_owner(
    b: &mut Brep,
    edge_id: EdgeId,
    env: &mut edge::NotifyEnv<'_>,
    curve_ref: crate::geom::object::ObjectRef,
) -> Result<()> {
    let owner = (env.key_of)(EntityRef::Edge(edge_id));
    let curve = required(b.edge_mut(edge_id)?.curve.as_mut(), "edge without curve")?;
    let (_, change) = curve.set_owner(Some(owner), None);
    b.set_geometry_owner_key(EntityRef::Edge(edge_id), Some(owner));
    if let Some(change) = change {
        ignore_status(crate::geom::a_object::notify(
            env.attributes,
            env.context.as_deref_mut(),
            NotifyOperation::ChangeOwner,
            curve_ref,
            change.new_owner,
            change.new_owner_brep,
            change.old_owner,
        ))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SplitUvInput {
    original: crate::topology::EdgeuseId,
    new: crate::topology::EdgeuseId,
    point: Point3,
    parameter: f64,
    scaled_zero: f64,
    orientation: OrientType,
    for_tessellation: bool,
}

fn split_uv_trim(k: &Kernel, b: &mut Brep, input: SplitUvInput) -> Result<()> {
    let SplitUvInput {
        original,
        new,
        point,
        parameter,
        scaled_zero,
        orientation,
        for_tessellation,
    } = input;
    crate::topology::brep::p1::make_vertex_split_uv_trim_curve(
        k,
        b,
        original,
        new,
        point,
        parameter,
        scaled_zero,
        orientation,
        for_tessellation,
    )
}

/// `MakeVertexSplitEdge`: preserve the scaled parameter boundary, the two-stage
/// length tests (2 and 10 samples), start-before-end precedence, and the nearby
/// endpoint returned together with `SM_ERR`.
///
/// The editing state is the B-rep's `m_bEditingEnabled` ([`Brep::editing_enabled`]):
/// false from the constructor, set by editing scopes. Ordinary no-split results are
/// successful Rust returns whose
/// [`SplitEdgeOutput::status`] is `SM_ERR`; this preserves outputs that C++ callers
/// consume despite that status. Dependency/invalid-graph errors remain `Err`.
///
/// # Errors
/// Invalid graph; evaluator errors; face notifications or the
/// unported UV-split helper reached by an interior split.
pub fn make_vertex_split_edge(
    k: &mut Kernel,
    b: &mut Brep,
    edge_id: EdgeId,
    parameter: f64,
    env: &mut edge::NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> crate::geom::object::ObjectRef,
) -> Result<SplitEdgeOutput> {
    make_vertex_split_edge_with_state(
        k,
        b,
        edge_id,
        parameter,
        &mut edge::NotifyState::from_env(env),
        curve_ref,
    )
}

/// [`make_vertex_split_edge`] notifying through the actual kernel context.
/// `stores` supplies real topology/attribute records; `curve_ref` identifies each
/// edge's curve, including the newly allocated edge. Refusal and errors retain the
/// same outputs and partial edits as the legacy entrypoint.
pub fn make_vertex_split_edge_with_kernel_context(
    k: &mut Kernel,
    b: &mut Brep,
    edge_id: EdgeId,
    parameter: f64,
    stores: &mut edge::NotificationStores<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> crate::geom::object::ObjectRef,
) -> Result<SplitEdgeOutput> {
    make_vertex_split_edge_with_state(
        k,
        b,
        edge_id,
        parameter,
        &mut edge::NotifyState::from_kernel_stores(stores),
        curve_ref,
    )
}

pub(crate) fn make_vertex_split_edge_with_state(
    k: &mut Kernel,
    b: &mut Brep,
    edge_id: EdgeId,
    parameter: f64,
    state: &mut edge::NotifyState<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> crate::geom::object::ObjectRef,
) -> Result<SplitEdgeOutput> {
    make_vertex_split_edge_with_identity(k, b, edge_id, parameter, state, &|e, kind| {
        Ok(curve_ref(e, kind))
    })
}

/// The same native split with a caller identity resolver that can refuse an
/// unregistered newly allocated curve. Partial topology and notifications remain
/// visible on an identity error, just as on downstream constructor errors.
pub(crate) fn make_vertex_split_edge_with_identity(
    k: &mut Kernel,
    b: &mut Brep,
    edge_id: EdgeId,
    parameter: f64,
    state: &mut edge::NotifyState<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> Result<crate::geom::object::ObjectRef>,
) -> Result<SplitEdgeOutput> {
    make_vertex_split_edge_with_curve_identity_state(
        k,
        b,
        edge_id,
        parameter,
        state,
        CurveIdentity::Checked(curve_ref),
    )
}

/// Split with an actual-curve issuer at Copy/SetCurve, retaining the native prefix.
/// # Errors
/// The original split/notification errors or checked identity issuance errors.
pub fn make_vertex_split_edge_with_curve_identity(
    k: &mut Kernel,
    b: &mut Brep,
    edge_id: EdgeId,
    parameter: f64,
    env: &mut edge::NotifyEnv<'_>,
    curve_identity: CurveIdentity<'_>,
) -> Result<SplitEdgeOutput> {
    make_vertex_split_edge_with_curve_identity_state(
        k,
        b,
        edge_id,
        parameter,
        &mut edge::NotifyState::from_env(env),
        curve_identity,
    )
}

/// Split through the actual kernel context with checked curve identity issuance.
/// # Errors
/// Original split errors or identity issuance failure after the retained prefix.
pub fn make_vertex_split_edge_with_kernel_curve_identity(
    k: &mut Kernel,
    b: &mut Brep,
    edge_id: EdgeId,
    parameter: f64,
    stores: &mut edge::NotificationStores<'_>,
    curve_identity: CurveIdentity<'_>,
) -> Result<SplitEdgeOutput> {
    make_vertex_split_edge_with_curve_identity_state(
        k,
        b,
        edge_id,
        parameter,
        &mut edge::NotifyState::from_kernel_stores(stores),
        curve_identity,
    )
}

pub(crate) fn make_vertex_split_edge_with_curve_identity_state(
    k: &mut Kernel,
    b: &mut Brep,
    edge_id: EdgeId,
    parameter: f64,
    state: &mut edge::NotifyState<'_>,
    curve_identity: CurveIdentity<'_>,
) -> Result<SplitEdgeOutput> {
    let out = SplitEdgeOutput::default();
    if !b.editing_enabled() {
        return Ok(out);
    }
    state.with_doing_boolean(k, |k, state| {
        let scaled_zero = EFF_ZERO_PARAM * (1.0 + parameter.abs());
        let interval = b.edge(edge_id)?.interval;
        if !interval.contains_value(parameter, scaled_zero) {
            return Ok(out);
        }
        let mut near_start = parameter < interval.min() + scaled_zero;
        let mut near_end = parameter > interval.max() - scaled_zero;
        if !near_start && !near_end {
            let start_ivl = Extent1d::from_min_max(interval.min(), parameter);
            let end_ivl = Extent1d::from_min_max(parameter, interval.max());
            let curve = required(b.edge(edge_id)?.curve.as_ref(), "split edge has no curve")?;
            let dist_to_start = curve.approximate_length(k, &start_ivl, 2, false)?.0;
            let dist_to_end = curve.approximate_length(k, &end_ivl, 2, false)?.0;
            let start = required(
                edge::start_vertex(b, edge_id)?,
                "split edge has no start vertex",
            )?;
            let end = required(
                edge::other_vertex(b, edge_id, Some(start))?,
                "split edge has no end vertex",
            )?;
            let start_tol = b.vertex(start)?.tolerance;
            let end_tol = b.vertex(end)?.tolerance;
            if dist_to_start < start_tol
                && curve.approximate_length(k, &start_ivl, 10, false)?.0 < start_tol
            {
                near_start = true;
            }
            if dist_to_end < end_tol
                && curve.approximate_length(k, &end_ivl, 10, false)?.0 < end_tol
            {
                near_end = true;
            }
            if near_start {
                vertex::set_exact(b, start, false)?;
            }
            if near_end {
                vertex::set_exact(b, end, false)?;
            }
        }
        if near_start {
            return Ok(SplitEdgeOutput {
                vertex: edge::start_vertex(b, edge_id)?,
                ..out
            });
        }
        if near_end {
            return Ok(SplitEdgeOutput {
                vertex: edge::end_vertex(b, edge_id)?,
                ..out
            });
        }

        let edge_ref = (state.key_of)(EntityRef::Edge(edge_id));
        state.with_env(k, |env| {
            ignore_status(edge::notify(
                b,
                edge_id,
                env,
                NotifyOperation::PreEdit,
                [Some(edge_ref), Some(brep_ref(b)), None],
            ))
        })?;
        for face in edge::faces(b, edge_id, None)? {
            state.with_env(k, |env| face_notify(b, face, env, NotifyOperation::PreEdit))?;
            b.face(face)?;
        }
        let start = required(edge::start_vertex(b, edge_id)?, "split edge without start")?;
        let end = required(edge::end_vertex(b, edge_id)?, "split edge without end")?;
        let new_edge = b.add_edge_record(crate::topology::Edge {
            vertices: [start, end],
            edgeuses: Vec::new(),
            interval: Extent1d::new(),
            tolerance: 0.0,
            curve: None,
        })?;
        let new_vertex = b.add_vertex_record(Vertex::default())?;
        register_state_entities(
            state,
            &[EntityRef::Edge(new_edge), EntityRef::Vertex(new_vertex)],
        );
        let mut ivl1 = Extent1d::from_min_max(interval.min(), parameter);
        let mut ivl2 = Extent1d::from_min_max(parameter, interval.max());
        let primary = required(
            edge::primary_edgeuse(b, edge_id)?,
            "split edge without primary use",
        )?;
        let same = b.edgeuse(primary)?.orientation == OrientType::Same;
        b.edge_mut(edge_id)?.interval = if same { ivl1 } else { ivl2 };
        b.edge_mut(new_edge)?.interval = if same { ivl2 } else { ivl1 };
        let mut copied =
            required(b.edge(edge_id)?.curve.as_ref(), "split edge without curve")?.copy(k)?;
        let old_curve_ref = state.with_env(k, |env| {
            let curve = required(
                b.edge_mut(edge_id)?.curve.as_mut(),
                "split edge without curve",
            )?;
            curve_identity.resolve(edge_id, curve, env)
        })?;
        let new_curve_ref =
            state.with_env(k, |env| curve_identity.resolve(new_edge, &mut copied, env))?;
        state.with_env(k, |env| {
            ignore_status(edge::set_curve(
                b,
                new_edge,
                Some(copied),
                edge::SetCurveOptions {
                    delete_old_curve: false,
                    ..edge::SetCurveOptions::default()
                },
                env,
                [Some(new_curve_ref), None],
            ))
        })?;
        state.with_env(k, |env| set_curve_owner(b, new_edge, env, new_curve_ref))?;
        state.with_env(k, |env| {
            ignore_status(crate::geom::a_object::notify(
                env.attributes,
                env.context.as_deref_mut(),
                NotifyOperation::Split,
                old_curve_ref,
                Some(old_curve_ref),
                Some(new_curve_ref),
                Some(edge_ref),
            ))
        })?;
        let split_parameter;
        if same {
            ignore_status(
                required(
                    b.edge_mut(edge_id)?.curve.as_mut(),
                    "split edge without curve",
                )?
                .trim(
                    k,
                    &mut ivl1,
                    crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
                ),
            )?;
            split_parameter = ivl1.max();
            ignore_status(
                required(
                    b.edge_mut(new_edge)?.curve.as_mut(),
                    "new split edge without curve",
                )?
                .trim(
                    k,
                    &mut ivl2,
                    crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
                ),
            )?;
            b.edge_mut(edge_id)?.interval = ivl1;
            b.edge_mut(new_edge)?.interval = ivl2;
        } else {
            ignore_status(
                required(
                    b.edge_mut(edge_id)?.curve.as_mut(),
                    "split edge without curve",
                )?
                .trim(
                    k,
                    &mut ivl2,
                    crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
                ),
            )?;
            split_parameter = ivl2.min();
            ignore_status(
                required(
                    b.edge_mut(new_edge)?.curve.as_mut(),
                    "new split edge without curve",
                )?
                .trim(
                    k,
                    &mut ivl1,
                    crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
                ),
            )?;
            b.edge_mut(edge_id)?.interval = ivl2;
            b.edge_mut(new_edge)?.interval = ivl1;
        }
        let point = required(b.edge(edge_id)?.curve.as_ref(), "split edge without curve")?
            .evaluate_point(k, split_parameter)?;
        state.with_env(k, |env| {
            ignore_status(set_vertex_point(b, new_vertex, point, env))
        })?;
        let original_uses = edge::edgeuses(b, edge_id, None, None)?;
        if original_uses.len() % 2 != 0 {
            return Err(SolidError::invalid_input(
                OP,
                "split edge has an incomplete mated pair",
            ));
        }
        let mut new_uses = Vec::new();
        let mut new_vertexuses = Vec::new();
        for _ in &original_uses {
            let eu = b.add_edgeuse_record(crate::topology::Edgeuse {
                edge: Some(new_edge),
                ..crate::topology::Edgeuse::default()
            })?;
            b.edge_mut(new_edge)?.edgeuses.push(eu);
            new_uses.push(eu);
            let vu = b.add_vertexuse_record(Vertexuse {
                vertex: Some(new_vertex),
                ..Vertexuse::default()
            })?;
            b.vertex_mut(new_vertex)?.vertexuses.push(vu);
            register_state_entities(state, &[EntityRef::Edgeuse(eu), EntityRef::Vertexuse(vu)]);
            new_vertexuses.push(vu);
        }
        for ((old_pair, new_pair), vu_pair) in original_uses
            .chunks_exact(2)
            .zip(new_uses.chunks_exact(2))
            .zip(new_vertexuses.chunks_exact(2))
        {
            let (&old1, &old2) = (
                required(old_pair.first(), "missing original pair")?,
                required(old_pair.get(1), "missing original mate")?,
            );
            let (&new1, &new2) = (
                required(new_pair.first(), "missing new pair")?,
                required(new_pair.get(1), "missing new mate")?,
            );
            let (&vu1, &vu2) = (
                required(vu_pair.first(), "missing split vertexuse")?,
                required(vu_pair.get(1), "missing split mate vertexuse")?,
            );
            let original1 = b.edgeuse(old1)?.clone();
            let original2 = b.edgeuse(old2)?.clone();
            let first = b.edgeuse_mut(new1)?;
            first.owner = original1.owner;
            first.vertexuse = Some(vu1);
            first.orientation = original1.orientation;
            b.vertexuse_mut(vu1)?.owner = VertexuseOwner::Edgeuse(Some(new1));
            if matches!(original1.owner, crate::topology::EdgeuseOwner::Loopuse(_)) {
                b.edgeuse_mut(new1)?.ccw = original1.ccw;
                if let Some(ccw) = original1.ccw {
                    b.edgeuse_mut(ccw)?.cw = Some(new1);
                }
                b.edgeuse_mut(old1)?.ccw = Some(new1);
                b.edgeuse_mut(new1)?.cw = Some(old1);
            }
            let old_vu = required(original2.vertexuse, "original mate without vertexuse")?;
            let second = b.edgeuse_mut(new2)?;
            second.owner = original2.owner;
            second.vertexuse = Some(old_vu);
            second.orientation = original2.orientation;
            b.vertexuse_mut(old_vu)?.owner = VertexuseOwner::Edgeuse(Some(new2));
            b.edgeuse_mut(old2)?.vertexuse = Some(vu2);
            b.vertexuse_mut(vu2)?.owner = VertexuseOwner::Edgeuse(Some(old2));
            if matches!(original2.owner, crate::topology::EdgeuseOwner::Loopuse(_)) {
                // Read the CURRENT CW link: the first insertion can have changed it.
                let cw = b.edgeuse(old2)?.cw;
                b.edgeuse_mut(new2)?.cw = cw;
                if let Some(cw) = cw {
                    b.edgeuse_mut(cw)?.ccw = Some(new2);
                }
                b.edgeuse_mut(old2)?.cw = Some(new2);
                b.edgeuse_mut(new2)?.ccw = Some(old2);
            }
        }
        let for_tessellation = state
            .attributes
            .find_attribute(b.key(), crate::geom::attribute::ids::TESSELLATING)
            .is_some();
        for (old_pair, new_pair) in original_uses.chunks_exact(2).zip(new_uses.chunks_exact(2)) {
            let old1 = *required(old_pair.first(), "missing original pair")?;
            let orientation = b.edgeuse(old1)?.orientation;
            for (&old, &new) in old_pair.iter().zip(new_pair) {
                ignore_status(split_uv_trim(
                    k,
                    b,
                    SplitUvInput {
                        original: old,
                        new,
                        point,
                        parameter: split_parameter,
                        scaled_zero,
                        orientation,
                        for_tessellation,
                    },
                ))?;
            }
        }
        let tolerance = b.edge(edge_id)?.tolerance;
        ignore_status(edge::set_tolerance(b, new_edge, tolerance, true, true))?;
        ignore_status(vertex::set_tolerance(b, new_vertex, tolerance, true, true))?;
        insert_brep_entity(b, EntityRef::Edge(new_edge))?;
        insert_brep_entity(b, EntityRef::Vertex(new_vertex))?;
        state.with_env(k, |env| {
            brep_notify(
                b,
                env,
                NotifyOperation::AddToBrep,
                EntityRef::Vertex(new_vertex),
                None,
                None,
            )
        })?;
        state.with_env(k, |env| {
            brep_notify(
                b,
                env,
                NotifyOperation::SplitInBrep,
                EntityRef::Edge(edge_id),
                Some(EntityRef::Edge(new_edge)),
                None,
            )
        })?;
        sync_edge_cache(b, edge_id)?;
        sync_edge_cache(b, new_edge)?;
        Ok(SplitEdgeOutput {
            split: true,
            edges: Some([edge_id, new_edge]),
            vertex: Some(new_vertex),
        })
    })
}

/// Inputs for `MakeEdgeInFaceWithVU`. Optional geometry is owned, as the native
/// operation consumes its optional trim and drop-failure record.
#[derive(Debug, Clone)]
pub struct EdgeInFaceInput {
    /// Initial target face; a shared VU face may replace it upstream.
    pub face: Option<FaceId>,
    /// Start and end VUs; null is a defined ordinary error.
    pub vertices: [Option<VertexuseId>; 2],
    /// Required 3D curve.
    pub curve: Option<Curve>,
    /// Optional UV trim curve.
    pub uv_trim: Option<Curve>,
    /// Optional state consumed with the UV trim.
    pub drop_fail:
        Option<Box<crate::surface::surface::p2::DropCurveFail<crate::topology::ObjectHandle>>>,
    /// Parameter interval of the 3D curve.
    pub interval: Extent1d,
    /// Direction of the new edge relative to its curve.
    pub orientation: OrientType,
    /// Converted `SmZoneTol3d` used for the curve.
    pub curve_zone_tol_3d: f64,
}

/// The edge created by `MakeEdgeInFaceWithVU`, and the optional split loop/face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeInFaceOutput {
    /// Newly created manifold edge.
    pub edge: EdgeId,
    /// Newly created loop when both vertices belong to the same loop.
    pub loop_: Option<LoopId>,
    /// Newly created face when insertion splits the original face.
    pub face: Option<FaceId>,
}

/// `MakeEdgeInFaceWithVU`'s defined null/editing/VU-compatibility failures.
///
/// # Errors
/// Null face/curve, disabled editing or incompatible VUs, then dependency errors
/// from curve projection/face editing. No topology success is simulated.
pub fn make_edge_in_face_with_vu(
    k: &Kernel,
    b: &mut Brep,
    input: EdgeInFaceInput,
    env: &mut edge::NotifyEnv<'_>,
    curve_ref: &dyn Fn(EdgeId, i64) -> crate::geom::object::ObjectRef,
) -> Result<EdgeInFaceOutput> {
    make_edge_in_face_with_vu_with_curve_identity(
        k,
        b,
        input,
        env,
        CurveIdentity::Legacy(curve_ref),
    )
}

/// Install the actual input Curve using checked identity issuance before `SetCurve`.
/// # Errors
/// Original construction/notification errors or checked identity issuance errors.
pub fn make_edge_in_face_with_vu_with_curve_identity(
    k: &Kernel,
    b: &mut Brep,
    input: EdgeInFaceInput,
    env: &mut edge::NotifyEnv<'_>,
    curve_identity: CurveIdentity<'_>,
) -> Result<EdgeInFaceOutput> {
    use crate::topology::{
        face::{p1 as fp1, p2 as fp2},
        loopuse as lu,
    };
    let face = input.face.ok_or_else(|| SolidError::null_pointer(OP))?;
    let mut curve = input.curve.ok_or_else(|| SolidError::null_pointer(OP))?;
    if !b.editing_enabled() {
        return Err(SolidError::sm_err(OP));
    }
    let [Some(start), Some(mut end)] = input.vertices else {
        return Err(SolidError::sm_err(OP));
    };
    for vu in [start, end] {
        if !matches!(
            b.vertexuse(vu)?.owner,
            VertexuseOwner::Edgeuse(_) | VertexuseOwner::Loopuse(_)
        ) {
            return Err(SolidError::sm_err(OP));
        }
    }
    let start_faceuse = vertexuse::faceuse(b, start)?;
    if start_faceuse != vertexuse::faceuse(b, end)? {
        return Err(SolidError::sm_err(OP));
    }
    let fu = required(start_faceuse, "VU pair has no faceuse")?;
    let actual_face = required(b.faceuse(fu)?.face, "VU faceuse has no face")?;
    let face = if face == actual_face {
        face
    } else {
        actual_face
    };
    let start_vertex = required(b.vertexuse(start)?.vertex, "start VU without vertex")?;
    let end_vertex = required(b.vertexuse(end)?.vertex, "end VU without vertex")?;
    let domain = required(b.face(face)?.surface.as_ref(), "face without surface")?
        .natural_uv_domain_with_nurb(k)?;
    let (end_orientation, t_start, t_end) = if input.orientation == OrientType::Same {
        (
            OrientType::Opposite,
            input.interval.min(),
            input.interval.max(),
        )
    } else {
        (OrientType::Same, input.interval.max(), input.interval.min())
    };
    // Decide before editing: a full periodic cut can split the boundary without
    // separating the face. A same-vertex/same-loop connection alone is insufficient.
    // Implementation restriction: only SmCrvOnSurf seams, as supplied by
    // SplitAtSeam, are recognized using their defining UV curves. Equivalent
    // SmCircle or SmBSplineCurve seams are not yet recognized and follow the
    // existing same-loop face-splitting path. This curve-type restriction is
    // an implementation limitation, not a requirement of the BRep topology.
    let new_loop_without_new_face = match curve.kind() {
        crate::curve::curve::CurveKind::CrvOnSurf(on_surf) => {
            super::p1::is_first_nonseparating_seam(
                k,
                b,
                face,
                start,
                end,
                on_surf,
                &input.interval,
            )?
        }
        _ => false,
    };

    let edge_id = b.add_edge_record(crate::topology::Edge::new([start_vertex, end_vertex]))?;
    let mut uses = Vec::new();
    let mut vertexuses = Vec::new();
    for _ in 0..4 {
        uses.push(b.add_edgeuse_record(crate::topology::Edgeuse::default())?);
    }
    for _ in 0..4 {
        vertexuses.push(b.add_vertexuse_record(Vertexuse::default())?);
    }
    register_entities(env, &[EntityRef::Edge(edge_id)]);
    for &eu in &uses {
        register_entities(env, &[EntityRef::Edgeuse(eu)]);
    }
    for &vu in &vertexuses {
        register_entities(env, &[EntityRef::Vertexuse(vu)]);
    }
    let [eu1, eu2, eu3, eu4] = uses.as_slice() else {
        return Err(SolidError::invalid_input(
            OP,
            "insertion allocation incomplete",
        ));
    };
    let [eu1, eu2, eu3, eu4] = [*eu1, *eu2, *eu3, *eu4];
    let mut uv_trim = input.uv_trim;
    let mut drop_fail = input.drop_fail;
    let mut max_dist = 0.0;
    let mut deviation = 0.0;
    if uv_trim.is_none() {
        drop_fail = None;
        let face_record = b.face(face)?;
        let surface = required(face_record.surface.as_ref(), "face without surface")?
            .clone_for_evaluation()?;
        let uv_domain = face_record.uv_domain;
        let face_tolerance = face_record.tolerance;
        let dropped = drop_edge_curve_in_face(
            k,
            b,
            face,
            &surface,
            &uv_domain,
            &curve,
            &input.interval,
            face_tolerance,
        )?;
        let mut curves = dropped.uv_curves;
        max_dist = dropped.max_drop_to_surf;
        deviation = dropped.max_approx_dev;
        if curves.len() == 1 {
            uv_trim = curves.pop();
            // pThisUVDropCurveFail is NULL and passed by value in the pinned
            // native signature, so projection cannot populate this pointer.
        }
    }
    let curve_identity = curve_identity.resolve(edge_id, &mut curve, env)?;
    // The native excessive-distance SE(SM_ERR) is logging-only.
    ignore_status(edge::set_curve(
        b,
        edge_id,
        Some(curve),
        edge::SetCurveOptions {
            delete_old_curve: false,
            ..edge::SetCurveOptions::default()
        },
        env,
        [Some(curve_identity), None],
    ))?;
    set_curve_owner(b, edge_id, env, curve_identity)?;
    b.edge_mut(edge_id)?.interval = input.interval;
    let start_lu = required(vertexuse::loopuse(b, start)?, "start VU without loopuse")?;
    let end_lu = required(vertexuse::loopuse(b, end)?, "end VU without loopuse")?;
    let start_loop = required(b.loopuse(start_lu)?.loop_, "start loopuse without loop")?;
    let end_loop = required(b.loopuse(end_lu)?.loop_, "end loopuse without loop")?;
    for ((eu, vu), orientation) in uses.into_iter().zip(vertexuses).zip([
        input.orientation,
        end_orientation,
        input.orientation,
        end_orientation,
    ]) {
        b.edge_mut(edge_id)?.edgeuses.push(eu);
        let record = b.edgeuse_mut(eu)?;
        record.edge = Some(edge_id);
        record.vertexuse = Some(vu);
        record.orientation = orientation;
        b.vertexuse_mut(vu)?.owner = VertexuseOwner::Edgeuse(Some(eu));
    }
    let mut seam = SurfParamType::Neither;
    let mut original_trim_owner = None;
    if let Some(mut trim) = uv_trim {
        // The native parameter has static type SmBSplineCurve*, so other kinds
        // cannot be supplied safely through that interface.
        if !matches!(
            trim.kind(),
            crate::curve::curve::CurveKind::BSpline(_)
                | crate::curve::curve::CurveKind::Line(_)
                | crate::curve::curve::CurveKind::Ellipse(_)
                | crate::curve::curve::CurveKind::Hyperbola(_)
                | crate::curve::curve::CurveKind::Parabola(_)
        ) {
            return Err(SolidError::invalid_input(
                OP,
                "UV trim must derive from SmBSplineCurve",
            ));
        }
        // Attach the first curve before copying, matching the native lifecycle.
        let top = if b.edgeuse(eu1)?.orientation == OrientType::Same {
            eu1
        } else {
            eu2
        };
        trim.set_owner(Some((env.key_of)(EntityRef::Edgeuse(top))), None);
        b.edgeuse_mut(top)?.uv_trim_curve = Some(trim);
        b.edgeuse_mut(top)?.drop_curve_fail = drop_fail;
        let mut copy =
            required(b.edgeuse(top)?.uv_trim_curve.as_ref(), "trim disappeared")?.copy(k)?;
        let fail_copy = b.edgeuse(top)?.drop_curve_fail.as_ref().map(|fail| {
            let mut copy = Box::new(crate::surface::surface::p2::DropCurveFail::default());
            copy.assign(fail);
            copy
        });
        seam = required(b.edgeuse(top)?.uv_trim_curve.as_ref(), "trim disappeared")?
            .is_domain_boundary(k, &domain, crate::math::EFF_ZERO)?;
        if seam != SurfParamType::Neither {
            let translation = match seam {
                SurfParamType::UMin => Vec3::new(domain.x_length(), 0.0, 0.0),
                SurfParamType::UMax => Vec3::new(-domain.x_length(), 0.0, 0.0),
                SurfParamType::VMin => Vec3::new(0.0, domain.y_length(), 0.0),
                SurfParamType::VMax => Vec3::new(0.0, -domain.y_length(), 0.0),
                _ => Vec3::ZERO,
            };
            let mut transform = Axis2Placement::default();
            transform.translate(translation);
            ignore_status(copy.transform(k, &transform, None))?;
        }
        let bottom = if b.edgeuse(eu4)?.orientation == OrientType::Same {
            eu4
        } else {
            eu3
        };
        copy.set_owner(Some((env.key_of)(EntityRef::Edgeuse(bottom))), None);
        b.edgeuse_mut(bottom)?.uv_trim_curve = Some(copy);
        b.edgeuse_mut(bottom)?.drop_curve_fail = fail_copy;
        original_trim_owner = Some(top);
    }
    let distance = (max_dist * max_dist + deviation * deviation).sqrt();
    b.edge_mut(edge_id)?.tolerance =
        crate::math::max3(distance, input.curve_zone_tol_3d, b.tolerance());
    face_notify(b, face, env, NotifyOperation::PreEdit)?;
    insert_brep_entity(b, EntityRef::Edge(edge_id))?;
    let mut output = EdgeInFaceOutput {
        edge: edge_id,
        loop_: None,
        face: None,
    };
    if start_loop == end_loop {
        edge::insert_manifold_edge_end(b, edge_id, Some(start), true)?;
        let old_top = required(
            vertexuse::loopuse(b, start)?,
            "inserted start without loopuse",
        )?;
        remove_old_loop_vertices(b, start)?;
        if start_vertex == end_vertex {
            let fu1 = required(fp2::upward_faceuse(b, face)?, "face without upward use")?;
            let fu2 = required(b.faceuse(fu1)?.mate, "faceuse without mate")?;
            let lu1 = *required(b.faceuse(fu1)?.loopuses.first(), "face without top loopuse")?;
            let lu2 = *required(
                b.faceuse(fu2)?.loopuses.first(),
                "face without bottom loopuse",
            )?;
            for (eu, lu) in [(eu1, lu1), (eu4, lu1), (eu2, lu2), (eu3, lu2)] {
                b.edgeuse_mut(eu)?.owner = crate::topology::EdgeuseOwner::Loopuse(Some(lu));
            }
            let curve = required(
                b.edge(edge_id)?.curve.as_ref(),
                "inserted edge without curve",
            )?
            .clone();
            let tolerance = b.tolerance();
            end = find_vertex_sector(
                k,
                b,
                VertexSectorInput {
                    face,
                    vertex: end_vertex,
                    curve: &curve,
                    uv_trim: None,
                    parameter: t_end,
                    other_parameter: t_start,
                    orientation: end_orientation,
                    tolerance,
                    validate_sector: false,
                },
            )?
            .unwrap_or(end);
        }
        edge::insert_manifold_edge_end(b, edge_id, Some(end), false)?;
        let (new_face, new_fu1, new_fu2) = if new_loop_without_new_face {
            let fu1 = required(fp2::upward_faceuse(b, face)?, "face without upward use")?;
            let fu2 = required(b.faceuse(fu1)?.mate, "faceuse without mate")?;
            (None, fu1, fu2)
        } else {
            // separating cut: create one new face, in the split face's shell as `GetShell`
            // reads it, through its primary face use (the port's `Face::shell` is a cache
            // the native shell moves do not maintain)
            let cached_shell = b.face(face)?.shell;
            let shell = match b.face(face)?.faceuse {
                Some(fu) => b.faceuse(fu)?.shell.unwrap_or(cached_shell),
                None => cached_shell,
            };
            let new_face = b.add_face_record(crate::topology::Face::new(shell))?;
            let mut surface =
                required(b.face(face)?.surface.as_ref(), "split face without surface")?
                    .copy_and_add_analytics(k)?;
            let new_face_owner = (env.key_of)(EntityRef::Face(new_face));
            surface.set_owner(Some(new_face_owner));
            b.face_mut(new_face)?.surface = Some(surface);
            b.set_geometry_owner_key(EntityRef::Face(new_face), Some(new_face_owner));
            set_face_tolerance(b, new_face, b.tolerance(), true, true)?;
            b.face_mut(new_face)?.uv_domain = b.face(face)?.uv_domain;
            let new_fu1 = b.add_faceuse_record(crate::topology::Faceuse::default())?;
            let new_fu2 = b.add_faceuse_record(crate::topology::Faceuse::default())?;
            register_entities(
                env,
                &[
                    EntityRef::Face(new_face),
                    EntityRef::Faceuse(new_fu1),
                    EntityRef::Faceuse(new_fu2),
                ],
            );
            b.face_mut(new_face)?.faceuse = Some(new_fu1);
            for (fu, mate, orientation) in [
                (new_fu1, new_fu2, OrientType::Same),
                (new_fu2, new_fu1, OrientType::Opposite),
            ] {
                let record = b.faceuse_mut(fu)?;
                record.face = Some(new_face);
                record.mate = Some(mate);
                record.orientation = orientation;
            }
            let up = required(fp2::upward_faceuse(b, face)?, "face without upward use")?;
            let down = required(b.faceuse(up)?.mate, "faceuse without mate")?;
            let up_shell = required(b.faceuse(up)?.shell, "upward use without shell")?;
            let down_shell = required(b.faceuse(down)?.shell, "downward use without shell")?;
            shell_post_insert(b, up_shell, new_fu1)?;
            shell_post_insert(b, down_shell, new_fu2)?;
            insert_brep_entity(b, EntityRef::Face(new_face))?;
            (Some(new_face), new_fu1, new_fu2)
        };
        let new_loop = b.add_loop_record(Loop {
            face: new_face.unwrap_or(face),
            edges: Vec::new(),
            loopuse: None,
        })?;
        let new_lu1 = b.add_loopuse_record(Loopuse::default())?;
        let new_lu2 = b.add_loopuse_record(Loopuse::default())?;
        register_entities(
            env,
            &[
                EntityRef::Loop(new_loop),
                EntityRef::Loopuse(new_lu1),
                EntityRef::Loopuse(new_lu2),
            ],
        );
        b.loop_mut(new_loop)?.loopuse = Some(new_lu1);
        for (lu, mate, fu) in [(new_lu1, new_lu2, new_fu1), (new_lu2, new_lu1, new_fu2)] {
            *b.loopuse_mut(lu)? = Loopuse {
                loop_: Some(new_loop),
                mate: Some(mate),
                faceuse: Some(fu),
                orientation: OrientType::Same,
                ..Loopuse::default()
            };
            b.faceuse_mut(fu)?.loopuses.push(lu);
        }
        let old_bottom = required(b.loopuse(old_top)?.mate, "old loopuse without mate")?;
        for (lu, eu) in [
            (old_top, eu1),
            (old_bottom, eu2),
            (new_lu1, eu4),
            (new_lu2, eu3),
        ] {
            lu::collect_edgeuses(b, lu, Some(eu))?;
        }
        redistribute_seam_trims(k, b, edge_id, original_trim_owner, seam, &domain)?;
        ignore_status(edge::set_tolerance(
            b,
            edge_id,
            crate::math::max2(input.curve_zone_tol_3d, b.tolerance()),
            true,
            true,
        ))?;
        if let Some(new_face) = new_face {
            // separating cut
            if !lu::is_outer_loop(b, start_loop)? {
                if let Some(orientation) = crate::error::ignored(compute_loop_orientation(
                    k, b, new_face, new_loop, None, true,
                ))? {
                    if orientation != OrientType::Same
                        && crate::error::ignored(compute_loop_orientation(
                            k, b, face, start_loop, None, true,
                        ))?
                        .is_some()
                    {
                        // The old-loop orientation is diagnostic only, even if opposite.
                        fp1::remove_loop(b, new_face, new_loop)?;
                        fp1::remove_loop(b, face, start_loop)?;
                        for (loop_, orientation) in [
                            (start_loop, OrientType::Same),
                            (new_loop, OrientType::Opposite),
                        ] {
                            let (a, z) = lu::loop_loopuses(b, loop_)?;
                            let z = required(z, "loop without mate")?;
                            b.loopuse_mut(a)?.orientation = orientation;
                            b.loopuse_mut(z)?.orientation = orientation;
                        }
                        fp1::insert_loop(b, new_face, start_loop)?;
                        fp1::insert_loop(b, face, new_loop)?;
                    }
                }
            }
            redistribute_inner_loops(k, b, face, new_face, start_loop, new_loop, env)?;
        } else {
            // Both boundary cycles belong to this face. Put one seam side first;
            // the orthogonal seam will join the two cycles into its outer loop.
            faceuse_remove_loopuse(b, new_fu1, old_top)?;
            faceuse_remove_loopuse(b, new_fu2, old_bottom)?;
            faceuse_pre_insert_loopuse(b, new_fu1, old_top)?;
            faceuse_pre_insert_loopuse(b, new_fu2, old_bottom)?;
            for (i, loop_) in fp2::loops(b, face, false, None)?.into_iter().enumerate() {
                let orientation = if i == 0 {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                };
                let (a, z) = lu::loop_loopuses(b, loop_)?;
                let z = required(z, "loop without mate")?;
                b.loopuse_mut(a)?.orientation = orientation;
                b.loopuse_mut(z)?.orientation = orientation;
            }
        }
        output.loop_ = Some(new_loop);
        output.face = new_face;
    } else {
        let (mut keep, mut delete) = if lu::is_outer_loop(b, start_loop)? {
            (start_loop, end_loop)
        } else {
            (end_loop, start_loop)
        };
        let keep_lu = required(b.loop_(keep)?.loopuse, "kept loop without loopuse")?;
        let delete_lu = required(b.loop_(delete)?.loopuse, "deleted loop without loopuse")?;
        if lu::is_vertex_loopuse(b, keep_lu)? && !lu::is_vertex_loopuse(b, delete_lu)? {
            core::mem::swap(&mut keep, &mut delete);
        }
        edge::insert_manifold_edge_end(b, edge_id, Some(start), true)?;
        edge::insert_manifold_edge_end(b, edge_id, Some(end), false)?;
        let (mut top, bottom) = lu::loop_loopuses(b, keep)?;
        let mut bottom = required(bottom, "kept loopuse without mate")?;
        let fu = required(b.loopuse(top)?.faceuse, "kept loopuse without faceuse")?;
        if b.faceuse(fu)?.orientation != OrientType::Same {
            core::mem::swap(&mut top, &mut bottom);
        }
        lu::collect_edgeuses(b, top, Some(eu1))?;
        lu::collect_edgeuses(b, bottom, Some(eu2))?;
        remove_old_loop_vertices(b, start)?;
        remove_old_loop_vertices(b, end)?;
        redistribute_seam_trims(k, b, edge_id, original_trim_owner, seam, &domain)?;
        ignore_status(fp1::remove_loop(b, face, delete))?;
        delete_loop_records(b, delete, env)?;
    }
    brep_notify(
        b,
        env,
        NotifyOperation::AddToBrep,
        EntityRef::Edge(edge_id),
        None,
        Some(curve_identity),
    )?;
    if let Some(new_face) = output.face {
        brep_notify(
            b,
            env,
            NotifyOperation::SplitInBrep,
            EntityRef::Face(face),
            Some(EntityRef::Face(new_face)),
            None,
        )?;
    } else {
        brep_notify(
            b,
            env,
            NotifyOperation::TrimNoSplitInBrep,
            EntityRef::Face(face),
            Some(EntityRef::Edge(edge_id)),
            None,
        )?;
    }
    if original_trim_owner.is_some() {
        ignore_status(edge::set_tolerance(b, edge_id, 2.0 * max_dist, true, true))?;
        ignore_status(edge::set_tolerance(b, edge_id, 2.0 * max_dist, true, true))?;
    }
    vertex::refresh_tolerance(b, start_vertex, false)?;
    vertex::refresh_tolerance(b, end_vertex, false)?;
    // Opt-in (WR-469): each vertex's tolerance grows to twice its gap to the new edge's end.
    if k.fixes.new_edge_vertex_tolerance_covers_gap {
        let record = b.edge(edge_id)?;
        let curve = required(record.curve.as_ref(), "new edge without curve")?;
        let (curve_start, curve_end) = (
            curve.evaluate_point(k, t_start)?,
            curve.evaluate_point(k, t_end)?,
        );
        let start_gap = curve_start.distance(vertex::point(b, start_vertex)?);
        let end_gap = curve_end.distance(vertex::point(b, end_vertex)?);
        vertex::set_tolerance(b, start_vertex, 2.0 * start_gap, true, true)?;
        vertex::set_tolerance(b, end_vertex, 2.0 * end_gap, true, true)?;
    }
    // These skeleton caches are derived from the authoritative use lists.
    sync_face_cache(b, face)?;
    if let Some(face) = output.face {
        sync_face_cache(b, face)?;
    }
    sync_edge_cache(b, edge_id)?;
    Ok(output)
}

#[derive(Clone, Copy)]
struct VertexSectorInput<'a> {
    face: FaceId,
    vertex: VertexId,
    curve: &'a Curve,
    uv_trim: Option<&'a Curve>,
    parameter: f64,
    other_parameter: f64,
    orientation: OrientType,
    tolerance: f64,
    validate_sector: bool,
}

/// `pFace->FindVertexSector(pVertex, *pCurve, pUVTrim, dT, dTOther, eOrient, tol,
/// bValidateSector, rpVU, ..)` (unit `topology.face_p4`; `bUVSpaceOkay` at its default
/// `TRUE`; the possible-problem outputs are unused): the vertex use of the sector.
fn find_vertex_sector(
    k: &Kernel,
    b: &mut Brep,
    input: VertexSectorInput<'_>,
) -> Result<Option<VertexuseId>> {
    let VertexSectorInput {
        face,
        vertex,
        curve,
        uv_trim,
        parameter,
        other_parameter,
        orientation,
        tolerance,
        validate_sector,
    } = input;
    let sector = crate::topology::face::p4::find_vertex_sector(
        k,
        b,
        face,
        crate::topology::face::p4::FindVertexSectorArgs {
            vertex,
            curve,
            uv_curve: uv_trim,
            param_at_vertex: parameter,
            param_ivl_end: other_parameter,
            orientation,
            zone_tol_3d: tolerance,
            validate_sector,
            uv_space_okay: true,
        },
    )?;
    Ok(sector.vertexuse)
}

fn remove_old_loop_vertices(b: &mut Brep, vu: VertexuseId) -> Result<()> {
    crate::topology::brep::p1::rm_and_del_old_loop_vertices(b, vu)
}

/// `sm_RedistributeSeamUVTrimCurves(pNewE, pUVTrimCurve, ..)`: the supplied trim
/// curve is now owned by `original_trim_owner`; the callee only tests it for null.
/// Its swapped/unswapped result is unused upstream.
fn redistribute_seam_trims(
    k: &Kernel,
    b: &mut Brep,
    edge: EdgeId,
    original_trim_owner: Option<crate::topology::EdgeuseId>,
    seam: SurfParamType,
    domain: &crate::geom::extent2d::Extent2d,
) -> Result<()> {
    let trim = match original_trim_owner {
        Some(owner) => Some(required(
            b.edgeuse(owner)?.uv_trim_curve.clone(),
            "supplied UV trim curve without owner",
        )?),
        None => None,
    };
    crate::topology::brep::p1::redistribute_seam_uv_trim_curves(
        k,
        b,
        edge,
        trim.as_ref(),
        seam,
        domain,
    )
    .map(|_| ())
}

fn set_face_tolerance(
    b: &mut Brep,
    face: FaceId,
    tolerance: f64,
    update_only_if_larger: bool,
    cascade_to_boundaries: bool,
) -> Result<()> {
    crate::topology::face::p3::set_tolerance(
        b,
        face,
        tolerance,
        update_only_if_larger,
        cascade_to_boundaries,
    )
}

fn compute_loop_orientation(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    loop_: LoopId,
    uv_bounds: Option<&mut crate::geom::extent2d::Extent2d>,
    do_3d_loop_computation: bool,
) -> Result<OrientType> {
    crate::topology::face::p3::compute_loop_orientation(
        k,
        b,
        face,
        loop_,
        uv_bounds,
        do_3d_loop_computation,
    )
}

/// `pFU->Remove(pLU)`: takes the loop use off the face use's list (`SM_ERR` when it is
/// not there).
fn faceuse_remove_loopuse(
    b: &mut Brep,
    fu: crate::topology::FaceuseId,
    lu: crate::topology::LoopuseId,
) -> Result<()> {
    if b.loopuse(lu)?.faceuse != Some(fu) {
        return Err(SolidError::sm_err(OP));
    }
    let list = &mut b.faceuse_mut(fu)?.loopuses;
    let Some(at) = list.iter().position(|&x| x == lu) else {
        return Err(SolidError::sm_err(OP));
    };
    list.remove(at);
    b.loopuse_mut(lu)?.faceuse = None;
    Ok(())
}

/// `pFU->PreInsert(pLU)`: puts the loop use first on the face use's list.
fn faceuse_pre_insert_loopuse(
    b: &mut Brep,
    fu: crate::topology::FaceuseId,
    lu: crate::topology::LoopuseId,
) -> Result<()> {
    b.faceuse_mut(fu)?.loopuses.insert(0, lu);
    b.loopuse_mut(lu)?.faceuse = Some(fu);
    Ok(())
}

fn shell_post_insert(
    b: &mut Brep,
    shell: ShellId,
    faceuse: crate::topology::FaceuseId,
) -> Result<()> {
    use crate::topology::shell::ShellMembers;
    let members = &mut b.shell_mut(shell)?.members;
    match members {
        ShellMembers::Empty => *members = ShellMembers::Faceuses(vec![faceuse]),
        ShellMembers::Faceuses(list) => list.push(faceuse),
        _ => {
            return Err(SolidError::invalid_input(
                OP,
                "faceuse insertion into a non-faceuse shell",
            ))
        }
    }
    b.faceuse_mut(faceuse)?.shell = Some(shell);
    let members = match &b.shell(shell)?.members {
        ShellMembers::Faceuses(members) => members.clone(),
        _ => {
            return Err(SolidError::invalid_input(
                OP,
                "new faceuse missing from shell list",
            ))
        }
    };
    for (i, &fu) in members.iter().enumerate() {
        let next = members.get((i + 1) % members.len()).copied();
        let previous = members
            .get(if i == 0 { members.len() - 1 } else { i - 1 })
            .copied();
        b.faceuse_mut(fu)?.sibling_links = Some((next, previous));
    }
    Ok(())
}

fn delete_loop_records(b: &mut Brep, loop_: LoopId, env: &mut edge::NotifyEnv<'_>) -> Result<()> {
    let (a, z) = crate::topology::loopuse::loop_loopuses(b, loop_)?;
    let z = required(z, "deleted loopuse without mate")?;
    for entity in [
        EntityRef::Loop(loop_),
        EntityRef::Loopuse(a),
        EntityRef::Loopuse(z),
    ] {
        let object = (env.key_of)(entity);
        ignore_status(crate::topology::topology::notify(
            env.records,
            env.attributes,
            env.context.as_deref_mut(),
            NotifyOperation::Destruction,
            object,
            Some(object),
            None,
            None,
        ))?;
    }
    // `delete` of the loop and both loop uses. An arena record cannot be freed, so
    // (as part 1's sm_RmAndDel_OldLoopVertices) the records are reset: RemoveLoop
    // already unlinked both loop uses from their face uses.
    *b.loopuse_mut(a)? = Loopuse::default();
    *b.loopuse_mut(z)? = Loopuse::default();
    let record = b.loop_mut(loop_)?;
    record.edges.clear();
    record.loopuse = None;
    Ok(())
}

fn sync_edge_cache(b: &mut Brep, edge_id: EdgeId) -> Result<()> {
    // The shared native-graph importer stores primary/mate VU order here,
    // independently of their orientation relative to the 3D curve.
    let primary_vertex = required(
        edge::vertex(b, edge_id)?,
        "edge cache without primary vertex",
    )?;
    let mate_vertex = required(
        edge::other_vertex(b, edge_id, Some(primary_vertex))?,
        "edge cache without mate vertex",
    )?;
    b.edge_mut(edge_id)?.vertices = [primary_vertex, mate_vertex];
    Ok(())
}

fn sync_face_cache(b: &mut Brep, face: FaceId) -> Result<()> {
    use crate::topology::loopuse as lu;
    let loops = crate::topology::face::p2::loops(b, face, false, None)?;
    b.face_mut(face)?.loops.clone_from(&loops);
    for loop_ in loops {
        let (primary, _) = lu::loop_loopuses(b, loop_)?;
        b.loop_mut(loop_)?.face = face;
        let mut edges = Vec::new();
        if matches!(b.loopuse(primary)?.start, LoopuseStart::Edgeuse(_)) {
            for eu in lu::edgeuses(b, primary, None, None)? {
                let record = b.edgeuse(eu)?;
                edges.push(crate::topology::LoopEdge {
                    edge: required(record.edge, "loop edgeuse without edge")?,
                    forward: record.orientation == OrientType::Same,
                });
            }
        }
        b.loop_mut(loop_)?.edges = edges;
    }
    Ok(())
}

fn redistribute_inner_loops(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    new_face: FaceId,
    start_loop: LoopId,
    new_loop: LoopId,
    env: &mut edge::NotifyEnv<'_>,
) -> Result<()> {
    use crate::intersect::point_class::{PointClassType, PointClassification};
    use crate::topology::{
        face::{p1 as fp1, p2 as fp2},
        loopuse as lu,
    };
    let outer = |b: &Brep, face| -> Result<crate::topology::LoopuseId> {
        let fu = required(
            fp2::upward_faceuse(b, face)?,
            "split face without upward use",
        )?;
        b.faceuse(fu)?
            .loopuses
            .first()
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP, "split face without outer loopuse"))
    };
    let original_outer = outer(b, face)?;
    let new_outer = outer(b, new_face)?;
    let missing_original = lu::is_missing_seam_at_vertices(k, b, original_outer)?;
    let missing_new = lu::is_missing_seam_at_vertices(k, b, new_outer)?;
    let classify_to_original = !missing_original;
    let mut loops = fp2::loops(b, face, false, None)?;

    // Opt-in (WR-527, #1526/#1527): on a plane the region a loop bounds lies within the
    // loop's box, so an inner loop sampled outside the box of the new face's outer loop
    // is not in the new face and stays: only when every edge has a box.
    let new_outer_box = if k.fixes.boolean_skips_faces_apart {
        new_face_outer_box(k, b, face, new_face)?
    } else {
        None
    };

    let mut i = 1;
    while let Some(&loop_) = loops.get(i) {
        if loop_ == new_loop || loop_ == start_loop {
            i += 1;
            continue;
        }
        let edges = lu::loop_edges(b, loop_)?;
        let (mut point, mut tolerance) = if let Some(edge) = edges.first() {
            edge_sample(k, b, required(*edge, "inner loop has null edge")?)?
        } else {
            let vertices = lu::loop_vertices(b, loop_)?;
            let v = required(
                *required(vertices.first(), "empty inner vertex loop")?,
                "inner loop has null vertex",
            )?;
            (b.vertex(v)?.point, b.vertex(v)?.tolerance)
        };
        // Opt-in (WR-527): no move - the test point is outside the new face's outer loop
        // box, even at the retry tolerance below
        if new_outer_box.is_some_and(|bbox| !bbox.contains_point(point, tolerance * 100.0)) {
            i += 1;
            continue;
        }
        let target = if classify_to_original { face } else { new_face };
        let other = if classify_to_original { new_face } else { face };
        let mut pc = PointClassification::<EntityRef>::new(tolerance);
        let need_move = if missing_new && missing_original {
            // Both statuses and the drop's success flag are ignored natively.
            let _old_domain = computed_uv_domain(k, b, face)?;
            let new_domain = computed_uv_domain(k, b, new_face)?;
            let surface = required(b.face(face)?.surface.as_ref(), "split face without surface")?;
            let uv = crate::error::ignored(surface.drop_point::<crate::topology::ObjectHandle>(
                k,
                point,
                &surface.natural_uv_domain_with_nurb(k)?,
                &crate::surface::surface::p5::DropPointOptions::default(),
            ))?
            .map_or(crate::geom::vector2d::Vec2::UNDEF, |drop| drop.result_uv);
            new_domain.contains_point(uv, 0.0)
        } else {
            classify_with_retries(
                k,
                b,
                FacePointInput {
                    face: target,
                    point,
                    tolerance,
                    intersect_boundaries: false,
                    guess: None,
                    outer_only: true,
                },
                classify_to_original,
                &mut pc,
            )?;
            if classify_to_original && pc.point_class() == PointClassType::Unknown {
                true
            } else if classify_to_original && pc.point_class() != PointClassType::Unknown {
                let target_edges = face_outer_edges(b, target)?;
                let other_edges = face_outer_edges(b, other)?;
                let no_nesting = target_edges.is_empty()
                    || other_edges.is_empty()
                    || target_edges.iter().any(|e| other_edges.contains(e));
                if no_nesting {
                    false
                } else {
                    classify_with_retries(
                        k,
                        b,
                        FacePointInput {
                            face: other,
                            point,
                            tolerance,
                            intersect_boundaries: false,
                            guess: None,
                            outer_only: true,
                        },
                        classify_to_original,
                        &mut pc,
                    )?;
                    if pc.point_class() == PointClassType::Unknown {
                        false
                    } else {
                        let edge =
                            *required(other_edges.first(), "nested other face without edge")?;
                        (point, tolerance) = edge_sample(k, b, edge)?;
                        classify_with_retries(
                            k,
                            b,
                            FacePointInput {
                                face: target,
                                point,
                                tolerance,
                                intersect_boundaries: false,
                                guess: None,
                                outer_only: true,
                            },
                            classify_to_original,
                            &mut pc,
                        )?;
                        pc.point_class() != PointClassType::Unknown
                    }
                }
            } else {
                // With a missing seam, an inside-new-face classification moves
                // the loop; UNKNOWN keeps it (the rest of that branch is debug).
                pc.point_class() != PointClassType::Unknown
            }
        };
        if need_move {
            face_notify(b, face, env, NotifyOperation::PreEdit)?;
            face_notify(b, new_face, env, NotifyOperation::PreEdit)?;
            ignore_status(fp1::remove_loop(b, face, loop_))?;
            ignore_status(fp1::insert_loop(b, new_face, loop_))?;
            let refreshed = fp2::loops(b, face, false, None)?;
            if refreshed.len() >= loops.len() {
                return Err(SolidError::invalid_input(
                    OP,
                    "inner-loop transfer made no progress",
                ));
            }
            loops = refreshed;
            // C++ resets its for-loop index to zero, then increments it.
            i = 1;
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// The box of `new_face`'s outer loop edges, each grown by its tolerance, when both
/// faces lie on planes and every one of those edges has a curve whose box can be
/// computed (the fork's `sNewFOuterBox`, WR-527).
fn new_face_outer_box(
    k: &Kernel,
    b: &Brep,
    face: FaceId,
    new_face: FaceId,
) -> Result<Option<crate::geom::extent3d::Extent3d>> {
    use crate::nurbs::b_spline_curve::BoundingBoxRequest;
    use crate::surface::surface::SurfaceKind;
    let on_plane = |f: FaceId| -> Result<bool> {
        Ok(b.face(f)?
            .surface
            .as_ref()
            .is_some_and(|s| matches!(s.kind(), SurfaceKind::Plane(_))))
    };
    if !on_plane(face)? || !on_plane(new_face)? {
        return Ok(None);
    }
    let edges = face_outer_edges(b, new_face)?;
    if edges.is_empty() {
        return Ok(None);
    }
    let req = BoundingBoxRequest {
        normal: true,
        pseudo: false,
        polar: false,
    };
    let mut outer_box = crate::geom::extent3d::Extent3d::new();
    for edge in edges {
        let record = b.edge(edge)?;
        let Some(curve) = record.curve.as_ref() else {
            return Ok(None);
        };
        let Some(mut edge_box) = curve
            .calculate_bounding_box(k, &record.interval, req, true)
            .ok()
            .and_then(|boxes| boxes.normal)
        else {
            return Ok(None);
        };
        edge_box.expand_absolute(record.tolerance);
        outer_box = outer_box.union(&edge_box);
    }
    Ok(Some(outer_box))
}

fn edge_sample(k: &Kernel, b: &Brep, edge: EdgeId) -> Result<(Point3, f64)> {
    let record = b.edge(edge)?;
    let point = required(record.curve.as_ref(), "sample edge without curve")?
        .evaluate_point(k, record.interval.evaluate(0.34567))?;
    Ok((point, record.tolerance))
}

fn face_outer_edges(b: &Brep, face: FaceId) -> Result<Vec<EdgeId>> {
    // SmFace::GetEdges(..., NULL, TRUE): only the upward outer loop, unique edges.
    let mut result = Vec::new();
    for loop_ in crate::topology::face::p2::outer_loops(b, face)? {
        for edge in crate::topology::loopuse::loop_edges(b, loop_)? {
            let edge = required(edge, "outer loop without edge")?;
            if !result.contains(&edge) {
                result.push(edge);
            }
        }
    }
    Ok(result)
}

fn computed_uv_domain(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
) -> Result<crate::geom::extent2d::Extent2d> {
    let initial = b.face(face)?.uv_domain;
    Ok(crate::error::ignored(
        crate::topology::face::p1::calculate_uv_domain_from_uv_trim_curves(
            k, b, face, true, false, false,
        ),
    )?
    .map_or(initial, |out| out.domain))
}

#[derive(Clone, Copy)]
struct FacePointInput {
    face: FaceId,
    point: Point3,
    tolerance: f64,
    intersect_boundaries: bool,
    guess: Option<crate::geom::vector2d::Point2>,
    outer_only: bool,
}

fn classify_with_retries(
    k: &Kernel,
    b: &mut Brep,
    mut input: FacePointInput,
    classify_to_original: bool,
    pc: &mut crate::intersect::point_class::PointClassification<EntityRef>,
) -> Result<()> {
    if !ray_success(&point_3d_classify(k, b, &input, pc))? {
        input.tolerance *= 100.0;
        if !ray_success(&point_3d_classify(k, b, &input, pc))? {
            pc.set_class_object(
                if classify_to_original {
                    crate::intersect::point_class::PointClassType::Unknown
                } else {
                    crate::intersect::point_class::PointClassType::Face
                },
                None,
            );
        }
    }
    Ok(())
}

/// `pTgtFace->Point3DClassify(point, tol, bDoBoundaryIntersections, sPointClass,
/// pOptGuessUV, bTgtLoopsOnly)` ([`crate::query::face::p1::point_3d_classify`]), with
/// no target loop list (the outer loop when `outer_only`).
fn point_3d_classify(
    k: &Kernel,
    b: &mut Brep,
    input: &FacePointInput,
    pc: &mut crate::intersect::point_class::PointClassification<EntityRef>,
) -> Result<()> {
    let FacePointInput {
        face,
        point,
        tolerance,
        intersect_boundaries,
        guess,
        outer_only,
    } = *input;
    crate::query::face::p1::point_3d_classify(
        k,
        b,
        face,
        point,
        tolerance,
        intersect_boundaries,
        pc,
        crate::query::face::p1::Point3dClassifyOptions {
            guess_uv: guess,
            tgt_loops_only: outer_only,
            ..crate::query::face::p1::Point3dClassifyOptions::default()
        },
    )
}

/// Geometry-only projection sequence used by `MakeEdgeInFaceWithVU`.
///
/// Tries face tolerance, then 10 and 300 times that tolerance, followed by
/// `DropAndTrimCurve` with 100 times the tolerance and both flags true. This
/// prefix is exposed independently so it can be compared before topology editing
/// dependencies become available. It does not create or insert an edge.
///
/// # Errors
/// Propagates projection errors and the actual missing callee's deferral.
pub fn drop_edge_curve(
    k: &Kernel,
    surface: &Surface,
    domain: &crate::geom::extent2d::Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    face_tolerance: f64,
) -> Result<EdgeCurveDrop> {
    for factor in [1.0, 10.0, 300.0] {
        let (uv_curves, max_drop_to_surf, max_approx_dev) =
            drop_curve(k, surface, domain, curve, interval, face_tolerance * factor)?;
        if !uv_curves.is_empty() {
            return Ok(EdgeCurveDrop {
                uv_curves,
                max_drop_to_surf,
                max_approx_dev,
            });
        }
    }
    let dropped = surface.drop_and_trim_curve::<crate::topology::ObjectHandle>(
        k,
        domain,
        curve,
        interval,
        face_tolerance * 100.0,
        true,
        true,
        None,
    )?;
    Ok(EdgeCurveDrop {
        uv_curves: dropped.uv_curves,
        max_drop_to_surf: dropped.max_drop_to_surf,
        max_approx_dev: dropped.max_approx_dev,
    })
}

/// Projection prefix outputs, without any topology mutation.
#[derive(Debug)]
pub struct EdgeCurveDrop {
    /// Projected UV curves; only a single result is adopted by the insertion body.
    pub uv_curves: Vec<Curve>,
    /// Largest distance of a dropped point from the curve.
    pub max_drop_to_surf: f64,
    /// Largest approximation deviation from the surface normal line.
    pub max_approx_dev: f64,
}

/// [`drop_edge_curve`] onto the surface of face `face` of `b` with the face's own
/// UV domain and tolerance: the projection prefix of `MakeEdgeInFaceWithVU` for a
/// face-owned surface, which classifies and builds its caches through the face.
///
/// # Errors
/// As [`drop_edge_curve`], and those of the face classification.
pub fn drop_edge_curve_on_face(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    curve: &Curve,
    interval: &Extent1d,
) -> Result<EdgeCurveDrop> {
    let record = b.face(face)?;
    let surface =
        required(record.surface.as_ref(), "face without surface")?.clone_for_evaluation()?;
    let (uv_domain, tolerance) = (record.uv_domain, record.tolerance);
    drop_edge_curve_in_face(k, b, face, &surface, &uv_domain, curve, interval, tolerance)
}

/// [`drop_edge_curve`] onto face `face` of `b`, whose surface `surface` is (a copy for
/// evaluation): upstream's surface is owned by the face, so `DropCurve` fetches the
/// face's cache (made here when its start point drop first asks) but, with that cache's
/// point testing off for the whole call, never classifies a drop on the face.
#[allow(clippy::too_many_arguments)]
fn drop_edge_curve_in_face(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    surface: &Surface,
    domain: &crate::geom::extent2d::Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    face_tolerance: f64,
) -> Result<EdgeCurveDrop> {
    let mut cache_made = false;
    for factor in [1.0, 10.0, 300.0] {
        // SmSurface::DropCurve turns the face cache's point testing off for its whole call
        // (`sChange1`, SmSurfaceDropLib.cpp:4315), so its start point drop keeps every
        // fast-drop solution without classifying it on the face; the face's cache is
        // still made there (`smsurf_GetSurfaceCache`, which may create its UV trims)
        let mut classify = |k: &Kernel, _: crate::math::Point2, _: f64| -> Result<bool> {
            if !cache_made {
                cache_made = true;
                // (`GetOwner()` is NULL while `CreateUVTrimCurve(s)` clears it: no face)
                if !b.is_surface_owner_cleared(face) {
                    crate::query::face::p2::face_surface_cache(k, b, face)?.release();
                }
            }
            Ok(true)
        };
        // the virtual SmSurface::DropCurve with the defaults bKeepAllDropCurves = TRUE,
        // no failure record, bCreatingUVTrimCurve = TRUE
        let dropped = surface.drop_curve_face_tested::<crate::surface::surface::p4::NoObject>(
            k,
            domain,
            curve,
            interval,
            face_tolerance * factor,
            true,
            None,
            true,
            Some(&mut classify),
        )?;
        if !dropped.uv_curves.is_empty() {
            return Ok(EdgeCurveDrop {
                uv_curves: dropped.uv_curves,
                max_drop_to_surf: dropped.max_drop_to_surf,
                max_approx_dev: dropped.max_approx_dev,
            });
        }
    }
    // the face's surface cache (upstream's tracer fetches it for the face-owned surface)
    let mut cache = crate::surface::surface::p1::SurfaceSolveContext::build_surface_cache(
        &mut crate::query::brep::BrepFaceServices::new(&mut *b),
        k,
        surface,
        Some(crate::topology::brep_cache::CacheObject::Surface(face)),
    )?;
    let dropped = surface.drop_and_trim_curve_in::<
        crate::topology::ObjectHandle,
        crate::topology::brep_cache::CacheObject,
    >(
        k,
        Some(&mut cache),
        domain,
        curve,
        interval,
        face_tolerance * 100.0,
        true,
        true,
        None,
    )?;
    Ok(EdgeCurveDrop {
        uv_curves: dropped.uv_curves,
        max_drop_to_surf: dropped.max_drop_to_surf,
        max_approx_dev: dropped.max_approx_dev,
    })
}

fn drop_curve(
    k: &Kernel,
    surface: &Surface,
    domain: &crate::geom::extent2d::Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    tolerance: f64,
) -> Result<(Vec<Curve>, f64, f64)> {
    // the virtual SmSurface::DropCurve with the defaults bKeepAllDropCurves = TRUE,
    // no failure record, bCreatingUVTrimCurve = TRUE
    let dropped = surface.drop_curve::<crate::surface::surface::p4::NoObject>(
        k, domain, curve, interval, tolerance, true, None, true,
    )?;
    Ok((
        dropped.uv_curves,
        dropped.max_drop_to_surf,
        dropped.max_approx_dev,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod notifications {
        use super::*;
        use crate::geom::attribute::{ids, Attribute, AttributeStore, Behavior, ObjectKey};
        use crate::geom::context::{Context, MarkLock, ALL_MARKS};
        use crate::geom::object::ObjectRef;
        use crate::topology::edge::notification_test_support as callbacks;
        use crate::topology::topology::Topology;
        use std::collections::BTreeMap;

        fn split_key(entity: EntityRef) -> ObjectRef {
            let (base, index, type_code) = match entity {
                EntityRef::Edge(id) => (100, id.index(), edge::TYPE),
                EntityRef::Vertex(id) => (200, id.index(), vertex::TYPE),
                EntityRef::Edgeuse(id) => (300, id.index(), crate::topology::edgeuse::p1::TYPE),
                EntityRef::Vertexuse(id) => (400, id.index(), vertexuse::TYPE),
                EntityRef::Shell(id) => (
                    500,
                    id.index(),
                    crate::topology::loopuse::notify_ref(entity).type_code,
                ),
                _ => panic!("unexpected split entity"),
            };
            ObjectRef {
                key: ObjectKey(base + u64::from(index)),
                type_code,
            }
        }

        fn split_curve_ref(edge: EdgeId, type_code: i64) -> ObjectRef {
            ObjectRef {
                key: ObjectKey(1000 + u64::from(edge.index())),
                type_code,
            }
        }

        fn split_wire() -> (Brep, EdgeId) {
            let mut b = Brep::new();
            b.set_key(ObjectKey(77));
            b.set_editing_enabled(true);
            let start = b.add_vertex(Point3::ZERO).unwrap();
            let end = b.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
            let edge = b.add_edge(start, end).unwrap();
            b.edge_mut(edge).unwrap().interval = Extent1d::from_min_max(0.0, 1.0);
            b.edge_mut(edge).unwrap().curve = Some(Curve::from(
                crate::curve::line::Line::from_points(Point3::ZERO, Point3::new(1.0, 0.0, 0.0), 3),
            ));
            for (vertex, orientation) in [(start, OrientType::Same), (end, OrientType::Opposite)] {
                let vu = b
                    .add_vertexuse_record(Vertexuse {
                        vertex: Some(vertex),
                        ..Vertexuse::default()
                    })
                    .unwrap();
                let eu = b
                    .add_edgeuse_record(crate::topology::Edgeuse {
                        edge: Some(edge),
                        owner: crate::topology::EdgeuseOwner::Shell(None),
                        vertexuse: Some(vu),
                        orientation,
                        ..crate::topology::Edgeuse::default()
                    })
                    .unwrap();
                b.vertexuse_mut(vu).unwrap().owner = VertexuseOwner::Edgeuse(Some(eu));
                b.vertex_mut(vertex).unwrap().vertexuses.push(vu);
                b.edge_mut(edge).unwrap().edgeuses.push(eu);
            }
            (b, edge)
        }

        #[test]
        fn actual_split_issues_copied_curve_identity_and_retains_refusal_prefix() {
            use std::cell::RefCell;

            for refuse_copy in [false, true] {
                let mut k = Kernel::default();
                let mut b = crate::primitive::api_primitives::create_box(
                    &mut k,
                    Point3::ZERO,
                    10.0,
                    20.0,
                    30.0,
                )
                .unwrap();
                b.set_editing_enabled(true);
                let edge = b.edge_list()[0];
                let serial = b.notify_serial().unwrap();
                let key_of = |entity| crate::topology::brep::p3::topology_ref(serial, entity);
                let interval = b.edge(edge).unwrap().interval;
                let parameter = (interval.min() + interval.max()) * 0.5;
                let primary = edge::primary_edgeuse(&b, edge).unwrap().unwrap();
                let same = b.edgeuse(primary).unwrap().orientation == OrientType::Same;
                let expected_interval = if same {
                    Extent1d::from_min_max(interval.min(), parameter)
                } else {
                    Extent1d::from_min_max(parameter, interval.max())
                };
                let original = b.edge_mut(edge).unwrap().curve.as_mut().unwrap();
                let original_key = k
                    .context
                    .geometry_key(original.notify_identity_slot())
                    .unwrap();
                let original_ref = ObjectRef {
                    key: original_key,
                    type_code: original.type_code(),
                };
                let before = (b.edge_list().len(), b.vertex_list().len());
                let mut records = crate::topology::topology::NotifyRecords::new();
                crate::topology::brep::p3::seed_records(&mut records, &b, &key_of);
                let mut attributes = AttributeStore::default();
                let label = attributes
                    .insert(Attribute::long(ids::USER_1, 73, Behavior::Copy))
                    .unwrap();
                attributes.add_attribute(original_key, label).unwrap();
                let events = callbacks::install(&mut k.context);
                let issued = RefCell::new(Vec::new());
                let issuer = |id: EdgeId, curve: &mut Curve, env: &mut edge::NotifyEnv<'_>| {
                    let before = curve.notify_key();
                    assert!(env.context.as_deref().unwrap().doing_boolean());
                    if refuse_copy && id != edge {
                        assert_eq!(before, None);
                        return Err(SolidError::invalid_input(
                            "test curve issuer",
                            "refused copied curve",
                        ));
                    }
                    let reference = issue_curve_identity(id, curve, env)?;
                    issued.borrow_mut().push((id, before, reference));
                    Ok(reference)
                };
                let result = make_vertex_split_edge_with_kernel_curve_identity(
                    &mut k,
                    &mut b,
                    edge,
                    parameter,
                    &mut edge::NotificationStores {
                        records: &mut records,
                        attributes: &mut attributes,
                        key_of: &key_of,
                    },
                    CurveIdentity::Actual(&issuer),
                );
                assert!(!k.context.doing_boolean());
                callbacks::assert_installed(&k.context, &events);
                assert_eq!(
                    (b.edge_list().len(), b.vertex_list().len()),
                    (before.0 + 1, before.1 + 1)
                );
                assert_eq!(b.edge(edge).unwrap().interval, expected_interval);
                assert_eq!(issued.borrow()[0], (edge, Some(original_key), original_ref));
                let new_edge = *b.edge_list().last().unwrap();
                let events = events.lock().unwrap();
                if refuse_copy {
                    assert!(result.is_err());
                    assert_eq!(issued.borrow().len(), 1);
                    assert!(b.edge(new_edge).unwrap().curve.is_none());
                    assert!(events
                        .iter()
                        .any(|event| event.operation == NotifyOperation::PreEdit));
                    assert!(!events
                        .iter()
                        .any(|event| event.operation == NotifyOperation::Split));
                } else {
                    let output = result.unwrap();
                    assert!(output.split);
                    assert_eq!(issued.borrow().len(), 2);
                    let (id, before, new_ref) = issued.borrow()[1];
                    assert_eq!((id, before), (new_edge, None));
                    assert_ne!(new_ref.key, original_key);
                    assert_ne!(new_ref.key, key_of(EntityRef::Edge(new_edge)).key);
                    let installed = b.edge(new_edge).unwrap().curve.as_ref().unwrap();
                    assert_eq!(installed.notify_key(), Some(new_ref.key));
                    assert_eq!(installed.owner(), Some(key_of(EntityRef::Edge(new_edge))));
                    let copied = attributes.find_attribute(new_ref.key, ids::USER_1).unwrap();
                    assert_ne!(copied, label);
                    assert_eq!(attributes.get(copied).unwrap().long_value(), Some(73));
                    let split: Vec<_> = events
                        .iter()
                        .filter(|event| event.operation == NotifyOperation::Split)
                        .collect();
                    assert_eq!(split.len(), 2);
                    assert_eq!((split[0].callback, split[1].callback), ("user", "system"));
                    for event in split {
                        assert_eq!(event.caller, original_ref);
                        assert_eq!(
                            event.data,
                            [
                                Some(original_ref),
                                Some(new_ref),
                                Some(key_of(EntityRef::Edge(edge)))
                            ]
                        );
                    }
                }
                assert_eq!(
                    b.edge(edge).unwrap().curve.as_ref().unwrap().notify_key(),
                    Some(original_key)
                );
            }
        }

        #[test]
        fn split_uses_real_stores_callbacks_and_restores_each_context_on_success() {
            #[derive(Clone, Copy)]
            enum Binding {
                Kernel,
                External(bool),
                Null,
            }

            for previous in [false, true] {
                for binding in [
                    Binding::Kernel,
                    Binding::External(false),
                    Binding::External(true),
                    Binding::Null,
                ] {
                    let mut k = Kernel::default();
                    k.context.set_doing_boolean(previous);
                    let kernel_events = callbacks::install(&mut k.context);
                    let mut external = Context::default();
                    let external_events = callbacks::install(&mut external);
                    if let Binding::External(previous) = binding {
                        external.set_doing_boolean(previous);
                    }
                    let mut outer =
                        MarkLock::new(core::slice::from_mut(&mut k.context), 0, ALL_MARKS).unwrap();
                    let mark = outer.mark_type();
                    let mark_value = k.context.current_mark(mark);
                    let (mut b, edge) = split_wire();
                    let edge_ref = split_key(EntityRef::Edge(edge));
                    let curve_type = b.edge(edge).unwrap().curve.as_ref().unwrap().type_code();
                    let original_curve = split_curve_ref(edge, curve_type);
                    let mut record = Topology::default();
                    record.set_user_index1(912);
                    record.mark(Some(&k.context), mark);
                    let mut records = crate::topology::topology::NotifyRecords::from(
                        BTreeMap::from([(edge_ref.key, record)]),
                    );
                    for (id, _) in b.vertices().iter() {
                        records.insert(split_key(EntityRef::Vertex(id)).key, Topology::default());
                    }
                    for (id, _) in b.edgeuses().iter() {
                        records.insert(split_key(EntityRef::Edgeuse(id)).key, Topology::default());
                    }
                    for (id, _) in b.vertexuses().iter() {
                        records
                            .insert(split_key(EntityRef::Vertexuse(id)).key, Topology::default());
                    }
                    let mut attributes = AttributeStore::default();
                    let label = attributes
                        .insert(Attribute::long(ids::USER_1, 73, Behavior::Copy))
                        .unwrap();
                    attributes.add_attribute(original_curve.key, label).unwrap();
                    let tessellating = attributes
                        .insert(Attribute::new(ids::TESSELLATING, Behavior::Reference))
                        .unwrap();
                    attributes.add_attribute(b.key(), tessellating).unwrap();
                    let result = match binding {
                        Binding::Kernel => make_vertex_split_edge_with_kernel_context(
                            &mut k,
                            &mut b,
                            edge,
                            0.5,
                            &mut edge::NotificationStores {
                                records: &mut records,
                                attributes: &mut attributes,
                                key_of: &split_key,
                            },
                            &split_curve_ref,
                        ),
                        Binding::External(_) | Binding::Null => make_vertex_split_edge(
                            &mut k,
                            &mut b,
                            edge,
                            0.5,
                            &mut edge::NotifyEnv {
                                records: &mut records,
                                attributes: &mut attributes,
                                context: if matches!(binding, Binding::External(_)) {
                                    Some(&mut external)
                                } else {
                                    None
                                },
                                key_of: &split_key,
                            },
                            &split_curve_ref,
                        ),
                    };
                    let output = result.unwrap();
                    assert!(output.split);
                    assert_eq!(output.edges, Some([edge, EdgeId::from_index(1)]));
                    assert_eq!(output.vertex, Some(VertexId::from_index(2)));
                    assert_eq!(
                        b.vertex(VertexId::from_index(2)).unwrap().point,
                        Point3::new(0.5, 0.0, 0.0)
                    );
                    assert_eq!((b.edge_list().len(), b.vertex_list().len()), (2, 3));
                    assert_eq!(k.context.doing_boolean(), previous);
                    assert!(k.context.is_mark_locked(mark));
                    assert_eq!(k.context.current_mark(mark), mark_value);
                    assert!(records
                        .get(&edge_ref.key)
                        .unwrap()
                        .is_marked(&k.context, mark));
                    assert_eq!(records.get(&edge_ref.key).unwrap().user_index1(), 912);
                    assert_eq!(
                        records
                            .get(&split_key(EntityRef::Edge(EdgeId::from_index(1))).key)
                            .unwrap()
                            .user_index1(),
                        912
                    );
                    if let Binding::External(previous) = binding {
                        assert_eq!(external.doing_boolean(), previous);
                    }
                    callbacks::assert_installed(&k.context, &kernel_events);
                    callbacks::assert_installed(&external, &external_events);
                    let new_edge = EdgeId::from_index(1);
                    let new_curve = split_curve_ref(new_edge, curve_type);
                    assert_eq!(
                        b.edge(new_edge).unwrap().curve.as_ref().unwrap().owner(),
                        Some(split_key(EntityRef::Edge(new_edge)))
                    );
                    let copied_label = attributes
                        .find_attribute(new_curve.key, ids::USER_1)
                        .unwrap();
                    assert_ne!(copied_label, label);
                    assert_eq!(attributes.get(copied_label).unwrap().long_value(), Some(73));
                    assert_eq!(
                        attributes.find_attribute(original_curve.key, ids::USER_1),
                        Some(label)
                    );
                    assert_eq!(
                        attributes.find_attribute(b.key(), ids::TESSELLATING),
                        Some(tessellating)
                    );
                    let kernel_log = kernel_events.lock().unwrap();
                    let external_log = external_events.lock().unwrap();
                    let events = match binding {
                        Binding::Kernel => {
                            assert!(external_log.is_empty());
                            &*kernel_log
                        }
                        Binding::External(_) => {
                            assert!(kernel_log.is_empty());
                            &*external_log
                        }
                        Binding::Null => {
                            assert!(kernel_log.is_empty());
                            assert!(external_log.is_empty());
                            outer
                                .release(core::slice::from_mut(&mut k.context))
                                .unwrap();
                            continue;
                        }
                    };
                    let expected = [
                        (NotifyOperation::PreEdit, edge_ref),
                        (
                            NotifyOperation::ChangeGeometry,
                            split_key(EntityRef::Edge(new_edge)),
                        ),
                        (NotifyOperation::ChangeOwner, new_curve),
                        (NotifyOperation::Split, original_curve),
                        (
                            NotifyOperation::ChangeGeometry,
                            split_key(EntityRef::Vertex(VertexId::from_index(2))),
                        ),
                        (NotifyOperation::AddToBrep, brep_ref(&b)),
                        (NotifyOperation::SplitInBrep, brep_ref(&b)),
                    ];
                    assert_eq!(events.len(), expected.len() * 2);
                    for (pair, (operation, caller)) in events.chunks_exact(2).zip(expected) {
                        assert_eq!((pair[0].callback, pair[1].callback), ("user", "system"));
                        for event in pair {
                            assert_eq!((event.operation, event.caller), (operation, caller));
                            assert_eq!(event.data, pair[0].data);
                        }
                    }
                    assert_eq!(events[0].data, [Some(edge_ref), Some(brep_ref(&b)), None]);
                    assert_eq!(
                        events[6].data,
                        [Some(original_curve), Some(new_curve), Some(edge_ref)]
                    );
                    assert_eq!(
                        events[10].data,
                        [
                            Some(split_key(EntityRef::Vertex(VertexId::from_index(2)))),
                            Some(brep_ref(&b)),
                            None
                        ]
                    );
                    assert_eq!(
                        events[12].data,
                        [
                            Some(edge_ref),
                            Some(edge_ref),
                            Some(split_key(EntityRef::Edge(new_edge)))
                        ]
                    );
                    outer
                        .release(core::slice::from_mut(&mut k.context))
                        .unwrap();
                }
            }
        }

        #[test]
        fn kernel_shell_vertex_registers_real_topology_and_notifies_in_order() {
            let mut k = Kernel::default();
            let events = callbacks::install(&mut k.context);
            let mut b = Brep::new();
            let region = b
                .add_region_record(crate::topology::Region::default())
                .unwrap();
            let mut records = crate::topology::topology::NotifyRecords::new();
            let mut attributes = AttributeStore::default();
            let result = make_shell_vertex_with_kernel_context(
                &mut k,
                &mut b,
                Some(region),
                Point3::ZERO,
                &mut edge::NotificationStores {
                    records: &mut records,
                    attributes: &mut attributes,
                    key_of: &split_key,
                },
            );
            let (shell, vertex) = result.unwrap();
            assert_eq!(b.vertices().len(), 1);
            assert_eq!(b.shells().len(), 1);
            assert_eq!(b.vertexuses().len(), 1);
            assert_eq!(b.region(region).unwrap().shells, vec![shell]);
            assert_eq!(b.shell(shell).unwrap().region, Some(region));
            assert_eq!(b.vertex(vertex).unwrap().point, Point3::ZERO);
            assert_eq!(b.vertex(vertex).unwrap().tolerance, b.tolerance());
            let vu = b.vertex(vertex).unwrap().vertexuses[0];
            assert_eq!(records.len(), 3);
            for entity in [
                EntityRef::Shell(shell),
                EntityRef::Vertex(vertex),
                EntityRef::Vertexuse(vu),
            ] {
                assert!(records.contains_key(&split_key(entity).key));
            }
            callbacks::assert_installed(&k.context, &events);
            let events = events.lock().unwrap();
            let vertex_ref = split_key(EntityRef::Vertex(vertex));
            let shell_ref = split_key(EntityRef::Shell(shell));
            let expected = [
                (
                    NotifyOperation::ChangeGeometry,
                    split_key(EntityRef::Vertexuse(vu)),
                ),
                (NotifyOperation::ChangeGeometry, vertex_ref),
                (NotifyOperation::AddToBrep, brep_ref(&b)),
                (NotifyOperation::AddToBrep, brep_ref(&b)),
            ];
            assert_eq!(events.len(), expected.len() * 2, "{events:?}");
            for (pair, (operation, caller)) in events.chunks_exact(2).zip(expected) {
                assert_eq!((pair[0].callback, pair[1].callback), ("user", "system"));
                for event in pair {
                    assert_eq!((event.operation, event.caller), (operation, caller));
                    assert_eq!(event.data, pair[0].data);
                }
            }
            assert_eq!(events[0].data, [None; 3]);
            assert_eq!(events[2].data, [None; 3]);
            assert_eq!(events[4].data, [Some(vertex_ref), Some(brep_ref(&b)), None]);
            assert_eq!(events[6].data, [Some(shell_ref), Some(brep_ref(&b)), None]);
        }

        #[test]
        fn kernel_split_refusals_restore_state_without_notifying() {
            for previous in [false, true] {
                let mut k = Kernel::default();
                k.context.set_doing_boolean(previous);
                let events = callbacks::install(&mut k.context);
                let (mut b, edge) = split_wire();
                let mut records =
                    crate::topology::topology::NotifyRecords::from(BTreeMap::from([(
                        split_key(EntityRef::Edge(edge)).key,
                        Topology::default(),
                    )]));
                let mut attributes = AttributeStore::default();
                let mut stores = edge::NotificationStores {
                    records: &mut records,
                    attributes: &mut attributes,
                    key_of: &split_key,
                };
                for parameter in [0.0, 1.0, -1.0, 2.0, f64::MAX] {
                    let output = make_vertex_split_edge_with_kernel_context(
                        &mut k,
                        &mut b,
                        edge,
                        parameter,
                        &mut stores,
                        &|_, _| panic!("refusal must precede curve identity lookup"),
                    )
                    .unwrap();
                    assert!(!output.split);
                    assert_eq!(output.status(), SM_ERR);
                    assert_eq!(k.context.doing_boolean(), previous);
                }
                b.set_editing_enabled(false);
                let output = make_vertex_split_edge_with_kernel_context(
                    &mut k,
                    &mut b,
                    EdgeId::from_index(999),
                    0.5,
                    &mut stores,
                    &split_curve_ref,
                )
                .unwrap();
                assert!(!output.split);
                assert_eq!(k.context.doing_boolean(), previous);
                assert!(events.lock().unwrap().is_empty());
                callbacks::assert_installed(&k.context, &events);
                assert_eq!((b.edge_list().len(), b.vertex_list().len()), (1, 2));
            }
        }
    }

    #[test]
    fn ray_errors_without_an_upstream_status_propagate() {
        assert!(ray_success(&Ok(())).unwrap());
        assert!(!ray_success(&Err(SolidError::sm_err(OP))).unwrap());
        assert!(!ray_success(&Err(SolidError::invalid_input(OP, "native"))).unwrap());
        let internal = [
            SolidError::Unported {
                upstream: "SmShell::RayFire",
                unit: "topology.shell",
            },
            SolidError::NotFound {
                operation: OP,
                entity: EntityRef::Edge(EdgeId::from_index(7)),
            },
            SolidError::PreparationRequired { operation: OP },
            SolidError::port_invariant("topology::brep_cache", "edge owner is not in source B-rep"),
        ];
        for e in internal {
            assert_eq!(ray_success(&Err(e.clone())), Err(e));
        }
    }

    #[test]
    fn zero_ray_direction_retains_the_defined_native_status() {
        assert!(matches!(
            ray_direction(Vec3::ZERO),
            Err(SolidError::Upstream { code: 1007, .. })
        ));
        assert_eq!(ray_direction(Vec3::Z).unwrap(), Vec3::Z);
    }

    /// WR-469: a box face's diagonal, given with its UV trim, overshooting its far corner
    /// by 0.7 of that corner's tolerance along the face, made with
    /// `new_edge_vertex_tolerance_covers_gap` set to `fix`. Returns the far corner's
    /// tolerance before and after and the end gap, or `None` when a dependency the
    /// success path needs is still unported.
    fn diagonal_overshooting_its_corner(fix: bool) -> Option<(f64, f64, f64)> {
        use crate::topology::edgeuse::p1 as eu1;
        let k = &mut Kernel::default();
        k.fixes.new_edge_vertex_tolerance_covers_gap = fix;
        let mut b = crate::primitive::api_primitives::create_box(k, Point3::ZERO, 10.0, 10.0, 10.0)
            .unwrap();
        b.set_editing_enabled(true);
        let eu = b
            .edgeuses()
            .ids()
            .find(|&e| eu1::face(&b, e).is_ok_and(|f| f.is_some()))
            .unwrap();
        let face = eu1::face(&b, eu).unwrap().unwrap();
        let opposite = eu1::cw_edgeuse(&b, eu1::cw_edgeuse(&b, eu).unwrap().unwrap())
            .unwrap()
            .unwrap();
        let (near, far) = (
            eu1::vertexuse(&b, eu).unwrap().unwrap(),
            eu1::vertexuse(&b, opposite).unwrap().unwrap(),
        );
        let vertex_of = |b: &Brep, vu: VertexuseId| b.vertexuse(vu).unwrap().vertex.unwrap();
        let (p, q) = (
            vertex::point(&b, vertex_of(&b, near)).unwrap(),
            vertex::point(&b, vertex_of(&b, far)).unwrap(),
        );
        let tolerance = vertex::tolerance(&b, vertex_of(&b, far)).unwrap();
        let gap = 0.7 * tolerance;
        let end = q + (q - p) * (gap / q.distance(p));
        // The UV trim given, as an intersection gives it: no curve drop, whose edge
        // tolerance would cover the gap.
        let crate::surface::surface::SurfaceKind::Plane(plane) =
            b.face(face).unwrap().surface.as_ref().unwrap().kind()
        else {
            panic!("box face is not a plane");
        };
        let (uv_p, uv_end) = (
            plane.project_point_to_uv_domain(p),
            plane.project_point_to_uv_domain(end),
        );
        let uv_trim = Curve::from(crate::curve::line::Line::from_points(
            Point3::new(uv_p.x, uv_p.y, 0.0),
            Point3::new(uv_end.x, uv_end.y, 0.0),
            3,
        ));
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let key_of = |e: EntityRef| crate::topology::loopuse::notify_ref(e);
        let mut env = edge::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key_of,
        };
        let curve_ref = |e: EdgeId, type_code: i64| crate::geom::object::ObjectRef {
            key: crate::geom::attribute::ObjectKey(1000 + u64::from(e.index())),
            type_code,
        };
        let made = make_edge_in_face_with_vu(
            k,
            &mut b,
            EdgeInFaceInput {
                face: Some(face),
                vertices: [Some(near), Some(far)],
                curve: Some(Curve::from(crate::curve::line::Line::from_points(
                    p, end, 3,
                ))),
                uv_trim: Some(uv_trim),
                drop_fail: None,
                interval: Extent1d::from_min_max(0.0, 1.0),
                orientation: OrientType::Same,
                curve_zone_tol_3d: crate::geom::tol::zone_tol3d(k, 1e-7),
            },
            &mut env,
            &curve_ref,
        );
        if let Err(e) = made {
            // A dependency the success path needs may still be unported.
            assert!(matches!(e, SolidError::Unported { .. }), "{e:?}");
            return None;
        }
        Some((
            tolerance,
            vertex::tolerance(&b, vertex_of(&b, far)).unwrap(),
            gap,
        ))
    }

    /// WR-469, opt-in: a new edge that ends `gap` from its vertex, above half the
    /// vertex's tolerance (steel-026: 5.44e-6 against 5.42e-6), leaves the vertex a
    /// tolerance of at least twice the gap.
    #[test]
    fn new_edge_grows_its_vertices_tolerance_to_twice_the_end_gap() {
        let Some((_, grown, gap)) = diagonal_overshooting_its_corner(true) else {
            return;
        };
        assert!(
            grown >= 2.0 * gap * (1.0 - 1e-12),
            "tolerance {grown} for an end gap {gap}"
        );
    }

    /// WR-469, default: the vertex keeps its tolerance, as the pinned C++ does.
    #[test]
    fn new_edge_keeps_its_vertices_tolerance_by_default() {
        let Some((before, after, gap)) = diagonal_overshooting_its_corner(false) else {
            return;
        };
        assert_eq!(after, before, "end gap {gap}");
        assert!(after < 2.0 * gap);
    }

    #[test]
    fn extreme_parameters_do_not_panic_and_editing_refusal_clears_outputs() {
        let mut k = Kernel::default();
        let mut b = Brep::new();
        let v0 = b.add_vertex(Point3::new(0.0, 0.0, 0.0)).unwrap();
        let v1 = b.add_vertex(Point3::new(1.0, 0.0, 0.0)).unwrap();
        let e = b.add_edge(v0, v1).unwrap();
        b.edge_mut(e).unwrap().interval = Extent1d::from_min_max(0.0, 1.0);
        // Endpoint outputs walk the primary/mate edge uses, as native does.
        for (vertex, orientation) in [(v0, OrientType::Same), (v1, OrientType::Opposite)] {
            let vu = b
                .add_vertexuse_record(Vertexuse {
                    vertex: Some(vertex),
                    ..Vertexuse::default()
                })
                .unwrap();
            let eu = b
                .add_edgeuse_record(crate::topology::Edgeuse {
                    edge: Some(e),
                    owner: crate::topology::EdgeuseOwner::Shell(None),
                    vertexuse: Some(vu),
                    orientation,
                    ..crate::topology::Edgeuse::default()
                })
                .unwrap();
            b.vertexuse_mut(vu).unwrap().owner = VertexuseOwner::Edgeuse(Some(eu));
            b.vertex_mut(vertex).unwrap().vertexuses.push(vu);
            b.edge_mut(e).unwrap().edgeuses.push(eu);
        }
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::default();
        let mut env = edge::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &|_| crate::geom::object::ObjectRef {
                key: crate::geom::attribute::ObjectKey(1),
                type_code: 16016,
            },
        };
        for (p, vertex) in [
            (f64::NAN, None),
            (f64::INFINITY, Some(v1)),
            (f64::NEG_INFINITY, Some(v0)),
        ] {
            b.set_editing_enabled(true);
            let result = make_vertex_split_edge(&mut k, &mut b, e, p, &mut env, &|_, kind| {
                crate::geom::object::ObjectRef {
                    key: crate::geom::attribute::ObjectKey(999),
                    type_code: kind,
                }
            })
            .unwrap();
            assert_eq!(
                (result.status(), result.edges, result.vertex),
                (SM_ERR, None, vertex)
            );
            b.set_editing_enabled(false);
            let disabled = make_vertex_split_edge(&mut k, &mut b, e, p, &mut env, &|_, kind| {
                crate::geom::object::ObjectRef {
                    key: crate::geom::attribute::ObjectKey(999),
                    type_code: kind,
                }
            })
            .unwrap();
            assert_eq!(disabled.status(), SM_ERR);
            assert_eq!((disabled.edges, disabled.vertex), (None, None));
        }
        assert!(!k.context.doing_boolean());
        k.context.set_doing_boolean(true);
        for p in [f64::MAX, -f64::MAX] {
            b.set_editing_enabled(true);
            let outside = make_vertex_split_edge(&mut k, &mut b, e, p, &mut env, &|_, kind| {
                crate::geom::object::ObjectRef {
                    key: crate::geom::attribute::ObjectKey(999),
                    type_code: kind,
                }
            })
            .unwrap();
            assert_eq!(
                (outside.status(), outside.edges, outside.vertex),
                (SM_ERR, None, None)
            );
            assert!(k.context.doing_boolean());
        }
        k.context.set_doing_boolean(false);
        b.edge_mut(e).unwrap().curve = Some(Curve::from(crate::curve::line::Line::from_points(
            Point3::ZERO,
            Point3::new(1.0, 0.0, 0.0),
            3,
        )));
        b.set_editing_enabled(true);
        let result = make_vertex_split_edge(&mut k, &mut b, e, 0.5, &mut env, &|edge, kind| {
            crate::geom::object::ObjectRef {
                key: crate::geom::attribute::ObjectKey(999 + u64::from(edge.index())),
                type_code: kind,
            }
        });
        // With parts 1 and 2 wired, a wire split completes, its final B-rep ADD
        // notification included.
        let out = result.unwrap();
        assert_eq!(out.status(), 0);
        let edges = out.edges.unwrap();
        assert!(edges.contains(&e));
        assert!(out.vertex.is_some());
        assert_eq!(b.edge_list().len(), 2);
        assert_eq!(b.vertex_list().len(), 3);
        assert!(!k.context.doing_boolean());
    }
}
