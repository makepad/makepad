//! The implicit offset surface `SmOffsetSurface`: a base surface moved a signed distance
//! along its unit normal, `Offset(uv) = Base(uv) + d * N(uv)`, evaluated from the base
//! surface's derivatives (up to third order), with step-off extrapolation at poles and
//! an optional extension of a B-spline base for parameters outside its domain.
//!
//! Upstream: `source/SMLib/src/SmOffsetSurface.cpp`, `source/SMLib/inc/SmOffsetSurface.h`.
//!
//! | Rust                                                  | Upstream                                         |
//! |-------------------------------------------------------|--------------------------------------------------|
//! | [`OffsetSurface`], [`OffsetSurface::new`]             | `SmOffsetSurface`, `SmOffsetSurface(d, rSurface, bOwnsSurface, lSingularities)`, `~SmOffsetSurface` (`Drop`) |
//! | [`OffsetSurface::copy`]                               | `Copy`                                           |
//! | [`OffsetSurface::approx_eq`]                          | `operator==`                                     |
//! | [`OffsetSurface::create_extended_surface`], [`OffsetSurface::create_extended_surface_dir`] | `CreateExtendedSurface` (both) |
//! | [`OffsetSurface::calculate_bounding_box`]             | `CalculateBoundingBox`                           |
//! | `extend_surface` (private)                            | `sm_ExtendSurface` (static)                      |
//! | [`OffsetSurface::knots`]                              | `GetKnots`                                       |
//! | [`OffsetSurface::evaluate_normal`], [`OffsetSurface::evaluate_normal_extending`] | `EvaluateNormal` |
//! | [`OffsetSurface::evaluate_point`], [`OffsetSurface::evaluate_point_extending`] | `EvaluatePoint`    |
//! | [`OffsetSurface::evaluate`], [`OffsetSurface::evaluate_extending`] | `Evaluate`                     |
//! | [`OffsetSurface::evaluate_simple`], [`OffsetSurface::evaluate_simple_extending`] | `EvaluateSimple` |
//! | `plain_or_simple` (private)                           | `EvaluatePlainOrSimple`                          |
//! | `find_nonsingular_stepoff` (private)                  | `sm_FindNonsingularStepoff` (static)             |
//! | [`OffsetSurface::natural_uv_domain`]                  | `GetNaturalUVDomain`                             |
//! | [`OffsetSurface::reverse`], [`OffsetSurface::split_at`], [`OffsetSurface::step_inversion`] (and `_extending`), [`OffsetSurface::swap_uv`], [`OffsetSurface::transform`], [`OffsetSurface::trim_with_domain`] | `Reverse`, `SplitAt`, `STEPInversion`, `SwapUV`, `Transform`, `TrimWithDomain` |
//! | [`OffsetSurface::create_offset_surface`]              | `CreateOffsetSurface`                            |
//! | [`OffsetSurface::read_from_db`]                       | `ReadFromDB`                                     |
//! | [`OffsetSurface::write_to_db`]                        | `WriteToDB`                                      |
//! | [`OffsetSurface::is_kind_of`], [`OffsetSurface::type_code`], [`OffsetSurface::type_string`], [`OffsetSurface::class_string`] | `IsKindOf`, `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`OffsetSurface::offset_distance`], [`OffsetSurface::base_surface`], [`OffsetSurface::extended_base_surface`], [`OffsetSurface::max_analytic_domain`], [`OffsetSurface::degree`] | `GetOffsetDistance`, `GetBaseSurface`, `GetExtendedBaseSurface`, `GetMaxAnalyticDomain`, `GetDegree` |
//! | [`OffsetSurface::set_offset_distance`], [`OffsetSurface::set_allow_extension`], [`OffsetSurface::set_context`] | `SetOffsetDistance`, `SetAllowExtension`, `SetContext` |
//! | [`SS_UNKNOWN`], [`MAX_DERIVS`]                        | `SM_SS_UNKNOWN` (`SmTypes.h`), `MAX_DERIVS` (local of `EvaluatePlainOrSimple`) |
//! | [`MAX_STEPOFF_DEPTH`]                                 | no counterpart (recursion bound)                 |
//! | [`BASE_OWNER`]                                        | the owner `m_pSurface->SetOwner(this)` gives an owned base |
//! | [`OffsetSurface::with_evaluator`], [`OffsetEval`]     | the offset as an `SmSurface` for inherited bodies (`Evaluate1stDerivatives`, `ComputeInwardVector`, ...) |
//!
//! Deviations:
//! - **Base surfaces.** `m_pSurface` and `m_pExtendedSurface` are owned boxes; a base
//!   upstream shares (`bOwnsSurface == FALSE`) is a copy here, and `m_bOwnsSurface` is
//!   kept as a flag because `Transform`, `SwapUV` and `TrimWithDomain` fail without it.
//!   An owning offset sets its base's owner to [`BASE_OWNER`] wherever upstream calls
//!   `m_pSurface->SetOwner(this)` (the constructor, `Copy`, the copy constructor and
//!   `ReadFromDB`); the extended surface has none. Upstream reads that owner: the
//!   point solver's loose tolerance (`LocalPointSolve_1`: `GetZoneTol3d(GetOwner())`
//!   when there is one) and every `GetFace()` walk (zone and approximation tolerances of
//!   the base, its expanded boxes). The base, held by value, cannot walk up, so the
//!   offset hands down the face its own owner chain reaches ([`Surface::set_owner`]
//!   keeps it in step) and [`Surface::face`] of the base returns it. A non-owned base
//!   keeps its caller's owner, as upstream's shared base does. The `SetBaseSurface` setter, the
//!   unused copy constructor, `CreateMirrorSurface`, `GetMemoryUsed`, `WriteToDB`,
//!   `Draw`, `DrawUV`, `Dump` and `AssertValid` are not ported (unused upstream).
//! - **Context.** Every surface has a context here, so the clamp-instead-of-extend
//!   branch for `GetContext() == NULL` is never taken; `SetContext` has nothing to set.
//!   The constructor and every member that constructs an offset ([`OffsetSurface::copy`],
//!   [`OffsetSurface::split_at`], [`OffsetSurface::create_extended_surface_dir`],
//!   [`OffsetSurface::create_offset_surface`], [`OffsetSurface::read_from_db`]) take the
//!   kernel for the base's `GetSingularities`.
//! - **Extension cache.** Upstream's `const` evaluators (`EvaluatePoint`, `Evaluate`,
//!   `EvaluateSimple`, `EvaluateNormal` through the base body, and `STEPInversion` through
//!   `EvaluatePoint`) build an extended
//!   base surface when extension is allowed (only the fillet standard solver allows
//!   it) and the point lies outside the domain, and store it in `m_pExtendedSurface`
//!   through a `const` cast. The `&self` evaluators keep it too, in a pending slot (a
//!   `Cell`-like slot) that supersedes the stored extension until a `&mut self` method folds it
//!   in: later evaluations, `GetNaturalUVDomain` and `GetKnots` see it as upstream (a
//!   point solve on the offset that leaves the base domain sees the extended natural
//!   domain from then on). The `*_extending` (`&mut self`) evaluators store it at once.
//!   [`OffsetSurface::step_inversion_extending`] also stores the one an offset base's
//!   inversion makes (its `EvaluatePoint`), as upstream's base offset does.
//!   [`OffsetSurface::with_evaluator`] runs an inherited `SmSurface` body on the
//!   offset (as [`OffsetEval`]) and stores an extension its evaluations make; the
//!   bodies [`OffsetEval`] runs on a copy keep the copy's new extension. A copy
//!   (`Clone`, [`OffsetSurface::copy`]) takes the current extension.
//!   [`OffsetSurface::trim_with_domain`] uses the storing evaluator. The `SM_NO_PRE_EDIT`,
//!   `SM_NO_COPY` and `SM_NO_SPLIT` notifications are the object layer's.
//! - **Evaluation output.** `Evaluate` / `EvaluateSimple` return `D[i][j]` (`i` times in
//!   `u`, `j` in `v`) built from upstream's flat array, whose untouched entries hold
//!   [`Vec3::UNDEF`] (the default `SmVector3d`). With `bOnlyUpperHalf` and different `u`
//!   and `v` orders upstream writes some derivatives into other entries' slots (kept)
//!   or past the end of the caller's array (undefined; [`SolidError::InvalidInput`]
//!   here). Never-assigned locals that upstream reads in those cases (`Cv`, `Nv`, ...)
//!   are [`Vec3::UNDEF`], as the default `SmVector3d`.
//! - **Upstream bugs kept** (`plan/upstream-issues.md`): the third derivatives `Cuuv` and
//!   `Cuvv` of the normal's numerator use the coefficient `3.0` for `Buuv x Buv` and
//!   `Buvv x Buv` (the derivative of `Cuu` / `Cvv` gives `1` and `-1`); the outputs
//!   `Duvv` and `Dvvv` add `Buuv` instead of `Buvv` / `Bvvv`; `SplitAt` with an extended
//!   surface splits the base surface a second time for the extended halves; `SwapUV`
//!   with an extended surface negates the offset distance twice; `STEPInversion`
//!   retries on the extended surface with the tolerance not widened by the offset.
//! - **Recursion.** The pole step-off of `EvaluatePlainOrSimple` evaluates itself at
//!   the step-off points; its depth is bounded by [`MAX_STEPOFF_DEPTH`] (upstream
//!   fixtures reach depth 1). Construction and restoration bound nested surface
//!   wrappers to [`MAX_SURFACE_NESTING`] (32, versus at most 3 in fixtures), including
//!   STEP/bounded wrappers and extended bases, before any recursive geometry query.
//!   Database dispatch also enforces the reader's nesting limit.
//! - **Database.** `ReadFromDB` always makes a new surface (the fill-an-existing-object
//!   mode is not used by `SmSurface::ReadFromDB`). An ASCII flag the stream cannot
//!   extract (uninitialised upstream) reads as `0`.
//! - **`GetSingularities`.** The constructor and `ReadFromDB` call it through an
//!   `SmSurface` reference, so a base that is itself an offset runs the `SmSurface` body
//!   too: `SmOffsetSurface::GetSingularities()` (no arguments, returning the stored
//!   flags) hides the virtual rather than overriding it.

