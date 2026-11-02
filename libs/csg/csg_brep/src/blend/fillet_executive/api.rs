//! Public fillet entry points: fillet B-rep edges through one live fillet session.
//!
//! Upstream: `source/SM_API/src/SmApiBrep.cpp` (`SmApiFilletEdges` 2049-2087,
//! `SmApiFilletEdgesPerEdge` 2094-2135, `SmApiVariableRadiusFillet` 2142-2178,
//! `SmApiSurfaceSurfaceFillet` 2189-2267, `SmApiSetBevelCorners` 2305-2338,
//! `SmApiFilletPreview` 2285-2309),
//! `source/SM_API/src/SmApiFillets.cpp` (`SmApiChamferFillet` 40-113,
//! `SmApiCircularFillet` 119-185).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`fillet_edges`] | `SmApiFilletEdges` |
//! | [`fillet_edges_per_edge`] | `SmApiFilletEdgesPerEdge` |
//! | [`variable_radius_fillet`] | `SmApiVariableRadiusFillet` |
//! | [`circular_fillet`] | `SmApiCircularFillet` |
//! | [`chamfer_fillet`] | `SmApiChamferFillet` |
//! | [`set_bevel_corners`] | `SmApiSetBevelCorners` |
//! | [`fillet_preview`] | `SmApiFilletPreview` |
//! | [`surface_surface_fillet`] | `SmApiSurfaceSurfaceFillet` |
//!
//! Deviations: the kernel argument replaces `SmApiGetOrCreateContext()`. Edges are
//! typed ids of `brep`. Upstream destroys its executive (and the executive's
//! fillet-edge attribute) after `StitchAndOrient`; here the session, which holds the
//! only borrow of `brep`, is dropped first. Attributes carry no geometry, so the
//! stitch and orientation see the same topology and geometry. A body whose corners
//! need an unported corner kind returns `Unported` from the session. The
//! whole-operation wrappers here drop their session and return only the later fillet
//! failure, so the executive's own error info is not in their `Result`: a caller that
//! needs the retained missing-corner note (`fixes.fillet_vertex_error_diagnostics`)
//! drives [`FilletSession`] directly and reads
//! [`FilletSession::error_info`].
//! [`circular_fillet`] and [`chamfer_fillet`] ignore `LoadFilletSolver`'s status
//! as upstream does, and skip an edge whose solver constructor fails (upstream's
//! constructor only signals the error and leaves a solver without surfaces, which
//! `LoadFilletSolver` then refuses); `Unported` still propagates. The stack-held
//! generator and handler become session-retained values. [`surface_surface_fillet`]
//! takes each surface as a [`FilletSurfaceArg`]: a free surface (upstream
//! `GetFace()` null, a temporary B-rep of its own), a face of a caller's B-rep, or a
//! second face of the first argument's B-rep (two `&mut` borrows of one B-rep are not
//! expressible); the same free surface reference twice is one surface, as upstream
//! the second `GetFace()` sees the first temporary face. The faces' surfaces are
//! copied into the solver (upstream the face's own surface pointer); the first base
//! B-rep is the session's target, a distinct second one its second base B-rep. The
//! fillet B-rep is returned (`None` when the offset surfaces do not intersect,
//! upstream `SM_SUCCESS` with a null result); the temporary B-reps are dropped (upstream
//! `SetOwner(NULL)` and `delete`). The port's cached topology fields (`Edge::vertices`,
//! `Face::shell`, `Face::loops`, `Loop::face`, `Loop::edges`, `Shell::faces`), which the
//! edits rewire only through the use links, are rewritten by
//! [`Brep::refresh_cached_topology`] before each public fillet operation returns: in the
//! returned and borrowed bodies on success and on an error after which the body keeps
//! the edits made before the failure (upstream reads those fields through the uses and
//! has no equivalent step; nothing else changes). Callers can run
//! [`crate::topology::integrity::check`] immediately, without a manual refresh.
//! Validation-only returns leave untouched bodies alone. The `create_face_from_surface`
//! construction helper adds one face; its callers finalize the body. [`fillet_preview`]
//! returns copies of the preview surfaces (upstream hands out the executive's own, which
//! it never frees); like upstream it does not check the radius
//! ([`is_radius_supported`]). No recursion.

