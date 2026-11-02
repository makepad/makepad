//! The volume base class `SmVolume`: a [`Volume`] (a concrete volume kind plus the
//! compounding next map and the orient map), the compound evaluations built from the
//! kinds' simple maps, and the chain-rule lifts of curve, surface and volume
//! derivatives through a volume.
//!
//! Upstream: `source/SMLib/src/SmVolume.cpp`, `source/SMLib/inc/SmVolume.h`.
//!
//! | Rust                                                   | Upstream                                         |
//! |--------------------------------------------------------|--------------------------------------------------|
//! | [`Volume`], [`VolumeKind`]                             | `SmVolume` and its derived classes               |
//! | [`Volume::new`]                                        | `SmVolume(const SmContext*)`                     |
//! | [`Volume::copy_from`], `Clone`                         | `SmVolume(const SmVolume&, SmBoolean)`           |
//! | [`Volume::copy`]                                       | `Copy` (virtual)                                 |
//! | `Drop` (implicit)                                      | `~SmVolume`                                      |
//! | [`Volume::approx_eq`]                                  | `operator==`                                     |
//! | [`Volume::last_map`], [`Volume::next_map`]             | `GetLastMap`, `m_pNextMap`                       |
//! | [`Volume::orient_map`], [`Volume::inv_orient_map`]     | `GetOrientMap`, `GetInvOrientMap`                |
//! | [`Volume::owner`], [`Volume::set_owner`]               | `m_pOwner`, `SetOwner`                           |
//! | [`Volume::orient_origin`], [`Volume::orient_x_axis`], [`Volume::orient_y_axis`], [`Volume::orient_z_axis`] | `GetOrientOrigin`, `GetOrientXAxis`, `GetOrientYAxis`, `GetOrientZAxis` |
//! | [`Volume::add_next_map`]                               | `AddNextMap`                                     |
//! | [`Volume::refresh_inv_orient_map`]                     | `RefreshInvOrientMap`                            |
//! | [`Volume::make_map_simple`]                            | `MakeMapSimple`                                  |
//! | [`Volume::natural_param_domain`]                       | `GetNaturalParamDomain` (virtual)                |
//! | [`Volume::is_point_in_in_space_domain`]                | `IsPointInInSpaceDomain`                         |
//! | [`Volume::is_line_in_param_domain`]                    | `IsLineInParamDomain`                            |
//! | [`Volume::is_curve_in_param_domain`]                   | `IsCurveInParamDomain`                           |
//! | [`Volume::is_curve_in_in_space_domain`]                | `IsCurveInInSpaceDomain`                         |
//! | [`Volume::evaluate`]                                   | `Evaluate` (virtual)                             |
//! | [`Volume::map`], [`Volume::map_point`]                 | `Map`, `MapPoint`                                |
//! | [`Volume::orient`], [`Volume::orient_point`]           | `Orient`, `OrientPoint`                          |
//! | [`Volume::orient_vector`]                              | `OrientVector`                                   |
//! | [`Volume::orient_directional_derivs`]                  | `OrientDirectionalDerivs`                        |
//! | [`Volume::transform`]                                  | `Transform`                                      |
//! | [`Volume::is_bounded`]                                 | `IsBounded`                                      |
//! | [`notify`]                                             | `Notify`                                         |
//! | [`Volume::evaluate_simple`]                            | `EvaluateSimple` (virtual)                       |
//! | [`Volume::evaluate_point_simple`]                      | `EvaluatePointSimple`                            |
//! | [`Volume::evaluate_1st_derivatives_simple`]            | `Evaluate1stDerivativesSimple`                   |
//! | [`Volume::inv_evaluate_guess_point_simple`]            | `InvEvaluateGuessPointSimple` (virtual)          |
//! | [`Volume::is_bounded_simple`]                          | `IsBoundedSimple` (virtual)                      |
//! | [`Volume::is_point_in_param_domain_simple`]            | `IsPointInParamDomainSimple` (virtual)           |
//! | [`Volume::is_line_in_param_domain_simple`]             | `IsLineInParamDomainSimple` (virtual)            |
//! | [`Volume::is_curve_in_param_domain_simple`]            | `IsCurveInParamDomainSimple` (virtual)           |
//! | [`Volume::trim_param_bounding_box_simple`]             | `TrimParamBoundingBoxSimple` (virtual)           |
//! | [`Volume::transform_simple`]                           | `TransformSimple` (virtual)                      |
//! | [`Volume::mirror`]                                     | `Mirror`                                         |
//! | [`Volume::mirror_simple`]                              | `MirrorSimple` (virtual)                         |
//! | [`Volume::has_discontinuities_simple`]                 | `HasDiscontinuitiesSimple` (virtual)             |
//! | [`Volume::calculate_continuities_simple`]              | `CalculateContinuitiesSimple` (virtual)          |
//! | [`Volume::type_code`], [`Volume::type_string`], [`Volume::class_string`], [`VOLUME_TYPE`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`Volume::is_kind_of`], [`is_kind_of`]                 | `IsKindOf`                                       |
//! | [`Discontinuities3d`]                                  | `SmDiscontinuities3d` (constructor, `Init`, `SetUnaligned`, destructor) |
//! | [`FindPvIntersect`]                                    | `SmFindPVIntersectENFO` (constructor, `Evaluate`) |
//! | [`lift_first_derivative`], [`lift_second_derivative`], [`lift_third_derivative`] | `smvol_Lift*Derivative` (curve forms)  |
//! | [`lift_first_derivative_srf`], [`lift_second_derivative_srf`], [`lift_third_derivative_srf`] | `smvol_Lift*Derivative` (surface forms) |
//! | [`lift_first_derivative_vol`], [`lift_second_derivative_vol`], [`lift_third_derivative_vol`] | `smvol_Lift*Derivative` (volume forms) |
//! | [`concat_evals`]                                       | `smvol_ConcatEvals`                              |
//!
//! Deviations:
//! - **Nesting.** `with_maps` and `add_next_map` return `InvalidInput` when the next-map chain would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`]
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream).
//! - **Kinds.** The class hierarchy is the [`VolumeKind`] enum: [`VolumeKind::Base`]
//!   is a plain `SmVolume` (as made by the slicing copy in `IsLineInParamDomain`),
//!   [`VolumeKind::Transform`] holds the [`Transform`] matrix of an `SmTransform`,
//!   [`VolumeKind::BSpline`] the [`BSplineVolume`] of an `SmBSplineVolume` (unit
//!   `surface.b_spline_volume`; its `HasDiscontinuitiesSimple`,
//!   `CalculateContinuitiesSimple` and `IsCurveInParamDomainSimple` come from unit
//!   `surface.b_spline_volume_n_lib`).
//!   [`VolumeKind::Twist`] holds the [`TwistVolume`] members (unit
//!   `surface.twist_volume`), [`VolumeKind::Unbend`] the [`UnbendVolume`] members
//!   (unit `surface.unbend_volume`) and [`VolumeKind::Bend`] the [`BendVolume`]
//!   members (unit `surface.bend_volume`); the virtuals here dispatch to their
//!   overrides.
//! - **Ownership.** The next map is held by value (`Box`), the orient maps as
//!   [`Transform`] matrices (upstream keeps them simple: `RefreshInvOrientMap` makes
//!   both simple maps). The owner flags `m_lNextOwnerFlag` / `m_lOrientOwnerFlag`
//!   only choose between sharing and copying, which gives the same geometry, and are
//!   dropped; [`Volume::add_next_map`] keeps the "copy" request (flag 1) because the
//!   virtual `Copy` it runs can fail. Contexts are not modelled: upstream silently
//!   drops a next map it cannot copy for want of a context; here there always is one.
//! - **Out-params** become return values. Evaluation arrays are `(n+1)^3` vectors
//!   laid out `D[u][v][w]`; slots upstream leaves unwritten (the cross derivatives
//!   above order `n` after a chain-rule concatenation) are [`Vec3::UNDEF`], the value
//!   of a fresh `SmVector3d` array. Optional boolean arguments become
//!   [`EvalOptions`].
//! - **Undefined behaviour.** `RefreshInvOrientMap` without an orient map calls
//!   `MakeMapSimple` through a null pointer, `IsCurveInParamDomain` dereferences the
//!   null copy when `Copy` fails, and `Evaluate` / `Map` dereference a missing inverse
//!   orient map: these return [`SolidError::InvalidInput`].
//! - **Statuses.** Where upstream ignores a status (`Orient` with an orient map,
//!   the orient points in `IsLineInParamDomain`, the inverse orient point in
//!   `IsPointInInSpaceDomain`, `InvertSimple` in `RefreshInvOrientMap`, the
//!   recursive `AddNextMap`) the port does too, except that
//!   [`SolidError::Unported`] always propagates.
//! - **Notifications.** [`notify`] only forwards to `SmAObject::Notify`; the
//!   constructors' and destructor's notifications are not sent (volumes keep no
//!   attributes here). `SetContext` and the `SmContext` arguments are dropped.
//! - **`smvol_ConcatEvals`** returns a fresh array; upstream's aliasing of the output
//!   with an input (through a temporary) is not needed.
//! - **Upstream typos kept.** The surface and volume `smvol_LiftThirdDerivative`
//!   reuse `Dxxz` where the `Dxzz` term belongs (`rDuuv`, `rDuvv`, `rDuuw`, `rDuww`,
//!   `rDvvw`, `rDvww`) and `rDuww` / `rDvww` use the wrong first-map partials;
//!   ported as written (`plan/findings.md`).

use crate::curve::crv_in_volume::CrvInVolume;
use crate::curve::curve::p2::Curve;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::a_object;
use crate::geom::attribute::AttributeStore;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::context::Context;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::vol_directional_derivs;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::local_solve_nd::EvalNFunctions;
use crate::geom::matrix::Matrix;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::transform::{Transform, MAX_DERIV as GW_MAX_DERIV};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, EFF_ZERO, EFF_ZERO_SQ};
use crate::surface::b_spline_volume::{BSplineVolume, VolumeParam};
use crate::surface::bend_volume::BendVolume;
use crate::surface::surface::p2::{check_geometry_nesting, NestedGeometry, MAX_GEOMETRY_NESTING};
use crate::surface::twist_volume::TwistVolume;
use crate::surface::unbend_volume::UnbendVolume;

const OP: &str = "surface::volume::p1::Volume";
const OP_EVAL: &str = "surface::volume::p1::Volume::evaluate";
const OP_MAP: &str = "surface::volume::p1::Volume::map";
const OP_ORIENT: &str = "surface::volume::p1::Volume::orient";
const OP_CONCAT: &str = "surface::volume::p1::concat_evals";
const OP_WITH_MAPS: &str = "surface::volume::p1::Volume::with_maps";
const OP_ADD_NEXT_MAP: &str = "surface::volume::p1::Volume::add_next_map";
const OP_PV: &str = "surface::volume::p1::FindPvIntersect::evaluate";

pub use crate::geom::assert_array::labels::{
    BEND_VOLUME_TYPE, B_SPLINE_VOLUME_TYPE, TRANSFORM_TYPE, TWIST_VOLUME_TYPE, UNBEND_VOLUME_TYPE,
    VOLUME_TYPE,
};

/// The highest derivative `Evaluate`, `Map` and `OrientDirectionalDerivs` support.
pub const MAX_DERIV: usize = 3;

/// Largest evaluation array an ignored orient-map failure hands back (`Orient`).
const MAX_UNDEF_ARRAY: usize = 1 << 20;

/// `(n+1)^3`, the size of an evaluation array of highest derivative `n`.
fn eval_size(n: usize, operation: &'static str) -> Result<usize> {
    let n1 = n.checked_add(1).ok_or(SolidError::invalid_input(
        operation,
        "derivative order overflows",
    ))?;
    n1.checked_mul(n1)
        .and_then(|s| s.checked_mul(n1))
        .ok_or(SolidError::invalid_input(
            operation,
            "derivative order overflows",
        ))
}

/// Element `i` of an evaluation array.
fn at(a: &[Vec3], i: usize, operation: &'static str) -> Result<Vec3> {
    a.get(i).copied().ok_or(SolidError::invalid_input(
        operation,
        "evaluation array too short",
    ))
}

/// The position of a simple map's evaluation (`EvaluatePointSimple` of a
/// transform).
fn transform_point(t: &Transform, p: Point3) -> Result<Point3> {
    at(&t.evaluate_simple(p, 0)?, 0, OP)
}

// ---- kinds --------------------------------------------------------------------------

/// The concrete class of a volume (upstream's derived classes of `SmVolume`).
#[derive(Debug, Clone, PartialEq)]
pub enum VolumeKind {
    /// A plain `SmVolume` (its simple map is undefined: `EvaluateSimple` fails).
    Base,
    /// `SmTransform`: a 4x4 matrix map.
    Transform(Transform),
    /// `SmBSplineVolume`: a trivariate NURBS map.
    BSpline(BSplineVolume),
    /// `SmBendVolume`: the bend members (the bend frame is the orient map).
    Bend(BendVolume),
    /// `SmUnbendVolume`: the unbend members (the unbend frame is the orient map).
    Unbend(UnbendVolume),
    /// `SmTwistVolume`: the twist members (the twist frame is the orient map).
    Twist(TwistVolume),
}

/// Options of the compound evaluations (`Evaluate`, `Map`, `EvaluateSimple`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalOptions {
    /// `bUFromLeft`, `bVFromLeft`, `bWFromLeft`: evaluate a point on an interval
    /// boundary in the upper (`true`) or lower interval, per direction.
    pub from_left: [bool; 3],
    /// `bNonZeroTangents`: replace zero tangents by tolerance-sized ones.
    pub non_zero_tangents: bool,
    /// `bDoZeroSampling` (internal; always `true` upstream).
    pub do_zero_sampling: bool,
    /// `bWithCompounding`: evaluate through the next maps too.
    pub with_compounding: bool,
}

impl Default for EvalOptions {
    /// Everything `true` (`MapPoint`'s arguments and upstream's defaults).
    fn default() -> Self {
        Self {
            from_left: [true; 3],
            non_zero_tangents: true,
            do_zero_sampling: true,
            with_compounding: true,
        }
    }
}

// ---- discontinuities ----------------------------------------------------------------

/// The discontinuities of a simple map along one parameter direction (the
/// `U`, `V` or `W` members of `SmDiscontinuities3d`).
#[derive(Debug, Clone, PartialEq)]
pub struct DirectionDiscontinuities {
    /// `m_bUnalignedU`: some discontinuity is not on a constant-parameter plane.
    pub unaligned: bool,
    /// `m_eMinContU`: the lowest continuity over the interior knots.
    pub min_cont: ContinuityType,
    /// `m_sParamsU`: the discontinuity parameters.
    pub params: Vec<f64>,
    /// `m_sContsU`: the continuity at each parameter.
    pub conts: Vec<ContinuityType>,
}

impl DirectionDiscontinuities {
    /// No discontinuities in this direction.
    const fn none() -> Self {
        Self {
            unaligned: false,
            min_cont: ContinuityType::CInfinity,
            params: Vec::new(),
            conts: Vec::new(),
        }
    }