use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::curve::curve::Curve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::attribute::ObjectKey;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::{Extent2d, SS_UMAX, SS_UMIN, SS_VMAX, SS_VMIN};
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::object::ObjectRef;
use crate::geom::polar_box::{PolarBox, PolarBoxState};
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tol::{are_same, is_zero};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{min3, EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::b_spline_surface::{BSplineSurface, LocationType, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::nurbs::srf_adv::p1::{eval_pt_derivs_unbounded, UnboundedExtension};
use crate::nurbs::srf_adv::p3::ExtendContinuity;
use crate::surface::surface::p1::SurfaceBoxes;
use crate::surface::surface::p2::{
    check_geometry_nesting, is_kind_of as surface_is_kind_of, NestedGeometry, SurfaceEvaluate,
    B_SPLINE_SURFACE_TYPE, MAX_GEOMETRY_NESTING, OFFSET_SURFACE_TYPE,
};
use crate::surface::surface::p7::base_evaluate_normal;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::database_io::{DatabaseReader, DatabaseWriter, FileType, InputStream};
use crate::topology::geometry_write::write_surface_at;

const OP: &str = "surface::offset_surface::OffsetSurface";
const OP_NEW: &str = "surface::offset_surface::OffsetSurface::new";
const OP_EVAL: &str = "surface::offset_surface::OffsetSurface::evaluate";
const OP_POINT: &str = "surface::offset_surface::OffsetSurface::evaluate_point";
const OP_EXTEND: &str = "surface::offset_surface::extend_surface";
const OP_STEPOFF: &str = "surface::offset_surface::find_nonsingular_stepoff";
const OP_EDIT: &str = "surface::offset_surface::OffsetSurface::edit";
const OP_INV: &str = "surface::offset_surface::OffsetSurface::step_inversion";
const OP_READ: &str = "surface::offset_surface::OffsetSurface::read_from_db";

/// `SM_SS_UNKNOWN`: the surface's singularities are not yet tested (the constructor
/// then asks the base surface).
pub const SS_UNKNOWN: u32 = 16;

/// `MAX_DERIVS`: the highest total derivative order an offset evaluation computes.
pub const MAX_DERIVS: usize = 3;

/// The largest depth of the pole step-off recursion of `EvaluatePlainOrSimple` (each
/// level evaluates the offset at two step-off points); past it the evaluation fails
/// with [`SolidError::InvalidInput`]. Upstream fixtures reach depth 1.
pub const MAX_STEPOFF_DEPTH: usize = 32;

/// The size of upstream's local derivative matrices (`(2*MAX_DERIVS+1)^2`).
const MAT_LEN: usize = (2 * MAX_DERIVS + 1) * (2 * MAX_DERIVS + 1);

/// The implicit offset of a base surface by a signed distance along its unit normal
/// (`SmOffsetSurface`); a positive distance goes to the side of the natural normal.
///
/// When extension is allowed, points outside the base domain are evaluated on a
/// B-spline base extended to contain them (`m_pExtendedSurface`); otherwise they are
/// clamped to the domain.
#[derive(Debug)]
pub struct OffsetSurface {
    /// `m_dOffsetDistance`. Interior-mutable ([`F64Cell`]) so that a copy of a fillet solver's offset
    /// surface (the surface a fillet trace intersects) can follow the distance the
    /// solver sets on its own, as upstream's intersector and solver share one object
    /// ([`OffsetSurface::set_offset_distance`] takes `&self`).
    offset_distance: F64Cell,
    /// `m_bAllowExtension`.
    allow_extension: bool,
    /// `m_bOwnsSurface`.
    owns_surface: bool,
    /// `m_pSurface`: the base surface.
    surface: Box<Surface>,
    /// `m_pExtendedSurface`: the base extended for evaluation outside its domain.
    extended_surface: Option<Box<Surface>>,
    /// An extension a `&self` evaluator made: upstream stores it in `m_pExtendedSurface`
    /// at once through a `const` cast (`SmOffsetSurface.cpp:710-713`, `:1064-1068`), so
    /// it supersedes `extended_surface` until a `&mut self` method folds it in.
    pending_extension: PendingExtension,
    /// `m_bIsPeriodicOnU` (an `SmBoolean`: values read from a file are kept as read).
    is_periodic_on_u: i32,
    /// `m_bIsPeriodicOnV`.
    is_periodic_on_v: i32,
    /// `m_lSingularities`: `SS_NONE` or an or of `SS_UMIN`, `SS_VMIN`, `SS_UMAX`,
    /// `SS_VMAX`.
    singularities: u32,
}

/// A field-wise copy (the Rust `Clone`, also standing for the same object in a
/// temporary): [`Surface`]'s `Clone` resets an owner, so the base's owner state (owned
/// by this offset or kept from its caller) is copied explicitly.
impl Clone for OffsetSurface {
    fn clone(&self) -> Self {
        Self {
            offset_distance: F64Cell::new(self.offset_distance.get()),
            allow_extension: self.allow_extension,
            owns_surface: self.owns_surface,
            surface: Box::new(self.surface.clone_keeping_owner()),
            extended_surface: self
                .with_current_extension(|e| e.map(|e| Box::new(e.clone_keeping_owner()))),
            pending_extension: PendingExtension::default(),
            is_periodic_on_u: self.is_periodic_on_u,
            is_periodic_on_v: self.is_periodic_on_v,
            singularities: self.singularities,
        }
    }
}

/// An `f64` with `Cell`'s `get`/`set` through `&self`, stored as its bits in an
/// [`AtomicU64`] (relaxed) so that the surface stays `Sync`: a B-rep's edge arenas,
/// which hold surfaces through their curves, are shared between a B-rep and its
/// classification snapshot through an `Arc`, and the B-rep must stay `Send`. The bits
/// round-trip exactly.
#[derive(Debug)]
struct F64Cell(AtomicU64);

impl F64Cell {
    fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }

    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// The extension an [`OffsetSurface`]'s `&self` evaluators made (upstream's `const`
/// evaluators write `m_pExtendedSurface`). A `Cell`-like slot (`take`/`set` through
/// `&self`) over a [`Mutex`] so that the surface stays `Sync` (see [`F64Cell`]); no lock
/// is held while geometry runs, and a poisoned lock is recovered (its value is a whole
/// `Option` either way).
#[derive(Default)]
struct PendingExtension(Mutex<Option<Box<Surface>>>);

impl PendingExtension {
    fn take(&self) -> Option<Box<Surface>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn set(&self, e: Option<Box<Surface>>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = e;
    }

    fn get_mut(&mut self) -> &mut Option<Box<Surface>> {
        self.0.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

impl core::fmt::Debug for PendingExtension {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let e = self.take();
        let r = f.debug_tuple("PendingExtension").field(&e).finish();
        self.set(e);
        r
    }
}

/// The owner an owning offset gives its base (`m_pSurface->SetOwner(this)`): an
/// `SmOffsetSurface`. Rust objects have no address identity; every owning offset uses
/// this key (only the type and the face reached through it are read upstream).
pub const BASE_OWNER: ObjectRef = ObjectRef {
    type_code: OFFSET_SURFACE_TYPE,
    key: ObjectKey(0),
};

impl From<OffsetSurface> for Surface {
    fn from(s: OffsetSurface) -> Self {
        Self::new(SurfaceKind::OffsetSurface(s))
    }
}

/// The extended base surface during one evaluation: the stored `m_pExtendedSurface`
/// until an evaluation through `&self` made a new one (`made`, the pending extension).
struct Extension<'a> {
    stored: Option<&'a Surface>,
    made: Option<Box<Surface>>,
}

impl Extension<'_> {
    /// `m_pExtendedSurface` as the evaluation sees it now.
    fn current(&self) -> Option<&Surface> {
        self.made.as_deref().or(self.stored)
    }
}

/// `SmBoolean` from `bool`.
const fn sm_bool(b: bool) -> i32 {
    if b {
        1
    } else {
        0
    }
}

/// `D[i][j]` of an evaluation matrix.
fn at(m: &[Vec<Vec3>], i: usize, j: usize) -> Result<Vec3> {
    m.get(i)
        .and_then(|r| r.get(j))
        .copied()
        .ok_or_else(|| SolidError::invalid_input(OP_EVAL, "evaluation matrix too small"))
}

/// `aDerivatives[idx] = v`; past the end of the caller's array is undefined upstream.
fn put(out: &mut [Vec3], idx: usize, v: Vec3) -> Result<()> {
    let slot = out.get_mut(idx).ok_or_else(|| {
        SolidError::invalid_input(
            OP_EVAL,
            "upstream writes past the end of the derivative array (bOnlyUpperHalf with unequal u and v orders)",
        )
    })?;
    *slot = v;
    Ok(())
}

/// `aDerivatives[idx]`.
fn get(out: &[Vec3], idx: usize) -> Result<Vec3> {
    out.get(idx).copied().ok_or_else(|| {
        SolidError::invalid_input(
            OP_EVAL,
            "upstream reads past the end of the derivative array",
        )
    })
}

/// `lHighestOffsetDeriv`: the highest total order an evaluation asks for (`None` on
/// overflow, which upstream wraps).
fn highest_offset_deriv(hu: usize, hv: usize, only_upper_half: bool) -> Option<usize> {
    if only_upper_half {
        Some(if hu > hv { hu } else { hv })
    } else {
        hu.checked_add(hv)
    }
}

/// `SM_CAST_PTR(SmBSplineSurface, surface)`: the B-spline kinds (planes, surfaces of
/// revolution, cones, spheres and tori are `SmBSplineSurface`s).
fn is_b_spline_kind(surface: &Surface) -> bool {
    matches!(
        surface.kind(),
        SurfaceKind::BSpline(_)
            | SurfaceKind::Plane(_)
            | SurfaceKind::SurfOfRevolution(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_)
    )
}

/// `rSurface.GetSingularities()` of a base surface through `SmSurface&`: the virtual
/// with default arguments, whose only body is `SmSurface`'s (also for an offset base;
/// see the module deviations).
fn base_singularities(k: &Kernel, surface: &Surface) -> Result<u32> {
    Ok(surface.get_singularities(k, false, false)?.singular)
}

impl OffsetSurface {
    /// `GetTypeString()`.
    pub const TYPE_STRING: &'static str = "SmOffsetSurface_TYPE";
    /// `GetClassString()`.
    pub const CLASS_STRING: &'static str = "SmOffsetSurface";

    /// The offset of `surface` by `offset_distance` (`SmOffsetSurface(dOffsetDistance,
    /// rSurface, bOwnsSurface, lSingularities)`; upstream defaults `FALSE` and
    /// [`SS_UNKNOWN`]). The periodicity flags and singularities are copied from an
    /// offset base; otherwise the flags are the base's `IsPeriodic` over its natural
    /// domain and the singularities are `singularities`, or the base's
    /// `GetSingularities` for [`SS_UNKNOWN`].
    ///
    /// When constructing an offset of an offset surface, `offset_distance` is measured
    /// from the original surface (evaluation adds the two distances).
    ///
    /// # Errors
    ///
    /// `InvalidInput` above [`MAX_SURFACE_NESTING`] nested surface wrappers;
    /// those of [`Surface::is_periodic`] and, for [`SS_UNKNOWN`], of
    /// [`Surface::get_singularities`].
    pub fn new(
        k: &Kernel,
        offset_distance: f64,
        surface: Surface,
        owns_surface: bool,
        singularities: u32,
    ) -> Result<Self> {
        check_surface_nesting(&surface, 1, OP_NEW)?;
        let (is_periodic_on_u, is_periodic_on_v, singularities) =
            if let SurfaceKind::OffsetSurface(off) = surface.kind() {
                (
                    off.is_periodic_on_u,
                    off.is_periodic_on_v,
                    off.singularities,
                )
            } else {
                let domain = surface.natural_uv_domain_with_nurb(k)?;
                let u = sm_bool(surface.is_periodic(k, &domain, SurfParamType::U)?);
                let v = sm_bool(surface.is_periodic(k, &domain, SurfParamType::V)?);
                let s = if singularities == SS_UNKNOWN {
                    base_singularities(k, &surface)?
                } else {
                    singularities
                };
                (u, v, s)
            };
        let mut off = Self {
            offset_distance: F64Cell::new(offset_distance),
            allow_extension: false,
            owns_surface,
            surface: Box::new(surface),
            extended_surface: None,
            pending_extension: PendingExtension::default(),
            is_periodic_on_u,
            is_periodic_on_v,
            singularities,
        };
        // if( m_bOwnsSurface == TRUE) { m_pSurface->SetOwner(this) ; } (a new offset has
        // no owner, so its base reaches no face yet)
        if off.owns_surface {
            off.surface.set_surface_owner(BASE_OWNER, None);
        }
        Ok(off)
    }

    /// The base surface when this offset owns it (whose owner it then is).
    pub(crate) fn owned_base_mut(&mut self) -> Option<&mut Surface> {
        if self.owns_surface {
            Some(&mut self.surface)
        } else {
            None
        }
    }

    // ---- type queries -------------------------------------------------------------------

    /// `GetType()`.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        OFFSET_SURFACE_TYPE
    }

    /// `GetTypeString()`.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        Self::TYPE_STRING
    }

    /// `GetClassString()`.
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        Self::CLASS_STRING
    }

    /// `IsKindOf`: `SmOffsetSurface_TYPE` or a base type of `SmSurface`.
    #[must_use]
    pub const fn is_kind_of(t: i64) -> bool {
        t == OFFSET_SURFACE_TYPE || surface_is_kind_of(t)
    }

    // ---- members ------------------------------------------------------------------------

    /// `GetOffsetDistance`.
    #[must_use]
    pub fn offset_distance(&self) -> f64 {
        self.offset_distance.get()
    }

    /// `SetOffsetDistance`.
    pub fn set_offset_distance(&self, offset_distance: f64) {
        self.offset_distance.set(offset_distance);
    }

    /// `m_bAllowExtension`.
    #[must_use]
    pub const fn allow_extension(&self) -> bool {
        self.allow_extension
    }

    /// `SetAllowExtension`.
    pub fn set_allow_extension(&mut self, allow_extension: bool) {
        self.allow_extension = allow_extension;
    }

    /// `m_bOwnsSurface`.
    #[must_use]
    pub const fn owns_surface(&self) -> bool {
        self.owns_surface
    }

    /// `GetBaseSurface`.
    #[must_use]
    pub fn base_surface(&self) -> &Surface {
        &self.surface
    }

    /// The base surface, mutably: for a caller's temporary change of the shared base's
    /// state (`SmTemporaryChangeValue` on its `GetOutOfBoundsEnabled()`), which that
    /// caller restores.
    pub(crate) fn base_surface_mut(&mut self) -> &mut Surface {
        &mut self.surface
    }

    /// The stored `m_pExtendedSurface` only. **Not `GetExtendedBaseSurface`:** an
    /// extension a `&self` evaluator made since the last `&mut self` call (a plain
    /// B-spline surface, pending) is not included; use
    /// [`OffsetSurface::with_extended_base_surface`] for upstream's value. (The two
    /// nesting walks over it are unaffected: a pending extension is a leaf at the depth
    /// of the base surface they always visit.)
    #[must_use]
    pub fn extended_base_surface(&self) -> Option<&Surface> {
        self.extended_surface.as_deref()
    }

    /// `GetExtendedBaseSurface`: runs `f` on the surface upstream's
    /// `m_pExtendedSurface` holds now (the pending extension, else the stored one).
    /// `f` must not evaluate this offset (it would not see the pending extension).
    pub fn with_extended_base_surface<T>(&self, f: impl FnOnce(Option<&Surface>) -> T) -> T {
        self.with_current_extension(f)
    }

    /// Runs `f` on the surface upstream's `m_pExtendedSurface` holds now: the pending
    /// extension, else the stored one.
    fn with_current_extension<T>(&self, f: impl FnOnce(Option<&Surface>) -> T) -> T {
        let pending = self.pending_extension.take();
        let r = f(pending.as_deref().or(self.extended_surface.as_deref()));
        self.pending_extension.set(pending);
        r
    }

    /// Runs an evaluation body with the current extension; an extension it makes
    /// becomes the pending one (upstream stores it at once, through a `const` cast).
    ///
    /// Invariant: the pending extension is taken out of its slot while `f` runs, so
    /// `f` must not call a `&self` method of this same offset (it would see only the
    /// stored extension, and an extension it made would be overwritten when this one is
    /// put back). Today's bodies (`point_with`, `evaluate_with`, `step_inversion_with`)
    /// only evaluate the base or the extension objects.
    fn with_extension<T>(&self, f: impl FnOnce(&mut Extension<'_>) -> T) -> T {
        let mut ext = Extension {
            stored: self.extended_surface.as_deref(),
            made: self.pending_extension.take(),
        };
        let r = f(&mut ext);
        self.pending_extension.set(ext.made);
        r
    }

    /// Stores a pending extension in `m_pExtendedSurface`.
    fn fold_pending_extension(&mut self) {
        if let Some(m) = self.pending_extension.get_mut().take() {
            self.extended_surface = Some(m);
        }
    }

    /// Takes the current extension (pending, else stored) of `other`, another copy of
    /// the same upstream object, when it covers a different domain than this one's
    /// current extension (`other` having none leaves this one's): the copies stand for
    /// one `SmOffsetSurface`, whose `m_pExtendedSurface` the last extension made sets.
    /// The extension is copied into the pending slot.
    pub(crate) fn follow_extension_of(&self, other: &Self) {
        // (kernel-free; an extension is made from the offset's NURB, never lazy)
        let theirs = other.with_current_extension(|e| e.map(Surface::natural_uv_domain_if_built));
        if theirs.is_none() {
            return;
        }
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        let mine = self.with_current_extension(|e| e.map(Surface::natural_uv_domain_if_built));
        if theirs != mine {
            let copy =
                other.with_current_extension(|e| e.map(|e| Box::new(e.clone_keeping_owner())));
            self.pending_extension.set(copy);
        }
    }

    /// Takes over an extension `other` (a copy of this offset that ran an inherited
    /// `SmSurface` body) made since it was copied: upstream runs that body on this
    /// object, whose `m_pExtendedSurface` it sets.
    pub(crate) fn keep_extension_made_by(&self, other: &Self) {
        if let Some(m) = other.pending_extension.take() {
            self.pending_extension.set(Some(m));
        }
    }

    /// Takes over the current extension of `other`, a copy of this offset (a copy
    /// takes the current extension) that was evaluated in its place, pending or stored:
    /// upstream evaluates this object, whose `m_pExtendedSurface` then holds it.
    pub(crate) fn take_extension_of(&mut self, other: &Self) {
        let current =
            other.with_current_extension(|e| e.map(|e| Box::new(e.clone_keeping_owner())));
        *self.pending_extension.get_mut() = None;
        self.extended_surface = current;
    }

    /// `m_bIsPeriodicOnU` and `m_bIsPeriodicOnV` (`SmBoolean` values).
    #[must_use]
    pub const fn is_periodic(&self) -> [i32; 2] {
        [self.is_periodic_on_u, self.is_periodic_on_v]
    }

    /// `m_lSingularities` (`GetSingularities()` of the offset class).
    #[must_use]
    pub const fn singularities(&self) -> u32 {
        self.singularities
    }

    /// `GetMaxAnalyticDomain`: the base surface's.
    ///
    /// # Errors
    ///
    /// Those of the base surface's [`Surface::max_analytic_domain`].
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent2d> {
        self.surface.max_analytic_domain(k)
    }

    /// `GetDegree`: always 3.
    #[must_use]
    pub const fn degree(&self) -> usize {
        3
    }

    /// `SetContext`: nothing to set (the kernel is passed to the calls that need it).
    pub const fn set_context(&mut self) {}

    /// `GetNaturalUVDomain`: the extended surface's domain when there is one, else the
    /// base surface's.
    #[must_use]
    pub fn natural_uv_domain(&self) -> Extent2d {
        self.with_current_extension(|e| domain_with(&self.surface, e))
    }

    /// [`OffsetSurface::natural_uv_domain`] as upstream's `GetNaturalUVDomain`, whose
    /// base and extended surfaces make a missing NURB (`GetOrCreateGwNurbPointer`): the
    /// domains are read with [`Surface::natural_uv_domain_with_nurb`], so a lazy
    /// analytic base answers the domain of the NURB it makes, not the NURB-less base's
    /// empty one.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::natural_uv_domain_with_nurb`].
    pub fn natural_uv_domain_with_nurb(&self, k: &Kernel) -> Result<Extent2d> {
        self.with_current_extension(|e| match e {
            Some(e) => e.natural_uv_domain_with_nurb(k),
            None => self.surface.natural_uv_domain_with_nurb(k),
        })
    }

    /// `GetKnots`: those of the extended surface when there is one, else those of the
    /// base surface.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::knots`].
    pub fn knots(
        &self,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.with_current_extension(|e| match e {
            Some(e) => e.knots(param, ivl),
            None => self.surface.knots(param, ivl),
        })
    }

    // ---- copies and comparison ------------------------------------------------------------

    /// A deep copy (`Copy`): an owned base is copied (`Copy`) and owned by the new
    /// offset, a shared base stays shared; the new offset is made by the constructor
    /// (default singularities), then takes the extension flag and a copy of the
    /// extended surface.
    ///
    /// # Errors
    ///
    /// Those of the base surface's [`Surface::copy`] and of [`OffsetSurface::new`].
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        let mut sur = if self.owns_surface {
            // when BaseSurface is owned
            let new_sur = self.surface.copy(k)?;
            let mut s = Self::new(k, self.offset_distance.get(), new_sur, false, SS_UNKNOWN)?;
            s.owns_surface = true;
            // pNewSur->SetOwner(pSur)
            s.surface.set_surface_owner(BASE_OWNER, None);
            s
        } else {
            // when BaseSurface is not owned (shared upstream: the same base surface)
            // (the same object upstream: its owner is unchanged)
            let shared = self.surface.clone_for_evaluation_depth(1)?;
            let mut s = Self::new(k, self.offset_distance.get(), shared, false, SS_UNKNOWN)?;
            s.owns_surface = false;
            s
        };

        sur.allow_extension = self.allow_extension;

        // Copy ExtendedSurface
        if let Some(e) = self.with_current_extension(|e| e.map(|e| e.copy(k)).transpose())? {
            sur.extended_surface = Some(Box::new(e));
        }
        Ok(sur)
    }

    /// `CreateMirrorSurface`: an offset of the mirrored base (owned by the new offset)
    /// and of the mirrored extension, if any. The distance changes sign, as the mirror
    /// reverses the base's orientation.
    ///
    /// # Errors
    ///
    /// Those of the base's and the extension's [`Surface::create_mirror_surface`] and of
    /// [`OffsetSurface::new`].
    pub fn create_mirror_surface(&self, k: &Kernel, mirror_plane: &Axis2Placement) -> Result<Self> {
        let mirror = self.surface.create_mirror_surface(k, mirror_plane)?;
        let mut off = Self::new(k, self.offset_distance.get(), mirror, true, SS_UNKNOWN)?;
        off.allow_extension = self.allow_extension;
        if let Some(e) = self.with_current_extension(|e| {
            e.map(|e| e.create_mirror_surface(k, mirror_plane))
                .transpose()
        })? {
            off.extended_surface = Some(Box::new(e));
        }
        // This needs to be done because mirror reverses orientation of the surface.
        off.offset_distance.set(-self.offset_distance.get());
        Ok(off)
    }

    /// `operator==`: an offset surface (`SmSurface::operator==`) with the same distance
    /// (to the scaled zero), equal base surfaces, and the same periodicity flags and
    /// singularities.
    ///
    /// # Errors
    ///
    /// Those of the base surfaces' [`Surface::approx_eq`].
    pub fn approx_eq(&self, k: &Kernel, other: &Surface) -> Result<bool> {
        // first check the base
        let SurfaceKind::OffsetSurface(o) = other.kind() else {
            return Ok(false);
        };
        // low work
        if ptr::eq(self, o) {
            return Ok(true);
        }

        // check equivalence of these objects
        if !is_zero(self.offset_distance.get() - o.offset_distance.get()) {
            return Ok(false);
        }
        let (a, b): (&Surface, &Surface) = (&self.surface, &o.surface);
        if !(ptr::eq(a, b) || a.approx_eq(k, b)?) {
            return Ok(false);
        }
        Ok(self.is_periodic_on_u == o.is_periodic_on_u
            && self.is_periodic_on_v == o.is_periodic_on_v
            && self.singularities == o.singularities)
    }

    // ---- new surfaces ---------------------------------------------------------------------

    /// An offset of the base surface extended by `dist` on every side
    /// (`CreateExtendedSurface(ctx, dist, continuity, ..)`: the directional overload
    /// with `SM_SP_BOTH`).
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::create_extended_surface_dir`].
    pub fn create_extended_surface(
        &self,
        k: &Kernel,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<Surface> {
        // pass the call along
        self.create_extended_surface_dir(k, SurfParamType::Both, dist, extension_continuity)
    }

    /// An offset (same distance, owning, default singularities) of the base surface
    /// extended by `dist` in `ext_direction` (`CreateExtendedSurface` with a direction).
    ///
    /// # Errors
    ///
    /// Those of the base surface's [`Surface::create_extended_surface_dir`] and of
    /// [`OffsetSurface::new`].
    pub fn create_extended_surface_dir(
        &self,
        k: &Kernel,
        ext_direction: SurfParamType,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<Surface> {
        let surface = self.surface.create_extended_surface_dir(
            k,
            ext_direction,
            dist,
            extension_continuity,
        )?;
        Ok(Surface::from(Self::new(
            k,
            self.offset_distance.get(),
            surface,
            true,
            SS_UNKNOWN,
        )?))
    }

    /// Exact evaluation copy (no knot fix-up) that takes the current extension, pending
    /// or stored, as main's copy does; the caller validates the complete geometry graph
    /// first.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        let surface = Box::new(self.surface.clone_exact_bounded());
        let extended_surface =
            self.with_current_extension(|e| e.map(|e| Box::new(e.clone_exact_bounded())));
        Self {
            offset_distance: F64Cell::new(self.offset_distance.get()),
            allow_extension: self.allow_extension,
            owns_surface: self.owns_surface,
            surface,
            extended_surface,
            pending_extension: PendingExtension::default(),
            is_periodic_on_u: self.is_periodic_on_u,
            is_periodic_on_v: self.is_periodic_on_v,
            singularities: self.singularities,
        }
    }

    /// The base surface and the stored extension, mutable, for
    /// [`Surface::detach_nurb_cells`] (a pending extension is not reached: an exact
    /// copy has folded it into the stored one).
    pub(crate) fn nested_surfaces_mut(&mut self) -> Vec<&mut Surface> {
        let mut v = vec![self.surface.as_mut()];
        if let Some(e) = self.extended_surface.as_deref_mut() {
            v.push(e);
        }
        v
    }

    /// A copy for evaluation: the members as they are, the base and extended
    /// surfaces copied exactly (no knot fix-up), `depth` the wrapper nesting
    /// already walked.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        let surface = Box::new(self.surface.clone_for_evaluation_depth(depth)?);
        let extended_surface = self.with_current_extension(|e| {
            e.map(|e| e.clone_for_evaluation_depth(depth).map(Box::new))
                .transpose()
        })?;
        Ok(Self {
            offset_distance: F64Cell::new(self.offset_distance.get()),
            allow_extension: self.allow_extension,
            owns_surface: self.owns_surface,
            surface,
            extended_surface,
            pending_extension: PendingExtension::default(),
            is_periodic_on_u: self.is_periodic_on_u,
            is_periodic_on_v: self.is_periodic_on_v,
            singularities: self.singularities,
        })
    }

    /// The offset of this surface by `signed_offset_distance` more
    /// (`CreateOffsetSurface`; the approximation tolerance is not used): a copy of the
    /// base surface when the compound distance is below `SM_EFF_ZERO`, else an offset
    /// of a copy of the base by the compound distance.
    ///
    /// # Errors
    ///
    /// Those of the base surface's [`Surface::copy`] and of [`OffsetSurface::new`].
    pub fn create_offset_surface(
        &self,
        k: &Kernel,
        signed_offset_distance: f64,
    ) -> Result<Surface> {
        let surf_copy = self.surface.copy(k)?;
        let compound_dist = signed_offset_distance + self.offset_distance.get();
        if compound_dist.abs() < EFF_ZERO {
            return Ok(surf_copy);
        }
        Ok(Surface::from(Self::new(
            k,
            compound_dist,
            surf_copy,
            true,
            SS_UNKNOWN,
        )?))
    }

    /// The two halves of the offset split at `param` in `surf_param`'s direction
    /// (`SplitAt`): offsets (owning, default singularities, the extension flag copied)
    /// of the base surface's halves. With an extended surface the halves get the base
    /// surface's halves again as their extended surfaces (as upstream).
    ///
    /// # Errors
    ///
    /// Those of the base surface's [`Surface::split_at`] and of [`OffsetSurface::new`].
    pub fn split_at(
        &self,
        k: &Kernel,
        param: f64,
        surf_param: SurfParamType,
    ) -> Result<(Surface, Surface)> {
        let (sur_left, sur_right) = self.surface.split_at(k, param, surf_param)?;

        let mut left_off = Self::new(k, self.offset_distance.get(), sur_left, true, SS_UNKNOWN)?;
        left_off.allow_extension = self.allow_extension;

        let mut right_off = Self::new(k, self.offset_distance.get(), sur_right, true, SS_UNKNOWN)?;
        right_off.allow_extension = self.allow_extension;

        if self.with_current_extension(|e| e.is_some()) {
            // (upstream splits m_pSurface, not m_pExtendedSurface)
            let (ext_left, ext_right) = self.surface.split_at(k, param, surf_param)?;
            left_off.extended_surface = Some(Box::new(ext_left));
            right_off.extended_surface = Some(Box::new(ext_right));
        }

        Ok((Surface::from(left_off), Surface::from(right_off)))
    }

    // ---- bounding boxes ---------------------------------------------------------------------

    /// The boxes of the offset over `uv_domain` (`CalculateBoundingBox`; `want` selects
    /// the normal, pseudo and polar boxes). A domain not contained in the base domain
    /// (to `SM_EFF_ZERO`) runs the base-class body on `this` (the surface holding this
    /// offset). Otherwise the base surface's three boxes are computed and, for a bounded
    /// polar box, every corner of the base's normal and pseudo boxes is moved by the
    /// offset distance along each boundary vector of its polar box; an unbounded polar
    /// box expands the base boxes by the distance. The polar box is the base's.
    ///
    /// # Errors
    ///
    /// Those of the base surface's [`Surface::calculate_bounding_box`] and of
    /// [`Surface::base_calculate_bounding_box`] (an owned base reaches this offset's face
    /// through its owner, so a face-owned B-spline-kind base's box is expanded by the
    /// face tolerance passed down to it, [`Surface::owner_zone_tol_3d`], as upstream asks
    /// that face; without it, `SmFace::GetTolerance`'s [`SolidError::Unported`]).
    pub fn calculate_bounding_box(
        &self,
        k: &Kernel,
        this: &Surface,
        uv_domain: &Extent2d,
        want: [bool; 3],
        expand_pos_boxes_by_zone_tol_3d: bool,
    ) -> Result<SurfaceBoxes> {
        self.calculate_bounding_box_with(
            k,
            this,
            uv_domain,
            want,
            expand_pos_boxes_by_zone_tol_3d,
            |base| {
                base.calculate_bounding_box(
                    k,
                    uv_domain,
                    [true, true, true],
                    expand_pos_boxes_by_zone_tol_3d,
                )
            },
        )
    }

    /// [`OffsetSurface::calculate_bounding_box`] with the base surface's three boxes over
    /// `uv_domain` from `base_boxes` (upstream's `m_pSurface->CalculateBoundingBox(..,
    /// pOptPseudoBasisGuess, pOptPolarBasisGuess, ..)`: a caller holding basis guesses
    /// forwards them there). Outside the base's natural domain the `SmSurface` body runs,
    /// which does not use the guesses.
    pub(crate) fn calculate_bounding_box_with(
        &self,
        k: &Kernel,
        this: &Surface,
        uv_domain: &Extent2d,
        want: [bool; 3],
        expand_pos_boxes_by_zone_tol_3d: bool,
        base_boxes: impl FnOnce(&Surface) -> Result<SurfaceBoxes>,
    ) -> Result<SurfaceBoxes> {
        if !uv_domain.is_contained_by(&self.surface.natural_uv_domain_with_nurb(k)?, EFF_ZERO) {
            return this.base_calculate_bounding_box(k, want, expand_pos_boxes_by_zone_tol_3d);
        }

        // init outputs
        let mut normal_box = want[0].then(|| {
            let mut b = Extent3d::new();
            b.init();
            b
        });
        let mut pseudo_box = want[1].then(|| {
            let mut b = PseudoBox::new();
            b.init(); // init intervals - leave basis vectors alone
            b
        });
        let mut polar_box = want[2].then(|| {
            let mut b = PolarBox::new();
            b.reset();
            b
        });

        // bound the base surface (pass the call along to the base surface)
        let base = base_boxes(&self.surface)?;
        let missing = || SolidError::invalid_input(OP, "base surface box not computed");
        let base_normal = base.normal_box.ok_or_else(missing)?;
        let base_pseudo = base.pseudo_box.ok_or_else(missing)?;
        let base_polar = base.polar_box.ok_or_else(missing)?;

        // set the polar box
        if let Some(p) = polar_box.as_mut() {
            p.assign(&base_polar);
        }

        // set the pseudo box basis (status ignored upstream)
        if let Some(p) = pseudo_box.as_mut() {
            let [b0, b1, b2] = *base_pseudo.basis();
            let _ = p.set_basis(b0, b1, b2);
        }

        match base_polar.state() {
            PolarBoxState::PointWithBasis
            | PolarBoxState::SinglePoint
            | PolarBoxState::Arc
            | PolarBoxState::Region => {
                // offset the bounding boxes
                let vectors = base_polar.boundary_vectors()?;
                let pseudo_corners = base_pseudo.corners();
                for (crn, corner) in (0u32..8).zip(pseudo_corners) {
                    if let Some(nb) = normal_box.as_mut() {
                        let point_base = base_normal.evaluate(
                            f64::from((crn / 4) % 2),
                            f64::from((crn / 2) % 2),
                            f64::from(crn % 2),
                        );
                        for v in &vectors {
                            nb.add_point(point_base + self.offset_distance.get() * *v);
                        }
                    }
                    if let Some(pb) = pseudo_box.as_mut() {
                        for v in &vectors {
                            pb.add_point(corner + self.offset_distance.get() * *v);
                        }
                    }
                }
            }
            PolarBoxState::Unbounded | PolarBoxState::Uninitialized => {
                // offset the bounding boxes in all directions
                if let Some(nb) = normal_box.as_mut() {
                    *nb = base_normal;
                    nb.expand_absolute(self.offset_distance.get().abs());
                }
                if let Some(pb) = pseudo_box.as_mut() {
                    *pb = base_pseudo;
                    pb.expand_absolute(self.offset_distance.get().abs());
                }
            }
        }

        Ok(SurfaceBoxes {
            normal_box,
            pseudo_box,
            polar_box,
        })
    }

    // ---- evaluation -------------------------------------------------------------------------

    /// The point at `uv` (`EvaluatePoint`): the base point moved along the unit normal
    /// (the cross product of the first derivatives, or the base's `EvaluateNormal` when
    /// that is below `SM_EFF_ZERO`). A nested offset base adds its distance and
    /// evaluates its own base. Outside the base domain the extended surface is used;
    /// still outside, the point is clamped, or the base is extended when extension is
    /// allowed (kept for this call only, see [`OffsetSurface::evaluate_point_extending`]).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an offset nested twice; `SM_ERR_NULL_POINTER` when a non-B-spline
    /// base must be extended; those of the extension and the base evaluations; the
    /// zero-vector error of `Unitize`.
    pub fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        self.with_extension(|ext| self.point_with(k, ext, uv))
    }

    /// [`OffsetSurface::evaluate_point`], storing an extension it makes in
    /// `m_pExtendedSurface` (upstream's `const` cast).
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::evaluate_point`].
    pub fn evaluate_point_extending(&mut self, k: &Kernel, uv: Point2) -> Result<Point3> {
        let r = self.evaluate_point(k, uv);
        self.fold_pending_extension();
        r
    }

    /// The point and partial derivatives at `uv` (`Evaluate`): `D[i][j]` is the
    /// derivative `i` times in `u` and `j` times in `v`, at most [`MAX_DERIVS`] in total
    /// (in the larger of the two directions with `flags.only_upper_half`). Exact
    /// tangents (`flags.non_zero_tangents == false`) evaluate as
    /// [`OffsetSurface::evaluate_simple`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` for more than [`MAX_DERIVS`] derivatives, an offset nested twice, or a
    /// vanishing normal; [`SolidError::InvalidInput`] where upstream writes past the
    /// derivative array or the pole step-off recursion exceeds [`MAX_STEPOFF_DEPTH`];
    /// those of the base evaluations and of the extension.
    pub fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.with_extension(|ext| {
            self.evaluate_with(
                k,
                ext,
                !flags.non_zero_tangents,
                uv,
                highest_u_deriv,
                highest_v_deriv,
                flags,
            )
        })
    }

    /// [`OffsetSurface::evaluate`], storing an extension it makes.
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::evaluate`].
    pub fn evaluate_extending(
        &mut self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        let r = self.evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags);
        self.fold_pending_extension();
        r
    }

    /// The point and partial derivatives at `uv` without zero-tangent corrections
    /// (`EvaluateSimple`; `flags.non_zero_tangents` is not used).
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::evaluate`].
    pub fn evaluate_simple(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.with_extension(|ext| {
            self.evaluate_with(k, ext, true, uv, highest_u_deriv, highest_v_deriv, flags)
        })
    }

    /// [`OffsetSurface::evaluate_simple`], storing an extension it makes.
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::evaluate`].
    pub fn evaluate_simple_extending(
        &mut self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        let r = self.evaluate_simple(k, uv, highest_u_deriv, highest_v_deriv, flags);
        self.fold_pending_extension();
        r
    }

    /// The unit normal at `uv` (`EvaluateNormal`): the base surface's inside its
    /// domain, else the `SmSurface::EvaluateNormal` body on the offset (whose
    /// evaluations may extend the base).
    ///
    /// # Errors
    ///
    /// Those of the base surface's [`Surface::evaluate_normal`] and of
    /// [`base_evaluate_normal`].
    pub fn evaluate_normal(
        &self,
        k: &Kernel,
        uv: Point2,
        u_side: Side,
        v_side: Side,
    ) -> Result<Vec3> {
        // see if domainPoint is within BaseSurface domain
        let domain = self.surface.natural_uv_domain_with_nurb(k)?;
        if domain.contains_point(uv, 0.0) {
            self.surface.evaluate_normal(k, uv, u_side, v_side)
        } else {
            // pass the call along to base virtual function which calls Evaluate
            let eval = OffsetEval::new(self);
            base_evaluate_normal(k, &eval, uv, u_side, v_side)
        }
    }

    /// [`OffsetSurface::evaluate_normal`], storing an extension its evaluations make.
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::evaluate_normal`].
    pub fn evaluate_normal_extending(
        &mut self,
        k: &Kernel,
        uv: Point2,
        u_side: Side,
        v_side: Side,
    ) -> Result<Vec3> {
        let domain = self.surface.natural_uv_domain_with_nurb(k)?;
        if domain.contains_point(uv, 0.0) {
            return self.surface.evaluate_normal(k, uv, u_side, v_side);
        }
        self.with_evaluator(|eval| base_evaluate_normal(k, eval, uv, u_side, v_side))
    }

    /// Runs `f` on the offset seen as a generic surface ([`OffsetEval`]), as upstream
    /// runs an inherited `SmSurface` body (`Evaluate1stDerivatives`,
    /// `ComputeInwardVector`, ...) whose virtual calls reach the offset's evaluators.
    /// An extension those evaluations make is stored in `m_pExtendedSurface`, as by
    /// the `*_extending` evaluators.
    pub fn with_evaluator<T>(&mut self, f: impl FnOnce(&OffsetEval<'_>) -> T) -> T {
        let r = f(&OffsetEval::new(self));
        self.fold_pending_extension();
        r
    }

    /// The base surface (or a nested offset's base) and the total distance
    /// (upstream's nested-offset check of `EvaluatePoint` / `EvaluatePlainOrSimple`).
    fn nested_base(&self, operation: &'static str) -> Result<(&Surface, f64)> {
        let mut surface: &Surface = &self.surface;
        let mut offset_distance = self.offset_distance.get();

        // when base surface is also an offset surface
        if let SurfaceKind::OffsetSurface(off) = surface.kind() {
            // use the nested surface and combine offsets
            surface = &off.surface;
            if matches!(surface.kind(), SurfaceKind::OffsetSurface(_)) {
                // Should not be nesting offset surfaces
                return Err(SolidError::sm_err(operation));
            }
            offset_distance += off.offset_distance.get();
        }
        Ok((surface, offset_distance))
    }

    /// Upstream's choice of the surface to evaluate at `uv` (shared by `EvaluatePoint`
    /// and `EvaluatePlainOrSimple`): the base; outside its domain the extended surface;
    /// still outside, the point clamped to that domain, or the surface extended when
    /// extension is allowed. Returns whether the extended surface is used, the point
    /// to evaluate and the domain upstream holds in `sDomain` (that of the surface
    /// before any new extension).
    fn select_surface(
        &self,
        k: &Kernel,
        ext: &mut Extension<'_>,
        base: &Surface,
        uv: Point2,
    ) -> Result<(bool, Point2, Extent2d)> {
        let mut uv = uv;
        let mut use_ext = false;

        // when evalPoint is outside of BaseSurface domain - use ExtendedSurface if possible
        let mut domain = base.natural_uv_domain_with_nurb(k)?;
        if !domain.contains_point(uv, 0.0) {
            if let Some(e) = ext.current() {
                // replace base surface with extended surface
                use_ext = true;
                domain = e.natural_uv_domain_with_nurb(k)?;
            }
        }

        // when evalPoint is still outside of BaseSurface domain - extend or clamp
        if !domain.contains_point(uv, 0.0) {
            if self.allow_extension {
                // extend original base surface so that it contains given sUV
                // (`fixes.offset_extension_from_base`: always the base, however the
                // extension was made before; the extension only sets a lower bound.)
                let from_base = k.fixes.offset_extension_from_base;
                let original = if use_ext && !from_base {
                    ext.current()
                } else {
                    Some(base)
                };
                let original = original.ok_or_else(|| SolidError::null_pointer(OP_EXTEND))?;
                // (kernel-free: an extension is a B-spline made from its NURB, never lazy)
                let current = ext.current().map(Surface::natural_uv_domain_if_built);
                let extended = extend_surface(
                    k,
                    original,
                    &mut uv,
                    self.is_periodic_on_u != 0,
                    self.is_periodic_on_v != 0,
                    self.singularities,
                    from_base,
                    current,
                )?;

                // when BaseSurface was extended - store and use the extension
                if let Some(e) = extended {
                    ext.made = Some(Box::new(e));
                    use_ext = true;
                }
            } else {
                // clamp UVPoint to domain
                uv = domain.clamp_point(uv);
            }
        }
        Ok((use_ext, uv, domain))
    }

    /// The body of [`OffsetSurface::evaluate_point`] with the extension of the call.
    fn point_with(&self, k: &Kernel, ext: &mut Extension<'_>, uv: Point2) -> Result<Point3> {
        let (base, offset_distance) = self.nested_base(OP_POINT)?;
        let (use_ext, uv, _domain) = self.select_surface(k, ext, base, uv)?;
        let surface = if use_ext {
            ext.current()
                .ok_or_else(|| SolidError::null_pointer(OP_POINT))?
        } else {
            base
        };

        // Optimization for zero offset distance
        if offset_distance == 0.0 {
            return surface.evaluate_point(k, uv);
        }

        // Evaluate the BaseSurface (either m_pBaseSurface or m_pExtendedSurface as needed)
        let (pnt, du, dv) = surface.evaluate_1st_derivatives(k, uv, true, true)?;
        let mut norm = du.cross(dv);
        if norm.length_squared() < EFF_ZERO_SQ {
            norm = surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;
        }
        let norm = norm.unitize()?;

        // compute and return the point
        Ok(pnt + offset_distance * norm)
    }

    /// The derivative matrix `D[i][j]` of `EvaluatePlainOrSimple` with the extension of
    /// the call.
    fn evaluate_with(
        &self,
        k: &Kernel,
        ext: &mut Extension<'_>,
        do_eval_simple: bool,
        uv: Point2,
        hu: usize,
        hv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        // (checked before the array is sized)
        match highest_offset_deriv(hu, hv, flags.only_upper_half) {
            Some(d) if d <= MAX_DERIVS => {}
            _ => return Err(SolidError::sm_err(OP_EVAL)),
        }
        let mut out = vec![Vec3::UNDEF; (hu + 1) * (hv + 1)];
        self.plain_or_simple(
            k,
            ext,
            0,
            do_eval_simple,
            uv,
            [hu, hv],
            [flags.u_side, flags.v_side],
            flags.only_upper_half,
            &mut out,
        )?;
        Ok(out.chunks(hv + 1).map(<[Vec3]>::to_vec).collect())
    }

    /// `EvaluatePlainOrSimple`: writes upstream's flat derivative array
    /// (`out[i * (hv + 1) + j]`). `depth` counts the pole step-off recursion.
    fn plain_or_simple(
        &self,
        k: &Kernel,
        ext: &mut Extension<'_>,
        depth: usize,
        do_eval_simple: bool,
        cr_uv: Point2,
        [hu, hv]: [usize; 2],
        [u_side, v_side]: [Side; 2],
        only_upper_half: bool,
        out: &mut [Vec3],
    ) -> Result<()> {
        // locals
        let hod = match highest_offset_deriv(hu, hv, only_upper_half) {
            Some(d) if d <= MAX_DERIVS => d,
            _ => return Err(SolidError::sm_err(OP_EVAL)),
        };
        let hb = hod + 1;

        // when base surface is also an offset surface
        let (base, offset_distance) = self.nested_base(OP_EVAL)?;
        let (use_ext, uv, domain) = self.select_surface(k, ext, base, cr_uv)?;
        let surface = if use_ext {
            ext.current()
                .ok_or_else(|| SolidError::null_pointer(OP_EVAL))?
        } else {
            base
        };

        // Optimization for zero offset distance
        if offset_distance == 0.0 {
            let flags = EvaluateFlags {
                u_side,
                v_side,
                only_upper_half,
                non_zero_tangents: true,
            };
            let m = if do_eval_simple {
                surface.evaluate_simple(k, uv, hu, hv, flags)?
            } else {
                // True: adjust to non-zero first derivatives
                surface.evaluate(k, uv, hu, hv, flags)?
            };
            for (i, row) in m.iter().enumerate().take(hu + 1) {
                for (j, v) in row.iter().enumerate().take(hv + 1) {
                    put(out, i * (hv + 1) + j, *v)?;
                }
            }
            return Ok(());
        }

        // evaluate the base surface one derivative higher than the offset
        let m = surface.evaluate_simple(
            k,
            uv,
            hb,
            hb,
            EvaluateFlags {
                u_side: Side::Left,
                v_side: Side::Left,
                only_upper_half: true,
                non_zero_tangents: false,
            },
        )?;
        // the base derivative `i` times in u and `j` times in v (NULL upstream above hb)
        let d = |i: usize, j: usize| -> Result<Vec3> {
            if i + j > hb {
                return Err(SolidError::null_pointer(OP_EVAL));
            }
            at(&m, i, j)
        };
        let b = d(0, 0)?;
        let mut bv = d(0, 1)?;
        let mut bu = d(1, 0)?;

        // Tolerance on zero derivatives
        let tol = 10.0 * EFF_ZERO * (1.0 + b.max_dimension());
        let bu_len = bu.length();
        let bv_len = bv.length();

        // the conditions of the mixed derivative outputs
        let cond_uv = (only_upper_half && hod >= 2) || (!only_upper_half && hu >= 1 && hv >= 1);
        let cond_uuv = (only_upper_half && hod >= 3) || (!only_upper_half && hu >= 2 && hv >= 1);
        let cond_uvv = (only_upper_half && hod >= 3) || (!only_upper_half && hu >= 1 && hv >= 2);

        // only use approx values at poles when not doing a Simple evaluation
        if !do_eval_simple && (bu_len < tol || bv_len < tol) {
            if depth >= MAX_STEPOFF_DEPTH {
                return Err(SolidError::invalid_input(
                    OP_EVAL,
                    "pole step-off recursion deeper than MAX_STEPOFF_DEPTH",
                ));
            }
            let param_step =
                find_nonsingular_stepoff(k, surface, cr_uv, &domain, bu_len, bv_len, tol)?;

            // evaluate at the two step-off points
            let mut mat1 = [Vec3::UNDEF; MAT_LEN];
            let mut mat2 = [Vec3::UNDEF; MAT_LEN];
            let step_uv = cr_uv + param_step;
            self.plain_or_simple(
                k,
                ext,
                depth + 1,
                do_eval_simple,
                step_uv,
                [hu, hv],
                [u_side, v_side],
                only_upper_half,
                &mut mat1,
            )?;
            let step_uv = cr_uv + 2.0 * param_step;
            self.plain_or_simple(
                k,
                ext,
                depth + 1,
                do_eval_simple,
                step_uv,
                [hu, hv],
                [u_side, v_side],
                only_upper_half,
                &mut mat2,
            )?;

            // linear extrapolation back to the pole
            let extrapolate = |idx: usize| -> Result<Vec3> {
                let v1 = get(&mat1, idx)?;
                let v2 = get(&mat2, idx)?;
                Ok(2.0 * v1 - v2)
            };
            // a first derivative: never return a zero one when not doing a Simple evaluation
            let extrapolate_tangent = |idx: usize| -> Result<Vec3> {
                let mut v1 = get(&mat1, idx)?;
                let mut v2 = get(&mat2, idx)?;
                let mut v0 = 2.0 * v1 - v2;
                if !do_eval_simple && v0.length() < tol {
                    v1 = v1.unitize()?;
                    v2 = v2.unitize()?;
                    v0 = 2.0 * v1 - v2;
                    v0 = v0.unitize_or_z();
                    v0 *= tol * 1.1;
                }
                Ok(v0)
            };

            put(out, 0, extrapolate(0)?)?;
            if hu >= 1 {
                let idx = hv + 1;
                put(out, idx, extrapolate_tangent(idx)?)?;
            }
            if hv >= 1 {
                put(out, 1, extrapolate_tangent(1)?)?;
            }
            if hu >= 2 {
                let idx = 2 * (hv + 1);
                put(out, idx, extrapolate(idx)?)?;
            }
            if cond_uv {
                let idx = hv + 2;
                put(out, idx, extrapolate(idx)?)?;
            }
            if hv >= 2 {
                put(out, 2, extrapolate(2)?)?;
            }
            if hu >= 3 {
                let idx = 3 * (hv + 1);
                put(out, idx, extrapolate(idx)?)?;
            }
            if cond_uuv {
                let idx = 2 * (hv + 1) + 1;
                put(out, idx, extrapolate(idx)?)?;
            }
            if cond_uvv {
                let idx = (hv + 1) + 2;
                put(out, idx, extrapolate(idx)?)?;
            }
            if hv >= 3 {
                put(out, 3, extrapolate(3)?)?;
            }
            return Ok(());
        }

        // the base normal's numerator N = Bu x Bv
        let mut n = bu.cross(bv);
        let mut l = n.length();
        if l < tol {
            if let Some((_pnt, du, dv)) =
                ignored(surface.evaluate_1st_derivatives(k, uv, true, true))?
            {
                n = du.cross(dv);
                l = n.length();
                if !do_eval_simple {
                    bu = du;
                    bv = dv;
                }
            }
        }

        // any Normal or a 1st deriv vec is zero: replace the zero one
        if !do_eval_simple {
            let bu_length = bu.length();
            let bv_length = bv.length();
            let lt = min3(l, bu_length, bv_length);
            if lt < tol {
                let norm = surface.evaluate_normal(k, uv, u_side, v_side)?;
                n = norm;
                l = n.length(); // L will be 1.0, EvaluateNormal returns a normalized vector
                if bu_length < bv_length {
                    let mut du = bv.cross(n).unitize_or_z();
                    du *= tol * 1.1; // Make it big enough: no other info on magnitude.
                    bu = du;
                } else {
                    let mut dv = n.cross(bu).unitize_or_z();
                    dv *= tol * 1.1;
                    bv = dv;
                }
            }
        }

        // Leave this tol at SM_EFF_ZERO: that's where output becomes invalid.
        if l < EFF_ZERO {
            return Err(SolidError::sm_err(OP_EVAL));
        }

        // never-assigned upstream locals are default SmVector3d values (UNDEF)
        let undef = Vec3::UNDEF;
        let (mut cu, mut cv, mut cuu, mut cuv, mut cvv) = (undef, undef, undef, undef, undef);
        let (mut mu, mut mv, mut muu, mut muv, mut mvv) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let (mut nu, mut nv, mut nuu, mut nuv, mut nvv) = (undef, undef, undef, undef, undef);

        let c = n;
        let m_len = c.length();
        let n = c / m_len;
        put(out, 0, b + offset_distance * n)?;

        if hu >= 1 {
            cu = d(2, 0)?.cross(bv) + bu.cross(d(1, 1)?);
            mu = n.dot(cu);
            nu = (cu - mu * n) / m_len;
            put(out, hv + 1, bu + offset_distance * nu)?;
        }
        if hv >= 1 {
            cv = d(1, 1)?.cross(bv) + bu.cross(d(0, 2)?);
            mv = n.dot(cv);
            nv = (cv - mv * n) / m_len;
            put(out, 1, bv + offset_distance * nv)?;
        }
        if hu >= 2 {
            cuu = d(3, 0)?.cross(bv) + (2.0 * d(2, 0)?).cross(d(1, 1)?) + bu.cross(d(2, 1)?);
            muu = (cu.dot(cu) + c.dot(cuu) - mu * mu) / m_len;
            nuu = (cuu - 2.0 * mu * nu - muu * n) / m_len;
            put(out, 2 * (hv + 1), d(2, 0)? + offset_distance * nuu)?;
        }
        if cond_uv {
            // (Buv x Buv vanishes)
            cuv = d(2, 1)?.cross(bv) + d(2, 0)?.cross(d(0, 2)?) + bu.cross(d(1, 2)?);
            muv = (cu.dot(cv) + c.dot(cuv) - mu * mv) / m_len;
            nuv = (cuv - mu * nv - mv * nu - muv * n) / m_len;
            put(out, hv + 2, d(1, 1)? + offset_distance * nuv)?;
        }
        if hv >= 2 {
            cvv = d(1, 2)?.cross(bv) + (2.0 * d(1, 1)?).cross(d(0, 2)?) + bu.cross(d(0, 3)?);
            mvv = (cv.dot(cv) + c.dot(cvv) - mv * mv) / m_len;
            nvv = (cvv - 2.0 * mv * nv - mvv * n) / m_len;
            put(out, 2, d(0, 2)? + offset_distance * nvv)?;
        }
        if hu >= 3 {
            let cuuu = d(4, 0)?.cross(bv)
                + (3.0 * d(3, 0)?).cross(d(1, 1)?)
                + (3.0 * d(2, 0)?).cross(d(2, 1)?)
                + bu.cross(d(3, 1)?);
            let muuu = (3.0 * cu.dot(cuu) + c.dot(cuuu) - 3.0 * mu * muu) / m_len;
            let nuuu = (cuuu - 3.0 * mu * nuu - 3.0 * muu * nu - muuu * n) / m_len;
            put(out, 3 * (hv + 1), d(3, 0)? + offset_distance * nuuu)?;
        }
        if cond_uuv {
            // (upstream's 3.0 for Buuv x Buv; the derivative of Cuu gives 1)
            let cuuv = d(3, 1)?.cross(bv)
                + d(3, 0)?.cross(d(0, 2)?)
                + (3.0 * d(2, 1)?).cross(d(1, 1)?)
                + (2.0 * d(2, 0)?).cross(d(1, 2)?)
                + bu.cross(d(2, 2)?);
            let muuv =
                (cuu.dot(cv) + 2.0 * cu.dot(cuv) + c.dot(cuuv) - muu * mv - 2.0 * mu * muv) / m_len;
            let nuuv =
                (cuuv - mv * nuu - 2.0 * muv * nu - 2.0 * mu * nuv - muu * nv - muuv * n) / m_len;
            put(out, 2 * (hv + 1) + 1, d(2, 1)? + offset_distance * nuuv)?;
        }
        if cond_uvv {
            // (upstream's 3.0 for Buvv x Buv, -1 by the derivative of Cvv; and Buuv in
            // the output where Buvv belongs)
            let cuvv = d(2, 2)?.cross(bv)
                + (2.0 * d(2, 1)?).cross(d(0, 2)?)
                + (3.0 * d(1, 2)?).cross(d(1, 1)?)
                + d(2, 0)?.cross(d(0, 3)?)
                + bu.cross(d(1, 3)?);
            let muvv =
                (cvv.dot(cu) + 2.0 * cv.dot(cuv) + c.dot(cuvv) - mvv * mu - 2.0 * mv * muv) / m_len;
            let nuvv =
                (cuvv - mu * nvv - 2.0 * muv * nv - 2.0 * mv * nuv - mvv * nu - muvv * n) / m_len;
            put(out, (hv + 1) + 2, d(2, 1)? + offset_distance * nuvv)?;
        }
        if hv >= 3 {
            // (upstream adds Buuv where Bvvv belongs)
            let cvvv = d(1, 3)?.cross(bv)
                + (3.0 * d(1, 2)?).cross(d(0, 2)?)
                + (3.0 * d(1, 1)?).cross(d(0, 3)?)
                + bu.cross(d(0, 4)?);
            let mvvv = (3.0 * cv.dot(cvv) + c.dot(cvvv) - 3.0 * mv * mvv) / m_len;
            let nvvv = (cvvv - 3.0 * mv * nvv - 3.0 * mvv * nv - mvvv * n) / m_len;
            put(out, 3, d(2, 1)? + offset_distance * nvvv)?;
        }

        if !do_eval_simple {
            // replace zero first derivatives by short base tangents
            let tolerance = EFF_ZERO * (1.0 + get(out, 0)?.max_dimension());
            if hu > 0 && get(out, hv + 1)?.length_squared() < tolerance * tolerance {
                if let Some((_pnt, du, _dv)) =
                    ignored(surface.evaluate_1st_derivatives(k, uv, true, true))?
                {
                    if du.length_squared() > tolerance * tolerance {
                        let du = du.unitize_or_z();
                        put(out, hv + 1, du * tolerance * 1.1)?;
                    }
                }
            }
            if hv > 0 && get(out, 1)?.length_squared() < tolerance * tolerance {
                if let Some((_pnt, _du, dv)) =
                    ignored(surface.evaluate_1st_derivatives(k, uv, true, true))?
                {
                    if dv.length_squared() > tolerance * tolerance {
                        let dv = dv.unitize_or_z();
                        put(out, 1, dv * tolerance * 1.1)?;
                    }
                }
            }
        }
        let _ = (cuv, mvv, nvv, muv, nuv);
        Ok(())
    }

    // ---- inversion and edits ---------------------------------------------------------------

    /// The parameter of `point_on_surf` in `anal_domain` and its location
    /// (`STEPInversion`): the base surface's inversion with the tolerance widened by the
    /// offset distance, retried on the extended surface (unwidened tolerance) when it
    /// fails or finds the point exterior; a result whose offset point (`EvaluatePoint`,
    /// which may extend the base for this call only, see
    /// [`OffsetSurface::step_inversion_extending`]) is farther than `tol_3d` from
    /// `point_on_surf` fails.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a result off the offset; those of the inversions and of
    /// [`OffsetSurface::evaluate_point`].
    pub fn step_inversion(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        let offset_tol = tol_3d + self.offset_distance.get().abs();
        let base = self
            .surface
            .step_inversion(k, anal_domain, point_on_surf, offset_tol, uv_guess);
        self.with_extension(|ext| {
            self.step_inversion_with(ext, base, k, anal_domain, point_on_surf, tol_3d, uv_guess)
        })
    }

    /// [`OffsetSurface::step_inversion`], storing an extension its `EvaluatePoint` makes,
    /// and, for an offset base, the one the base's inversion makes (upstream's `const`
    /// evaluators store it in the base offset).
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::step_inversion`].
    pub fn step_inversion_extending(
        &mut self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        let offset_tol = tol_3d + self.offset_distance.get().abs();
        let base = match self.surface.kind_mut() {
            SurfaceKind::OffsetSurface(inner) => {
                inner.step_inversion_extending(k, anal_domain, point_on_surf, offset_tol, uv_guess)
            }
            _ => self
                .surface
                .step_inversion(k, anal_domain, point_on_surf, offset_tol, uv_guess),
        };
        let r = self.with_extension(|ext| {
            self.step_inversion_with(ext, base, k, anal_domain, point_on_surf, tol_3d, uv_guess)
        });
        self.fold_pending_extension();
        r
    }

    /// The body of [`OffsetSurface::step_inversion`] after the base surface's
    /// inversion (`base`), with the extension of the call.
    fn step_inversion_with(
        &self,
        ext: &mut Extension<'_>,
        base: Result<(Point2, LocationType)>,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        let mut result = base;
        let retry = match &result {
            Ok((_, location)) => *location == LocationType::Exterior,
            Err(e) if e.is_unported() => return result,
            Err(_) => true,
        };
        if retry {
            if let Some(e) = ext.current() {
                result = e.step_inversion(k, anal_domain, point_on_surf, tol_3d, uv_guess);
            }
        }
        let (uv, location) = result?;
        let pt = self.point_with(k, ext, uv)?;
        if pt.distance(point_on_surf) > tol_3d {
            return Err(SolidError::sm_err(OP_INV));
        }
        Ok((uv, location))
    }

    /// Reverses the base surface (and the extended surface) in `param`'s direction
    /// (`Reverse`).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::reverse`].
    pub fn reverse(&mut self, k: &Kernel, param: SurfParamType) -> Result<()> {
        self.fold_pending_extension();
        self.surface.reverse(k, param)?;
        if let Some(e) = self.extended_surface.as_deref_mut() {
            e.reverse(k, param)?;
        }
        Ok(())
    }

    /// Swaps `u` and `v` of the extended and the owned base surface, negating the
    /// distance for each (`SwapUV`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the base surface is not owned (after the extended surface was
    /// swapped); those of [`Surface::swap_uv`].
    pub fn swap_uv(&mut self, k: &Kernel) -> Result<()> {
        self.swap_uv_notifying(k, &mut |_, _, _| Ok(()))
    }

    /// [`OffsetSurface::swap_uv`] with the extended and base surfaces' edit
    /// notifications sent through `notify` ([`Surface::swap_uv_notifying`]); the offset
    /// surface notifies nothing itself, as upstream.
    ///
    /// # Errors
    ///
    /// Those of [`OffsetSurface::swap_uv`] and of `notify`.
    pub(crate) fn swap_uv_notifying(
        &mut self,
        k: &Kernel,
        notify: &mut crate::surface::surface::p2::SurfaceEditNotify<'_>,
    ) -> Result<()> {
        self.fold_pending_extension();
        if let Some(e) = self.extended_surface.as_deref_mut() {
            e.swap_uv_notifying(k, notify)?;
            self.offset_distance.set(-self.offset_distance.get());
        }
        if self.owns_surface {
            self.surface.swap_uv_notifying(k, notify)?;
            self.offset_distance.set(-self.offset_distance.get());
        } else {
            // Can not swap UV if I do not own the surface
            return Err(SolidError::sm_err(OP_EDIT));
        }
        Ok(())
    }

    /// Scales (optionally, uniform only: the distance scales too) then rotates and
    /// moves the extended and the owned base surface (`Transform`); the identity changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform scale, or when the base surface is not owned (after
    /// the distance and the extended surface were changed); those of
    /// [`Surface::transform`].
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        self.fold_pending_extension();
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }
        if let Some(s) = scale {
            if are_same(s.x, s.y) && are_same(s.x, s.z) {
                // (upstream: x * distance; the product is the same either way round)
                self.offset_distance.set(self.offset_distance.get() * s.x);
            } else {
                // OffsetSurface NonUniform scaling not supported
                return Err(SolidError::sm_err(OP_EDIT));
            }
        }
        if let Some(e) = self.extended_surface.as_deref_mut() {
            e.transform_at_depth(k, rotate_n_move, scale, depth + 1)?;
        }
        if self.owns_surface {
            self.surface
                .transform_at_depth(k, rotate_n_move, scale, depth + 1)?;
        } else {
            // Can not transform if I do not own the surface
            return Err(SolidError::sm_err(OP_EDIT));
        }
        Ok(())
    }

    /// Trims to `trim_domain` (`TrimWithDomain`): corners outside the base domain are
    /// evaluated first (which may extend and store the extended surface); the extended
    /// surface is trimmed, and the owned base surface when the domain does not grow.
    /// `trim_domain` may be snapped by the trims.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the base surface is not owned (after the other edits); those of
    /// the evaluations and of [`Surface::trim_with_domain`].
    pub fn trim_with_domain(&mut self, k: &Kernel, trim_domain: &mut Extent2d) -> Result<()> {
        self.fold_pending_extension();
        let base_domain = self.surface.natural_uv_domain_with_nurb(k)?;
        let mut gets_bigger = false;
        if !base_domain.contains_point(trim_domain.min(), 0.0) {
            self.evaluate_point_extending(k, trim_domain.min())?;
            gets_bigger = true;
        }
        if !base_domain.contains_point(trim_domain.max(), 0.0) {
            self.evaluate_point_extending(k, trim_domain.max())?;
            gets_bigger = true;
        }
        if let Some(e) = self.extended_surface.as_deref_mut() {
            e.trim_with_domain(k, trim_domain)?;
        }
        if self.owns_surface && !gets_bigger {
            self.surface.trim_with_domain(k, trim_domain)?;
        }
        if !self.owns_surface {
            return Err(SolidError::sm_err(OP_EDIT));
        }
        Ok(())
    }

    // ---- database ---------------------------------------------------------------------------

    /// Reads an offset surface written by `WriteToDB` (`ReadFromDB`): the distance, the
    /// extension and periodicity flags, whether an extended surface follows, then the
    /// base (and extended) surface records. The new offset owns its base; its
    /// singularities are the base's `GetSingularities` (not stored in the file).
    ///
    /// # Errors
    ///
    /// Those of the reader and of [`Surface::read_from_db`]; `InvalidInput` for an
    /// unreadable type header (undefined upstream) or nesting too deep; those of
    /// [`Surface::get_singularities`].
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        db.enter_nested(OP_READ)?;
        let result = read_body(k, db, version);
        db.leave_nested();
        result
    }

    /// `WriteToDB`: writes the offset distance, the extension and periodicity flags
    /// and whether there is an extended surface, then the base surface and the
    /// extended surface (if any), each with its type header. `depth` is this
    /// surface's nesting depth (see [`write_surface_at`]).
    ///
    /// # Errors
    ///
    /// Those of [`write_surface_at`] for the base and extended surfaces.
    pub fn write_to_db(&self, db: &mut DatabaseWriter, version: u64, depth: usize) -> Result<()> {
        let ascii = db.file_type() == FileType::Ascii;
        let distance = self.offset_distance.get();
        let allow = i32::from(self.allow_extension);
        self.with_current_extension(|extended| {
            let has_extended = i32::from(extended.is_some());
            if ascii {
                db.out_f64(distance);
                db.out_str(" SmOffsetSurface Offset Distance \n");
                db.out_i64(allow.into());
                db.out_str(" SmOffsetSurface Allow Extension \n");
                db.out_i64(self.is_periodic_on_u.into());
                db.out_str(" SmOffsetSurface PeriodicOnU \n");
                db.out_i64(self.is_periodic_on_v.into());
                db.out_str(" SmOffsetSurface PeriodicOnV \n");
                db.out_i64(has_extended.into());
                db.out_str(" SmOffsetSurface HasExtendedSurface \n");
            } else {
                db.write_double(distance);
                db.write_boolean(allow);
                db.write_boolean(self.is_periodic_on_u);
                db.write_boolean(self.is_periodic_on_v);
                db.write_boolean(has_extended);
            }

            if ascii {
                db.out_str(" OffsetSurface->BaseSurface \n");
            }
            db.write_type(self.surface.type_code(), None);
            write_surface_at(&self.surface, db, version, depth + 1)?;
            if let Some(e) = extended {
                if ascii {
                    db.out_str(" OffsetSurface->ExtendedSurface \n");
                }
                db.write_type(e.type_code(), None);
                write_surface_at(e, db, version, depth + 1)?;
            } else if ascii {
                db.out_str("Has No OffsetSurface->ExtendedSurface \n");
            }
            Ok(())
        })
    }
}