use super::p1::is_radius_supported;
use super::session::finish::{is_non_manifold_refusal, without_entity};
use super::session::FilletSession;
use crate::blend::fillet_executive::p2::SurfaceFilletOptions;
use crate::blend::fillet_geom::p1::FilletSolverId;
use crate::blend::fillet_solver::p1::{
    FilletSurfaceGenerator, FilletSurfaceGeneratorType, MakeSurfaceBlendSih,
};
use crate::blend::fillet_standard_solver::p1::{BoundaryTrimmingType, FilletLaw};
use crate::error::{Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::kernel::Kernel;
use crate::math::PI;
use crate::surface::surface::Surface;
use crate::topology::brep::p1;
use crate::topology::brep::p5::{stitch_and_orient, StitchAndOrientOptions};
use crate::topology::{Brep, EdgeId, FaceId, VertexId};

const OP: &str = "blend::fillet_executive::api";

/// Run `fillet` (a session's set-up, `do_fillet` and the stitch) on `brep`; with
/// `fixes.fillet_refuse_non_manifold_result` (WR-212, not upstream), a refusal of a
/// result that is not a closed manifold solid puts back the body as it was before the
/// call (a [`Brep::clone_for_rollback`] taken first) and names no entity, as the
/// refused result's edge is gone with it. Every other result is `fillet`'s own.
fn refusing_non_manifold_result(
    k: &mut Kernel,
    brep: &mut Brep,
    fillet: impl FnOnce(&mut Kernel, &mut Brep) -> Result<()>,
) -> Result<()> {
    let saved = if k.fixes.fillet_refuse_non_manifold_result {
        Some(brep.clone_for_rollback())
    } else {
        None
    };
    let result = fillet(k, brep);
    match (saved, result) {
        (Some(saved), Err(error)) if is_non_manifold_refusal(&error) => {
            brep.restore_from(saved);
            Err(without_entity(error))
        }
        (_, result) => {
            // (not upstream: the port's cached topology fields, true again for the
            // caller; the body keeps the edits the executive made before a failure,
            // as upstream)
            brep.refresh_cached_topology();
            result
        }
    }
}

fn invalid_input() -> SolidError {
    SolidError::Upstream {
        operation: OP,
        code: SM_ERR_INVALID_INPUT,
        entity: None,
    }
}

/// `SmApiFilletEdges`: fillet `edges` of `brep` with one constant `radius` and
/// cross-section (`xsect_type` 0 linear, 2 blend curve, any other value circular),
/// then `StitchAndOrient` with its defaults. `continuity` (1, 2 or 3 for G1-G3) and
/// `thumbweight` apply to blend-curve cross-sections.
///
/// The executive's own error info is not returned here: this wrapper drops its session
/// and returns only the later fillet failure. A caller that needs the retained
/// missing-corner note (`fixes.fillet_vertex_error_diagnostics`) drives
/// [`FilletSession`] directly and reads [`FilletSession::error_info`].
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for an empty edge list or a radius below 8 times the
/// B-rep tolerance ([`is_radius_supported`]); the fillet executive's errors (the body
/// keeps the edits made before the failure, as upstream).
pub fn fillet_edges(
    k: &mut Kernel,
    brep: &mut Brep,
    edges: &[EdgeId],
    radius: f64,
    xsect_type: u32,
    continuity: u32,
    thumbweight: f64,
) -> Result<()> {
    if edges.is_empty() {
        return Err(invalid_input());
    }
    if !is_radius_supported(radius, brep.tolerance()) {
        return Err(invalid_input());
    }
    refusing_non_manifold_result(k, brep, |k, brep| {
        {
            let mut session = FilletSession::new(k, brep)?;
            session
                .prepare_constant_radius_edges(
                    k,
                    edges,
                    radius,
                    xsect_type,
                    continuity,
                    thumbweight,
                )
                .and_then(|()| session.do_fillet(k))
        }?;
        stitch_and_orient(k, brep, &StitchAndOrientOptions::default())
    })
}

/// `SmApiFilletEdgesPerEdge`: fillet `edges` of `brep` with a constant radius and
/// cross-section type per edge, then `StitchAndOrient` with its defaults.
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for an empty edge list, when `radii` or `xsect_types`
/// does not hold one entry per edge, or for a radius below 8 times the B-rep
/// tolerance ([`is_radius_supported`]); the fillet executive's errors.
pub fn fillet_edges_per_edge(
    k: &mut Kernel,
    brep: &mut Brep,
    edges: &[EdgeId],
    radii: &[f64],
    xsect_types: &[FilletSurfaceGeneratorType],
    continuity: u32,
    thumbweight: f64,
) -> Result<()> {
    if edges.is_empty() {
        return Err(invalid_input());
    }
    if radii.len() != edges.len() || xsect_types.len() != edges.len() {
        return Err(invalid_input());
    }
    for &radius in radii {
        if !is_radius_supported(radius, brep.tolerance()) {
            return Err(invalid_input());
        }
    }
    refusing_non_manifold_result(k, brep, |k, brep| {
        {
            let mut session = FilletSession::new(k, brep)?;
            session
                .prepare_per_edge_constant_radius(
                    k,
                    edges,
                    radii,
                    xsect_types,
                    continuity,
                    thumbweight,
                )
                .and_then(|()| session.do_fillet(k))
        }?;
        stitch_and_orient(k, brep, &StitchAndOrientOptions::default())
    })
}

/// `SmApiVariableRadiusFillet`: fillet `edges` of `brep` with a radius running
/// linearly from `start_radius` to `end_radius` along each edge
/// (`SmLinearFilletLaw`) and one cross-section (`xsect_type` 0 linear, 2 blend
/// curve, any other value circular), then `StitchAndOrient` with its defaults.
/// `continuity` (1, 2 or 3 for G1-G3) applies to blend-curve cross-sections, whose
/// thumbweight is the `SetFilletParameters` default `1.0`.
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for an empty edge list or a start or end radius below 8
/// times the B-rep tolerance ([`is_radius_supported`]); the fillet executive's errors
/// (the body keeps the edits made before the failure, as upstream).
pub fn variable_radius_fillet(
    k: &mut Kernel,
    brep: &mut Brep,
    edges: &[EdgeId],
    start_radius: f64,
    end_radius: f64,
    xsect_type: u32,
    continuity: u32,
) -> Result<()> {
    if edges.is_empty() {
        return Err(invalid_input());
    }
    if !is_radius_supported(start_radius, brep.tolerance())
        || !is_radius_supported(end_radius, brep.tolerance())
    {
        return Err(invalid_input());
    }
    let law = FilletLaw::linear(start_radius, end_radius, None);
    refusing_non_manifold_result(k, brep, |k, brep| {
        {
            let mut session = FilletSession::new(k, brep)?;
            session
                .prepare_variable_radius_edges(k, edges, start_radius, law, xsect_type, continuity)
                .and_then(|()| session.do_fillet(k))
        }?;
        stitch_and_orient(k, brep, &StitchAndOrientOptions::default())
    })
}

/// Keep a value of an upstream call whose status is ignored; `Unported` propagates.
fn ignored<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_unported() => Err(error),
        Err(_) => Ok(None),
    }
}