    /// Sets `unaligned` and `min_cont` and empties the lists (`ReSet`).
    fn reset(&mut self, unaligned: bool, min_cont: ContinuityType) {
        self.unaligned = unaligned;
        self.min_cont = min_cont;
        self.params.clear();
        self.conts.clear();
    }
}

/// The parameter-space discontinuities of a simple map (`SmDiscontinuities3d`).
#[derive(Debug, Clone, PartialEq)]
pub struct Discontinuities3d {
    /// `m_bHasDiscontinuities`.
    pub has_discontinuities: bool,
    /// `m_eMinContinuity`: the lowest continuity anywhere in the map.
    pub min_continuity: ContinuityType,
    /// The `U` direction (`m_bUnalignedU`, `m_eMinContU`, `m_sParamsU`, `m_sContsU`).
    pub u: DirectionDiscontinuities,
    /// The `V` direction.
    pub v: DirectionDiscontinuities,
    /// The `W` direction.
    pub w: DirectionDiscontinuities,
}

impl Default for Discontinuities3d {
    fn default() -> Self {
        Self::new()
    }
}

impl Discontinuities3d {
    /// No discontinuities (the constructor, which runs `Init`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            has_discontinuities: false,
            min_continuity: ContinuityType::CInfinity,
            u: DirectionDiscontinuities::none(),
            v: DirectionDiscontinuities::none(),
            w: DirectionDiscontinuities::none(),
        }
    }

    /// Resets to "no discontinuities" (`Init`; also the destructor's work).
    pub fn init(&mut self) {
        self.has_discontinuities = false;
        self.min_continuity = ContinuityType::CInfinity;
        self.u.reset(false, ContinuityType::CInfinity);
        self.v.reset(false, ContinuityType::CInfinity);
        self.w.reset(false, ContinuityType::CInfinity);
    }

    /// Marks C0 discontinuities not aligned with any parameter plane
    /// (`SetUnaligned`).
    pub fn set_unaligned(&mut self) {
        self.has_discontinuities = true;
        self.min_continuity = ContinuityType::C0;
        self.u.reset(true, ContinuityType::C0);
        self.v.reset(true, ContinuityType::C0);
        self.w.reset(true, ContinuityType::C0);
    }
}

// ---- the volume ---------------------------------------------------------------------

/// A volume (`SmVolume`): a concrete simple map from parameter space to projection
/// space, an optional orient map placing parameter space in in-space (and its
/// inverse), and an optional next map compounding the result into further
/// out-spaces.
#[derive(Debug, Clone)]
pub struct Volume {
    /// The concrete class and its data.
    pub(super) kind: VolumeKind,
    /// `m_pOwner` (not used upstream beyond `SetOwner`).
    owner: Option<ObjectRef>,
    /// `m_pNextMap`: `Evaluate(x) = NextMap(ThisMap(x))`.
    pub(super) next_map: Option<Box<Volume>>,
    /// `m_lNextOwnerFlag`: `0` the next map is shared (a copy of this volume keeps
    /// it as is), `1` / `2` it is owned (a copy copies it with its virtual `Copy`).
    pub(super) next_owner_flag: u64,
    /// `m_pOrientMap`: parameter space to in-space (`None` = identity).
    pub(super) orient_map: Option<Transform>,
    /// `m_pInvOrientMap`: in-space back to parameter space.
    pub(super) inv_orient_map: Option<Transform>,
}

impl Volume {
    /// A simple, unoriented volume of `kind` (`SmVolume(const SmContext*)`, the base
    /// part of every derived constructor).
    #[must_use]
    pub const fn new(kind: VolumeKind) -> Self {
        Self {
            kind,
            owner: None,
            next_map: None,
            next_owner_flag: 0,
            orient_map: None,
            inv_orient_map: None,
        }
    }

    /// `kind` oriented by `orient` (the orient map set as `SetOrientMap` does, with
    /// [`Volume::refresh_inv_orient_map`]), without next maps.
    #[must_use]
    pub fn oriented(kind: VolumeKind, orient: Option<Transform>) -> Self {
        let mut v = Self::new(kind);
        if let Some(o) = orient {
            v.orient_map = Some(o);
            // an orient map is present: the refresh cannot fail
            let _ = v.refresh_inv_orient_map();
        }
        v
    }

    /// [`Volume::oriented`] compounded with `next` (which becomes the whole
    /// next-map chain, owned: owner flag 2).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the volume would hold more than
    /// [`MAX_GEOMETRY_NESTING`] next maps.
    pub fn with_maps(
        kind: VolumeKind,
        orient: Option<Transform>,
        next: Option<Self>,
    ) -> Result<Self> {
        if let Some(next) = &next {
            check_geometry_nesting(NestedGeometry::Volume(next), 1, OP_WITH_MAPS)?;
        }
        let mut v = Self::oriented(kind, orient);
        if let Some(next) = next {
            v.next_map = Some(Box::new(next));
            v.next_owner_flag = 2;
        }
        Ok(v)
    }

    /// The copy constructor `SmVolume(const SmVolume&, SmBoolean)` with the derived
    /// part copied as well. With `simple_map_only` the next maps are omitted;
    /// otherwise an owned next map is copied with its virtual `Copy` (when that
    /// fails, as for a plain `SmVolume`, the copy has no next map: upstream ignores
    /// the status) and a shared one is kept.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from copying an unported kind.
    pub fn copy_from(source: &Self, simple_map_only: bool) -> Result<Self> {
        let next_map = match &source.next_map {
            // when asked omit any NextMap making the copy a 'simple' map
            _ if simple_map_only => None,
            // copy an independently owned NextMap keeping the copy a 'compound' map
            Some(next) if source.next_owner_flag >= 1 => ignored(next.copy(false))?.map(Box::new),
            shared => shared.clone(),
        };
        Ok(Self {
            kind: match &source.kind {
                // the derived copy constructor fixes the knot vectors up again
                VolumeKind::BSpline(b) => VolumeKind::BSpline(b.copy()),
                k => k.clone(),
            },
            owner: source.owner,
            next_map,
            next_owner_flag: source.next_owner_flag,
            orient_map: source.orient_map,
            inv_orient_map: source.inv_orient_map,
        })
    }

    /// The virtual `Copy`: the derived class's copy constructor.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a plain `SmVolume` (the base `Copy`);
    /// [`SolidError::Unported`] for the unported kinds.
    pub fn copy(&self, simple_map_only: bool) -> Result<Self> {
        match &self.kind {
            VolumeKind::Base => Err(SolidError::sm_err("surface::volume::p1::Volume::copy")),
            VolumeKind::Transform(_)
            | VolumeKind::BSpline(_)
            | VolumeKind::Bend(_)
            | VolumeKind::Twist(_)
            | VolumeKind::Unbend(_) => Self::copy_from(self, simple_map_only),
        }
    }

    // ---- simple access --------------------------------------------------------------

    /// The concrete class and its data.
    #[must_use]
    pub const fn kind(&self) -> &VolumeKind {
        &self.kind
    }

    /// The next map of the compound map (`GetNextMap`).
    #[must_use]
    pub fn next_map(&self) -> Option<&Self> {
        self.next_map.as_deref()
    }

    /// The last map of the next-map chain, `None` without a next map
    /// (`GetLastMap`).
    #[must_use]
    pub fn last_map(&self) -> Option<&Self> {
        let mut map = self.next_map.as_deref()?;
        while let Some(next) = map.next_map.as_deref() {
            map = next;
        }
        Some(map)
    }

    /// Mutable [`Volume::last_map`] (a loop over the chain, no recursion).
    fn last_map_mut(&mut self) -> Option<&mut Self> {
        let mut map = self.next_map.as_deref_mut()?;
        while map.next_map.is_some() {
            map = map.next_map.as_deref_mut()?;
        }
        Some(map)
    }

    /// The orient map (`GetOrientMap`).
    #[must_use]
    pub const fn orient_map(&self) -> Option<&Transform> {
        self.orient_map.as_ref()
    }

    /// The inverse orient map (`GetInvOrientMap`).
    #[must_use]
    pub const fn inv_orient_map(&self) -> Option<&Transform> {
        self.inv_orient_map.as_ref()
    }

    /// The owner (`m_pOwner`).
    #[must_use]
    pub const fn owner(&self) -> Option<ObjectRef> {
        self.owner
    }

    /// Sets the owner and returns the previous one (`SetOwner`).
    pub const fn set_owner(&mut self, owner: Option<ObjectRef>) -> Option<ObjectRef> {
        let previous = self.owner;
        self.owner = owner;
        previous
    }

    /// The in-space orient origin (`GetOrientOrigin`): the orient map's
    /// displacement, else the origin.
    #[must_use]
    pub fn orient_origin(&self) -> Vec3 {
        self.orient_map
            .as_ref()
            .map_or(Vec3::new(0.0, 0.0, 0.0), Transform::to_disp)
    }

    /// The in-space orient x axis (`GetOrientXAxis`), else `(1, 0, 0)`.
    #[must_use]
    pub fn orient_x_axis(&self) -> Vec3 {
        self.orient_map
            .as_ref()
            .map_or(Vec3::new(1.0, 0.0, 0.0), Transform::to_x_axis)
    }

    /// The in-space orient y axis (`GetOrientYAxis`), else `(0, 1, 0)`.
    #[must_use]
    pub fn orient_y_axis(&self) -> Vec3 {
        self.orient_map
            .as_ref()
            .map_or(Vec3::new(0.0, 1.0, 0.0), Transform::to_y_axis)
    }

    /// The in-space orient z axis (`GetOrientZAxis`), else `(0, 0, 1)`.
    #[must_use]
    pub fn orient_z_axis(&self) -> Vec3 {
        self.orient_map
            .as_ref()
            .map_or(Vec3::new(0.0, 0.0, 1.0), Transform::to_z_axis)
    }

    // ---- type -----------------------------------------------------------------------