/// Maximum wrapper depth accepted by offset construction and restoration.
///
/// Bounds recursive domain queries, edits, cloning and destruction on wasm32. The
/// crate-wide [`MAX_GEOMETRY_NESTING`], counted across curves, surfaces and volumes;
/// equal to the database reader's bound
/// ([`MAX_READ_NESTING`](crate::topology::database_io::MAX_READ_NESTING)).
pub const MAX_SURFACE_NESTING: usize = MAX_GEOMETRY_NESTING;

/// Check `surface` (at `depth`) and everything it owns, without recursion.
fn check_surface_nesting(surface: &Surface, depth: usize, op: &'static str) -> Result<()> {
    check_geometry_nesting(NestedGeometry::Surface(surface), depth, op)
}

/// `GetNaturalUVDomain` of an offset over `base` with `extended`.
fn domain_with(base: &Surface, extended: Option<&Surface>) -> Extent2d {
    // (kernel-free: OffsetSurface::natural_uv_domain_with_nurb is the lazy-safe form)
    extended.map_or_else(
        || base.natural_uv_domain_if_built(),
        Surface::natural_uv_domain_if_built,
    )
}

/// An ASCII `SmBoolean`: `0` when the extraction fails.
fn ascii_bool(db: &mut DatabaseReader) -> i32 {
    let v = db
        .in_stream_mut()
        .and_then(InputStream::extract_int)
        .unwrap_or(0);
    db.go_to_next_line();
    v
}