/// The edge solver kind the all-edge wrappers construct.
#[derive(Clone, Copy)]
enum AllEdgeSolver {
    /// `SmConstantRadiusFS(.., dRadius, pEU)`.
    ConstantRadius,
    /// `SmConstantDistanceFS(.., dDistance, pEU)`.
    ConstantDistance,
}

/// The shared body of `SmApiCircularFillet` and `SmApiChamferFillet`: an
/// `SmMakeSurfaceBlendSIH(1.0)` handler, global merge, one solver per edge with a
/// blend edge use (in `GetEdges` order) with `generator`, then
/// `CreateFilletCorners`, every corner a bevel when `bevel_corners`, and
/// `DoFilleting`.
fn fillet_all_edges(
    k: &mut Kernel,
    session: &mut FilletSession<'_>,
    solver_kind: AllEdgeSolver,
    value: f64,
    generator: FilletSurfaceGenerator,
    bevel_corners: bool,
) -> Result<()> {
    let tol = session.target().tolerance();
    let generator = session.add_surface_generator(generator)?;

    // Define self-intersection handler for edge fillets
    let step_back_factor = 1.0;
    session.set_self_intersection_handler(Some(MakeSurfaceBlendSih::new(step_back_factor)));
    session.set_do_global_merge(true);

    for edge in p1::edges(session.target(), None)? {
        let Some(edgeuse) = session.blend_edgeuse(k, edge)? else {
            continue;
        };
        let ang_tol = 30.0 * PI / 180.0;
        let tan_tol = 2.0 * PI / 180.0;
        let solver: Option<FilletSolverId> = ignored(match solver_kind {
            AllEdgeSolver::ConstantRadius => {
                session.new_constant_radius_solver(k, edgeuse, value, tol, ang_tol, tan_tol)
            }
            AllEdgeSolver::ConstantDistance => {
                session.new_constant_distance_solver(k, edgeuse, value, tol, ang_tol, tan_tol)
            }
        })?;
        let Some(solver) = solver else {
            continue;
        };
        session.set_solver_generator(solver, generator)?;
        ignored(session.load_solver(k, solver))?;
    }

    session.create_corners(k, None)?;
    if bevel_corners {
        for corner in session.fillet_corners()? {
            session.set_corner_bevel(corner, true)?;
        }
    }
    session.do_filleting(k, None)
}

/// `SmApiCircularFillet`: fillet every edge of `brep` that bounds a blendable
/// sector with a circular cross-section of `radius` (exact arcs,
/// `SmCircularCrossSectionFSG(FALSE, 1.0e-3)`), then `StitchAndOrient` with its
/// defaults.
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for a radius below 8 times the B-rep tolerance
/// ([`is_radius_supported`]); the fillet executive's errors (the body keeps the edits
/// made before the failure, as upstream).
pub fn circular_fillet(k: &mut Kernel, brep: &mut Brep, radius: f64) -> Result<()> {
    if !is_radius_supported(radius, brep.tolerance()) {
        return Err(invalid_input());
    }
    let fillet = {
        let mut session = FilletSession::new(k, brep)?;
        fillet_all_edges(
            k,
            &mut session,
            AllEdgeSolver::ConstantRadius,
            radius,
            FilletSurfaceGenerator::circular(false, 1.0e-3, false, false),
            false,
        )
    };
    // This operation requires stitching for some reason
    let result =
        fillet.and_then(|()| stitch_and_orient(k, brep, &StitchAndOrientOptions::default()));
    // (not upstream: the port's cached topology fields, true again for the caller;
    // the body keeps the edits the executive made before a failure, as upstream)
    brep.refresh_cached_topology();
    result
}

/// `SmApiChamferFillet`: chamfer every edge of `brep` that bounds a blendable
/// sector with a linear cross-section of chord length `distance`
/// (`SmConstantDistanceFS`), every corner a bevel. Unlike the other entry points
/// there is no `StitchAndOrient`.
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for a distance below 8 times the B-rep tolerance
/// ([`is_radius_supported`]); the fillet executive's errors (the body keeps the edits
/// made before the failure, as upstream).
pub fn chamfer_fillet(k: &mut Kernel, brep: &mut Brep, distance: f64) -> Result<()> {
    if !is_radius_supported(distance, brep.tolerance()) {
        return Err(invalid_input());
    }
    let result = {
        let mut session = FilletSession::new(k, brep)?;
        fillet_all_edges(
            k,
            &mut session,
            AllEdgeSolver::ConstantDistance,
            distance,
            FilletSurfaceGenerator::Linear,
            true,
        )
    };
    // (not upstream: the port's cached topology fields, true again for the caller;
    // the body keeps the edits the executive made before a failure, as upstream)
    brep.refresh_cached_topology();
    result
}