    /// The class type code (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        match self.kind {
            VolumeKind::Base => VOLUME_TYPE,
            VolumeKind::Transform(_) => TRANSFORM_TYPE,
            VolumeKind::BSpline(_) => B_SPLINE_VOLUME_TYPE,
            VolumeKind::Bend(_) => BEND_VOLUME_TYPE,
            VolumeKind::Unbend(_) => UNBEND_VOLUME_TYPE,
            VolumeKind::Twist(_) => TWIST_VOLUME_TYPE,
        }
    }

    /// The type code's name (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        match self.kind {
            VolumeKind::Base => "SmVolume_TYPE",
            VolumeKind::Transform(_) => "SmTransform_TYPE",
            VolumeKind::BSpline(_) => "SmBSplineVolume_TYPE",
            VolumeKind::Bend(_) => "SmBendVolume_TYPE",
            VolumeKind::Unbend(_) => "SmUnbendVolume_TYPE",
            VolumeKind::Twist(_) => "SmTwistVolume_TYPE",
        }
    }

    /// The class name (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        match self.kind {
            VolumeKind::Base => "SmVolume",
            VolumeKind::Transform(_) => "SmTransform",
            VolumeKind::BSpline(_) => "SmBSplineVolume",
            VolumeKind::Bend(_) => "SmBendVolume",
            VolumeKind::Unbend(_) => "SmUnbendVolume",
            VolumeKind::Twist(_) => "SmTwistVolume",
        }
    }

    /// True when this volume is a `t` (`IsKindOf`): its own class or a base class.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == self.type_code() || is_kind_of(t)
    }

    // ---- equality -------------------------------------------------------------------

    /// `operator==` (virtual): the base comparison (same class, equal next maps,
    /// equal orient maps) and, for a transform, the matrices within `SM_IS_ZERO`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the unported kinds' comparisons.
    pub fn approx_eq(&self, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }
        match &self.kind {
            VolumeKind::Base => self.base_eq(other),
            VolumeKind::Transform(t) => {
                // first check the base
                if !self.base_eq(other)? {
                    return Ok(false);
                }
                // check equivalence of these objects
                Ok(match &other.kind {
                    VolumeKind::Transform(o) => t.approx_eq(o),
                    _ => false,
                })
            }
            VolumeKind::BSpline(b) => {
                // first check the base
                if !self.base_eq(other)? {
                    return Ok(false);
                }
                // distinct volumes never share their NURBS records
                Ok(match &other.kind {
                    VolumeKind::BSpline(o) => b.approx_eq(o),
                    _ => false,
                })
            }
            VolumeKind::Twist(t) => {
                // low work - different types are not equal
                let VolumeKind::Twist(o) = &other.kind else {
                    return Ok(false);
                };
                // first check the base, then the members
                Ok(self.base_eq(other)? && t.members_eq(o))
            }
            VolumeKind::Unbend(u) => {
                // low work - different types are not equal
                let VolumeKind::Unbend(o) = &other.kind else {
                    return Ok(false);
                };
                // first check the base, then the members
                Ok(self.base_eq(other)? && u.members_eq(o))
            }
            VolumeKind::Bend(b) => {
                // low work - different types are not equal
                let VolumeKind::Bend(o) = &other.kind else {
                    return Ok(false);
                };
                // first check the base, then the members
                Ok(self.base_eq(other)? && b.members_eq(o))
            }
        }
    }

    /// `SmVolume::operator==`: same class, same next maps and same orient maps.
    fn base_eq(&self, other: &Self) -> Result<bool> {
        // same derived types
        if self.type_code() != other.type_code() {
            return Ok(false);
        }
        // and same next maps
        let same_next = match (&self.next_map, &other.next_map) {
            (None, None) => true,
            (Some(a), Some(b)) => a.approx_eq(b)?,
            _ => false,
        };
        if !same_next {
            return Ok(false);
        }
        // and same orient maps (simple transforms: the base test holds)
        Ok(match (&self.orient_map, &other.orient_map) {
            (None, None) => true,
            (Some(a), Some(b)) => a.approx_eq(b),
            _ => false,
        })
    }

    // ---- compounding ----------------------------------------------------------------

    /// Appends `next` at the end of the next-map chain with owner flag
    /// `owner_flag` (`AddNextMap`): `0` shared, `1` a copy of `next` is stored, `2`
    /// owned. When the copy fails the chain is left without the map. Upstream
    /// ignores the status of the recursion into an existing next map.
    ///
    /// # Errors
    ///
    /// The failed `Copy` of `next` when it is added here; `InvalidInput` (nothing
    /// is added) when the chain would hold more than
    /// [`MAX_GEOMETRY_NESTING`] next maps.
    pub fn add_next_map(&mut self, next: Self, owner_flag: u64) -> Result<()> {
        // (port) bound the chain: `next` lands below every map already in it
        let mut depth = 1;
        let mut map = self.next_map.as_deref();
        while let Some(m) = map {
            depth += 1;
            map = m.next_map.as_deref();
        }
        check_geometry_nesting(NestedGeometry::Volume(&next), depth, OP_ADD_NEXT_MAP)?;
        // branch on existence of m_pNextMap
        match self.next_map.as_deref_mut() {
            None => {
                // add transformation here!
                self.next_owner_flag = owner_flag;
                if owner_flag == 1 {
                    // when asked - copy the input
                    self.next_map = Some(Box::new(next.copy(false)?));
                } else {
                    self.next_map = Some(Box::new(next));
                }
                Ok(())
            }
            // This volume is already compounded: compound the m_pNextMap Volume
            Some(map) => ignore_status(map.add_next_map(next, owner_flag)),
        }
    }

    /// Rebuilds the inverse orient map from the orient map (`RefreshInvOrientMap`);
    /// a singular orient map's inverse is a plain copy (`InvertSimple` fails and is
    /// ignored).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without an orient map (upstream then calls
    /// `MakeMapSimple` through a null pointer).
    pub fn refresh_inv_orient_map(&mut self) -> Result<()> {
        // clear existing InvOrientMap
        self.inv_orient_map = None;
        let Some(orient) = self.orient_map else {
            return Err(SolidError::invalid_input(
                "surface::volume::p1::Volume::refresh_inv_orient_map",
                "no orient map (upstream dereferences a null pointer)",
            ));
        };
        // Copy m_pOrientMap into m_pInvOrientMap and invert it
        let mut inv = orient;
        let _ = inv.invert_simple();
        self.inv_orient_map = Some(inv);
        // both maps are simple, unoriented transforms already
        Ok(())
    }

    /// Drops the next map and the orient maps (`MakeMapSimple`).
    pub fn make_map_simple(&mut self) {
        self.next_map = None;
        self.orient_map = None;
        self.inv_orient_map = None;
    }

    /// True when this map or any compounded map is bounded (`IsBounded`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind.
    pub fn is_bounded(&self) -> Result<bool> {
        Ok(self.is_bounded_simple()?
            || match &self.next_map {
                Some(next) => next.is_bounded()?,
                None => false,
            })
    }

    // ---- domains --------------------------------------------------------------------

    /// The parameter domain of the simple map (`GetNaturalParamDomain`, virtual).
    /// The base class reports an error and returns the zero box.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the unported kinds.
    pub fn natural_param_domain(&self) -> Result<Extent3d> {
        match &self.kind {
            VolumeKind::Base => {
                let zero = Vec3::new(0.0, 0.0, 0.0);
                Ok(Extent3d::from_points(zero, zero))
            }
            VolumeKind::Transform(t) => Ok(t.natural_param_domain()),
            VolumeKind::BSpline(b) => Ok(b.natural_param_domain()),
            VolumeKind::Twist(t) => Ok(t.natural_param_domain()),
            VolumeKind::Unbend(u) => Ok(u.natural_param_domain()),
            VolumeKind::Bend(b) => Ok(b.natural_param_domain()),
        }
    }

    /// Whether the in-space point `in_point` maps through the inverse orient map
    /// into the parameter domain (`IsPointInInSpaceDomain`), and that parameter
    /// point. `with_compounding` is unused upstream.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind.
    pub fn is_point_in_in_space_domain(
        &self,
        in_point: Point3,
        with_compounding: bool,
    ) -> Result<(bool, Point3)> {
        let _ = with_compounding;
        // everything can map through the OrientMap
        let param_point = match &self.inv_orient_map {
            Some(inv) => ignored(transform_point(inv, in_point))?.unwrap_or(Vec3::UNDEF),
            None => in_point,
        };
        // pass the check along
        let is_in = self.is_point_in_param_domain_simple(param_point)?;
        Ok((is_in, param_point))
    }

    /// Whether the parameter line segment `start`-`end` lies in the parameter domain
    /// and, with `with_compounding`, its image in every next map's domain
    /// (`IsLineInParamDomain`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind.
    pub fn is_line_in_param_domain(
        &self,
        k: &Kernel,
        start: Point3,
        end: Point3,
        with_compounding: bool,
    ) -> Result<bool> {
        // pass the call along
        let mut is_in = self.is_line_in_param_domain_simple(start, end)?;

        // when asked - check compounded volumes as well
        if let (true, true, Some(next)) = (is_in, with_compounding, self.next_map.as_deref()) {
            // map Param points to InSpace
            let (start_in, end_in) = if self.orient_map.is_some() {
                (
                    ignored(self.orient_point(start))?.unwrap_or(Vec3::UNDEF),
                    ignored(self.orient_point(end))?.unwrap_or(Vec3::UNDEF),
                )
            } else {
                (start, end)
            };

            // build curve in OutSpace = NextInSpace: a plain SmVolume copy of the
            // simple map (the slicing copy constructor)
            let mut simple = Self::copy_from(self, true)?;
            simple.kind = VolumeKind::Base;
            let line = Line::from_points(start_in, end_in, 3);
            let next_in_space_curve =
                Curve::from(CrvInVolume::new(Curve::from(line), false, simple, 0, 0)?);

            // Check the NextInSpaceCurve
            is_in &= next.is_curve_in_in_space_domain(k, &next_in_space_curve, true)?;
        }
        Ok(is_in)
    }

    /// Whether the parameter-space curve lies in the parameter domain and, with
    /// `with_compounding`, its image in every next map's domain
    /// (`IsCurveInParamDomain`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind; [`SolidError::InvalidInput`]
    /// when the simple-map copy fails (upstream dereferences the null copy).
    pub fn is_curve_in_param_domain(
        &self,
        k: &Kernel,
        curve: &Curve,
        with_compounding: bool,
    ) -> Result<bool> {
        // pass the call along
        let mut is_in = self.is_curve_in_param_domain_simple(k, curve)?;

        // when asked - check compounded volumes as well
        if let (true, true, Some(next)) = (is_in, with_compounding, self.next_map.as_deref()) {
            // Copy this Volume omitting any compounding volumes
            let simple = match self.copy(true) {
                Ok(v) => v,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {
                    return Err(SolidError::invalid_input(
                        "surface::volume::p1::Volume::is_curve_in_param_domain",
                        "simple map copy failed (upstream dereferences a null volume)",
                    ))
                }
            };
            // project Curve to 1st OutSpace = Next InSpace
            let out_space_curve = Curve::from(CrvInVolume::new(curve.clone(), true, simple, 0, 0)?);
            // Check the NextInSpaceCurve
            is_in &= next.is_curve_in_in_space_domain(k, &out_space_curve, true)?;
        }
        Ok(is_in)
    }

    /// Whether the in-space curve maps through the inverse orient map into the
    /// parameter domain, as [`Volume::is_curve_in_param_domain`]
    /// (`IsCurveInInSpaceDomain`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::is_curve_in_param_domain`].
    pub fn is_curve_in_in_space_domain(
        &self,
        k: &Kernel,
        curve: &Curve,
        with_compounding: bool,
    ) -> Result<bool> {
        match &self.inv_orient_map {
            // project InSpaceCurve to Param space
            Some(inv) => {
                let param_space_curve = Curve::from(CrvInVolume::new(
                    curve.clone(),
                    true,
                    Self::new(VolumeKind::Transform(*inv)),
                    0,
                    0,
                )?);
                self.is_curve_in_param_domain(k, &param_space_curve, with_compounding)
            }
            // InSpaceCurve == ParamSpaceCurve
            None => self.is_curve_in_param_domain(k, curve, with_compounding),
        }
    }

    // ---- compound evaluation --------------------------------------------------------

    /// Out-space position and partial derivatives `0..=n` (`n <= 3`) of the
    /// parameter point, `D[u][v][w]` (`Evaluate`, virtual):
    /// `NextMap(Orient(EvaluateSimple(p)))`, chained with the chain rule.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `n > 3`; the simple maps' failures; [`SolidError::Unported`]
    /// from an unported kind.
    pub fn evaluate(&self, param_point: Point3, n: usize, opts: &EvalOptions) -> Result<Vec<Vec3>> {
        match &self.kind {
            VolumeKind::Twist(t) => return t.evaluate(self, param_point, n, opts),
            VolumeKind::Unbend(u) => return u.evaluate(self, param_point, n, opts),
            VolumeKind::Bend(b) => return b.evaluate(self, param_point, n, opts),
            _ => {}
        }
        // check input - number of supported derivatives is exceeded
        if n > MAX_DERIV {
            return Err(SolidError::sm_err(OP_EVAL));
        }
        let next_map = if opts.with_compounding {
            self.next_map.as_deref()
        } else {
            None
        };

        // Map InSpace point to 1st OutSpace evaluations
        let first = if let Some(orient) = &self.orient_map {
            // Map ParamPoint to ProjSpace
            let simple = self.evaluate_simple(param_point, n, opts)?;
            // Map ProjPoint to 1st OutSpace
            let oriented = orient.evaluate_simple(at(&simple, 0, OP_EVAL)?, n)?;
            // concatenate with the chain rule the individual evaluates
            concat_evals(n, &simple, &oriented)?
        } else {
            // no Orient map, InSpace == ParamSpace and ProjSpace == 1stOutSpace
            self.evaluate_simple(param_point, n, opts)?
        };

        // Map OutSpace Point through concatenated NextMaps
        match next_map {
            Some(next) => {
                let next_derivs = next.map(at(&first, 0, OP_EVAL)?, n, opts)?;
                // compound the evaluations with the chain rule
                concat_evals(n, &first, &next_derivs)
            }
            None => Ok(first),
        }
    }

    /// Out-space position and partial derivatives `0..=n` (`n <= 3`) of the
    /// in-space point (`Map`):
    /// `NextMap(Orient(EvaluateSimple(InvOrient(p))))`, chained with the chain rule.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `n > 3`; the simple maps' failures; [`SolidError::InvalidInput`]
    /// for an orient map without its inverse; [`SolidError::Unported`] from an
    /// unported kind.
    pub fn map(&self, in_point: Point3, n: usize, opts: &EvalOptions) -> Result<Vec<Vec3>> {
        // check input - number of supported derivatives is exceeded
        if n > MAX_DERIV {
            return Err(SolidError::sm_err(OP_MAP));
        }
        let next_map = if opts.with_compounding {
            self.next_map.as_deref()
        } else {
            None
        };

        // Map InSpace point to 1st OutSpace evaluations
        let first = if let Some(orient) = &self.orient_map {
            let inv = self
                .inv_orient_map
                .as_ref()
                .ok_or(SolidError::invalid_input(
                    OP_MAP,
                    "orient map without its inverse",
                ))?;
            // map crInSpacePoint to ParamSpace
            let inv_oriented = inv.evaluate_simple(in_point, n)?;
            // Map ParamPoint to ProjSpace
            let simple = self.evaluate_simple(at(&inv_oriented, 0, OP_MAP)?, n, opts)?;
            // Map ProjPoint to 1st OutSpace
            let oriented = orient.evaluate_simple(at(&simple, 0, OP_MAP)?, n)?;
            // concatenate with the chain rule the individual evaluates
            let mid1 = concat_evals(n, &inv_oriented, &simple)?;
            concat_evals(n, &mid1, &oriented)?
        } else {
            // no Orient map, InSpace == ParamSpace and ProjSpace == 1stOutSpace
            self.evaluate_simple(in_point, n, opts)?
        };

        // Map OutSpace Point through concatenated NextMaps
        match next_map {
            Some(next) => {
                let next_derivs = next.map(at(&first, 0, OP_MAP)?, n, opts)?;
                // compound the evaluations with the chain rule
                concat_evals(n, &first, &next_derivs)
            }
            None => Ok(first),
        }
    }

    /// The out-space image of the in-space point (`MapPoint`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::map`].
    pub fn map_point(&self, in_point: Point3, with_compounding: bool) -> Result<Point3> {
        let opts = EvalOptions {
            with_compounding,
            ..EvalOptions::default()
        };
        at(&self.map(in_point, 0, &opts)?, 0, OP_MAP)
    }

    /// In-space position and partial derivatives `0..=n` of the parameter point
    /// (`Orient`): the orient map's simple evaluation, else the identity
    /// (`n <= GW_MAX_DERIV`). Upstream ignores a failure of the orient map, which
    /// leaves the caller's array: [`Vec3::UNDEF`] here.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `n > GW_MAX_DERIV` without an orient map (and
    /// for an array too large to hand back).
    pub fn orient(
        &self,
        param_point: Point3,
        n: usize,
        from_left: [bool; 3],
        non_zero_tangents: bool,
        do_zero_sampling: bool,
    ) -> Result<Vec<Vec3>> {
        // the transform's simple evaluation has no use for these
        let _ = (from_left, non_zero_tangents, do_zero_sampling);
        if let Some(orient) = &self.orient_map {
            // return OrientMap's simple evaluation (status ignored)
            return match orient.evaluate_simple(param_point, n) {
                Ok(d) => Ok(d),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => {
                    let size = eval_size(n, OP_ORIENT)?;
                    if size > MAX_UNDEF_ARRAY {
                        return Err(SolidError::invalid_input(
                            OP_ORIENT,
                            "derivative order too large",
                        ));
                    }
                    Ok(vec![Vec3::UNDEF; size])
                }
            };
        }
        // without OrientMap, ParamSpace == InSpace, evaluate unity mapping
        if n > GW_MAX_DERIV {
            return Err(SolidError::invalid_input(
                OP_ORIENT,
                "highest derivative exceeds GW_MAX_DERIV",
            ));
        }
        let n1 = n + 1;
        let sv = |u: usize, v: usize, w: usize| (u * n1 + v) * n1 + w;
        // init output array: position, all higher derivatives zero
        let mut d = vec![Vec3::new(0.0, 0.0, 0.0); n1 * n1 * n1];
        if let Some(p) = d.get_mut(0) {
            *p = param_point;
        }
        // 1st derivs are unit axis vectors
        if n >= 1 {
            for (idx, axis) in [
                (sv(1, 0, 0), Vec3::new(1.0, 0.0, 0.0)),
                (sv(0, 1, 0), Vec3::new(0.0, 1.0, 0.0)),
                (sv(0, 0, 1), Vec3::new(0.0, 0.0, 1.0)),
            ] {
                if let Some(slot) = d.get_mut(idx) {
                    *slot = axis;
                }
            }
        }
        Ok(d)
    }

    /// The in-space image of the parameter point (`OrientPoint`).
    ///
    /// # Errors
    ///
    /// The orient map's evaluation failure.
    pub fn orient_point(&self, param_point: Point3) -> Result<Point3> {
        match &self.orient_map {
            // return OrientMap's simple evaluation
            Some(orient) => transform_point(orient, param_point),
            // return Unity mapping
            None => Ok(param_point),
        }
    }

    /// The in-space first directional derivative along `param_dir` (unitized)
    /// (`OrientVector`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::orient_directional_derivs`].
    pub fn orient_vector(
        &self,
        param_point: Point3,
        param_dir: Vec3,
        from_left: [bool; 3],
    ) -> Result<Vec3> {
        let d = self.orient_directional_derivs(param_point, param_dir, 1, from_left)?;
        at(&d, 1, OP_ORIENT)
    }

    /// In-space position and directional derivatives `1..=n` (`n <= 3`) along
    /// `param_dir` (unitized; a zero direction becomes `(0, 0, 1)`)
    /// (`OrientDirectionalDerivs`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `n > 3`; those of [`Volume::orient`].
    pub fn orient_directional_derivs(
        &self,
        param_point: Point3,
        param_dir: Vec3,
        n: usize,
        from_left: [bool; 3],
    ) -> Result<Vec<Vec3>> {
        // check state
        if n > MAX_DERIV {
            return Err(SolidError::sm_err(
                "surface::volume::p1::Volume::orient_directional_derivs",
            ));
        }
        // make the evaluation: exact tangent values, zero sampling
        let derivs = self.orient(param_point, n, from_left, false, true)?;
        // pass the call along
        vol_directional_derivs(param_dir, n, &derivs)
    }

    /// Applies the out-space rigid motion `rotate_n_move` and then the optional
    /// `scale` to the volume's final out-space image (`Transform`): built into the
    /// last map of the chain (or this map) by its `TransformSimple`. An identity
    /// motion with no (or unit) scale does nothing.
    ///
    /// # Errors
    ///
    /// Those of [`Volume::transform_simple`]. Appending a map is refused when it
    /// would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`]
    /// (including the enclosing curve/surface).
    pub fn transform(&mut self, rotate_n_move: &Axis2Placement, scale: Option<Vec3>) -> Result<()> {
        self.transform_at_depth(rotate_n_move, scale, 0)
    }

    /// Transform with this volume's depth in the enclosing geometry graph.
    pub(crate) fn transform_at_depth(
        &mut self,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        mut depth: usize,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // Build into the terminal map, carrying its actual geometry depth. Only
        // the fallback that appends a map consumes another level.
        let mut last = self;
        loop {
            match last {
                Self {
                    next_map: Some(next),
                    ..
                } => {
                    last = next;
                    depth += 1;
                }
                terminal => {
                    return terminal.transform_simple_at_depth(rotate_n_move, scale, depth);
                }
            }
        }
    }

    /// Mirrors the volume's final out-space image about the XY plane of
    /// `out_mirror_plane` (`Mirror`): built into the last map of the chain (or this
    /// map) by its `MirrorSimple`.
    ///
    /// # Errors
    ///
    /// Those of [`Volume::mirror_simple`].
    pub fn mirror(&mut self, out_mirror_plane: &Axis2Placement) -> Result<()> {
        let pt = out_mirror_plane.origin();
        let normal = out_mirror_plane.z_axis();
        // save on m_pNextMap linked list length - try to build the Mirror into the
        // last NextMap
        if let Some(last) = self.last_map_mut() {
            return last.mirror_simple(pt, normal);
        }
        // build the Mirror into this map
        self.mirror_simple(pt, normal)
    }

    /// Builds the out-space mirror about the plane through `plane_pt` with normal
    /// `plane_normal` into the simple map (`MirrorSimple`, virtual): a transform
    /// concatenates it; a B-spline volume transforms its control points
    /// ([`BSplineVolume::mirror_simple`]); the base class appends a copy of an
    /// `SmTransform` holding it to the next maps.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline volume; the error of [`Vec3::unitize`] for a zero
    /// normal.
    pub fn mirror_simple(&mut self, plane_pt: Point3, plane_normal: Vec3) -> Result<()> {
        match &mut self.kind {
            VolumeKind::Transform(t) => return t.mirror_simple(plane_pt, plane_normal),
            VolumeKind::BSpline(b) => {
                return b.mirror_simple(
                    self.orient_map.as_ref(),
                    self.inv_orient_map.as_ref(),
                    plane_pt,
                    plane_normal,
                );
            }
            _ => {}
        }
        // Create a Mirror SmTransform to compound with current mapping
        let mut transform = Transform::IDENTITY; // identity map with no Orient map
        transform.mirror_simple(plane_pt, plane_normal)?;
        // Add mirror map to end of compound map list (a copy: owner flag 1)
        self.add_next_map(Self::new(VolumeKind::Transform(transform)), 1)
    }

    // ---- simple maps ----------------------------------------------------------------

    /// Projection-space position and partial derivatives `0..=n` of the parameter
    /// point, without orientation or compounding (`EvaluateSimple`, virtual).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a plain `SmVolume`; the transform's failure for `n` above
    /// `GW_MAX_DERIV`; [`SolidError::Unported`] for the unported kinds.
    pub fn evaluate_simple(
        &self,
        param_point: Point3,
        n: usize,
        opts: &EvalOptions,
    ) -> Result<Vec<Vec3>> {
        match &self.kind {
            VolumeKind::Base => Err(SolidError::sm_err(
                "surface::volume::p1::Volume::evaluate_simple",
            )),
            VolumeKind::Transform(t) => t.evaluate_simple(param_point, n),
            VolumeKind::BSpline(b) => b.evaluate_simple(param_point, n, opts),
            VolumeKind::Twist(t) => t.evaluate_simple(param_point, n),
            VolumeKind::Unbend(u) => u.evaluate_simple(param_point, n),
            VolumeKind::Bend(b) => b.evaluate_simple(param_point, n),
        }
    }

    /// The projection-space position of the parameter point
    /// (`EvaluatePointSimple`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::evaluate_simple`].
    pub fn evaluate_point_simple(&self, param_point: Point3) -> Result<Point3> {
        let d = self.evaluate_simple(param_point, 0, &EvalOptions::default())?;
        at(&d, 0, OP)
    }

    /// Projection-space position and first partials `[P, Du, Dv, Dw]` of the
    /// parameter point (`Evaluate1stDerivativesSimple`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::evaluate_simple`].
    pub fn evaluate_1st_derivatives_simple(
        &self,
        param_point: Point3,
        from_left: [bool; 3],
        non_zero_tangents: bool,
    ) -> Result<[Vec3; 4]> {
        let opts = EvalOptions {
            from_left,
            non_zero_tangents,
            ..EvalOptions::default()
        };
        // sMat[2][2][2]
        let m = self.evaluate_simple(param_point, 1, &opts)?;
        Ok([
            at(&m, 0, OP)?,
            at(&m, 4, OP)?,
            at(&m, 2, OP)?,
            at(&m, 1, OP)?,
        ])
    }

    /// Parameter-space start points for inverting the projection-space point
    /// (`InvEvaluateGuessPointSimple`, virtual).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a plain `SmVolume` and a singular transform;
    /// [`SolidError::Unported`] for the unported kinds.
    pub fn inv_evaluate_guess_point_simple(&self, proj_point: Point3) -> Result<Vec<Point3>> {
        match &self.kind {
            VolumeKind::Base => Err(SolidError::sm_err(
                "surface::volume::p1::Volume::inv_evaluate_guess_point_simple",
            )),
            VolumeKind::Transform(t) => t.inv_evaluate_guess_point_simple(proj_point),
            VolumeKind::BSpline(b) => b.inv_evaluate_guess_point_simple(proj_point),
            VolumeKind::Twist(t) => Ok(t.inv_evaluate_guess_point_simple(proj_point)),
            VolumeKind::Unbend(u) => Ok(u.inv_evaluate_guess_point_simple(proj_point)),
            VolumeKind::Bend(b) => match b.inv_evaluate_guess_point_simple(proj_point) {
                (points, true) => Ok(points),
                // the guesses written with SM_ERR are dropped (callers propagate it)
                (_, false) => Err(SolidError::sm_err(
                    "surface::bend_volume::BendVolume::inv_evaluate_guess_point_simple",
                )),
            },
        }
    }

    /// Whether the simple map's parameter space is bounded (`IsBoundedSimple`,
    /// virtual; `false` for the base class and transforms).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the unported kinds.
    pub fn is_bounded_simple(&self) -> Result<bool> {
        match &self.kind {
            VolumeKind::Base => Ok(false),
            VolumeKind::Transform(t) => Ok(t.is_bounded_simple()),
            VolumeKind::BSpline(b) => Ok(b.is_bounded_simple()),
            VolumeKind::Twist(t) => Ok(t.is_bounded_simple()),
            VolumeKind::Unbend(u) => Ok(u.is_bounded_simple()),
            VolumeKind::Bend(b) => Ok(b.is_bounded_simple()),
        }
    }

    /// Whether the parameter point is in the natural domain (always, unbounded)
    /// (`IsPointInParamDomainSimple`, virtual).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the unported kinds.
    pub fn is_point_in_param_domain_simple(&self, param_point: Point3) -> Result<bool> {
        match &self.kind {
            VolumeKind::Twist(t) => Ok(t.is_point_in_param_domain_simple()),
            VolumeKind::Unbend(u) => Ok(u.is_point_in_param_domain_simple()),
            VolumeKind::Bend(b) => Ok(b.is_point_in_param_domain_simple(param_point)),
            _ => {
                if self.is_bounded_simple()? {
                    return Ok(self
                        .natural_param_domain()?
                        .contains_point(param_point, 0.0));
                }
                Ok(true)
            }
        }
    }

    /// Whether the parameter segment is in the natural domain (always, unbounded;
    /// upstream's `ContainsLineSeg3d` tests either end) (`IsLineInParamDomainSimple`,
    /// virtual).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the unported kinds.
    pub fn is_line_in_param_domain_simple(&self, start: Point3, end: Point3) -> Result<bool> {
        match &self.kind {
            VolumeKind::Twist(t) => Ok(t.is_line_in_param_domain_simple()),
            VolumeKind::Unbend(u) => Ok(u.is_line_in_param_domain_simple(start, end)),
            VolumeKind::Bend(b) => Ok(b.is_line_in_param_domain_simple(start, end)),
            _ => {
                if self.is_bounded_simple()? {
                    return Ok(self
                        .natural_param_domain()?
                        .contains_line_seg(start, end, 0.0));
                }
                Ok(true)
            }
        }
    }

    /// Whether the parameter curve is in the natural domain
    /// (`IsCurveInParamDomainSimple`, virtual): an error report and `false` for the
    /// base class, `true` for a transform (which ignores the curve), the box and
    /// plane tests of [`BSplineVolume::is_curve_in_param_domain_simple`] for a
    /// B-spline volume (which needs `k` for the curve's boxes).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the unported kinds and from the B-spline
    /// override's callees.
    pub fn is_curve_in_param_domain_simple(&self, k: &Kernel, curve: &Curve) -> Result<bool> {
        match &self.kind {
            VolumeKind::Base => Ok(false),
            VolumeKind::Transform(t) => Ok(t.is_curve_in_param_domain_simple()),
            VolumeKind::BSpline(b) => b.is_curve_in_param_domain_simple(k, curve),
            VolumeKind::Twist(t) => Ok(t.is_curve_in_param_domain_simple()),
            VolumeKind::Unbend(u) => u.is_curve_in_param_domain_simple(k, curve),
            VolumeKind::Bend(b) => b.is_curve_in_param_domain_simple(k, curve),
        }
    }

    /// Trims `param_box` to the natural parameter domain into `trim_box`
    /// (`TrimParamBoundingBoxSimple`, virtual): the overlap when the compound map
    /// is bounded (left unchanged when they are disjoint), else a copy.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for the unported kinds.
    pub fn trim_param_bounding_box_simple(
        &self,
        param_box: &Extent3d,
        trim_box: &mut Extent3d,
    ) -> Result<()> {
        match &self.kind {
            VolumeKind::Unbend(u) => {
                *trim_box = u.trim_param_bounding_box_simple(param_box);
                Ok(())
            }
            VolumeKind::Bend(b) => b.trim_param_bounding_box_simple(param_box, trim_box),
            _ => {
                // for bounded Natural Param Domains
                if self.is_bounded()? {
                    let domain = self.natural_param_domain()?;
                    // TrimBox = XSect(ParamBox, NaturalDomainBox), status ignored
                    if let Ok(b) = param_box.intersect(&domain) {
                        *trim_box = b;
                    }
                } else {
                    // Unbounded Natural Param Domain - let TrimBox = ParamBox
                    *trim_box = *param_box;
                }
                Ok(())
            }
        }
    }

    /// Builds the out-space motion and scale into the simple map
    /// (`TransformSimple`, virtual): a transform concatenates them; the base class
    /// appends a copy of an `SmTransform` holding them to the next maps.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a B-spline volume (its control points cannot be transformed
    /// without `USE_VOLUMES`; [`BSplineVolume::transform_simple`]).
    /// [`SolidError::InvalidInput`] if appending would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`].
    pub fn transform_simple(
        &mut self,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_simple_at_depth(rotate_n_move, scale, 0)
    }

    fn transform_simple_at_depth(
        &mut self,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        mut depth: usize,
    ) -> Result<()> {
        match &mut self.kind {
            VolumeKind::Transform(t) => {
                t.transform_simple(rotate_n_move, scale);
                return Ok(());
            }
            VolumeKind::BSpline(b) => {
                return b.transform_simple(
                    self.orient_map.as_ref(),
                    self.inv_orient_map.as_ref(),
                    rotate_n_move,
                    scale,
                );
            }
            _ => {}
        }
        // Check at the actual growing branch, after the native Transform and
        // BSpline overrides. TransformSimple can also be called on a compounded
        // non-terminal map, whose append still goes at the end of the chain.
        let mut last = &*self;
        while let Some(next) = last.next_map() {
            last = next;
            depth += 1;
        }
        if depth >= MAX_GEOMETRY_NESTING {
            return Err(SolidError::invalid_input(
                "surface::volume::p1::Volume::transform_simple",
                "owned geometry nesting exceeds MAX_GEOMETRY_NESTING",
            ));
        }
        // Create a Transform SmTransform to compound with current mapping
        let mut transform = Transform::IDENTITY;
        transform.transform_simple(rotate_n_move, scale);
        // Add map to end of compound map list (a copy: owner flag 1)
        self.add_next_map(Self::new(VolumeKind::Transform(transform)), 1)
    }

    /// Whether the simple map has internal discontinuities
    /// (`HasDiscontinuitiesSimple`, virtual): never for the base class and
    /// transforms, which reset `disconts` to "none"; a B-spline volume reports its
    /// knot planes ([`BSplineVolume::has_discontinuities_simple`]).
    /// `calc_geometric` is `bCalcGeometric` (upstream default `TRUE`; only the
    /// B-spline override reads it).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the B-spline override's callees.
    pub fn has_discontinuities_simple(
        &self,
        k: &Kernel,
        disconts: Option<&mut Discontinuities3d>,
        calc_geometric: bool,
    ) -> Result<bool> {
        match &self.kind {
            VolumeKind::Transform(t) => Ok(t.has_discontinuities_simple(disconts)),
            VolumeKind::BSpline(b) => b.has_discontinuities_simple(k, disconts, calc_geometric),
            VolumeKind::Base
            | VolumeKind::Bend(_)
            | VolumeKind::Unbend(_)
            | VolumeKind::Twist(_) => {
                // set output to be unbounded and continuous
                if let Some(d) = disconts {
                    d.init();
                }
                Ok(false)
            }
        }
    }

    /// The continuities of the simple map across its parameter planes in direction
    /// `param` into `out` (`CalculateContinuitiesSimple`, virtual): none for every
    /// kind but a B-spline volume ([`BSplineVolume::calculate_continuities_simple`]),
    /// whose outputs are written even when it fails. `calc_geometric` is
    /// `bCalcGeometric` (upstream default `TRUE`).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineVolume::calculate_continuities_simple`].
    pub fn calculate_continuities_simple(
        &self,
        k: &Kernel,
        param: VolumeParam,
        out: &mut DirectionDiscontinuities,
        calc_geometric: bool,
    ) -> Result<()> {
        if let VolumeKind::BSpline(b) = &self.kind {
            return b.calculate_continuities_simple(k, param, out, calc_geometric);
        }
        out.min_cont = ContinuityType::CInfinity;
        out.params.clear();
        out.conts.clear();
        Ok(())
    }
}