/// A surface type header (`ReadType`).
fn read_surface_type(db: &mut DatabaseReader) -> Result<i64> {
    db.read_type(false)?
        .type_code
        .ok_or_else(|| SolidError::invalid_input(OP_READ, "surface type not read"))
}

/// The body of [`OffsetSurface::read_from_db`].
fn read_body(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<OffsetSurface> {
    let ascii = db.file_type() == FileType::Ascii;

    // read the members
    let (offset_distance, allow_extension, is_periodic_on_u, is_periodic_on_v, has_extended) =
        if ascii {
            let d = db
                .in_stream_mut()
                .and_then(InputStream::extract_double)
                .unwrap_or(0.0);
            db.go_to_next_line();
            let allow = ascii_bool(db);
            let pu = ascii_bool(db);
            let pv = ascii_bool(db);
            let has_ext = ascii_bool(db);
            (d, allow, pu, pv, has_ext)
        } else {
            let d = db.read_double()?;
            let allow = db.read_boolean()?;
            let pu = db.read_boolean()?;
            let pv = db.read_boolean()?;
            let has_ext = db.read_boolean()?;
            (d, allow, pu, pv, has_ext)
        };

    // base surface
    if ascii {
        db.go_to_next_line();
    }
    let base_type = read_surface_type(db)?;
    let base_surface = Surface::read_from_db(k, base_type, db, version)?;

    // extended surface
    if ascii {
        db.go_to_next_line();
    }
    let extended_surface = if has_extended != 0 {
        let extended_type = read_surface_type(db)?;
        Some(Box::new(Surface::read_from_db(
            k,
            extended_type,
            db,
            version,
        )?))
    } else {
        None
    };

    check_surface_nesting(&base_surface, 1, OP_READ)?;
    if let Some(extended) = extended_surface.as_deref() {
        check_surface_nesting(extended, 1, OP_READ)?;
    }
    // pOffsetSurface->m_pSurface->SetOwner(pOffsetSurface) (before the singularities)
    let mut base_surface = base_surface;
    base_surface.set_surface_owner(BASE_OWNER, None);
    // regen here - this data is not persistent
    let singularities = base_singularities(k, &base_surface)?;

    Ok(OffsetSurface {
        offset_distance: F64Cell::new(offset_distance),
        allow_extension: allow_extension != 0,
        owns_surface: true,
        surface: Box::new(base_surface),
        extended_surface,
        pending_extension: PendingExtension::default(),
        is_periodic_on_u,
        is_periodic_on_v,
        singularities,
    })
}

// ---- sm_ExtendSurface ------------------------------------------------------------------------

/// The NURB of a B-spline kind surface (`GetOrCreateGwNurbPointer`, which calls the
/// virtual `MakeNurb` when it is missing): the B-spline's (a plain B-spline cannot make
/// one), the plane's (made on a copy when missing), the surface of revolution's (also
/// of a cone, cylinder, sphere or torus) and the surface of extrusion's. A lazy
/// analytic's missing NURB is made with the kernel's tolerances and kept on the surface
/// ([`Surface::kind_with_nurb`], as its evaluations make it); a make that leaves none
/// (a failing `MakeNurb`, whose status upstream ignores) is `SM_ERR_NULL_POINTER`.
fn nurb_of(k: &Kernel, surface: &Surface) -> Result<crate::nurbs::srf_basic::Surface> {
    let missing = || SolidError::null_pointer(OP_EXTEND);
    match surface.kind_with_nurb(k)? {
        SurfaceKind::BSpline(b) => b.gw_nurb().cloned().ok_or_else(missing),
        SurfaceKind::Plane(p) => p.base_with_nurb()?.gw_nurb().cloned().ok_or_else(missing),
        SurfaceKind::SurfOfRevolution(s) => s.base().gw_nurb().cloned().ok_or_else(missing),
        SurfaceKind::SurfOfExtrusion(s) => s.base().gw_nurb().cloned().ok_or_else(missing),
        SurfaceKind::Cone(s) => s.rev().base().gw_nurb().cloned().ok_or_else(missing),
        SurfaceKind::Sphere(s) => s.base().base().gw_nurb().cloned().ok_or_else(missing),
        SurfaceKind::Torus(s) => s.base().base().gw_nurb().cloned().ok_or_else(missing),
        _ => Err(missing()),
    }
}

/// One NLib G1R extension of `surface` reaching `uv` (the fork's `sm_ExtendToward`).
fn extend_toward(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Surface> {
    let sur = nurb_of(k, surface)?;
    let mut esur: Option<UnboundedExtension> = None;
    match eval_pt_derivs_unbounded(&sur, uv.x, uv.y, 0, ExtendContinuity::G1R, &mut esur) {
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => return Err(SolidError::sm_err(OP_EXTEND)),
        Ok(_) => {}
    }
    let ext = esur.ok_or_else(|| {
        SolidError::invalid_input(
            OP_EXTEND,
            "no extension made (upstream reads an empty NURB)",
        )
    })?;
    Ok(Surface::from(BSplineSurface::from_gw_nurb(&ext.surface)))
}

/// The extension of `fixes.offset_extension_from_base` (WR-458, #419; the fork's
/// source fork@2d7c7ed): `base` is extended on every side that can grow at once,
/// each by the base domain's size times the least power of two that reaches `uv`, and
/// never less than the `current` extension's domain, so one point always gives one
/// extension whatever was evaluated first. `Ok(None)` when `have` already contains `uv`.
fn extend_from_base(
    k: &Kernel,
    base: &Surface,
    uv: Point2,
    current: Option<Extent2d>,
    periodic: [bool; 2],
    singularities: u32,
) -> Result<Option<Surface>> {
    let domain = base.natural_uv_domain_with_nurb(k)?;
    let size = domain.size();
    let have = current.unwrap_or(domain);
    if domain.contains_point(uv, 0.0) || have.contains_point(uv, 0.0) {
        return Ok(None);
    }
    // the least power of two sizes that takes the end `end` (towards `dir`) past `target`
    let reach = |end: f64, size: f64, target: f64, dir: f64| {
        let mut k = 1.0;
        while k < 1.0e6 && (end + dir * k * size - target) * dir < 0.0 {
            k *= 2.0;
        }
        end + dir * k * size
    };
    let (dmin, dmax) = (domain.min(), domain.max());
    let (mut lo, mut hi) = (have.min(), have.max());
    if !periodic[0] && (singularities & SS_UMIN) == 0 {
        lo.x = lo.x.min(reach(dmin.x, size.x, uv.x, -1.0));
    }
    if !periodic[0] && (singularities & SS_UMAX) == 0 {
        hi.x = hi.x.max(reach(dmax.x, size.x, uv.x, 1.0));
    }
    if !periodic[1] && (singularities & SS_VMIN) == 0 {
        lo.y = lo.y.min(reach(dmin.y, size.y, uv.y, -1.0));
    }
    if !periodic[1] && (singularities & SS_VMAX) == 0 {
        hi.y = hi.y.max(reach(dmax.y, size.y, uv.y, 1.0));
    }
    // toward the low corner from the base, then the result toward the high corner (a side
    // that does not grow keeps its middle coordinate)
    let mid = domain.mid();
    let low_uv = Point2::new(
        if lo.x < dmin.x { lo.x } else { mid.x },
        if lo.y < dmin.y { lo.y } else { mid.y },
    );
    let high_uv = Point2::new(
        if hi.x > dmax.x { hi.x } else { mid.x },
        if hi.y > dmax.y { hi.y } else { mid.y },
    );
    let low = if domain.contains_point(low_uv, 0.0) {
        None
    } else {
        Some(extend_toward(k, base, low_uv)?)
    };
    let from = low.as_ref().unwrap_or(base);
    if from
        .natural_uv_domain_with_nurb(k)?
        .contains_point(high_uv, 0.0)
    {
        return Ok(low);
    }
    Ok(Some(extend_toward(k, from, high_uv)?))
}

/// `sm_ExtendSurface`: makes `uv` lie in the domain of `surface` by wrapping it in
/// periodic directions and clamping it at singular sides; if it is still outside,
/// returns the B-spline surface extended (G1, ruled) to contain a point one domain
/// size farther out (or `uv` itself when that fails), `None` otherwise. `uv` is updated
/// in place. With `fixed` (`fixes.offset_extension_from_base`; `current` is the current
/// extension's domain) the extension is made by [`extend_from_base`] instead.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a surface that is not a B-spline kind (upstream's `NER`
/// in the caller); `SM_ERR` when both extensions fail; `InvalidInput` when the
/// extension leaves no surface (upstream builds a surface from an empty NURB).
fn extend_surface(
    k: &Kernel,
    surface: &Surface,
    uv: &mut Point2,
    is_periodic_on_u: bool,
    is_periodic_on_v: bool,
    singularities: u32,
    fixed: bool,
    current: Option<Extent2d>,
) -> Result<Option<Surface>> {
    // SM_CAST_PTR(SmBSplineSurface, pSurface); NER(pOriginal) (in the caller)
    if !is_b_spline_kind(surface) {
        return Err(SolidError::null_pointer(OP_EXTEND));
    }

    // locals
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    let ivl_u = domain.u_interval();
    let ivl_v = domain.v_interval();
    let size: Vec2 = domain.size();

    // when periodic - wrap UV coordinates into the domain
    if is_periodic_on_u {
        uv.x = ivl_u.periodic_wrap(uv.x);
    }
    if is_periodic_on_v {
        uv.y = ivl_v.periodic_wrap(uv.y);
    }

    // when singular - clamp UV coordinates to the singular sides
    if (singularities & SS_UMIN) != 0 && uv.x < ivl_u.min() {
        uv.x = ivl_u.min();
    }
    if (singularities & SS_VMIN) != 0 && uv.y < ivl_v.min() {
        uv.y = ivl_v.min();
    }
    if (singularities & SS_UMAX) != 0 && uv.x > ivl_u.max() {
        uv.x = ivl_u.max();
    }
    if (singularities & SS_VMAX) != 0 && uv.y > ivl_v.max() {
        uv.y = ivl_v.max();
    }

    // when point is in domain - no extension needed
    if domain.contains_point(*uv, 0.0) {
        return Ok(None);
    }
    if !surface.is_kind_of(B_SPLINE_SURFACE_TYPE)? {
        return Err(SolidError::sm_err(OP_EXTEND));
    }

    // `fixes.offset_extension_from_base`: `fixed` holds the current extension's domain
    if fixed {
        let periodic = [is_periodic_on_u, is_periodic_on_v];
        return extend_from_base(k, surface, *uv, current, periodic, singularities);
    }

    // extend a domain size farther out than needed
    let mut s_uv = *uv;
    if s_uv.x < domain.min().x {
        s_uv.x -= size.x;
    } else if s_uv.x > domain.max().x {
        s_uv.x += size.x;
    }
    if s_uv.y < domain.min().y {
        s_uv.y -= size.y;
    } else if s_uv.y > domain.max().y {
        s_uv.y += size.y;
    }

    // extend the NURB (the extension is kept between the two tries, as upstream's esur)
    let sur = nurb_of(k, surface)?;
    let mut esur: Option<UnboundedExtension> = None;
    let first = eval_pt_derivs_unbounded(&sur, s_uv.x, s_uv.y, 0, ExtendContinuity::G1R, &mut esur);
    if let Err(e) = first {
        if e.is_unported() {
            return Err(e);
        }
        let second =
            eval_pt_derivs_unbounded(&sur, uv.x, uv.y, 0, ExtendContinuity::G1R, &mut esur);
        match second {
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP_EXTEND)),
            Ok(_) => {}
        }
    }
    let ext = esur.ok_or_else(|| {
        SolidError::invalid_input(
            OP_EXTEND,
            "no extension made (upstream reads an empty NURB)",
        )
    })?;
    // (upstream warns when the new domain is more than 20 times the old size)
    Ok(Some(Surface::from(BSplineSurface::from_gw_nurb(
        &ext.surface,
    ))))
}

