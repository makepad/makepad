//! `SM_API` trimmed surfaces: a surface trimmed by model-space curve loops, planar
//! faces bounded by coplanar curves, and B-reps or surfaces trimmed by a projected
//! curve.
//!
//! Upstream: `source/SM_API/src/SmApiTrimmedSurfaces.cpp`, `source/SM_API/src/SmApiBrep.cpp`,
//! `source/SM_API/inc/SmApiTrimmedSurfaces.h`, `source/SM_API/inc/SmApiBrep.h`.
//!
//! | Rust                               | Upstream                                       |
//! |------------------------------------|------------------------------------------------|
//! | [`trim_surface_with_3d_curves`]    | `SmApiTrimSurfaceWith3dCurves`                 |
//! | [`trim_project_parallel`], [`TrimObject`] | `SmApiTrimProjectParallel`              |
//! | [`create_planar_faces`]            | `SmApiCreatePlanarFaces`                       |
//! | [`project_and_trim`]               | `SmApiProjectAndTrim`                          |
//! | `trim_into_new_brep` (private)     | `SmTrimmingTools::TrimSurfaceWithModelSpaceCurves` (B-rep overload) |
//!
//! Deviations:
//! - **Ownership.** Upstream consumes the surface and curves it is given (they become
//!   the new B-rep's geometry); the port takes them by value or copies them.
//!   `SmApiCreatePlanarFaces`' detaching of the caller's curves from the edges of a
//!   failed B-rep has no counterpart: the port's edges own copies.
//! - **Out-params.** The new or trimmed B-rep is the return value; on failure there is
//!   none, as upstream's `rpResult` is `NULL` then. [`project_and_trim`] edits the
//!   caller's B-rep in place, as upstream (a failure can leave it partly edited).
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`]; the B-rep edits
//!   notify through a default record per object (`ArenaNotifyState`); the projection
//!   names the B-rep's objects by the identities its geometry owners use
//!   ([`BrepIdentity::of_brep`]).
//! - **Projection curve.** Upstream's `SmBSplineCurve*` is a [`Curve`] of one of its
//!   classes (B-spline, line, ellipse, hyperbola, parabola); the projection dispatches on
//!   its class as upstream's virtuals do.
//! - **`SmObject*`.** `SmApiTrimProjectParallel`'s object is a [`TrimObject`] (a surface
//!   or a B-rep); upstream's check that it is one of the two cannot fail.
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; the called
//!   kernel functions' errors pass through unchanged.

use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::tol::USE_DEFAULT;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::brep::BrepIdentity;
use crate::surface::surface::Surface;
use crate::topology::brep::p10::{ProjectAndTrimOptions, ProjectEnv, TrimType};
use crate::topology::brep::p2::ArenaNotifyState;
use crate::topology::brep::p8::FaceBuildEnv;
use crate::topology::loopuse::notify_ref;
use crate::topology::topology_traverser::Marks;
use crate::topology::trimming_tools::{trim_surface_with_model_space_curves, TrimInput};
use crate::topology::{Brep, FaceId};

const OP_PROJECT_PARALLEL: &str = "primitive::api_trim_ops::trim_project_parallel";
const OP_PROJECT_AND_TRIM: &str = "primitive::api_trim_ops::project_and_trim";

/// The tolerance `SmApiTrimSurfaceWith3dCurves` and `SmApiCreatePlanarFaces` pass.
const API_TOL: f64 = 0.01;

/// `new(*SmApiGetOrCreateContext()) SmBrep()`.
fn new_api_brep(k: &mut Kernel) -> Result<Brep> {
    crate::topology::brep::p3::new_brep(k, USE_DEFAULT, USE_DEFAULT)
}

/// `SmTrimmingTools::TrimSurfaceWithModelSpaceCurves`, B-rep overload: a new B-rep,
/// then the region overload in its infinite region.
#[allow(clippy::fn_params_excessive_bools)]
fn trim_into_new_brep(
    k: &mut Kernel,
    tol_3d: f64,
    input: TrimInput,
    first_is_outer: bool,
    oriented_correctly: bool,
    may_cross_seam: bool,
) -> Result<Brep> {
    let mut b = new_api_brep(k)?;
    let region = b.infinite_region();
    let mut state = ArenaNotifyState::new(&b);
    let mut env = state.env(None);
    trim_surface_with_model_space_curves(
        k,
        &mut b,
        region,
        tol_3d,
        input,
        first_is_outer,
        oriented_correctly,
        may_cross_seam,
        &mut env,
    )?;
    Ok(b)
}

/// A new B-rep of `surface` trimmed by the model-space curve loops `curves` (the first
/// `loop_counts[0]` curves the outer loop, and so on; each loop head to tail, every
/// curve oriented as in its loop) over the surface's natural UV domain
/// (`SmApiTrimSurfaceWith3dCurves`, through
/// `SmTrimmingTools::TrimSurfaceWithModelSpaceCurves` with tolerance `0.01`, loops taken
/// as ordered and oriented correctly, and allowed to cross a seam).
///
/// # Errors
///
/// Those of the surface's natural domain, of the new B-rep and of
/// [`trim_surface_with_model_space_curves`].
pub fn trim_surface_with_3d_curves(
    k: &mut Kernel,
    surface: Surface,
    loop_counts: &[usize],
    curves: Vec<Curve>,
) -> Result<Brep> {
    let orientations = vec![OrientType::Same; curves.len()];
    let uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    trim_into_new_brep(
        k,
        API_TOL,
        TrimInput {
            loop_counts: loop_counts.to_vec(),
            curves,
            orientations,
            loop_points: Vec::new(),
            surface,
            uv_domain,
            surface_orientation: OrientType::Same,
        },
        true,
        true,
        true,
    )
}

/// The object [`trim_project_parallel`] trims (upstream an `SmObject*` that is a surface
/// or a B-rep).
#[derive(Debug)]
pub enum TrimObject {
    /// A surface without owner: it becomes the one face of a new B-rep.
    Surface(Box<Surface>),
    /// A B-rep, trimmed in place.
    Brep(Box<Brep>),
}

/// `SmBrep::ProjectAndTrim(curve, direction, reference, trim_type, NULL, NULL, NULL)` on
/// `b`, its objects named by the identities its geometry owners use (its naming scheme
/// with the proven producer owner keys it records, as [`BrepIdentity::of_brep`]: a
/// filleted B-rep's geometry keeps the fillet's owner keys).
fn project_onto(
    k: &mut Kernel,
    b: &mut Brep,
    curve: &Curve,
    direction: Vec3,
    reference: Point3,
    trim_type: TrimType,
) -> Result<()> {
    let proven = BrepIdentity::of_brep(b);
    let identity = proven.identity();
    let env = ProjectEnv {
        brep_id: identity.brep_id,
        keys: identity.keys,
        owner_of: identity.owner_of,
    };
    let mut state = ArenaNotifyState::new(b);
    let mut notify = state.env(None);
    crate::topology::brep::p10::project_and_trim(
        k,
        b,
        &env,
        &mut notify,
        curve,
        direction,
        reference,
        trim_type,
        &ProjectAndTrimOptions::default(),
    )
}

/// `object` trimmed by `curve` projected along `direction`
/// (`SmApiTrimProjectParallel`): a surface first becomes the one face of a new B-rep
/// over its natural UV domain (`SmBrep::CreateFaceFromSurface`), then
/// `SmBrep::ProjectAndTrim` keeps the side of `reference`, deletes it, or keeps both
/// (`trim_type`).
///
/// # Errors
///
/// `InvalidInput` for a surface with an owner or, with a surface, a direction shorter
/// than `SM_EFF_ZERO`; those of the face construction and of
/// [`crate::topology::brep::p10::project_and_trim`].
pub fn trim_project_parallel(
    k: &mut Kernel,
    object: TrimObject,
    curve: &Curve,
    direction: Vec3,
    reference: Point3,
    trim_type: TrimType,
) -> Result<Brep> {
    let mut b = match object {
        TrimObject::Surface(surface) => {
            if surface.owner().is_some() {
                return Err(SolidError::invalid_input(
                    OP_PROJECT_PARALLEL,
                    "surface has an owner",
                ));
            }
            if direction.is_zero(EFF_ZERO) {
                return Err(SolidError::invalid_input(
                    OP_PROJECT_PARALLEL,
                    "zero projection direction",
                ));
            }
            let mut b = new_api_brep(k)?;
            let domain = surface.natural_uv_domain_with_nurb(k)?;
            let mut state = ArenaNotifyState::new(&b);
            let mut env = state.env(None);
            let mut marks = Marks::default();
            let this_face = |f: FaceId| notify_ref(EntityRef::Face(f));
            let mut fenv = FaceBuildEnv {
                env: &mut env,
                marks: &mut marks,
                this_face: &this_face,
            };
            crate::topology::brep::p8::create_face_from_surface(
                k,
                &mut b,
                Some(*surface),
                &domain,
                &mut fenv,
            )?;
            b
        }
        TrimObject::Brep(b) => *b,
    };
    project_onto(k, &mut b, curve, direction, reference, trim_type)?;
    Ok(b)
}

/// A new B-rep of planar faces bounded by the closed loops of the coplanar `curves`
/// (any order and orientation; `SmApiCreatePlanarFaces`, through
/// `SmBrep::CreatePlanarFacesWith3DCurves` in the infinite region with tolerance
/// `0.01`).
///
/// # Errors
///
/// Those of the new B-rep and of
/// [`crate::topology::brep::p8::create_planar_faces_with_3d_curves`].
pub fn create_planar_faces(k: &mut Kernel, curves: &[Curve]) -> Result<Brep> {
    let mut b = new_api_brep(k)?;
    let region = b.infinite_region();
    let mut state = ArenaNotifyState::new(&b);
    let mut env = state.env(None);
    let mut marks = Marks::default();
    let this_face = |f: FaceId| notify_ref(EntityRef::Face(f));
    let mut fenv = FaceBuildEnv {
        env: &mut env,
        marks: &mut marks,
        this_face: &this_face,
    };
    crate::topology::brep::p8::create_planar_faces_with_3d_curves(
        k, &mut b, region, curves, API_TOL, &mut fenv,
    )?;
    Ok(b)
}

/// Trims `brep` in place by `curve` projected along `direction`, keeping the side of
/// `reference` (`SmApiProjectAndTrim`, through `SmBrep::ProjectAndTrim` with
/// `SM_TT_KEEP_POINT` and the unit direction).
///
/// # Errors
///
/// `InvalidInput` for a direction that does not unitize; those of
/// [`crate::topology::brep::p10::project_and_trim`].
pub fn project_and_trim(
    k: &mut Kernel,
    brep: &mut Brep,
    curve: &Curve,
    direction: Vec3,
    reference: Point3,
) -> Result<()> {
    let direction = direction
        .unitize()
        .map_err(|_| SolidError::invalid_input(OP_PROJECT_AND_TRIM, "zero projection direction"))?;
    project_onto(k, brep, curve, direction, reference, TrimType::KeepPoint)
}