/// `SmVolume::IsKindOf`: `SmVolume_TYPE` or a base class's type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == VOLUME_TYPE || a_object::is_kind_of(t)
}

/// `SmVolume::Notify`: no volume-specific work (`SM_NO_UNKNOWN` only reports an
/// error upstream); the notification goes on to `SmAObject::Notify`.
///
/// # Errors
///
/// Those of `SmAObject::Notify`.
pub fn notify(
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
) -> Result<()> {
    // Propagate notification up hierarchy
    a_object::notify(attributes, context, operation, this, data1, data2, data3)
}

// ---- inverse point solving ----------------------------------------------------------

/// The Newton-Raphson equations of a projection-space point on a volume
/// (`SmFindPVIntersectENFO`): `F = EvaluateSimple(u, v, w) - P`, Jacobian columns
/// the first partials `Du`, `Dv`, `Dw`.
#[derive(Debug, Clone, Copy)]
pub struct FindPvIntersect<'a> {
    /// `m_crProjSpacePoint`: the target point.
    proj_space_point: Point3,
    /// `m_crVolume`: the volume searched.
    volume: &'a Volume,
    /// `m_dSquaredTolerance`: the squared convergence gap (scaled).
    squared_tolerance: f64,
}

impl<'a> FindPvIntersect<'a> {
    /// The equations for `proj_space_point` on `volume` (the constructor).
    #[must_use]
    pub const fn new(proj_space_point: Point3, volume: &'a Volume, squared_tolerance: f64) -> Self {
        Self {
            proj_space_point,
            volume,
            squared_tolerance,
        }
    }
}