/// `sm_FindNonsingularStepoff`: at a pole of `surface` (a first derivative shorter than
/// `tol`), the parameter step towards the domain middle (first along the
/// non-degenerate direction, then along the degenerate one; `0.001`, `0.01`, `0.1` of
/// the way) at which that derivative is longer than `tol`.
///
/// # Errors
///
/// `SM_ERR` when no step works; those of the base `EvaluateSimple`.
fn find_nonsingular_stepoff(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    domain: &Extent2d,
    bu_len: f64,
    bv_len: f64,
    tol: f64,
) -> Result<Vec2> {
    let u_is_degen = bu_len < bv_len;
    let mid_param = domain.mid();
    let flags = EvaluateFlags {
        u_side: Side::Left,
        v_side: Side::Left,
        only_upper_half: true,
        non_zero_tangents: false,
    };
    // the base derivative a good step must lengthen (Su when u is degenerate)
    let long_enough = |m: &[Vec<Vec3>]| -> Result<bool> {
        Ok(if u_is_degen {
            at(m, 1, 0)?.length() > tol
        } else {
            at(m, 0, 1)?.length() > tol
        })
    };

    // Loop three times, increasing stepoff size: .001, .01, .1.
    let mut step_size = 0.001;
    while step_size < 0.2 {
        // The usual: a pole, step in the other direction.
        let mut param_step = if u_is_degen {
            Vec2::new(0.0, (mid_param.y - uv.y) * step_size)
        } else {
            Vec2::new((mid_param.x - uv.x) * step_size, 0.0)
        };
        let m = surface.evaluate_simple(k, uv + param_step, 1, 1, flags)?;
        if long_enough(&m)? {
            return Ok(param_step);
        }

        // The usual direction didn't work.  Try the other.
        param_step = if u_is_degen {
            Vec2::new((mid_param.x - uv.x) * step_size, 0.0)
        } else {
            Vec2::new(0.0, (mid_param.y - uv.y) * step_size)
        };
        let m = surface.evaluate_simple(k, uv + param_step, 1, 1, flags)?;
        if long_enough(&m)? {
            return Ok(param_step);
        }

        // Try a bigger stepoff size.
        step_size *= 10.0;
    }

    // Neither direction worked.  We can't get an offset direction.
    Err(SolidError::sm_err(OP_STEPOFF))
}