/// `SmApiSetBevelCorners`: fillet `edges` of `brep` with a constant circular
/// `radius` (`SetFilletParameters` with its default continuity 1 and thumbweight
/// 1.0), make the corners at `bevel_vertices` bevels, then `DoFilleting` and
/// `StitchAndOrient` with its defaults.
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for an empty edge list or a radius below 8 times the B-rep
/// tolerance ([`is_radius_supported`]); `SM_ERR_NULL_POINTER` for a bevel vertex
/// without a corner entry; the fillet executive's errors.
pub fn set_bevel_corners(
    k: &mut Kernel,
    brep: &mut Brep,
    edges: &[EdgeId],
    radius: f64,
    bevel_vertices: &[VertexId],
) -> Result<()> {
    if edges.is_empty() {
        return Err(invalid_input());
    }
    if !is_radius_supported(radius, brep.tolerance()) {
        return Err(invalid_input());
    }
    let fillet = {
        let mut session = FilletSession::new(k, brep)?;
        // SetFilletParameters(.., SM_FSG_CIRCULAR, SM_FS_CONST_RADIUS, dRadius, NULL,
        // -1, -1, -1, -1) with lContinuity = 1, dThumbweight = 1.0 defaulted
        session
            .prepare_constant_radius_edges(k, edges, radius, 1, 1, 1.0)
            .and_then(|()| session.create_corners(k, None))
            .and_then(|()| {
                for vertex in bevel_vertices {
                    session.set_bevel_corner(*vertex)?;
                }
                Ok(())
            })
            .and_then(|()| session.do_filleting(k, None))
    };
    let result =
        fillet.and_then(|()| stitch_and_orient(k, brep, &StitchAndOrientOptions::default()));
    // (not upstream: the port's cached topology fields, true again for the caller;
    // the body keeps the edits the executive made before a failure, as upstream)
    brep.refresh_cached_topology();
    result
}

/// `SmApiFilletPreview`: the untrimmed fillet surfaces a constant circular `radius`
/// fillet of `edges` of `brep` would use (`SetFilletParameters` with its default
/// continuity 1 and thumbweight 1.0, then `GetPreviewFilletSurfaces`), without filleting:
/// each solver's surfaces in order, `None` for a fillet geometry without one. The corners
/// are built in the session's pseudo-B-rep; `brep` keeps its faces.
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for an empty edge list; the fillet executive's errors (`SM_ERR`
/// when no edge gets a solver).
pub fn fillet_preview(
    k: &mut Kernel,
    brep: &mut Brep,
    edges: &[EdgeId],
    radius: f64,
) -> Result<Vec<Option<Surface>>> {
    if edges.is_empty() {
        return Err(invalid_input());
    }
    let preview = {
        let mut session = FilletSession::new(k, brep)?;
        // SetFilletParameters(.., SM_FSG_CIRCULAR, SM_FS_CONST_RADIUS, dRadius, NULL,
        // -1, -1, -1, -1) with lContinuity = 1, dThumbweight = 1.0 defaulted
        session
            .prepare_constant_radius_edges(k, edges, radius, 1, 1, 1.0)
            .and_then(|()| session.preview_fillet_surfaces(k))
    };
    // (not upstream: the port's cached topology fields, true again for the caller)
    brep.refresh_cached_topology();
    preview
}

/// `SmBrep::CreateFaceFromSurface(pSurface, pSurface->GetNaturalUVDomain(), pFace)`:
/// a face of `brep` on a copy of `surface` over its natural domain (the temporary
/// B-reps of [`surface_surface_fillet`]).
///
/// # Errors
/// Those of the face construction.
pub fn create_face_from_surface(
    k: &mut Kernel,
    brep: &mut Brep,
    surface: &Surface,
) -> Result<FaceId> {
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    let mut state = crate::topology::brep::p2::ArenaNotifyState::new(brep);
    let mut env = state.env(None);
    let mut marks = crate::topology::topology_traverser::Marks::default();
    let this_face =
        |f: FaceId| crate::topology::loopuse::notify_ref(crate::error::EntityRef::Face(f));
    let mut fenv = crate::topology::brep::p8::FaceBuildEnv {
        env: &mut env,
        marks: &mut marks,
        this_face: &this_face,
    };
    crate::topology::brep::p8::create_face_from_surface(
        k,
        brep,
        Some(surface.clone()),
        &domain,
        &mut fenv,
    )
}

/// One surface argument of [`surface_surface_fillet`] (upstream an `SmSurface*` that
/// may or may not belong to a face).
#[derive(Debug)]
pub enum FilletSurfaceArg<'a> {
    /// A surface without a face (`GetFace()` null): it gets a face over its natural
    /// domain in a temporary B-rep of its own. The same reference passed for both
    /// surfaces is one surface (one temporary face).
    Free(&'a Surface),
    /// The surface of `face` of `body`; the fillet's rail on it is merged into `body`.
    Face {
        /// The face's B-rep.
        body: &'a mut Brep,
        /// The face.
        face: FaceId,
    },
    /// Second argument only, with a [`FilletSurfaceArg::Face`] first: the surface of
    /// `face` of the first argument's body (the first face itself for the same
    /// surface twice).
    FirstBodyFace(FaceId),
}

/// A base surface's B-rep: the caller's, or a temporary one.
enum BaseBrep<'a> {
    Caller(&'a mut Brep),
    Temporary(Box<Brep>),
}
impl BaseBrep<'_> {
    fn brep(&mut self) -> &mut Brep {
        match self {
            Self::Caller(body) => body,
            Self::Temporary(body) => body,
        }
    }

    /// Rewrite a caller's cached topology fields from its use links; a temporary B-rep
    /// is dropped with them, so it is skipped.
    fn refresh_cached_topology(&mut self) {
        if let Self::Caller(body) = self {
            body.refresh_cached_topology();
        }
    }
}