impl EvalNFunctions for FindPvIntersect<'_> {
    /// `Evaluate`: the gap and the Jacobian at `x = [u v w]`; `true` once the gap
    /// is below the tolerance scaled by `1 + max(|P|, |V|)`.
    ///
    /// Errors: `SM_ERR` when a first partial is (nearly) zero; the volume's
    /// evaluation failure; `InvalidInput` for `x` or `f` shorter than 3.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let short =
            || SolidError::invalid_input(OP_PV, "parameter or function array shorter than 3");
        let (Some(&u), Some(&v), Some(&w)) = (x.first(), x.get(1), x.get(2)) else {
            return Err(short());
        };
        if f.len() < 3 {
            return Err(short());
        }

        // get Volume evaluations
        let param_space_point = Vec3::new(u, v, w);
        let [proj_space_point, du, dv, dw] =
            self.volume
                .evaluate_1st_derivatives_simple(param_space_point, [true; 3], true)?;

        // failure - zero length volume tangents
        if du.length_squared() < EFF_ZERO_SQ
            || dv.length_squared() < EFF_ZERO_SQ
            || dw.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP_PV));
        }

        // get gap vector
        let gap = proj_space_point - self.proj_space_point;
        let gap_length_squared = gap.length_squared();

        if let Some(jac) = jacobian {
            jac.set_at(0, 0, du.x)?;
            jac.set_at(0, 1, dv.x)?;
            jac.set_at(0, 2, dw.x)?;

            jac.set_at(1, 0, du.y)?;
            jac.set_at(1, 1, dv.y)?;
            jac.set_at(1, 2, dw.y)?;

            jac.set_at(2, 0, du.z)?;
            jac.set_at(2, 1, dv.z)?;
            jac.set_at(2, 2, dw.z)?;
        }

        // Compute function values
        for (slot, value) in f.iter_mut().zip([gap.x, gap.y, gap.z]) {
            *slot = value;
        }

        // convergence criteria
        let s_point_max = proj_space_point.max_dimension();
        let cr_point_max = self.proj_space_point.max_dimension();
        let scale = 1.0 + max2(s_point_max, cr_point_max);
        Ok(gap_length_squared < self.squared_tolerance * scale * scale)
    }
}

// ---- chain-rule lifts ---------------------------------------------------------------

/// `smvol_LiftFirstDerivative` (curve form): the first derivative of `D(C(s))`
/// from the curve's first derivative `cs` and the volume's first partials
/// `[Du, Dv, Dw]`.
#[must_use]
pub fn lift_first_derivative(cs: Vec3, d1: [Vec3; 3]) -> Vec3 {
    let [du, dv, dw] = d1;
    cs.x * du + cs.y * dv + cs.z * dw
}

/// `smvol_LiftSecondDerivative` (curve form): from `[C', C'']`, the first partials
/// and the second partials `[Duu, Duv, Duw, Dvv, Dvw, Dww]`.
#[must_use]
pub fn lift_second_derivative(c: [Vec3; 2], d1: [Vec3; 3], d2: [Vec3; 6]) -> Vec3 {
    let [cs, css] = c;
    let [du, dv, dw] = d1;
    let [duu, duv, duw, dvv, dvw, dww] = d2;
    cs.x * cs.x * duu
        + cs.y * cs.y * dvv
        + cs.z * cs.z * dww
        + 2.0 * cs.x * cs.y * duv
        + 2.0 * cs.x * cs.z * duw
        + 2.0 * cs.y * cs.z * dvw
        + css.x * du
        + css.y * dv
        + css.z * dw
}

/// `smvol_LiftThirdDerivative` (curve form): from `[C', C'', C''']`, the first and
/// second partials and the third partials `[Duuu, Duuv, Duuw, Duvv, Duvw, Duww,
/// Dvvv, Dvvw, Dvww, Dwww]`.
#[must_use]
pub fn lift_third_derivative(c: [Vec3; 3], d1: [Vec3; 3], d2: [Vec3; 6], d3: [Vec3; 10]) -> Vec3 {
    let [cs, css, csss] = c;
    let [du, dv, dw] = d1;
    let [duu, duv, duw, dvv, dvw, dww] = d2;
    let [duuu, duuv, duuw, duvv, duvw, duww, dvvv, dvvw, dvww, dwww] = d3;
    let mut ret = csss.x * du + csss.y * dv + csss.z * dw;

    ret += 3.0 * (css.x * cs.x) * duu
        + 3.0 * (css.x * cs.y + css.y * cs.x) * duv
        + 3.0 * (css.x * cs.z + css.z * cs.x) * duw
        + 3.0 * (css.y * cs.y) * dvv
        + 3.0 * (css.y * cs.z + css.z * cs.y) * dvw
        + 3.0 * (css.z * cs.z) * dww;

    ret += cs.x * cs.x * cs.x * duuu
        + 3.0 * cs.x * cs.x * cs.y * duuv
        + 3.0 * cs.x * cs.x * cs.z * duuw
        + 3.0 * cs.x * cs.y * cs.y * duvv
        + 6.0 * cs.x * cs.y * cs.z * duvw
        + 3.0 * cs.x * cs.z * cs.z * duww
        + cs.y * cs.y * cs.y * dvvv
        + 3.0 * cs.y * cs.y * cs.z * dvvw
        + 3.0 * cs.y * cs.z * cs.z * dvww
        + cs.z * cs.z * cs.z * dwww;

    ret
}

/// `smvol_LiftFirstDerivative` (surface form): the first partials `[Du, Dv]` of
/// `D(S(u, v))` from the surface's `[Su, Sv]` and the volume's `[Dx, Dy, Dz]`.
#[must_use]
pub fn lift_first_derivative_srf(s1: [Vec3; 2], d1: [Vec3; 3]) -> [Vec3; 2] {
    let [su, sv] = s1;
    let [dx, dy, dz] = d1;
    let du = su.x * dx + su.y * dy + su.z * dz;

    let dv = sv.x * dx + sv.y * dy + sv.z * dz;
    [du, dv]
}

/// `smvol_LiftSecondDerivative` (surface form): `[Duu, Duv, Dvv]` from the
/// surface's `[Su, Sv]` and `[Suu, Suv, Svv]`, the volume's first partials and its
/// second partials `[Dxx, Dxy, Dxz, Dyy, Dyz, Dzz]`.
#[must_use]
pub fn lift_second_derivative_srf(
    s1: [Vec3; 2],
    s2: [Vec3; 3],
    d1: [Vec3; 3],
    d2: [Vec3; 6],
) -> [Vec3; 3] {
    let [su, sv] = s1;
    let [suu, suv, svv] = s2;
    let [dx, dy, dz] = d1;
    let [dxx, dxy, dxz, dyy, dyz, dzz] = d2;
    let duu = su.x * su.x * dxx
        + su.y * su.y * dyy
        + su.z * su.z * dzz
        + 2.0 * su.x * su.y * dxy
        + 2.0 * su.x * su.z * dxz
        + 2.0 * su.y * su.z * dyz
        + suu.x * dx
        + suu.y * dy
        + suu.z * dz;

    let duv = sv.x * su.x * dxx
        + sv.y * su.y * dyy
        + sv.z * su.z * dzz
        + (sv.x * su.y + sv.y * su.x) * dxy
        + (sv.x * su.z + sv.z * su.x) * dxz
        + (sv.y * su.z + sv.z * su.y) * dyz
        + suv.x * dx
        + suv.y * dy
        + suv.z * dz;

    let dvv = sv.x * sv.x * dxx
        + sv.y * sv.y * dyy
        + sv.z * sv.z * dzz
        + 2.0 * sv.x * sv.y * dxy
        + 2.0 * sv.x * sv.z * dxz
        + 2.0 * sv.y * sv.z * dyz
        + svv.x * dx
        + svv.y * dy
        + svv.z * dz;
    [duu, duv, dvv]
}

/// `smvol_LiftThirdDerivative` (surface form): `[Duuu, Duuv, Duvv, Dvvv]` from the
/// surface's first, second and third partials `[Suuu, Suuv, Suvv, Svvv]` and the
/// volume's first, second and third partials `[Dxxx, Dxxy, Dxxz, Dxyy, Dxyz, Dxzz,
/// Dyyy, Dyyz, Dyzz, Dzzz]` (upstream's `Dxxz` for `Dxzz` kept).
#[must_use]
pub fn lift_third_derivative_srf(
    s1: [Vec3; 2],
    s2: [Vec3; 3],
    s3: [Vec3; 4],
    d1: [Vec3; 3],
    d2: [Vec3; 6],
    d3: [Vec3; 10],
) -> [Vec3; 4] {
    let [su, sv] = s1;
    let [suu, suv, svv] = s2;
    let [suuu, suuv, suvv, svvv] = s3;
    let [dx, dy, dz] = d1;
    let [dxx, dxy, dxz, dyy, dyz, dzz] = d2;
    let [dxxx, dxxy, dxxz, dxyy, dxyz, dxzz, dyyy, dyyz, dyzz, dzzz] = d3;
    let duuu = suuu.x * dx
        + suuu.y * dy
        + suuu.z * dz
        + 3.0 * (suu.x * su.x) * dxx
        + 3.0 * (suu.x * su.y + suu.y * su.x) * dxy
        + 3.0 * (suu.x * su.z + suu.z * su.x) * dxz
        + 3.0 * (suu.y * su.y) * dyy
        + 3.0 * (suu.y * su.z + suu.z * su.y) * dyz
        + 3.0 * (suu.z * su.z) * dzz
        + su.x * su.x * su.x * dxxx
        + 3.0 * su.x * su.x * su.y * dxxy
        + 3.0 * su.x * su.x * su.z * dxxz
        + 3.0 * su.x * su.y * su.y * dxyy
        + 6.0 * su.x * su.y * su.z * dxyz
        + 3.0 * su.x * su.z * su.z * dxzz
        + su.y * su.y * su.y * dyyy
        + 3.0 * su.y * su.y * su.z * dyyz
        + 3.0 * su.y * su.z * su.z * dyzz
        + su.z * su.z * su.z * dzzz;

    let duuv = suuv.x * dx
        + suuv.y * dy
        + suuv.z * dz
        + (2.0 * suv.x * su.x + sv.x * suu.x) * dxx
        + (2.0 * suv.y * su.y + sv.y * suu.y) * dyy
        + (2.0 * suv.z * su.z + sv.z * suu.z) * dzz
        + (2.0 * suv.x * su.y + sv.y * suu.x + 2.0 * su.x * suv.y + sv.x * suu.y) * dxy
        + (2.0 * suv.x * su.z + sv.z * suu.x + 2.0 * su.x * suv.z + sv.x * suu.z) * dxz
        + (2.0 * suv.y * su.z + sv.z * suu.y + 2.0 * su.y * suv.z + sv.y * suu.z) * dyz
        + (sv.y * su.x * su.x + 2.0 * sv.x * su.x * su.y) * dxxy
        + (sv.z * su.x * su.x + 2.0 * sv.x * su.x * su.z) * dxxz
        + (sv.x * su.y * su.y + 2.0 * sv.y * su.x * su.y) * dxyy
        + (2.0 * sv.y * su.x * su.z + 2.0 * sv.z * su.x * su.y + 2.0 * sv.x * su.y * su.z) * dxyz
        + (sv.x * su.z * su.z + 2.0 * sv.z * su.x * su.z) * dxxz
        + (sv.z * su.y * su.y + 2.0 * sv.y * su.y * su.z) * dyyz
        + (sv.y * su.z * su.z + 2.0 * sv.z * su.y * su.z) * dyzz
        + (sv.x * su.x * su.x) * dxxx
        + (sv.y * su.y * su.y) * dyyy
        + (sv.z * su.z * su.z) * dzzz;

    let duvv = suvv.x * dx
        + suvv.y * dy
        + suvv.z * dz
        + (2.0 * suv.x * sv.x + su.x * svv.x) * dxx
        + (2.0 * suv.y * sv.y + su.y * svv.y) * dyy
        + (2.0 * suv.z * sv.z + su.z * svv.z) * dzz
        + (2.0 * suv.x * sv.y + su.y * svv.x + 2.0 * sv.x * suv.y + su.x * svv.y) * dxy
        + (2.0 * suv.x * sv.z + su.z * svv.x + 2.0 * sv.x * suv.z + su.x * svv.z) * dxz
        + (2.0 * suv.y * sv.z + su.z * svv.y + 2.0 * sv.y * suv.z + su.y * svv.z) * dyz
        + (su.y * sv.x * sv.x + 2.0 * su.x * sv.x * sv.y) * dxxy
        + (su.z * sv.x * sv.x + 2.0 * su.x * sv.x * sv.z) * dxxz
        + (su.x * sv.y * sv.y + 2.0 * su.y * sv.x * sv.y) * dxyy
        + (2.0 * su.y * sv.x * sv.z + 2.0 * su.z * sv.x * sv.y + 2.0 * su.x * sv.y * sv.z) * dxyz
        + (su.x * sv.z * sv.z + 2.0 * su.z * sv.x * sv.z) * dxxz
        + (su.z * sv.y * sv.y + 2.0 * su.y * sv.y * sv.z) * dyyz
        + (su.y * sv.z * sv.z + 2.0 * su.z * sv.y * sv.z) * dyzz
        + (su.x * sv.x * sv.x) * dxxx
        + (su.y * sv.y * sv.y) * dyyy
        + (su.z * sv.z * sv.z) * dzzz;

    let dvvv = svvv.x * dx
        + svvv.y * dy
        + svvv.z * dz
        + 3.0 * (svv.x * sv.x) * dxx
        + 3.0 * (svv.x * sv.y + svv.y * sv.x) * dxy
        + 3.0 * (svv.x * sv.z + svv.z * sv.x) * dxz
        + 3.0 * (svv.y * sv.y) * dyy
        + 3.0 * (svv.y * sv.z + svv.z * sv.y) * dyz
        + 3.0 * (svv.z * sv.z) * dzz
        + sv.x * sv.x * sv.x * dxxx
        + 3.0 * sv.x * sv.x * sv.y * dxxy
        + 3.0 * sv.x * sv.x * sv.z * dxxz
        + 3.0 * sv.x * sv.y * sv.y * dxyy
        + 6.0 * sv.x * sv.y * sv.z * dxyz
        + 3.0 * sv.x * sv.z * sv.z * dxzz
        + sv.y * sv.y * sv.y * dyyy
        + 3.0 * sv.y * sv.y * sv.z * dyyz
        + 3.0 * sv.y * sv.z * sv.z * dyzz
        + sv.z * sv.z * sv.z * dzzz;
    [duuu, duuv, duvv, dvvv]
}