// ---- the offset seen through the generic surface interface ------------------------------

/// The offset as an `SmSurface` for the base-class bodies run on it
/// (`SmSurface::EvaluateNormal` here, and the bodies passed to
/// [`OffsetSurface::with_evaluator`]): its evaluations share the extension made
/// during the call, which upstream stores in `m_pExtendedSurface` at once.
pub struct OffsetEval<'a> {
    off: &'a OffsetSurface,
}

impl<'a> OffsetEval<'a> {
    const fn new(off: &'a OffsetSurface) -> Self {
        Self { off }
    }

    /// Runs `f` on a copy of the offset (with its current extension) as a [`Surface`];
    /// an extension made on the copy is kept by the offset (upstream runs the body on
    /// the offset itself).
    fn on_snapshot<T>(&self, f: impl FnOnce(&Surface) -> T) -> T {
        let snapshot = Surface::from(self.off.clone());
        let r = f(&snapshot);
        if let SurfaceKind::OffsetSurface(o) = snapshot.kind() {
            self.off.keep_extension_made_by(o);
        }
        r
    }

    /// Runs `f` with the extension of the call, keeping a new one it makes.
    fn with_extension<T>(&self, f: impl FnOnce(&mut Extension<'_>) -> T) -> T {
        self.off.with_extension(f)
    }
}

impl SurfaceEvaluate for OffsetEval<'_> {
    fn local_point_solve_minimize(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        point: Point3,
        guess: Point2,
    ) -> Result<(bool, f64, Point2)> {
        self.on_snapshot(|s| s.local_point_solve_minimize(k, domain, point, guess))
    }