/// `new(*pContext) SmBrep()` and `CreateFaceFromSurface(pSurface,
/// pSurface->GetNaturalUVDomain(), pFace)`: a temporary B-rep for a free surface.
fn temporary_face(k: &mut Kernel, surface: &Surface) -> Result<(Brep, FaceId)> {
    let mut temp = crate::topology::brep::p3::new_brep(
        k,
        crate::geom::tol::USE_DEFAULT,
        crate::geom::tol::USE_DEFAULT,
    )?;
    let face = create_face_from_surface(k, &mut temp, surface)?;
    Ok((temp, face))
}

/// A face argument's face, which must carry a surface (upstream a surface pointer).
fn owned_face(body: &Brep, face: FaceId) -> Result<FaceId> {
    if body.face(face)?.surface.is_none() {
        return Err(invalid_input());
    }
    Ok(face)
}

/// `SmApiSurfaceSurfaceFillet`: a fillet between two arbitrary surfaces with signed
/// radii from each (`SmFilletExecutive::SurfaceSurfaceFillet` with cross-section
/// accuracy 0.1, `SM_BT_MAXIMAL` fillet trimming and no base-surface trimming).
/// A surface that already belongs to a face is filleted on that face, whose B-rep the
/// rail is merged into (the two faces may belong to one B-rep or to two); a free
/// surface gets a face over its natural domain in a temporary B-rep, discarded
/// afterwards. `tolerance` is the largest distance of the rails from the base
/// surfaces; `xsect_type` is 0 linear, 1 approximated circular, 2 exact circular;
/// `mirror` makes an inside-out fillet and `complement` the complementary circular
/// cross section. The B-rep of the fillet surfaces, `None` when the offset surfaces do
/// not intersect.
///
/// # Errors
/// `SM_ERR_INVALID_INPUT` for a [`FilletSurfaceArg::FirstBodyFace`] first argument or
/// after a free first one, or a face without surface; those of the temporary faces
/// and of `SurfaceSurfaceFillet`.
#[allow(clippy::too_many_arguments)] // the upstream entry point's parameters
pub fn surface_surface_fillet(
    k: &mut Kernel,
    surface1: FilletSurfaceArg<'_>,
    surface2: FilletSurfaceArg<'_>,
    radius1: f64,
    radius2: f64,
    tolerance: f64,
    xsect_type: u32,
    mirror: bool,
    complement: bool,
) -> Result<Option<Brep>> {
    // if( !pSurface1->GetFace() ): a temporary B-rep with a face on the surface
    let free1 = match &surface1 {
        FilletSurfaceArg::Free(surface) => Some(*surface),
        _ => None,
    };
    let (mut base1, face1) = match surface1 {
        FilletSurfaceArg::Free(surface) => {
            let (temp, face) = temporary_face(k, surface)?;
            (BaseBrep::Temporary(Box::new(temp)), face)
        }
        FilletSurfaceArg::Face { body, face } => {
            let face = owned_face(body, face)?;
            (BaseBrep::Caller(body), face)
        }
        FilletSurfaceArg::FirstBodyFace(_) => return Err(invalid_input()),
    };
    // if( !pSurface2->GetFace() ): the same surface as the first now has a face
    let (mut base2, face2) = match surface2 {
        FilletSurfaceArg::Free(surface) if free1.is_some_and(|s| std::ptr::eq(s, surface)) => {
            (None, face1)
        }
        FilletSurfaceArg::Free(surface) => {
            let (temp, face) = temporary_face(k, surface)?;
            (Some(BaseBrep::Temporary(Box::new(temp))), face)
        }
        FilletSurfaceArg::Face { body, face } => {
            let face = owned_face(body, face)?;
            (Some(BaseBrep::Caller(body)), face)
        }
        FilletSurfaceArg::FirstBodyFace(face) => {
            if free1.is_some() {
                return Err(invalid_input());
            }
            (None, owned_face(base1.brep(), face)?)
        }
    };
    let options = SurfaceFilletOptions {
        xsect_type,
        xsect_accuracy: 0.1,
        trim_type: BoundaryTrimmingType::Maximal,
        base_trim_type: 0,
        mirror,
        complement,
    };
    let result = {
        let mut session =
            FilletSession::with_second_base(k, base1.brep(), base2.as_mut().map(BaseBrep::brep))?;
        let made = session.surface_surface_fillet(
            k,
            [face1, face2],
            radius1,
            radius2,
            tolerance,
            &options,
        );
        match made {
            Ok(true) => Ok(session.take_fillet_brep()),
            Ok(false) => Ok(None),
            Err(error) => Err(error),
        }
    };
    // (not upstream: the port's cached topology fields, true again for the caller in the
    // bodies whose faces a rail merged into and in the returned fillet B-rep. A rail merge
    // rewires only the use links, and on an error the bodies keep the edits made before
    // it, as upstream. The temporary B-reps are dropped with their faces; the surfaces
    // stay the caller's.)
    base1.refresh_cached_topology();
    if let Some(base2) = base2.as_mut() {
        base2.refresh_cached_topology();
    }
    match result {
        Ok(Some(mut fillet)) => {
            fillet.refresh_cached_topology();
            Ok(Some(fillet))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec3;
    use crate::topology::brep::p1;

    fn boxed(k: &mut Kernel) -> Brep {
        crate::primitive::api_primitives::create_box(k, Vec3::ZERO, 10.0, 20.0, 30.0).unwrap()
    }

    #[test]
    fn second_radius_offsets_the_second_surface_by_its_own_radius() {
        let offsets = |second: Option<f64>| {
            let mut k = Kernel::default();
            let mut body = boxed(&mut k);
            let mut session = FilletSession::new(&mut k, &mut body).unwrap();
            let edge = p1::edges(session.target(), None).unwrap()[0];
            let edgeuse = session.blend_edgeuse(&k, edge).unwrap().unwrap();
            let solver = match second {
                Some(r) => session
                    .new_constant_radius_solver_with_second(&k, edgeuse, 0.5, r, 1e-6, 0.5, 0.03)
                    .unwrap(),
                None => session
                    .new_constant_radius_solver(&k, edgeuse, 0.5, 1e-6, 0.5, 0.03)
                    .unwrap(),
            };
            session
                .solver(solver)
                .unwrap()
                .base()
                .offsets_data()
                .unwrap()
                .surfaces
                .map(|s| s.unwrap().surface.offset_distance().abs())
        };
        assert_eq!(offsets(None), [0.5, 0.5]);
        assert_eq!(offsets(Some(1.0)), [0.5, 1.0]);
    }

    #[test]
    fn per_edge_constant_distance_prepares_constant_distance_solvers() {
        let mut k = Kernel::default();
        let mut body = boxed(&mut k);
        let edges = p1::edges(&body, None).unwrap();
        let mut session = FilletSession::new(&mut k, &mut body).unwrap();
        session
            .prepare_per_edge_constant_distance(
                &mut k,
                &edges[..2],
                &[0.5, 0.75],
                &[FilletSurfaceGeneratorType::Circular; 2],
                1,
                1.0,
            )
            .unwrap();
        for i in 0..2 {
            let solver = session.solver(FilletSolverId(i)).unwrap();
            assert!(matches!(
                solver,
                crate::blend::fillet_executive::session::SolverObject::ConstantDistance(_)
            ));
        }
    }

    #[test]
    fn all_edge_setup_loads_one_solver_per_edge_with_the_blending_handler() {
        for solver_kind in [
            AllEdgeSolver::ConstantRadius,
            AllEdgeSolver::ConstantDistance,
        ] {
            let mut k = Kernel::default();
            let mut body = boxed(&mut k);
            let mut session = FilletSession::new(&mut k, &mut body).unwrap();
            let generator = session
                .add_surface_generator(FilletSurfaceGenerator::Linear)
                .unwrap();
            session.set_self_intersection_handler(Some(MakeSurfaceBlendSih::new(1.0)));
            let edges = p1::edges(session.target(), None).unwrap();
            for (i, edge) in edges.iter().enumerate() {
                let edgeuse = session.blend_edgeuse(&k, *edge).unwrap().unwrap();
                let solver = match solver_kind {
                    AllEdgeSolver::ConstantRadius => session
                        .new_constant_radius_solver(&k, edgeuse, 0.5, 1e-6, 0.5, 0.03)
                        .unwrap(),
                    AllEdgeSolver::ConstantDistance => session
                        .new_constant_distance_solver(&k, edgeuse, 0.5, 1e-6, 0.5, 0.03)
                        .unwrap(),
                };
                assert_eq!(solver.0 as usize, i);
                session.set_solver_generator(solver, generator).unwrap();
                session.load_solver(&k, solver).unwrap();
                let base = session.solver(solver).unwrap().base();
                assert_eq!(
                    base.self_intersection_handler().unwrap(),
                    Some(crate::blend::fillet_intersector::HandlingAlgorithmType::Blending)
                );
            }
            // clearing the executive's handler is seen by every loaded solver
            session.set_self_intersection_handler(None);
            for i in 0..edges.len() {
                let id = FilletSolverId(u32::try_from(i).unwrap());
                let base = session.solver(id).unwrap().base();
                assert_eq!(base.self_intersection_handler().unwrap(), None);
            }
            session.create_corners(&k, None).unwrap();
            let corners = session.fillet_corners().unwrap();
            assert_eq!(corners.len(), 8);
            for corner in &corners {
                session.set_corner_bevel(*corner, true).unwrap();
                assert!(session.corner(*corner).unwrap().is_bevel_corner());
            }
        }
    }

    fn sized_box(k: &mut Kernel, length: f64, width: f64, height: f64) -> Brep {
        crate::primitive::api_primitives::create_box(k, Vec3::ZERO, length, width, height).unwrap()
    }

    #[test]
    fn bevel_corner_of_a_vertex_and_of_a_vertex_without_entry() {
        let mut k = Kernel::default();
        let mut body = boxed(&mut k);
        let edges = p1::edges(&body, None).unwrap();
        let vertices = p1::vertices(&body, None).unwrap();
        let mut session = FilletSession::new(&mut k, &mut body).unwrap();
        session
            .prepare_constant_radius_edges(&mut k, &edges[..1], 0.5, 1, 1, 1.0)
            .unwrap();
        session.create_corners(&k, None).unwrap();
        let mut beveled = 0;
        for vertex in &vertices {
            match session.set_bevel_corner(*vertex) {
                Ok(()) => beveled += 1,
                Err(error) => assert!(
                    matches!(error, SolidError::Upstream { code, .. } if code == crate::error::SM_ERR_NULL_POINTER)
                ),
            }
        }
        // one edge: corners at its two end vertices only
        assert_eq!(beveled, 2);
        for corner in session.fillet_corners().unwrap() {
            assert!(session.corner(corner).unwrap().is_bevel_corner());
        }
    }

    #[test]
    fn bevel_corners_without_edges_is_invalid_input() {
        let mut k = Kernel::default();
        let mut body = boxed(&mut k);
        let r = set_bevel_corners(&mut k, &mut body, &[], 1.0, &[]);
        assert!(
            matches!(r, Err(SolidError::Upstream { code, .. }) if code == SM_ERR_INVALID_INPUT)
        );
        assert_eq!(p1::faces(&body, None).unwrap().len(), 6);
    }

    #[test]
    fn single_box_edge_fillet_is_a_closed_manifold_solid() {
        let mut k = Kernel::default();
        let mut body = boxed(&mut k);
        let edge = p1::edges(&body, None).unwrap()[0];
        fillet_edges(&mut k, &mut body, &[edge], 0.5, 1, 1, 1.0).unwrap();
        assert_eq!(p1::faces(&body, None).unwrap().len(), 7);
        for edge in p1::edges(&body, None).unwrap() {
            assert!(crate::topology::edge::is_manifold(&body, edge).unwrap());
        }
        assert!(!k.context.doing_boolean());
    }

    #[test]
    fn per_edge_fillet_matches_the_shared_parameter_fillet() {
        let mut k = Kernel::default();
        let mut shared = boxed(&mut k);
        let edge = p1::edges(&shared, None).unwrap()[0];
        fillet_edges(&mut k, &mut shared, &[edge], 0.5, 1, 1, 1.0).unwrap();
        let mut k = Kernel::default();
        let mut per_edge = boxed(&mut k);
        fillet_edges_per_edge(
            &mut k,
            &mut per_edge,
            &[edge],
            &[0.5],
            &[FilletSurfaceGeneratorType::Circular],
            1,
            1.0,
        )
        .unwrap();
        assert_eq!(
            p1::edges(&per_edge, None).unwrap().len(),
            p1::edges(&shared, None).unwrap().len()
        );
        assert_eq!(
            p1::vertices(&per_edge, None).unwrap().len(),
            p1::vertices(&shared, None).unwrap().len()
        );
    }

    #[test]
    fn empty_or_mismatched_lists_are_invalid_input_without_edits() {
        let mut k = Kernel::default();
        let mut body = boxed(&mut k);
        let edges = p1::edges(&body, None).unwrap();
        let invalid = |r: Result<()>| matches!(r, Err(SolidError::Upstream { code, .. }) if code == SM_ERR_INVALID_INPUT);
        assert!(invalid(fillet_edges(
            &mut k,
            &mut body,
            &[],
            0.5,
            1,
            1,
            1.0
        )));
        assert!(invalid(fillet_edges_per_edge(
            &mut k,
            &mut body,
            &edges[..2],
            &[0.5],
            &[FilletSurfaceGeneratorType::Circular; 2],
            1,
            1.0,
        )));
        assert_eq!(p1::faces(&body, None).unwrap().len(), 6);
    }

    fn volume(k: &Kernel, body: &mut Brep) -> f64 {
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::mass::integrator::properties::{brep_volume, PropertyAttributes};
        let store = AttributeStore::default();
        let key_of = |_: crate::EntityRef| ObjectKey(0);
        brep_volume(
            k,
            body,
            1e-9,
            &PropertyAttributes {
                store: &store,
                key_of: &key_of,
            },
        )
        .unwrap()
    }

    #[test]
    fn issue_1421_all_edge_fillet_leaves_the_cached_topology_current() {
        const RADIUS: f64 = 3.0;
        let mut k = Kernel::default();
        let mut body = sized_box(&mut k, 600.0, 400.0, 18.0);
        let edges = p1::edges(&body, None).unwrap();
        fillet_edges(&mut k, &mut body, &edges, RADIUS, 1, 1, 1.0).unwrap();
        // Immediately, with no caller refresh: the port's cached face, loop and shell
        // fields have to agree with the use links the fillet rewired.
        crate::topology::integrity::check(&body).unwrap();
        assert_eq!(p1::faces(&body, None).unwrap().len(), 26);
        assert_eq!(p1::edges(&body, None).unwrap().len(), 48);
        assert_eq!(p1::vertices(&body, None).unwrap().len(), 24);
        assert!(crate::topology::brep::p2::is_manifold_solid(&body).unwrap());
        // The rounded box is its inner box grown by the radius ball: the inner box, six
        // face slabs, twelve quarter cylinders and eight sphere octants.
        let [a, b, c] = [
            600.0 - 2.0 * RADIUS,
            400.0 - 2.0 * RADIUS,
            18.0 - 2.0 * RADIUS,
        ];
        let want = a * b * c
            + 2.0 * (a * b + b * c + c * a) * RADIUS
            + PI * (a + b + c) * RADIUS * RADIUS
            + 4.0 / 3.0 * PI * RADIUS * RADIUS * RADIUS;
        let got = volume(&k, &mut body);
        assert!((got - want).abs() <= 1e-9 * want, "{got} vs {want}");
    }

    #[test]
    fn every_fillet_entry_point_leaves_the_cached_topology_current() {
        fn check_entry(name: &str, fillet: impl FnOnce(&mut Kernel, &mut Brep)) {
            let mut k = Kernel::default();
            let mut body = boxed(&mut k);
            fillet(&mut k, &mut body);
            if let Err(error) = crate::topology::integrity::check(&body) {
                panic!("{name}: {error:?}");
            }
        }
        check_entry("fillet_edges", |k, body| {
            let edges = p1::edges(body, None).unwrap();
            fillet_edges(k, body, &edges[..1], 0.5, 1, 1, 1.0).unwrap();
        });
        check_entry("fillet_edges_per_edge", |k, body| {
            let edges = p1::edges(body, None).unwrap();
            fillet_edges_per_edge(
                k,
                body,
                &edges[..1],
                &[0.5],
                &[FilletSurfaceGeneratorType::Circular],
                1,
                1.0,
            )
            .unwrap();
        });
        check_entry("variable_radius_fillet", |k, body| {
            let edges = p1::edges(body, None).unwrap();
            variable_radius_fillet(k, body, &edges[..1], 0.5, 2.0, 1, 1).unwrap();
        });
        check_entry("circular_fillet", |k, body| {
            circular_fillet(k, body, 0.5).unwrap();
        });
        check_entry("chamfer_fillet", |k, body| {
            chamfer_fillet(k, body, 0.5).unwrap();
        });
        check_entry("set_bevel_corners", |k, body| {
            let edges = p1::edges(body, None).unwrap();
            let vertices = p1::vertices(body, None).unwrap();
            set_bevel_corners(k, body, &edges, 0.5, &vertices).unwrap();
        });
    }

    fn bilinear_surface(corners: &[[f64; 3]; 4]) -> Surface {
        Surface::from(
            crate::nurbs::b_spline_surface::BSplineSurface::create_bilinear_surface(
                Vec3::from(corners[0]),
                Vec3::from(corners[1]),
                Vec3::from(corners[2]),
                Vec3::from(corners[3]),
            )
            .unwrap(),
        )
    }

    fn sheet(k: &mut Kernel, surfaces: &[&Surface]) -> (Brep, Vec<FaceId>) {
        let mut body = crate::topology::brep::p3::new_brep(
            k,
            crate::geom::tol::USE_DEFAULT,
            crate::geom::tol::USE_DEFAULT,
        )
        .unwrap();
        let mut faces = Vec::new();
        for surface in surfaces {
            faces.push(create_face_from_surface(k, &mut body, surface).unwrap());
        }
        (body, faces)
    }

    const FIRST_SURFACE: [[f64; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [0.0, 10.0, 0.0],
        [10.0, 10.0, 0.0],
    ];
    const SECOND_SURFACE: [[f64; 3]; 4] = [
        [5.0, 0.0, -5.0],
        [5.0, 0.0, 5.0],
        [5.0, 10.0, -5.0],
        [5.0, 10.0, 5.0],
    ];
    const ADJACENT_SURFACE: [[f64; 3]; 4] = [
        [10.0, 0.0, 0.0],
        [10.0, 0.0, 10.0],
        [10.0, 10.0, 0.0],
        [10.0, 10.0, 10.0],
    ];

    #[test]
    fn surface_surface_fillet_leaves_the_caller_and_result_cached_topology_current() {
        let mut k = Kernel::default();
        let first = bilinear_surface(&FIRST_SURFACE);
        let second = bilinear_surface(&SECOND_SURFACE);
        let (mut body, faces) = sheet(&mut k, &[&first, &second]);
        let fillet = surface_surface_fillet(
            &mut k,
            FilletSurfaceArg::Face {
                body: &mut body,
                face: faces[0],
            },
            FilletSurfaceArg::FirstBodyFace(faces[1]),
            2.0,
            2.0,
            0.01,
            0,
            false,
            false,
        )
        .unwrap()
        .unwrap();
        // Both the borrowed body the rails merged into and the returned B-rep.
        crate::topology::integrity::check(&fillet).unwrap();
        crate::topology::integrity::check(&body).unwrap();
        assert_eq!(p1::faces(&body, None).unwrap().len(), 4);

        // Free surfaces have only the returned B-rep to finalize.
        let mut k = Kernel::default();
        let first = bilinear_surface(&FIRST_SURFACE);
        let second = bilinear_surface(&SECOND_SURFACE);
        let fillet = surface_surface_fillet(
            &mut k,
            FilletSurfaceArg::Free(&first),
            FilletSurfaceArg::Free(&second),
            2.0,
            2.0,
            0.01,
            0,
            false,
            false,
        )
        .unwrap()
        .unwrap();
        crate::topology::integrity::check(&fillet).unwrap();
    }

    #[test]
    fn surface_surface_fillet_error_propagates_and_finalizes_the_caller_topology() {
        let mut k = Kernel::default();
        let first = bilinear_surface(&FIRST_SURFACE);
        let second = bilinear_surface(&ADJACENT_SURFACE);
        let (mut body, faces) = sheet(&mut k, &[&first, &second]);
        let result = surface_surface_fillet(
            &mut k,
            FilletSurfaceArg::Face {
                body: &mut body,
                face: faces[0],
            },
            FilletSurfaceArg::FirstBodyFace(faces[1]),
            0.001,
            0.001,
            0.01,
            2,
            false,
            false,
        );
        // The executive's own error, not a finalization error.
        assert!(matches!(
            result,
            Err(SolidError::Upstream { code, .. }) if code == crate::error::SM_ERR
        ));
        crate::topology::integrity::check(&body).unwrap();
        assert_eq!(p1::faces(&body, None).unwrap().len(), 2);
    }

    #[test]
    fn a_failed_variable_radius_fillet_keeps_its_error_and_a_current_body() {
        let mut k = Kernel::default();
        let mut body = boxed(&mut k);
        let edge = p1::edges(&body, None).unwrap()[0];
        let result = variable_radius_fillet(&mut k, &mut body, &[edge], 0.5, 30.0, 1, 1);
        assert!(matches!(
            result,
            Err(SolidError::Upstream { code, .. }) if code == crate::error::SM_ERR
        ));
        crate::topology::integrity::check(&body).unwrap();
        assert_eq!(p1::faces(&body, None).unwrap().len(), 6);
    }
}