/// `smvol_LiftFirstDerivative` (volume form): the first partials `[Du, Dv, Dw]` of
/// `D(F(u, v, w))` from the base volume's `[Fu, Fv, Fw]` and the compounding
/// volume's `[Dx, Dy, Dz]`.
#[must_use]
pub fn lift_first_derivative_vol(f1: [Vec3; 3], d1: [Vec3; 3]) -> [Vec3; 3] {
    let [fu, fv, fw] = f1;
    let [dx, dy, dz] = d1;
    let du = fu.x * dx + fu.y * dy + fu.z * dz;

    let dv = fv.x * dx + fv.y * dy + fv.z * dz;

    let dw = fw.x * dx + fw.y * dy + fw.z * dz;
    [du, dv, dw]
}

/// `smvol_LiftSecondDerivative` (volume form): `[Duu, Duv, Duw, Dvv, Dvw, Dww]` from
/// the base volume's first and second partials `[Fuu, Fuv, Fuw, Fvv, Fvw, Fww]` and
/// the compounding volume's first and second partials `[Dxx, Dxy, Dxz, Dyy, Dyz,
/// Dzz]`.
#[must_use]
pub fn lift_second_derivative_vol(
    f1: [Vec3; 3],
    f2: [Vec3; 6],
    d1: [Vec3; 3],
    d2: [Vec3; 6],
) -> [Vec3; 6] {
    let [fu, fv, fw] = f1;
    let [fuu, fuv, fuw, fvv, fvw, fww] = f2;
    let [dx, dy, dz] = d1;
    let [dxx, dxy, dxz, dyy, dyz, dzz] = d2;
    let duu = fu.x * fu.x * dxx
        + fu.y * fu.y * dyy
        + fu.z * fu.z * dzz
        + 2.0 * fu.x * fu.y * dxy
        + 2.0 * fu.x * fu.z * dxz
        + 2.0 * fu.y * fu.z * dyz
        + fuu.x * dx
        + fuu.y * dy
        + fuu.z * dz;

    let duv = fv.x * fu.x * dxx
        + fv.y * fu.y * dyy
        + fv.z * fu.z * dzz
        + (fv.x * fu.y + fv.y * fu.x) * dxy
        + (fv.x * fu.z + fv.z * fu.x) * dxz
        + (fv.y * fu.z + fv.z * fu.y) * dyz
        + fuv.x * dx
        + fuv.y * dy
        + fuv.z * dz;

    let duw = fw.x * fu.x * dxx
        + fw.y * fu.y * dyy
        + fw.z * fu.z * dzz
        + (fw.x * fu.y + fw.y * fu.x) * dxy
        + (fw.x * fu.z + fw.z * fu.x) * dxz
        + (fw.y * fu.z + fw.z * fu.y) * dyz
        + fuw.x * dx
        + fuw.y * dy
        + fuw.z * dz;

    let dvv = fv.x * fv.x * dxx
        + fv.y * fv.y * dyy
        + fv.z * fv.z * dzz
        + 2.0 * fv.x * fv.y * dxy
        + 2.0 * fv.x * fv.z * dxz
        + 2.0 * fv.y * fv.z * dyz
        + fvv.x * dx
        + fvv.y * dy
        + fvv.z * dz;

    let dvw = fw.x * fv.x * dxx
        + fw.y * fv.y * dyy
        + fw.z * fv.z * dzz
        + (fw.x * fv.y + fw.y * fv.x) * dxy
        + (fw.x * fv.z + fw.z * fv.x) * dxz
        + (fw.y * fv.z + fw.z * fv.y) * dyz
        + fvw.x * dx
        + fvw.y * dy
        + fvw.z * dz;

    let dww = fw.x * fw.x * dxx
        + fw.y * fw.y * dyy
        + fw.z * fw.z * dzz
        + 2.0 * fw.x * fw.y * dxy
        + 2.0 * fw.x * fw.z * dxz
        + 2.0 * fw.y * fw.z * dyz
        + fww.x * dx
        + fww.y * dy
        + fww.z * dz;
    [duu, duv, duw, dvv, dvw, dww]
}

/// `smvol_LiftThirdDerivative` (volume form): `[Duuu, Duuv, Duuw, Duvv, Duvw, Duww,
/// Dvvv, Dvvw, Dvww, Dwww]` from the base volume's first, second and third partials
/// `[Fuuu, Fuuv, Fuuw, Fuvv, Fuvw, Fuww, Fvvv, Fvvw, Fvww, Fwww]` and the
/// compounding volume's first, second and third partials (upstream's typos kept:
/// `Dxxz` for `Dxzz`, and `Duww` / `Dvww` built from the wrong base partials).
#[must_use]
pub fn lift_third_derivative_vol(
    f1: [Vec3; 3],
    f2: [Vec3; 6],
    f3: [Vec3; 10],
    d1: [Vec3; 3],
    d2: [Vec3; 6],
    d3: [Vec3; 10],
) -> [Vec3; 10] {
    let [fu, fv, fw] = f1;
    let [fuu, fuv, fuw, fvv, fvw, fww] = f2;
    let [fuuu, fuuv, fuuw, fuvv, fuvw, fuww, fvvv, fvvw, fvww, fwww] = f3;
    let [dx, dy, dz] = d1;
    let [dxx, dxy, dxz, dyy, dyz, dzz] = d2;
    let [dxxx, dxxy, dxxz, dxyy, dxyz, dxzz, dyyy, dyyz, dyzz, dzzz] = d3;
    let duuu = fuuu.x * dx
        + fuuu.y * dy
        + fuuu.z * dz
        + 3.0 * (fuu.x * fu.x) * dxx
        + 3.0 * (fuu.x * fu.y + fuu.y * fu.x) * dxy
        + 3.0 * (fuu.x * fu.z + fuu.z * fu.x) * dxz
        + 3.0 * (fuu.y * fu.y) * dyy
        + 3.0 * (fuu.y * fu.z + fuu.z * fu.y) * dyz
        + 3.0 * (fuu.z * fu.z) * dzz
        + fu.x * fu.x * fu.x * dxxx
        + 3.0 * fu.x * fu.x * fu.y * dxxy
        + 3.0 * fu.x * fu.x * fu.z * dxxz
        + 3.0 * fu.x * fu.y * fu.y * dxyy
        + 6.0 * fu.x * fu.y * fu.z * dxyz
        + 3.0 * fu.x * fu.z * fu.z * dxzz
        + fu.y * fu.y * fu.y * dyyy
        + 3.0 * fu.y * fu.y * fu.z * dyyz
        + 3.0 * fu.y * fu.z * fu.z * dyzz
        + fu.z * fu.z * fu.z * dzzz;

    let duuv = fuuv.x * dx
        + fuuv.y * dy
        + fuuv.z * dz
        + (2.0 * fuv.x * fu.x + fv.x * fuu.x) * dxx
        + (2.0 * fuv.y * fu.y + fv.y * fuu.y) * dyy
        + (2.0 * fuv.z * fu.z + fv.z * fuu.z) * dzz
        + (2.0 * fuv.x * fu.y + fv.y * fuu.x + 2.0 * fu.x * fuv.y + fv.x * fuu.y) * dxy
        + (2.0 * fuv.x * fu.z + fv.z * fuu.x + 2.0 * fu.x * fuv.z + fv.x * fuu.z) * dxz
        + (2.0 * fuv.y * fu.z + fv.z * fuu.y + 2.0 * fu.y * fuv.z + fv.y * fuu.z) * dyz
        + (fv.y * fu.x * fu.x + 2.0 * fv.x * fu.x * fu.y) * dxxy
        + (fv.z * fu.x * fu.x + 2.0 * fv.x * fu.x * fu.z) * dxxz
        + (fv.x * fu.y * fu.y + 2.0 * fv.y * fu.x * fu.y) * dxyy
        + (2.0 * fv.y * fu.x * fu.z + 2.0 * fv.z * fu.x * fu.y + 2.0 * fv.x * fu.y * fu.z) * dxyz
        + (fv.x * fu.z * fu.z + 2.0 * fv.z * fu.x * fu.z) * dxxz
        + (fv.z * fu.y * fu.y + 2.0 * fv.y * fu.y * fu.z) * dyyz
        + (fv.y * fu.z * fu.z + 2.0 * fv.z * fu.y * fu.z) * dyzz
        + (fv.x * fu.x * fu.x) * dxxx
        + (fv.y * fu.y * fu.y) * dyyy
        + (fv.z * fu.z * fu.z) * dzzz;

    let duuw = fuuw.x * dx
        + fuuw.y * dy
        + fuuw.z * dz
        + (2.0 * fuw.x * fu.x + fw.x * fuu.x) * dxx
        + (2.0 * fuw.y * fu.y + fw.y * fuu.y) * dyy
        + (2.0 * fuw.z * fu.z + fw.z * fuu.z) * dzz
        + (2.0 * fuw.x * fu.y + fw.y * fuu.x + 2.0 * fu.x * fuw.y + fw.x * fuu.y) * dxy
        + (2.0 * fuw.x * fu.z + fw.z * fuu.x + 2.0 * fu.x * fuw.z + fw.x * fuu.z) * dxz
        + (2.0 * fuw.y * fu.z + fw.z * fuu.y + 2.0 * fu.y * fuw.z + fw.y * fuu.z) * dyz
        + (fw.y * fu.x * fu.x + 2.0 * fw.x * fu.x * fu.y) * dxxy
        + (fw.z * fu.x * fu.x + 2.0 * fw.x * fu.x * fu.z) * dxxz
        + (fw.x * fu.y * fu.y + 2.0 * fw.y * fu.x * fu.y) * dxyy
        + (2.0 * fw.y * fu.x * fu.z + 2.0 * fw.z * fu.x * fu.y + 2.0 * fw.x * fu.y * fu.z) * dxyz
        + (fw.x * fu.z * fu.z + 2.0 * fw.z * fu.x * fu.z) * dxxz
        + (fw.z * fu.y * fu.y + 2.0 * fw.y * fu.y * fu.z) * dyyz
        + (fw.y * fu.z * fu.z + 2.0 * fw.z * fu.y * fu.z) * dyzz
        + (fw.x * fu.x * fu.x) * dxxx
        + (fw.y * fu.y * fu.y) * dyyy
        + (fw.z * fu.z * fu.z) * dzzz;

    let duvv = fuvv.x * dx
        + fuvv.y * dy
        + fuvv.z * dz
        + (2.0 * fuv.x * fv.x + fu.x * fvv.x) * dxx
        + (2.0 * fuv.y * fv.y + fu.y * fvv.y) * dyy
        + (2.0 * fuv.z * fv.z + fu.z * fvv.z) * dzz
        + (2.0 * fuv.x * fv.y + fu.y * fvv.x + 2.0 * fv.x * fuv.y + fu.x * fvv.y) * dxy
        + (2.0 * fuv.x * fv.z + fu.z * fvv.x + 2.0 * fv.x * fuv.z + fu.x * fvv.z) * dxz
        + (2.0 * fuv.y * fv.z + fu.z * fvv.y + 2.0 * fv.y * fuv.z + fu.y * fvv.z) * dyz
        + (fu.y * fv.x * fv.x + 2.0 * fu.x * fv.x * fv.y) * dxxy
        + (fu.z * fv.x * fv.x + 2.0 * fu.x * fv.x * fv.z) * dxxz
        + (fu.x * fv.y * fv.y + 2.0 * fu.y * fv.x * fv.y) * dxyy
        + (2.0 * fu.y * fv.x * fv.z + 2.0 * fu.z * fv.x * fv.y + 2.0 * fu.x * fv.y * fv.z) * dxyz
        + (fu.x * fv.z * fv.z + 2.0 * fu.z * fv.x * fv.z) * dxxz
        + (fu.z * fv.y * fv.y + 2.0 * fu.y * fv.y * fv.z) * dyyz
        + (fu.y * fv.z * fv.z + 2.0 * fu.z * fv.y * fv.z) * dyzz
        + (fu.x * fv.x * fv.x) * dxxx
        + (fu.y * fv.y * fv.y) * dyyy
        + (fu.z * fv.z * fv.z) * dzzz;

    let duvw = fuvw.x * dx
        + fuvw.y * dy
        + fuvw.z * dz
        + (fvw.x * fu.x + fw.x * fuv.x + fv.x * fuw.x) * dxx
        + (fvw.x * fu.y + fvw.y * fu.x + fw.y * fuv.x + fv.x * fuw.y + fv.y * fuw.x + fw.x * fuv.y)
            * dxy
        + (fvw.x * fu.z + fvw.z * fu.x + fw.z * fuv.x + fv.x * fuw.z + fv.z * fuw.x + fw.x * fuv.z)
            * dxz
        + (fvw.y * fu.y + fw.y * fuv.y + fv.y * fuw.y) * dyy
        + (fvw.y * fu.z + fvw.z * fu.y + fw.z * fuv.y + fv.y * fuw.z + fv.z * fuw.y + fw.y * fuv.z)
            * dyz
        + (fvw.z * fu.z + fw.z * fuv.z + fv.z * fuw.z) * dzz
        + (fw.x * fv.x * fu.x) * dxxx
        + (fw.y * fv.x * fu.x + fw.x * (fv.x * fu.y + fv.y * fu.x)) * dxxy
        + (fw.z * fv.x * fu.x + fw.x * (fv.x * fu.z + fv.z * fu.x)) * dxxz
        + (fw.x * fv.y * fu.y + fw.y * (fv.x * fu.y + fv.y * fu.x)) * dxyy
        + (fw.z * (fv.x * fu.y + fv.y * fu.x)
            + fw.y * (fv.x * fu.z + fv.z * fu.x)
            + fw.x * (fv.y * fu.z + fv.z * fu.y))
            * dxyz
        + (fw.y * fv.y * fu.y) * dyyy
        + (fw.z * fv.y * fu.y + fw.y * (fv.y * fu.z + fv.z * fu.y)) * dyyz
        + (fw.x * fv.z * fu.z + fw.z * (fv.x * fu.z + fv.z * fu.x)) * dxzz
        + (fw.y * fv.z * fu.z + fw.z * (fv.y * fu.z + fv.z * fu.y)) * dyzz
        + (fw.z * fv.z * fu.z) * dzzz;

    let duww = fuww.x * dx
        + fuww.y * dy
        + fuww.z * dz
        + (2.0 * fuv.x * fv.x + fu.x * fww.x) * dxx
        + (2.0 * fuv.y * fv.y + fu.y * fww.y) * dyy
        + (2.0 * fuv.z * fv.z + fu.z * fww.z) * dzz
        + (2.0 * fuv.x * fv.y + fu.y * fww.x + 2.0 * fv.x * fuv.y + fu.x * fww.y) * dxy
        + (2.0 * fuv.x * fv.z + fu.z * fww.x + 2.0 * fv.x * fuv.z + fu.x * fww.z) * dxz
        + (2.0 * fuv.y * fv.z + fu.z * fww.y + 2.0 * fv.y * fuv.z + fu.y * fww.z) * dyz
        + (fu.y * fv.x * fv.x + 2.0 * fu.x * fv.x * fv.y) * dxxy
        + (fu.z * fv.x * fv.x + 2.0 * fu.x * fv.x * fv.z) * dxxz
        + (fu.x * fv.y * fv.y + 2.0 * fu.y * fv.x * fv.y) * dxyy
        + (2.0 * fu.y * fv.x * fv.z + 2.0 * fu.z * fv.x * fv.y + 2.0 * fu.x * fv.y * fv.z) * dxyz
        + (fu.x * fv.z * fv.z + 2.0 * fu.z * fv.x * fv.z) * dxxz
        + (fu.z * fv.y * fv.y + 2.0 * fu.y * fv.y * fv.z) * dyyz
        + (fu.y * fv.z * fv.z + 2.0 * fu.z * fv.y * fv.z) * dyzz
        + (fu.x * fv.x * fv.x) * dxxx
        + (fu.y * fv.y * fv.y) * dyyy
        + (fu.z * fv.z * fv.z) * dzzz;

    let dvvv = fvvv.x * dx
        + fvvv.y * dy
        + fvvv.z * dz
        + 3.0 * (fvv.x * fv.x) * dxx
        + 3.0 * (fvv.x * fv.y + fvv.y * fv.x) * dxy
        + 3.0 * (fvv.x * fv.z + fvv.z * fv.x) * dxz
        + 3.0 * (fvv.y * fv.y) * dyy
        + 3.0 * (fvv.y * fv.z + fvv.z * fv.y) * dyz
        + 3.0 * (fvv.z * fv.z) * dzz
        + fv.x * fv.x * fv.x * dxxx
        + 3.0 * fv.x * fv.x * fv.y * dxxy
        + 3.0 * fv.x * fv.x * fv.z * dxxz
        + 3.0 * fv.x * fv.y * fv.y * dxyy
        + 6.0 * fv.x * fv.y * fv.z * dxyz
        + 3.0 * fv.x * fv.z * fv.z * dxzz
        + fv.y * fv.y * fv.y * dyyy
        + 3.0 * fv.y * fv.y * fv.z * dyyz
        + 3.0 * fv.y * fv.z * fv.z * dyzz
        + fv.z * fv.z * fv.z * dzzz;

    let dvvw = fvvw.x * dx
        + fvvw.y * dy
        + fvvw.z * dz
        + (2.0 * fvw.x * fv.x + fw.x * fvv.x) * dxx
        + (2.0 * fvw.y * fv.y + fw.y * fvv.y) * dyy
        + (2.0 * fvw.z * fv.z + fw.z * fvv.z) * dzz
        + (2.0 * fvw.x * fv.y + fw.y * fvv.x + 2.0 * fv.x * fvw.y + fw.x * fvv.y) * dxy
        + (2.0 * fvw.x * fv.z + fw.z * fvv.x + 2.0 * fv.x * fvw.z + fw.x * fvv.z) * dxz
        + (2.0 * fvw.y * fv.z + fw.z * fvv.y + 2.0 * fv.y * fvw.z + fw.y * fvv.z) * dyz
        + (fw.y * fv.x * fv.x + 2.0 * fw.x * fv.x * fv.y) * dxxy
        + (fw.z * fv.x * fv.x + 2.0 * fw.x * fv.x * fv.z) * dxxz
        + (fw.x * fv.y * fv.y + 2.0 * fw.y * fv.x * fv.y) * dxyy
        + (2.0 * fw.y * fv.x * fv.z + 2.0 * fw.z * fv.x * fv.y + 2.0 * fw.x * fv.y * fv.z) * dxyz
        + (fw.x * fv.z * fv.z + 2.0 * fw.z * fv.x * fv.z) * dxxz
        + (fw.z * fv.y * fv.y + 2.0 * fw.y * fv.y * fv.z) * dyyz
        + (fw.y * fv.z * fv.z + 2.0 * fw.z * fv.y * fv.z) * dyzz
        + (fw.x * fv.x * fv.x) * dxxx
        + (fw.y * fv.y * fv.y) * dyyy
        + (fw.z * fv.z * fv.z) * dzzz;

    let dvww = fvww.x * dx
        + fvww.y * dy
        + fvww.z * dz
        + (2.0 * fvv.x * fv.x + fv.x * fww.x) * dxx
        + (2.0 * fvv.y * fv.y + fv.y * fww.y) * dyy
        + (2.0 * fvv.z * fv.z + fv.z * fww.z) * dzz
        + (2.0 * fvv.x * fv.y + fv.y * fww.x + 2.0 * fv.x * fvv.y + fv.x * fww.y) * dxy
        + (2.0 * fvv.x * fv.z + fv.z * fww.x + 2.0 * fv.x * fvv.z + fv.x * fww.z) * dxz
        + (2.0 * fvv.y * fv.z + fv.z * fww.y + 2.0 * fv.y * fvv.z + fv.y * fww.z) * dyz
        + (fv.y * fv.x * fv.x + 2.0 * fv.x * fv.x * fv.y) * dxxy
        + (fv.z * fv.x * fv.x + 2.0 * fv.x * fv.x * fv.z) * dxxz
        + (fv.x * fv.y * fv.y + 2.0 * fv.y * fv.x * fv.y) * dxyy
        + (2.0 * fv.y * fv.x * fv.z + 2.0 * fv.z * fv.x * fv.y + 2.0 * fv.x * fv.y * fv.z) * dxyz
        + (fv.x * fv.z * fv.z + 2.0 * fv.z * fv.x * fv.z) * dxxz
        + (fv.z * fv.y * fv.y + 2.0 * fv.y * fv.y * fv.z) * dyyz
        + (fv.y * fv.z * fv.z + 2.0 * fv.z * fv.y * fv.z) * dyzz
        + (fv.x * fv.x * fv.x) * dxxx
        + (fv.y * fv.y * fv.y) * dyyy
        + (fv.z * fv.z * fv.z) * dzzz;

    let dwww = fwww.x * dx
        + fwww.y * dy
        + fwww.z * dz
        + 3.0 * (fww.x * fw.x) * dxx
        + 3.0 * (fww.x * fw.y + fww.y * fw.x) * dxy
        + 3.0 * (fww.x * fw.z + fww.z * fw.x) * dxz
        + 3.0 * (fww.y * fw.y) * dyy
        + 3.0 * (fww.y * fw.z + fww.z * fw.y) * dyz
        + 3.0 * (fww.z * fw.z) * dzz
        + fw.x * fw.x * fw.x * dxxx
        + 3.0 * fw.x * fw.x * fw.y * dxxy
        + 3.0 * fw.x * fw.x * fw.z * dxxz
        + 3.0 * fw.x * fw.y * fw.y * dxyy
        + 6.0 * fw.x * fw.y * fw.z * dxyz
        + 3.0 * fw.x * fw.z * fw.z * dxzz
        + fw.y * fw.y * fw.y * dyyy
        + 3.0 * fw.y * fw.y * fw.z * dyyz
        + 3.0 * fw.y * fw.z * fw.z * dyzz
        + fw.z * fw.z * fw.z * dzzz;
    [duuu, duuv, duuw, duvv, duvw, duww, dvvv, dvvw, dvww, dwww]
}