    fn drop_point_minimize(
        &self,
        k: &Kernel,
        point: Point3,
        domain: &Extent2d,
        guess: Option<Point2>,
    ) -> Result<Option<(Point2, f64)>> {
        self.on_snapshot(|s| s.drop_point_minimize(k, point, domain, guess))
    }

    fn local_curve_maximize(
        &self,
        k: &Kernel,
        curve: &Curve,
        domain: &Extent2d,
        interval: &Extent1d,
        guess_uv: Point2,
        guess_t: f64,
    ) -> Result<Option<(f64, Point2)>> {
        self.on_snapshot(|s| s.local_curve_maximize(k, curve, domain, interval, guess_uv, guess_t))
    }

    fn natural_uv_domain_if_built(&self) -> Extent2d {
        self.with_extension(|ext| domain_with(&self.off.surface, ext.current()))
    }

    fn natural_uv_domain_with_nurb(&self, k: &Kernel) -> Result<Extent2d> {
        // the extension of the call, else the base (as OffsetSurface::natural_uv_domain_with_nurb)
        self.with_extension(|ext| match ext.current() {
            Some(e) => e.natural_uv_domain_with_nurb(k),
            None => self.off.surface.natural_uv_domain_with_nurb(k),
        })
    }

    fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.with_extension(|ext| {
            self.off.evaluate_with(
                k,
                ext,
                !flags.non_zero_tangents,
                uv,
                highest_u_deriv,
                highest_v_deriv,
                flags,
            )
        })
    }

    fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        self.with_extension(|ext| self.off.point_with(k, ext, uv))
    }

    fn surface_approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        self.off.approx_eq(k, &Surface::from(other.off.clone()))
    }

    fn knots(
        &self,
        k: &Kernel,
        param: SurfParamType,
        ivl: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.with_extension(|ext| match ext.current() {
            Some(e) => e.knots_with_nurb(k, param, ivl),
            None => self.off.surface.knots_with_nurb(k, param, ivl),
        })
    }

    fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(OffsetSurface::is_kind_of(t))
    }

    fn evaluate_normal(&self, k: &Kernel, uv: Point2, u_side: Side, v_side: Side) -> Result<Vec3> {
        // the offset's override, with this call's extension
        let domain = self.off.surface.natural_uv_domain_with_nurb(k)?;
        if domain.contains_point(uv, 0.0) {
            self.off.surface.evaluate_normal(k, uv, u_side, v_side)
        } else {
            base_evaluate_normal(k, self, uv, u_side, v_side)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::vector2d::Vec2;
    use crate::surface::plane::Plane;

    fn plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn flags(non_zero_tangents: bool, only_upper_half: bool) -> EvaluateFlags {
        EvaluateFlags {
            u_side: Side::Left,
            v_side: Side::Left,
            only_upper_half,
            non_zero_tangents,
        }
    }

    fn offset(d: f64) -> OffsetSurface {
        let k = Kernel::default();
        OffsetSurface::new(&k, d, plane(), true, crate::geom::extent2d::SS_NONE).unwrap()
    }

    #[test]
    fn plane_offset_point_and_derivatives() {
        let k = &Kernel::default();
        let off = offset(2.0);
        let p = off.evaluate_point(k, Point2::new(0.25, 0.5)).unwrap();
        assert_eq!(p, Vec3::new(0.25, 0.5, 2.0));
        let d = off
            .evaluate(k, Point2::new(0.25, 0.5), 1, 1, flags(true, false))
            .unwrap();
        assert_eq!(d[0][0], Vec3::new(0.25, 0.5, 2.0));
        assert_eq!(d[1][0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(d[0][1], Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(d[1][1], Vec3::ZERO);
    }

    #[test]
    fn too_many_derivatives_and_clamping() {
        let k = &Kernel::default();
        let off = offset(-1.0);
        assert!(off
            .evaluate(k, Point2::new(0.5, 0.5), 2, 2, flags(true, false))
            .is_err());
        // outside the domain without extension: clamped
        let p = off.evaluate_point(k, Point2::new(2.0, -1.0)).unwrap();
        assert_eq!(p, Vec3::new(1.0, 0.0, -1.0));
        assert_eq!(off.degree(), 3);
    }

    #[test]
    fn upper_half_with_unequal_orders_writes_past_the_array() {
        let k = &Kernel::default();
        let off = offset(1.0);
        let r = off.evaluate(k, Point2::new(0.5, 0.5), 0, 2, flags(true, true));
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn nested_offsets_add_and_twice_nested_fails() {
        let k = Kernel::default();
        let inner = offset(1.0);
        let outer = OffsetSurface::new(&k, 2.0, Surface::from(inner), true, SS_UNKNOWN).unwrap();
        let p = outer.evaluate_point(&k, Point2::new(0.5, 0.5)).unwrap();
        assert_eq!(p, Vec3::new(0.5, 0.5, 3.0));
        let twice = OffsetSurface::new(&k, 1.0, Surface::from(outer), true, SS_UNKNOWN).unwrap();
        assert!(twice.evaluate_point(&k, Point2::new(0.5, 0.5)).is_err());
    }

    #[test]
    fn constructor_bounds_nesting_before_domain_queries() {
        let k = Kernel::default();
        let mut surface = plane();
        for _ in 0..MAX_SURFACE_NESTING {
            surface = OffsetSurface::new(&k, 1.0, surface, true, 0)
                .unwrap()
                .into();
        }
        assert_eq!(
            surface.natural_uv_domain_if_built(),
            plane().natural_uv_domain_if_built()
        );
        let copy = surface.clone();
        assert!(matches!(
            OffsetSurface::new(&k, 1.0, surface, true, 0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            crate::surface::surface::p1::StepSurface::new(copy, true),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    fn face_owned(s: Surface) -> Surface {
        let mut s = s;
        s.set_owner(Some(ObjectRef {
            type_code: crate::geom::assert_array::labels::FACE_TYPE,
            key: ObjectKey(0),
        }));
        s
    }

    fn is_face_tolerance<T>(r: &Result<T>) -> bool {
        matches!(
            r,
            Err(SolidError::Unported {
                unit: "topology.face_p2",
                ..
            })
        )
    }

    /// A face-owned offset asks its face for the box expansion (through the base's
    /// owner chain upstream), as a face-owned B-spline does; it never falls back to
    /// the default zone tolerance.
    #[test]
    fn face_owned_box_expansion_needs_the_face_tolerance() {
        let k = Kernel::default();
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let off = face_owned(Surface::from(offset(2.0)));
        assert!(is_face_tolerance(&off.calculate_bounding_box(
            &k,
            &dom,
            [true, false, false],
            true
        )));
        assert!(is_face_tolerance(&off.calculate_bounding_box(
            &k,
            &dom,
            [false, false, true],
            true
        )));
        // no expansion, no tolerance: the box is computed
        assert!(off.is_planar(&k, 1e-6).is_ok());
        assert!(off
            .calculate_bounding_box(&k, &dom, [true, true, true], false)
            .is_ok());

        // a nested offset passes the face down to its base
        let nested = OffsetSurface::new(&k, 1.0, Surface::from(offset(2.0)), true, 0).unwrap();
        let nested = face_owned(Surface::from(nested));
        assert!(is_face_tolerance(&nested.calculate_bounding_box(
            &k,
            &dom,
            [true, false, false],
            true
        )));

        // without a face the base's default expansion is used
        let free = Surface::from(offset(2.0));
        assert!(free
            .calculate_bounding_box(&k, &dom, [true, true, true], true)
            .is_ok());
    }

    fn base_of(s: &Surface) -> &Surface {
        match s.kind() {
            SurfaceKind::OffsetSurface(o) => o.base_surface(),
            _ => panic!("not an offset"),
        }
    }

    /// An owning offset is its base's owner (`m_pSurface->SetOwner(this)`), so the
    /// base's `GetFace()` follows the offset's owner chain at every nesting level; a
    /// copy re-owns its base without a face; a shared base keeps its caller's owner.
    #[test]
    fn owned_base_follows_the_owner_chain() {
        use crate::surface::surface::p1::zone_tol_3d;
        let k = Kernel::default();
        let face = ObjectRef {
            type_code: crate::geom::assert_array::labels::FACE_TYPE,
            key: ObjectKey(3),
        };
        let inner = Surface::from(OffsetSurface::new(&k, 1.0, plane(), true, 0).unwrap());
        assert_eq!(base_of(&inner).owner(), Some(BASE_OWNER));
        assert_eq!(base_of(&inner).face().unwrap(), None);

        let mut outer = Surface::from(OffsetSurface::new(&k, 1.0, inner, true, 0).unwrap());
        outer.set_owner(Some(face));
        let mid = base_of(&outer);
        assert_eq!(mid.owner(), Some(BASE_OWNER));
        assert_eq!(mid.face().unwrap(), Some(face));
        assert_eq!(base_of(mid).face().unwrap(), Some(face));
        // the solver's loose tolerance asks the face (GetZoneTol3d(GetOwner()))
        assert!(is_face_tolerance(&zone_tol_3d(&k, base_of(mid))));

        // the copy constructor resets the owner; the copy still owns its bases
        let copy = outer.clone();
        assert_eq!(copy.owner(), None);
        assert_eq!(base_of(&copy).owner(), Some(BASE_OWNER));
        assert_eq!(base_of(base_of(&copy)).face().unwrap(), None);

        outer.set_owner(None);
        assert_eq!(base_of(base_of(&outer)).face().unwrap(), None);

        // a non-owning offset shares its caller's surface, owner included; its copy
        // reruns GetSingularities on that surface, which asks the caller's face
        let mut caller = plane();
        caller.set_owner(Some(face));
        let shared = OffsetSurface::new(&k, 1.0, caller.clone_keeping_owner(), false, 0).unwrap();
        assert_eq!(shared.base_surface().owner(), Some(face));
        assert!(is_face_tolerance(&shared.copy(&k)));
        caller.set_owner(Some(BASE_OWNER));
        let shared = OffsetSurface::new(&k, 1.0, caller.clone_keeping_owner(), false, 0).unwrap();
        let copy = shared.copy(&k).unwrap();
        assert_eq!(copy.base_surface().owner(), Some(BASE_OWNER));
    }

    #[test]
    fn default_singularities_come_from_the_base() {
        let k = Kernel::default();
        let off = OffsetSurface::new(&k, 1.0, plane(), true, SS_UNKNOWN).unwrap();
        let base = plane()
            .get_singularities(&k, false, false)
            .unwrap()
            .singular;
        assert_eq!(off.singularities(), base);
        assert_eq!(off.singularities(), crate::geom::extent2d::SS_NONE);
    }

    #[test]
    fn transform_needs_an_owned_base_and_uniform_scale() {
        let k = Kernel::default();
        let mut off =
            OffsetSurface::new(&k, 1.0, plane(), false, crate::geom::extent2d::SS_NONE).unwrap();
        let id = Axis2Placement::default();
        assert!(off.transform(&k, &id, None).is_ok());
        assert!(off
            .transform(&k, &id, Some(Vec3::new(1.0, 2.0, 1.0)))
            .is_err());
        assert!(off.swap_uv(&k).is_err());
    }

    /// An offset over a lazy analytic base (a cylinder or torus built without its NURB,
    /// as the B-rep reader's recogniser leaves them) answers the natural domain of the
    /// NURB its base makes, as upstream's `GetNaturalUVDomain` does through the base's
    /// `GetOrCreateGwNurbPointer`; the kernel-free path would answer the NURB-less base's
    /// empty domain.
    #[test]
    fn offset_of_a_lazy_base_reads_the_made_nurb_domain() {
        use crate::surface::cone::Cone;
        use crate::surface::torus::{Torus, TorusOptions};
        let k = Kernel::default();
        let (o, x, y) = (
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let cylinder = Surface::from(Cone::new_bounded(
            o, x, y, 2.0, 2.0, 0.0, 270.0, 3.0, false, false, false,
        ));
        let torus = Surface::from(
            Torus::new(
                &k,
                o,
                x,
                y,
                Extent2d::from_bounds(0.0, 0.0, 180.0, 180.0),
                3.0,
                1.0,
                TorusOptions {
                    swap_uv: false,
                    inside_out: false,
                    circle_nurb: None,
                },
            )
            .unwrap(),
        );
        let empty = Extent2d::from_bounds(0.0, 0.0, 0.0, 0.0);
        for (name, base) in [("cylinder", cylinder), ("torus", torus)] {
            assert_eq!(
                base.natural_uv_domain_if_built(),
                empty,
                "{name}: the base starts lazy"
            );
            let want = base.natural_uv_domain_with_nurb(&k).unwrap();
            assert_ne!(want, empty, "{name}: the made NURB has a domain");
            let off = Surface::from(OffsetSurface::new(&k, 0.5, base, true, SS_UNKNOWN).unwrap());
            assert_eq!(off.natural_uv_domain_with_nurb(&k).unwrap(), want, "{name}");
        }
    }

    /// Two copies of one offset of a half cylinder (r 5, offset 4) evaluated beyond both
    /// ends of its arc, one straight at the farther points, the other after two nearer
    /// ones first (source issue #419, WR-458): upstream's extension reaches
    /// one domain size past the first point, so the copies disagree; with
    /// `fixes.offset_extension_from_base` the same points give the same extension.
    fn offset_extension_gap(from_base: bool) -> f64 {
        use crate::surface::cone::Cone;
        let mut k = Kernel::default();
        k.fixes.offset_extension_from_base = from_base;
        let mut half = Cone::new_cylinder_bounded(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            5.0,
            0.0,
            180.0,
            10.0,
            false,
            false,
            false,
        );
        half.make_nurb(&k).unwrap();
        let half = Surface::from(half);
        let d = half.natural_uv_domain_with_nurb(&k).unwrap();
        let len = d.max().x - d.min().x;
        let v = 0.5 * (d.min().y + d.max().y);
        let make = || {
            let mut o =
                OffsetSurface::new(&k, 4.0, half.clone(), true, crate::geom::extent2d::SS_NONE)
                    .unwrap();
            o.set_allow_extension(true);
            o
        };
        let (mut first, mut second) = (make(), make());
        for u in [d.max().x + 0.067 * len, d.min().x - 0.067 * len] {
            first
                .evaluate_point_extending(&k, Point2::new(u, v))
                .unwrap();
        }
        let mut gap: f64 = 0.0;
        for u in [d.max().x + 0.116 * len, d.min().x - 0.116 * len] {
            let uv = Point2::new(u, v);
            let b = second.evaluate_point_extending(&k, uv).unwrap();
            let a = first.evaluate_point_extending(&k, uv).unwrap();
            gap = gap.max((a - b).length());
        }
        gap
    }

    #[test]
    fn issue_419_offset_extension_does_not_depend_on_evaluation_order() {
        let upstream = offset_extension_gap(false);
        assert!(upstream > 1e-6, "upstream copies agree: {upstream}");
        let fixed = offset_extension_gap(true);
        assert!(fixed < 1e-9, "fixed copies disagree by {fixed}");
    }
}