/// `smvol_ConcatEvals`: the evaluations `0..=n` (`D[u][v][w]`, `(n+1)^3` each) of
/// `Second(First(uvw))` from `first` and `second` (taken at `first`'s position),
/// with the chain rule up to third derivatives. Slots the chain rule does not fill
/// (mixed partials above order `n`) are [`Vec3::UNDEF`].
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when an array is shorter than `(n+1)^3` or the size
/// overflows.
pub fn concat_evals(n: usize, first: &[Vec3], second: &[Vec3]) -> Result<Vec<Vec3>> {
    let size = eval_size(n, OP_CONCAT)?;
    if first.len() < size || second.len() < size {
        return Err(SolidError::invalid_input(
            OP_CONCAT,
            "evaluation array shorter than (n+1)^3",
        ));
    }
    let n1 = n + 1;
    let sv = |u: usize, v: usize, w: usize| (u * n1 + v) * n1 + w;
    let f = |u: usize, v: usize, w: usize| at(first, sv(u, v, w), OP_CONCAT);
    let s = |u: usize, v: usize, w: usize| at(second, sv(u, v, w), OP_CONCAT);

    let mut calc = vec![Vec3::UNDEF; size];
    let mut put = |(u, vv, w): (usize, usize, usize), v: Vec3| {
        if let Some(slot) = calc.get_mut(sv(u, vv, w)) {
            *slot = v;
        }
    };

    // compounded position
    put((0, 0, 0), s(0, 0, 0)?);

    // compounded 1st derivs
    if n >= 1 {
        let f1 = [f(1, 0, 0)?, f(0, 1, 0)?, f(0, 0, 1)?];
        let s1 = [s(1, 0, 0)?, s(0, 1, 0)?, s(0, 0, 1)?];
        let [du, dv, dw] = lift_first_derivative_vol(f1, s1);
        put((1, 0, 0), du);
        put((0, 1, 0), dv);
        put((0, 0, 1), dw);

        // 2nd derivs
        if n >= 2 {
            let f2 = [
                f(2, 0, 0)?,
                f(1, 1, 0)?,
                f(1, 0, 1)?,
                f(0, 2, 0)?,
                f(0, 1, 1)?,
                f(0, 0, 2)?,
            ];
            let s2 = [
                s(2, 0, 0)?,
                s(1, 1, 0)?,
                s(1, 0, 1)?,
                s(0, 2, 0)?,
                s(0, 1, 1)?,
                s(0, 0, 2)?,
            ];
            let [duu, duv, duw, dvv, dvw, dww] = lift_second_derivative_vol(f1, f2, s1, s2);
            put((2, 0, 0), duu);
            put((1, 1, 0), duv);
            put((1, 0, 1), duw);
            put((0, 2, 0), dvv);
            put((0, 1, 1), dvw);
            put((0, 0, 2), dww);

            // 3rd derivs
            if n >= 3 {
                let f3 = [
                    f(3, 0, 0)?,
                    f(2, 1, 0)?,
                    f(2, 0, 1)?,
                    f(1, 2, 0)?,
                    f(1, 1, 1)?,
                    f(1, 0, 2)?,
                    f(0, 3, 0)?,
                    f(0, 2, 1)?,
                    f(0, 1, 2)?,
                    f(0, 0, 3)?,
                ];
                let s3 = [
                    s(3, 0, 0)?,
                    s(2, 1, 0)?,
                    s(2, 0, 1)?,
                    s(1, 2, 0)?,
                    s(1, 1, 1)?,
                    s(1, 0, 2)?,
                    s(0, 3, 0)?,
                    s(0, 2, 1)?,
                    s(0, 1, 2)?,
                    s(0, 0, 3)?,
                ];
                let d3 = lift_third_derivative_vol(f1, f2, f3, s1, s2, s3);
                let slots = [
                    (3, 0, 0),
                    (2, 1, 0),
                    (2, 0, 1),
                    (1, 2, 0),
                    (1, 1, 1),
                    (1, 0, 2),
                    (0, 3, 0),
                    (0, 2, 1),
                    (0, 1, 2),
                    (0, 0, 3),
                ];
                for (slot, v) in slots.into_iter().zip(d3) {
                    put(slot, v);
                }
            }
        }
    }
    Ok(calc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-12
    }

    /// `p -> o + p.x X + p.y Y + p.z Z`.
    fn affine(o: Vec3, x: Vec3, y: Vec3, z: Vec3) -> Transform {
        Transform::new(o, x, y, Some(z))
    }

    fn sample() -> Transform {
        affine(
            v(1.0, -2.0, 0.5),
            v(2.0, 0.0, 0.0),
            v(0.0, 3.0, 1.0),
            v(0.5, 0.0, 1.0),
        )
    }

    fn rot_z() -> Transform {
        affine(
            v(0.0, 0.0, 1.0),
            v(0.0, 1.0, 0.0),
            v(-1.0, 0.0, 0.0),
            v(0.0, 0.0, 1.0),
        )
    }

    fn tv(t: Transform) -> Volume {
        Volume::new(VolumeKind::Transform(t))
    }

    #[test]
    fn types_and_kinds() {
        let b = Volume::new(VolumeKind::Base);
        assert_eq!(b.type_code(), VOLUME_TYPE);
        assert_eq!(b.type_string(), "SmVolume_TYPE");
        assert_eq!(b.class_string(), "SmVolume");
        assert!(b.is_kind_of(VOLUME_TYPE) && b.is_kind_of(a_object::TYPE));
        assert!(!b.is_kind_of(TRANSFORM_TYPE));
        let t = tv(Transform::IDENTITY);
        assert!(t.is_kind_of(TRANSFORM_TYPE) && t.is_kind_of(VOLUME_TYPE));
        assert_eq!(
            Volume::new(VolumeKind::Twist(TwistVolume::new())).class_string(),
            "SmTwistVolume"
        );
        assert!(is_kind_of(VOLUME_TYPE) && !is_kind_of(1));
    }

    #[test]
    fn identity_orient_and_simple_maps() {
        let b = Volume::new(VolumeKind::Base);
        let d = b
            .orient(v(1.0, 2.0, 3.0), 2, [true; 3], true, true)
            .unwrap();
        assert_eq!(d.len(), 27);
        assert_eq!(d[0], v(1.0, 2.0, 3.0));
        assert_eq!(d[9], v(1.0, 0.0, 0.0));
        assert_eq!(d[3], v(0.0, 1.0, 0.0));
        assert_eq!(d[1], v(0.0, 0.0, 1.0));
        assert_eq!(d[18], Vec3::ZERO);
        assert!(b
            .orient(v(0.0, 0.0, 0.0), 6, [true; 3], true, true)
            .is_err());
        assert!(b
            .evaluate_simple(v(0.0, 0.0, 0.0), 0, &EvalOptions::default())
            .is_err());
        assert!(b.copy(false).is_err());
        assert!(!b
            .is_curve_in_param_domain_simple(
                &Kernel::default(),
                &Curve::from(Line::from_points(v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), 3))
            )
            .unwrap());
        assert_eq!(b.orient_x_axis(), v(1.0, 0.0, 0.0));
        assert_eq!(b.orient_origin(), Vec3::ZERO);
    }

    #[test]
    fn evaluate_compounds_with_the_chain_rule() {
        // T2(R(T1(p))) for affine maps is affine: compare with direct composition
        let t1 = sample();
        let r = rot_z();
        let t2 = affine(
            v(0.0, 1.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 2.0, 0.0),
            v(0.0, 0.0, 1.0),
        );
        let vol = Volume::with_maps(VolumeKind::Transform(t1), Some(r), Some(tv(t2))).unwrap();
        let p = v(0.3, -0.7, 1.1);
        let d = vol.evaluate(p, 3, &EvalOptions::default()).unwrap();
        assert_eq!(d.len(), 64);
        let x = |q: Vec3| {
            let a = t1.evaluate_simple(q, 0).unwrap()[0];
            let b = r.evaluate_simple(a, 0).unwrap()[0];
            t2.evaluate_simple(b, 0).unwrap()[0]
        };
        assert!(close(d[0], x(p)));
        // first partials are differences of the affine map
        let o = x(v(0.0, 0.0, 0.0));
        assert!(close(d[16], x(v(1.0, 0.0, 0.0)) - o));
        assert!(close(d[4], x(v(0.0, 1.0, 0.0)) - o));
        assert!(close(d[1], x(v(0.0, 0.0, 1.0)) - o));
        // second and third partials vanish, unfilled cross slots are undefined
        assert_eq!(d[32], Vec3::ZERO);
        assert_eq!(d[48], Vec3::ZERO);
        assert_eq!(d[63], Vec3::UNDEF);
        // without compounding only the oriented first map counts
        let opts = EvalOptions {
            with_compounding: false,
            ..EvalOptions::default()
        };
        let d1 = vol.evaluate(p, 0, &opts).unwrap();
        let a = t1.evaluate_simple(p, 0).unwrap()[0];
        assert!(close(d1[0], r.evaluate_simple(a, 0).unwrap()[0]));
        assert!(vol.evaluate(p, 4, &opts).is_err());
    }

    #[test]
    fn map_inverts_the_orientation_first() {
        let r = rot_z();
        let vol =
            Volume::with_maps(VolumeKind::Transform(Transform::IDENTITY), Some(r), None).unwrap();
        // Orient(Identity(InvOrient(p))) = p
        let p = v(2.0, -1.0, 4.0);
        let d = vol.map(p, 1, &EvalOptions::default()).unwrap();
        assert!(close(d[0], p));
        assert!(close(d[4], v(1.0, 0.0, 0.0)));
        assert!(close(vol.map_point(p, true).unwrap(), p));
        let (is_in, q) = vol.is_point_in_in_space_domain(p, true).unwrap();
        assert!(is_in);
        assert!(close(r.evaluate_simple(q, 0).unwrap()[0], p));
        assert!(close(vol.orient_point(q).unwrap(), p));
    }

    #[test]
    fn transform_builds_into_the_last_map() {
        let ax = Axis2Placement::new(v(1.0, 2.0, 3.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
        // a transform absorbs it
        let mut t = tv(Transform::IDENTITY);
        t.transform(&ax, None).unwrap();
        assert!(t.next_map().is_none());
        assert!(close(
            t.map_point(Vec3::ZERO, true).unwrap(),
            v(1.0, 2.0, 3.0)
        ));
        // a plain volume gets a transform appended
        let mut b = Volume::new(VolumeKind::Base);
        b.transform(&ax, None).unwrap();
        assert_eq!(b.last_map().map(Volume::type_code), Some(TRANSFORM_TYPE));
        // identity does nothing
        let mut c = Volume::new(VolumeKind::Base);
        let id = Axis2Placement::new(Vec3::ZERO, v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
        c.transform(&id, Some(v(1.0, 1.0, 1.0))).unwrap();
        assert!(c.next_map().is_none());
        // a B-spline volume cannot (no USE_VOLUMES upstream)
        let mut s = Volume::new(VolumeKind::BSpline(b_spline()));
        let before = s.kind().clone();
        assert!(matches!(
            s.transform(&ax, None),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        assert_eq!(s.kind(), &before);
    }

    #[test]
    fn copies_drop_maps_they_cannot_copy() {
        let chain = Volume::with_maps(
            VolumeKind::Transform(sample()),
            None,
            Some(Volume::with_maps(VolumeKind::Base, None, Some(tv(Transform::IDENTITY))).unwrap()),
        )
        .unwrap();
        let full = chain.copy(false).unwrap();
        assert!(full.next_map().is_none());
        let simple = chain.copy(true).unwrap();
        assert!(simple.next_map().is_none());
        let mut a = tv(sample());
        assert!(a.add_next_map(Volume::new(VolumeKind::Base), 1).is_err());
        assert!(a.next_map().is_none());
        a.add_next_map(Volume::new(VolumeKind::Base), 2).unwrap();
        a.add_next_map(tv(rot_z()), 0).unwrap();
        assert_eq!(a.last_map().map(Volume::type_code), Some(TRANSFORM_TYPE));
        // an owned plain map is lost by the copy, a shared one is kept
        assert!(a.copy(false).unwrap().next_map().is_none());
        let mut s = tv(sample());
        s.add_next_map(Volume::new(VolumeKind::Base), 0).unwrap();
        let kept = s.copy(false).unwrap();
        assert_eq!(kept.next_map().map(Volume::type_code), Some(VOLUME_TYPE));
    }

    #[test]
    fn equality() {
        let a = Volume::with_maps(VolumeKind::Transform(sample()), Some(rot_z()), None).unwrap();
        let b = Volume::with_maps(VolumeKind::Transform(sample()), Some(rot_z()), None).unwrap();
        let c = Volume::with_maps(VolumeKind::Transform(sample()), None, None).unwrap();
        assert!(a.approx_eq(&b).unwrap());
        assert!(!a.approx_eq(&c).unwrap());
        assert!(!a.approx_eq(&Volume::new(VolumeKind::Base)).unwrap());
        let bend = Volume::new(VolumeKind::Bend(BendVolume::new()));
        assert!(!bend.approx_eq(&a).unwrap());
        assert!(bend.approx_eq(&bend).unwrap());
    }

    #[test]
    fn domains_and_discontinuities() {
        let t = tv(sample());
        assert!(!t.is_bounded().unwrap());
        let line = Curve::from(Line::from_points(Vec3::ZERO, v(1.0, 0.0, 0.0), 3));
        let chain =
            Volume::with_maps(VolumeKind::Transform(sample()), Some(rot_z()), Some(t)).unwrap();
        let k = Kernel::default();
        assert!(chain.is_curve_in_param_domain(&k, &line, true).unwrap());
        assert!(chain.is_curve_in_in_space_domain(&k, &line, true).unwrap());
        assert!(chain
            .is_line_in_param_domain(&k, Vec3::ZERO, v(1.0, 0.0, 0.0), true)
            .unwrap());
        let with_base = Volume::with_maps(
            VolumeKind::Transform(sample()),
            None,
            Some(Volume::new(VolumeKind::Base)),
        )
        .unwrap();
        assert!(!with_base
            .is_line_in_param_domain(&k, Vec3::ZERO, v(1.0, 0.0, 0.0), true)
            .unwrap());
        let bx = Extent3d::from_points(v(-1.0, -1.0, -1.0), v(1.0, 1.0, 1.0));
        let mut trim = Extent3d::new();
        chain
            .trim_param_bounding_box_simple(&bx, &mut trim)
            .unwrap();
        assert_eq!(trim, bx);
        let mut d = Discontinuities3d::new();
        d.set_unaligned();
        assert!(d.u.unaligned && d.min_continuity == ContinuityType::C0);
        assert!(!chain
            .has_discontinuities_simple(&k, Some(&mut d), true)
            .unwrap());
        assert_eq!(d, Discontinuities3d::default());
    }

    #[test]
    fn refresh_and_make_simple() {
        let mut a =
            Volume::with_maps(VolumeKind::Transform(sample()), Some(sample()), None).unwrap();
        let inv = *a.inv_orient_map().unwrap();
        let p = v(0.5, 0.25, -1.0);
        let q = inv
            .evaluate_simple(sample().evaluate_simple(p, 0).unwrap()[0], 0)
            .unwrap()[0];
        assert!(close(p, q));
        a.make_map_simple();
        assert!(a.orient_map().is_none() && a.inv_orient_map().is_none());
        assert!(a.refresh_inv_orient_map().is_err());
        // a singular orient map keeps its copy as the inverse
        let s = affine(
            Vec3::ZERO,
            v(1.0, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            v(0.0, 0.0, 1.0),
        );
        let b = Volume::with_maps(VolumeKind::Base, Some(s), None).unwrap();
        assert_eq!(b.inv_orient_map(), Some(&s));
    }

    #[test]
    fn owner() {
        let mut a = Volume::new(VolumeKind::Base);
        assert_eq!(a.owner(), None);
        let r = ObjectRef {
            type_code: 1,
            key: crate::geom::attribute::ObjectKey(7),
        };
        assert_eq!(a.set_owner(Some(r)), None);
        assert_eq!(a.set_owner(None), Some(r));
    }

    #[test]
    fn lifts_match_the_chain_rule_of_linear_maps() {
        // a linear outer map: only the first partials matter
        let d1 = [v(1.0, 2.0, 0.0), v(0.0, 1.0, 3.0), v(4.0, 0.0, 1.0)];
        let z6 = [Vec3::ZERO; 6];
        let z10 = [Vec3::ZERO; 10];
        let cs = v(0.5, -1.0, 2.0);
        let css = v(1.0, 0.0, -1.0);
        let csss = v(0.0, 2.0, 1.0);
        let lin = |c: Vec3| c.x * d1[0] + c.y * d1[1] + c.z * d1[2];
        assert!(close(lift_first_derivative(cs, d1), lin(cs)));
        assert!(close(lift_second_derivative([cs, css], d1, z6), lin(css)));
        assert!(close(
            lift_third_derivative([cs, css, csss], d1, z6, z10),
            lin(csss)
        ));
        let [du, dv] = lift_first_derivative_srf([cs, css], d1);
        assert!(close(du, lin(cs)) && close(dv, lin(css)));
        let r = lift_first_derivative_vol([cs, css, csss], d1);
        assert!(close(r[2], lin(csss)));
        // identity inner map: the outer partials come through
        let f1 = [v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0)];
        let d2 = [
            v(1.0, 0.0, 0.0),
            v(2.0, 0.0, 0.0),
            v(3.0, 0.0, 0.0),
            v(4.0, 0.0, 0.0),
            v(5.0, 0.0, 0.0),
            v(6.0, 0.0, 0.0),
        ];
        assert_eq!(lift_second_derivative_vol(f1, z6, d1, d2), d2);
        let s2 = lift_second_derivative_srf([f1[0], f1[1]], [Vec3::ZERO; 3], d1, d2);
        assert_eq!(s2, [d2[0], d2[1], d2[3]]);
        let d3: [Vec3; 10] = core::array::from_fn(|i| v(i as f64, 1.0, 0.0));
        let t = lift_third_derivative_vol(f1, z6, z10, d1, d2, d3);
        assert_eq!(t[0], d3[0]);
        assert_eq!(t[4], d3[4]);
        let s3 =
            lift_third_derivative_srf([f1[0], f1[1]], [Vec3::ZERO; 3], [Vec3::ZERO; 4], d1, d2, d3);
        assert_eq!(s3[0], d3[0]);
        assert_eq!(s3[3], d3[6]);
    }

    #[test]
    fn concat_with_identity_and_short_arrays() {
        let o = tv(Transform::IDENTITY);
        let first = o
            .orient(v(1.0, 2.0, 3.0), 2, [true; 3], true, true)
            .unwrap();
        let second = sample().evaluate_simple(first[0], 2).unwrap();
        let c = concat_evals(2, &first, &second).unwrap();
        assert_eq!(c[0], second[0]);
        assert_eq!(c[9], second[9]);
        assert_eq!(c[26], Vec3::UNDEF);
        assert!(concat_evals(2, &first[..5], &second).is_err());
        assert_eq!(concat_evals(0, &first, &second).unwrap(), vec![second[0]]);
    }

    #[test]
    fn find_pv_intersect() {
        let vol = tv(sample());
        let target = sample().evaluate_simple(v(0.5, 0.5, 0.5), 0).unwrap()[0];
        let mut e = FindPvIntersect::new(target, &vol, 1e-20);
        let mut f = [0.0; 3];
        let mut j = Matrix::new(3, 3);
        assert!(e.evaluate(&[0.5, 0.5, 0.5], &mut f, Some(&mut j)).unwrap());
        assert_eq!(f, [0.0; 3]);
        assert_eq!(j.at(0, 0).unwrap(), 2.0);
        assert!(!e.evaluate(&[0.0, 0.0, 0.0], &mut f, None).unwrap());
        assert!(e.evaluate(&[0.0, 0.0], &mut f, None).is_err());
        let flat = tv(affine(
            Vec3::ZERO,
            Vec3::ZERO,
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
        ));
        let mut e = FindPvIntersect::new(target, &flat, 1e-20);
        assert!(e.evaluate(&[0.0, 0.0, 0.0], &mut f, None).is_err());
    }

    fn b_spline() -> BSplineVolume {
        let mut n = crate::nurbs::spiral::allocate_nurb_volume([1; 3], [1; 3], [3; 3]).unwrap();
        for d in 0..3 {
            n.knots_mut(d).copy_from_slice(&[0.0, 0.0, 1.0, 1.0]);
        }
        for (i, c) in n.cpts_mut().iter_mut().enumerate() {
            let f = i as f64;
            *c = crate::nurbs::math::CPoint::new(f, f * f, 1.0 - f, 1.0);
        }
        BSplineVolume::new(n)
    }

    #[test]
    fn b_spline_overrides_and_inherited_base() {
        let k = &Kernel::default();
        // the B-spline volume's NLib overrides
        let bs = Volume::new(VolumeKind::BSpline(b_spline()));
        assert!(!bs.has_discontinuities_simple(k, None, true).unwrap());
        let mut d = DirectionDiscontinuities::none();
        assert!(bs
            .calculate_continuities_simple(k, VolumeParam::U, &mut d, false)
            .is_ok());
        assert!(!d.params.is_empty());
        assert!(bs.natural_param_domain().is_ok());
        assert!(bs.is_bounded().unwrap());
        assert!(bs
            .evaluate_simple(Vec3::ZERO, 0, &EvalOptions::default())
            .is_err());
        let copy = bs.copy(false).unwrap();
        assert!(bs.approx_eq(&bs).unwrap());
        assert!(!bs.approx_eq(&copy).unwrap());
        // inherited base behaviour still runs
        let mut twist = Volume::new(VolumeKind::Twist(TwistVolume::new()));
        assert!(!twist.has_discontinuities_simple(k, None, true).unwrap());
        let ax = Axis2Placement::new(v(1.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
        twist.transform(&ax, None).unwrap();
        assert!(twist.next_map().is_some());
    }

    #[test]
    fn orient_directional_derivs() {
        let vol = Volume::with_maps(VolumeKind::Base, Some(sample()), None).unwrap();
        let d = vol
            .orient_directional_derivs(Vec3::ZERO, v(0.0, 0.0, 2.0), 3, [true; 3])
            .unwrap();
        assert_eq!(d.len(), 4);
        assert!(close(d[1], sample().to_z_axis()));
        assert!(close(
            vol.orient_vector(Vec3::ZERO, v(0.0, 0.0, 0.0), [true; 3])
                .unwrap(),
            sample().to_z_axis()
        ));
        assert!(vol
            .orient_directional_derivs(Vec3::ZERO, v(1.0, 0.0, 0.0), 4, [true; 3])
            .is_err());
    }
}
