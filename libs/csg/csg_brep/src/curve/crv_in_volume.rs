//! Curves in volumes: a curve mapped through a (compound) volume, from the volume's
//! parameter space or in-space to its final out-space (the generated curve
//! `SmCrvInVolume`).
//!
//! Upstream: `source/SMLib/src/SmCrvInVolume.cpp`, `source/SMLib/inc/SmCrvInVolume.h`.
//!
//! | Rust                                           | Upstream                                          |
//! |------------------------------------------------|---------------------------------------------------|
//! | [`CrvInVolume`]                                | `SmCrvInVolume`                                   |
//! | [`CrvInVolume::new`]                           | `SmCrvInVolume(SmCurve&, SmBoolean, SmVolume&, ULONG, ULONG, const SmContext*)` |
//! | [`CrvInVolume::copy`], `Clone`                 | copy constructor, `Copy`                          |
//! | `Drop` (implicit)                              | `~SmCrvInVolume`                                  |
//! | [`CrvInVolume::approx_eq`]                     | `operator==`                                      |
//! | [`CrvInVolume::TYPE`], [`CrvInVolume::type_code`], [`CrvInVolume::type_string`], [`CrvInVolume::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`CrvInVolume::is_kind_of`]                    | `IsKindOf`                                        |
//! | [`CrvInVolume::natural_interval`]              | `GetNaturalInterval`                              |
//! | [`CrvInVolume::owns_curve`], [`CrvInVolume::owns_volume`] | `OwnsCurve`, `OwnsVolume`              |
//! | [`CrvInVolume::is_analytic`], [`CrvInVolume::is_bounded`] | `IsAnalytic`, `IsBounded`              |
//! | [`CrvInVolume::evaluate`]                      | `Evaluate`                                        |
//! | [`CrvInVolume::evaluate_point`]                | `EvaluatePoint`                                   |
//! | [`CrvInVolume::calculate_continuities`]        | `CalculateContinuities`                           |
//! | [`CrvInVolume::knots`], [`CrvInVolume::knots_ref`] | `GetKnots`                                    |
//! | [`CrvInVolume::generate_mults`]                | `GenerateMults`                                   |
//! | [`CrvInVolume::number_natural_knots`]          | `GetNumberNaturalKnots`                           |
//! | [`CrvInVolume::edit_parameterization`]         | `EditParameterization`                            |
//! | [`CrvInVolume::reverse_parameterization`]      | `ReverseParameterization`                         |
//! | [`CrvInVolume::trim`]                          | `Trim`                                            |
//! | [`CrvInVolume::transform`]                     | `Transform`                                       |
//! | [`CrvInVolume::read_from_db`]                  | `ReadFromDB`                                      |
//! | [`CrvInVolume::make_exact_b_spline_if_possible_with`] | `MakeExactBSplineIfPossible`               |
//! | `make_exact_b_spline_out_curve_from_{in,param}_space` (private) | `SmVolume::MakeExactBSplineOutCurveFrom{In,Param}Space` |
//! | `make_exact_b_spline_1st_out_curve` (private)  | `SmVolume`/`SmBSplineVolume`/`SmTransform::MakeExactBSpline1stOutCurve` |
//! | `SetContext` (dropped)                         | `SetContext`                                      |
//!
//! Deviations:
//! - **Nesting.** `new` returns `InvalidInput` when the curve or the volume would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING)
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream). A database read bounds
//!   the same graph through `DatabaseReader::enter_nested` (32) on both nested readers.
//!   No method replaces the curve or the volume after construction, so every
//!   `CrvInVolume` holds a bounded graph; that is what [`CrvInVolume::copy`] and the
//!   infallible `Clone` rely on.
//! - **Volumes.** The volume is a [`Volume`] (units `surface.volume_p1`, `_p2`);
//!   the queries of the unported volume kinds return [`SolidError::Unported`].
//! - **Curve classification.** `GetKnots` classifies the base curve against the
//!   volume's knot iso-planes with `SmCurveClassification` (units
//!   `intersect.curve_class_p1`, `_p4`, `_p8`); that call returns
//!   [`SolidError::Unported`].
//! - **Ownership.** The base curve and the volume are held by value. Upstream either
//!   shares them with the caller or owns (and maybe copies) them, as the copy and
//!   owner flags say; here the copy flags only set the owner bits (the value moved
//!   in is already this object's copy) and the owner flag is kept as data: it
//!   decides [`CrvInVolume::owns_curve`], [`CrvInVolume::owns_volume`] and whether
//!   `Trim` / `ReverseParameterization` are allowed, as upstream. The copy
//!   constructor - [`CrvInVolume::copy`] and `Clone` alike - follows upstream part by
//!   part: a part this object owns is copied with that part's copy constructor, one it
//!   does not own is an exact copy that keeps its owner (upstream shares it), and the
//!   owner flag is normalized using the recognized ownership predicates, not a bit
//!   mask: noncanonical flags own neither part. The volume is copied field-wise; upstream
//!   copy-constructs the volume whenever it owns either part (also for flag 1, which
//!   owns only the curve) and shares it at flag 0, and the `Copy` knot fix-up of a
//!   B-spline volume is not modelled, as in [`CrvInVolume::copy`]. The break cache is
//!   copied only when `m_bNeedBreaks` is FALSE, as upstream; the operations here leave
//!   the cache empty whenever they invalidate it (`Trim` and `EditParameterization`
//!   clear both arrays when they set the flag), so that branch is not observable
//!   through them. The owner links of the contained objects (`SetOwner(this)`) and the
//!   notifications (`Notify`) are not modelled; `SetContext` and the `SmContext`
//!   arguments (they only choose where upstream allocates) are dropped.
//! - **Equality.** Upstream first compares the curve and volume pointers; held by
//!   value, the parts are always compared by value ([`Volume::approx_eq`]), after the parameter-space flag and the curves, in
//!   upstream's order. `m_bInParamSpace` is a `bool` (upstream any nonzero
//!   `SmBoolean` is true in every test this unit makes).
//! - **Break cache.** `GetKnots` caches the breaks through a `const_cast`: here
//!   [`CrvInVolume::knots`] takes `&mut self` and stores them;
//!   [`CrvInVolume::knots_ref`] (used by the `&self` methods and the [`Curve`]
//!   dispatch) computes the same answer without storing it. A later call therefore
//!   recomputes instead of reading the cache. [`CrvInVolume::set_breaks_cache`]
//!   loads the cache as `ReadFromDB` does.
//! - **Out-params.** Outputs are return values; upstream appends to the caller's
//!   arrays (`GetKnots` on the cached path with an interval) or resizes them in
//!   place (`GenerateMults`), here they start empty. `GenerateMults` reports an
//!   error without the partly filled multiplicities; `GetKnots` ignores its status
//!   as upstream and keeps them.
//! - **Undefined behaviour.** Where upstream reads outside its arrays (`GetKnots`
//!   with an empty knot list or no classification interval, `GenerateMults` with
//!   more knots matched than continuities, `CalculateContinuities` without breaks)
//!   the port returns [`SolidError::InvalidInput`]. `CalculateContinuities` ignores
//!   `GetKnots`' and `EvaluateContinuity`'s statuses upstream (leaving an
//!   uninitialised continuity on failure); the port propagates a failed
//!   evaluation. `Evaluate` leaves derivatives the volume could not provide as the
//!   caller had them; the port returns [`Vec3::UNDEF`] there.
//! - **`ReadFromDB`** always builds a new object (the "fill the curve passed in"
//!   branch and its `IsKindOf` check have no counterpart; `lType`, `lDim` and the
//!   context are unused). A type header whose ASCII extraction was skipped (the
//!   stream had already failed) leaves upstream's variable uninitialised; the port
//!   returns [`SolidError::InvalidInput`]. Counts are bounded by the bytes left
//!   (`plan/findings.md`, "header-declared sizes").
//! - **Exact B-spline.** `MakeExactBSplineIfPossible` and the volume functions it
//!   needs (`MakeExactBSplineOutCurveFrom{Param,In}Space`, the
//!   `MakeExactBSpline1stOutCurve` of `SmVolume`, `SmBSplineVolume` (both `NULL`) and
//!   `SmTransform`) are in no unit in `units.json`; they are ported here as
//!   [`CrvInVolume::make_exact_b_spline_if_possible_with`], which takes the kernel the
//!   curve copies need (upstream's takes the context). The bend, unbend and twist
//!   overrides are ported beside their volumes
//!   ([`BendVolume::make_exact_b_spline_1st_out_curve`](crate::surface::bend_volume::BendVolume::make_exact_b_spline_1st_out_curve)
//!   and the unbend and twist counterparts). The compounding loops stop with
//!   `InvalidInput` past `MAX_GEOMETRY_NESTING` maps (upstream walks the whole chain);
//!   the result is a [`Curve`] (upstream an `SmBSplineCurve*`, which a twist fills with
//!   a copy of any input kind); a failed `Copy` (upstream then dereferences `NULL`) is
//!   returned as its error.
//! - **Not in this unit.** `EvaluateVolume`,
//!   `CreateMirrorCurve`, `SetBaseCurve`, `SetVolume`, `SetOwnerFlag`, `MakeOwner`,
//!   `GetMemoryUsed`, `WriteToDB`, `Dump`, `AssertValid` and `GetRootCurve` belong
//!   to other units or are out of scope; the base constructor `SmCurve(ULONG)`
//!   (unit `curve.curve_p5`) only sets the dimension, 3.
//! - **Derivative order bound.** `Evaluate` returns [`SolidError::InvalidInput`] when
//!   `num_derivs` exceeds [`crate::curve::MAX_EVAL_DERIVS`] (upstream fills a
//!   caller-supplied array of any length; the port's output `Vec` must not be sized by
//!   an unbounded caller value).

use crate::curve::curve::p2::{
    at, evaluate_continuity, is_kind_of as curve_is_kind_of, Curve, CurveEvaluate, KnotsWithStatus,
    BSPLINE_CURVE_TYPE, CRV_IN_VOLUME_TYPE,
};
use crate::error::{ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::tol::is_zero_to_tol;
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::curve_class::p8::volume_discontinuity_intervals;
use crate::kernel::Kernel;
use crate::math::{max2, BIG_DOUBLE, BOUNDED_INFINITE_PARAM, EFF_ZERO};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::surface::srf_in_volume::transform_axis2_placement;
use crate::surface::surface::p2::{check_geometry_nesting, NestedGeometry, MAX_GEOMETRY_NESTING};
use crate::surface::volume::p1::{
    lift_first_derivative, lift_second_derivative, lift_third_derivative,
};
use crate::surface::volume::{EvalOptions, Volume, VolumeKind};
use crate::topology::database_io::{DatabaseReader, FileType};

const OP: &str = "curve::crv_in_volume::CrvInVolume";
const OP_KNOTS: &str = "curve::crv_in_volume::CrvInVolume::knots";
const OP_MULTS: &str = "curve::crv_in_volume::CrvInVolume::generate_mults";
const OP_CONTS: &str = "curve::crv_in_volume::CrvInVolume::calculate_continuities";
const OP_READ: &str = "curve::crv_in_volume::CrvInVolume::read_from_db";
const OP_EXACT: &str = "curve::crv_in_volume::CrvInVolume::make_exact_b_spline_if_possible_with";

/// `MAX_DERIVS` of `Evaluate`: the volume lifts go up to third derivatives.
const MAX_DERIVS: usize = 3;

/// `SM_CONTINUITY_ANGLE` (degrees): the default G1 angle tolerance.
pub const CONTINUITY_ANGLE: f64 = 0.5;

// ---- back-edges ----------------------------------------------------------------------

/// `SmVolume::EvaluatePoint` (with compounding).
fn volume_evaluate_point(volume: &Volume, point: Point3) -> Result<Point3> {
    volume.evaluate_point(point, true)
}

/// `SmVolume::ReadFromDB` of a volume of type `volume_type`.
fn volume_read_from_db(volume_type: i64, db: &mut DatabaseReader, version: u64) -> Result<Volume> {
    Volume::read_from_db(volume_type, db, version)
}

/// The parameter intervals of the `SmCurveClassification` of `curve` over `interval`
/// (tolerance `tol`) against the volume's discontinuity iso-planes
/// (`Intersect3DDiscontinuities`), in order.
fn classify_against_volume(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
    volume: &Volume,
    in_param_space: bool,
) -> Result<Vec<Extent1d>> {
    volume_discontinuity_intervals(k, curve, *interval, tol, volume, in_param_space)
}

// ---- exact B-spline images ------------------------------------------------------------

/// `MakeExactBSpline1stOutCurve(eInputSpace, rInputCurve, rpNewCurve)` of one map (no
/// compounding), from in-space when `in_space` (`SM_VS_IN_SPACE`), else from parameter
/// space: `None` when the map cannot produce an exact B-spline.
///
/// `SmVolume` and `SmBSplineVolume` never can (header bodies); `SmTransform` copies a
/// B-spline-derived curve and transforms it by its placement when the matrix is a
/// right-handed unit orthonormal one (upstream asserts the orient map is NULL for a
/// parameter-space input and uses the same mapping either way); the bend, unbend and
/// twist overrides are [`BendVolume::make_exact_b_spline_1st_out_curve`](crate::surface::bend_volume::BendVolume::make_exact_b_spline_1st_out_curve)
/// and its unbend and twist counterparts.
fn make_exact_b_spline_1st_out_curve(
    k: &Kernel,
    map: &Volume,
    in_space: bool,
    curve: &Curve,
) -> Result<Option<Curve>> {
    match map.kind() {
        VolumeKind::Base | VolumeKind::BSpline(_) => Ok(None),
        VolumeKind::Transform(t) => {
            // for geometry derived from SmBSpline
            if curve.is_kind_of(BSPLINE_CURVE_TYPE) {
                // for Transforms which can be represented by SmAxis2Placement mappings
                if let Some(placement) = transform_axis2_placement(t) {
                    // copy and transform the curve (statuses ignored)
                    let mut new_curve = curve.copy(k)?;
                    ignored(new_curve.transform(k, &placement, None))?;
                    return Ok(Some(new_curve));
                }
            }
            Ok(None)
        }
        VolumeKind::Bend(b) => b.make_exact_b_spline_1st_out_curve(k, map, in_space, curve),
        VolumeKind::Unbend(u) => u.make_exact_b_spline_1st_out_curve(k, map, in_space, curve),
        VolumeKind::Twist(t) => t.make_exact_b_spline_1st_out_curve(k, map, in_space, curve),
    }
}

/// `SmVolume::MakeExactBSplineOutCurveFromInSpace`: `curve` projected exactly through
/// every compounded map, or `None` when one cannot.
fn make_exact_b_spline_out_curve_from_in_space(
    k: &Kernel,
    volume: &Volume,
    in_space_curve: &Curve,
) -> Result<Option<Curve>> {
    // init output and locals
    let mut this_map = Some(volume);
    let mut this_in_space: Option<Curve> = None;

    // for every compounding map
    let mut depth = 0;
    while let Some(map) = this_map {
        depth += 1;
        if depth > MAX_GEOMETRY_NESTING {
            return Err(SolidError::invalid_input(
                OP_EXACT,
                "volume chain deeper than MAX_GEOMETRY_NESTING",
            ));
        }
        // see if the current InSpaceCurve can project exactly to the Next OutSpace
        let input = this_in_space.as_ref().unwrap_or(in_space_curve);
        let Some(new_curve) = make_exact_b_spline_1st_out_curve(k, map, true, input)? else {
            // return NULL when an exact projection was not possible
            return Ok(None);
        };

        // set up for the next iteration
        this_in_space = Some(new_curve);
        this_map = map.next_map();
    }
    Ok(this_in_space)
}

/// `SmVolume::MakeExactBSplineOutCurveFromParamSpace`: the first map from parameter
/// space, then every compounded map from in-space; `None` when one cannot.
fn make_exact_b_spline_out_curve_from_param_space(
    k: &Kernel,
    volume: &Volume,
    param_space_curve: &Curve,
) -> Result<Option<Curve>> {
    // for 1st volume mapping - Get 1stOutSpace project if possible
    let Some(mut new_curve) =
        make_exact_b_spline_1st_out_curve(k, volume, false, param_space_curve)?
    else {
        // return NULL when an exact projection was not possible
        return Ok(None);
    };

    // for every subsequent compounding map
    let mut this_map = volume.next_map();
    let mut depth = 1;
    while let Some(map) = this_map {
        depth += 1;
        if depth > MAX_GEOMETRY_NESTING {
            return Err(SolidError::invalid_input(
                OP_EXACT,
                "volume chain deeper than MAX_GEOMETRY_NESTING",
            ));
        }
        // see if the current InSpaceCurve can project exactly to the Next OutSpace
        let Some(next) = make_exact_b_spline_1st_out_curve(k, map, true, &new_curve)? else {
            // return NULL when an exact projection was not possible
            return Ok(None);
        };
        new_curve = next;
        this_map = map.next_map();
    }
    Ok(Some(new_curve))
}

// ---- continuity codes ---------------------------------------------------------------

/// The `SmContinuityType` of integer `code` (the C cast in `ReadFromDB`); `None`
/// outside the enum.
#[must_use]
pub const fn continuity_from_code(code: i64) -> Option<ContinuityType> {
    Some(match code {
        0 => ContinuityType::Undefined,
        1 => ContinuityType::Discontinuous,
        2 => ContinuityType::C0,
        3 => ContinuityType::G1,
        4 => ContinuityType::G1R,
        5 => ContinuityType::G1G2,
        6 => ContinuityType::G1G2G3,
        7 => ContinuityType::C1,
        8 => ContinuityType::C1G2,
        9 => ContinuityType::C1G2G3,
        10 => ContinuityType::C1C2,
        11 => ContinuityType::C1C2G3,
        12 => ContinuityType::C1C2C3,
        13 => ContinuityType::CInfinity,
        _ => return None,
    })
}

// ---- the curve ----------------------------------------------------------------------

/// A curve mapped through a volume (`SmCrvInVolume`): the base curve lives in the
/// volume's parameter space or in-space and is mapped to the volume's final
/// out-space. Its "knots" are the base curve's knots plus the parameters where the
/// base curve crosses the volume's discontinuity iso-planes, cached once computed.
#[derive(Debug)]
pub struct CrvInVolume {
    /// `m_pCurve`: the base curve.
    curve: Curve,
    /// `m_bInParamSpace`: the base curve is in the volume's parameter space (else
    /// in its in-space).
    in_param_space: bool,
    /// `m_pVolume`: the mapping volume.
    volume: Volume,
    /// `m_bNeedBreaks`: the break cache is not computed.
    need_breaks: bool,
    /// `m_sBreaks`: cached breaks (curve knots plus volume crossings).
    breaks: Vec<f64>,
    /// `m_sConts`: continuity at each cached break.
    conts: Vec<ContinuityType>,
    /// `m_lOwnerFlag`: bit 1 owns the curve, bit 2 owns the volume (only the values
    /// 1 and 3, and 2 and 3, count as owning).
    owner_flag: u64,
}

/// The copy constructor `SmCrvInVolume::SmCrvInVolume(const SmCrvInVolume&)`
/// (`SmCrvInVolume.cpp:102-159`), not the virtual `Copy` (that is
/// [`CrvInVolume::copy`]): the owner flag is normalized from the recognized ownership
/// predicates, the break cache is kept only when it is current, and the base curve is
/// copied with the copy constructor (which fixes its knots up) when this object owns
/// it, but exactly - keeping the curve and its owner - when upstream would share it
/// instead. The volume is held by value, so it is a field-wise copy either way.
///
/// Infallible, unlike [`CrvInVolume::copy`]: the exact copy of a borrowed curve is
/// [`Curve`]'s `clone_exact_bounded`, which walks the same nesting-bounded graph the
/// derived `Clone` this replaces walked (see the module's nesting deviation).
impl Clone for CrvInVolume {
    fn clone(&self) -> Self {
        let owns_curve = self.owns_curve();
        let owns_volume = self.owns_volume();
        let owner_flag = match (owns_curve, owns_volume) {
            (true, true) => 3,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 0,
        };
        // copy knots when appropriate
        let (breaks, conts) = if self.need_breaks {
            (Vec::new(), Vec::new())
        } else {
            (self.breaks.clone(), self.conts.clone())
        };
        Self {
            curve: if owns_curve {
                // an owned curve is copied by its copy constructor
                self.curve.clone()
            } else {
                // a curve shared upstream is an exact copy here
                self.curve.clone_exact_bounded()
            },
            in_param_space: self.in_param_space,
            volume: self.volume.clone(),
            need_breaks: self.need_breaks,
            breaks,
            conts,
            owner_flag,
        }
    }
}

impl CrvInVolume {
    /// Exact evaluation copy after the caller validates the geometry depth.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            curve: self.curve.clone_exact_bounded(),
            in_param_space: self.in_param_space,
            volume: self.volume.clone(),
            need_breaks: self.need_breaks,
            breaks: self.breaks.clone(),
            conts: self.conts.clone(),
            owner_flag: self.owner_flag,
        }
    }

    /// An exact evaluation copy retaining owners, raw flags and cached breaks.
    /// Check the entire volume chain against the shared `MAX_GEOMETRY_NESTING` (32) bound before
    /// the volume's field-wise recursive `Clone`.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        crate::surface::surface::Surface::check_volume_evaluation_copy_depth(&self.volume, depth)?;
        Ok(Self {
            curve: self.curve.clone_for_evaluation_depth(depth)?,
            in_param_space: self.in_param_space,
            volume: self.volume.clone(),
            need_breaks: self.need_breaks,
            breaks: self.breaks.clone(),
            conts: self.conts.clone(),
            owner_flag: self.owner_flag,
        })
    }

    /// `SmCrvInVolume_TYPE`.
    pub const TYPE: i64 = CRV_IN_VOLUME_TYPE;

    /// A curve in `volume` (the constructor). `copy_flag` bit 1 (curve) and bit 2
    /// (volume) ask for copies, which upstream then owns: here they set those owner
    /// bits (the parts are held by value). `owner_flag` is the initial owner flag.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `curve` or `volume` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1.
    pub fn new(
        curve: Curve,
        in_param_space: bool,
        volume: Volume,
        copy_flag: u64,
        owner_flag: u64,
    ) -> Result<Self> {
        check_geometry_nesting(NestedGeometry::Curve(&curve), 1, OP)?;
        check_geometry_nesting(NestedGeometry::Volume(&volume), 1, OP)?;
        let mut owner_flag = owner_flag;
        // When Curve is to be copied - copy it
        if copy_flag == 1 || copy_flag == 3 {
            owner_flag |= 1;
        }
        // when Volume is to be copied - copy it
        if copy_flag == 2 || copy_flag == 3 {
            owner_flag |= 2;
        }
        Ok(Self {
            curve,
            in_param_space,
            volume,
            need_breaks: true,
            breaks: Vec::new(),
            conts: Vec::new(),
            owner_flag,
        })
    }

    /// The copy constructor (`Copy`): copies both parts, keeps the break cache when
    /// it is current, and sets the owner flag to 3, 1, 2 or 0 from what this object
    /// owns. A curve this object does not own is shared upstream, so here it is an
    /// exact copy that keeps its owner (`Curve::clone` would fix its knots up).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::clone_for_evaluation`] on a curve not owned (geometry
    /// nested too deeply).
    pub fn copy(&self) -> Result<Self> {
        let owns_curve = self.owns_curve();
        let owns_volume = self.owns_volume();
        let owner_flag = if owns_curve && owns_volume {
            3
        } else if owns_curve {
            1
        } else if owns_volume {
            2
        } else {
            0
        };
        let (breaks, conts) = if self.need_breaks {
            (Vec::new(), Vec::new())
        } else {
            (self.breaks.clone(), self.conts.clone())
        };
        let curve = if owns_curve {
            self.curve.clone()
        } else {
            self.curve.clone_for_evaluation()?
        };
        Ok(Self {
            curve,
            in_param_space: self.in_param_space,
            volume: self.volume.clone(),
            need_breaks: self.need_breaks,
            breaks,
            conts,
            owner_flag,
        })
    }

    /// `CreateMirrorCurve`: a copy of this curve whose volume is mirrored about
    /// `mirror_plane`. The parts are held by value, so the copy never shares (and the
    /// mirror never edits) this curve's volume, which upstream does for a volume the
    /// curve does not own.
    ///
    /// # Errors
    ///
    /// Those of [`Volume::mirror`].
    pub fn create_mirror_curve(&self, mirror_plane: &Axis2Placement) -> Result<Self> {
        // copy this curve
        let mut new_curve = self.copy()?;
        // mirror the the volume
        new_curve.volume.mirror(mirror_plane)?;
        Ok(new_curve)
    }

    // ---- simple access --------------------------------------------------------------

    /// The base curve (`GetCurve`).
    #[must_use]
    pub const fn curve(&self) -> &Curve {
        &self.curve
    }

    /// The mapping volume (`GetVolume`).
    #[must_use]
    pub const fn volume(&self) -> &Volume {
        &self.volume
    }

    /// Whether the base curve is in the volume's parameter space (`GetInParamSpace`).
    #[must_use]
    pub const fn in_param_space(&self) -> bool {
        self.in_param_space
    }

    /// The owner flag (`m_lOwnerFlag`).
    #[must_use]
    pub const fn owner_flag(&self) -> u64 {
        self.owner_flag
    }

    /// Whether the break cache still has to be computed (`m_bNeedBreaks`).
    #[must_use]
    pub const fn need_breaks(&self) -> bool {
        self.need_breaks
    }

    /// The cached breaks (`m_sBreaks`).
    #[must_use]
    pub fn breaks(&self) -> &[f64] {
        &self.breaks
    }

    /// The cached continuities (`m_sConts`).
    #[must_use]
    pub fn conts(&self) -> &[ContinuityType] {
        &self.conts
    }

    /// Loads the break cache, as `ReadFromDB` does: `need_breaks`, the breaks and
    /// their continuities (upstream reads the same count of each; any lengths are
    /// accepted here, [`Self::generate_mults`] checks them).
    pub fn set_breaks_cache(
        &mut self,
        need_breaks: bool,
        breaks: Vec<f64>,
        conts: Vec<ContinuityType>,
    ) {
        self.need_breaks = need_breaks;
        self.breaks = breaks;
        self.conts = conts;
    }

    /// `OwnsCurve`: owner flag 1 or 3.
    #[must_use]
    pub const fn owns_curve(&self) -> bool {
        self.owner_flag == 1 || self.owner_flag == 3
    }

    /// `OwnsVolume`: owner flag 2 or 3.
    #[must_use]
    pub const fn owns_volume(&self) -> bool {
        self.owner_flag == 2 || self.owner_flag == 3
    }

    /// `IsAnalytic`: never.
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        false
    }

    /// `IsBounded`: the base curve's.
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.curve.is_bounded()
    }

    /// `GetNaturalInterval`: the base curve's.
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        self.curve.natural_interval()
    }

    /// Image-space dimension (`GetDim`): always 3.
    #[must_use]
    pub const fn dim(&self) -> u64 {
        3
    }

    /// `GetType`.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `GetTypeString`.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmCrvInVolume_TYPE"
    }

    /// `GetClassString`.
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmCrvInVolume"
    }

    /// `IsKindOf`: `SmCrvInVolume_TYPE` or a base type.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || curve_is_kind_of(t)
    }

    /// `operator==`: same parameter-space flag, equal base curves and equal volumes
    /// (`SmCurve::operator==` holds: same type, dimension 3).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the curve comparison or from comparing an
    /// unported volume kind (reached only when the flags and curves agree).
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }
        // check equivalence of these objects
        Ok(self.in_param_space == other.in_param_space
            && self.curve.approx_eq(k, &other.curve)?
            && self.volume.approx_eq(&other.volume)?)
    }

    // ---- evaluation -----------------------------------------------------------------

    /// The volume's partial derivatives `0..=n` at the base curve point `crv[0]`,
    /// each direction evaluated from `from_left` where the curve tangent `crv[1]`
    /// points forward along it, else from the other side.
    fn volume_derivs(
        &self,
        crv: &[Vec3],
        n: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        let p = at(crv, 0);
        let t = at(crv, 1);
        let sides = [
            if t.x > 0.0 { from_left } else { !from_left },
            if t.y > 0.0 { from_left } else { !from_left },
            if t.z > 0.0 { from_left } else { !from_left },
        ];
        let opts = EvalOptions {
            from_left: sides,
            non_zero_tangents,
            ..EvalOptions::default()
        };
        if self.in_param_space {
            // Volume Evaluate - from Param to compounded last OutSpace
            self.volume.evaluate(p, n, &opts)
        } else {
            // Volume map - from InSpace to compounded last OutSpace
            self.volume.map(p, n, &opts)
        }
    }

    /// Position and derivatives `0..=num_derivs` at `t` (`Evaluate`): the base curve
    /// evaluated (up to third derivatives) and lifted through the volume. When the
    /// volume cannot give that many derivatives, fewer are tried; derivatives above
    /// the third are zero.
    ///
    /// # Errors
    ///
    /// Those of the base curve's evaluation; the volume's last failure;
    /// [`SolidError::Unported`] from the volume.
    pub fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        // The output holds `num_derivs + 1` vectors: bound the caller's order.
        crate::curve::check_eval_derivs(num_derivs, OP)?;
        // limit the number of derivatives to 3
        let mut n_eval = num_derivs.min(MAX_DERIVS);

        // Curve eval - get param point
        let crv = self
            .curve
            .evaluate(k, t, n_eval, from_left, non_zero_tangents)?;

        // Volume Evaluate / Map - with fewer derivatives while the volume can't
        let mut vol = self.volume_derivs(&crv, n_eval, from_left, non_zero_tangents);
        while let Err(e) = &vol {
            if e.is_unported() || n_eval == 0 {
                break;
            }
            n_eval -= 1;
            vol = self.volume_derivs(&crv, n_eval, from_left, non_zero_tangents);
        }
        let vol = vol?;

        // For subscripting into the volume pt/deriv array
        let ss = |u: usize, v: usize, w: usize| at(&vol, (u * (n_eval + 1) + v) * (n_eval + 1) + w);

        let mut out = vec![Vec3::UNDEF; num_derivs + 1];
        let mut put = |i: usize, v: Vec3| {
            if let Some(o) = out.get_mut(i) {
                *o = v;
            }
        };

        // Position
        put(0, at(&vol, 0));

        let v1 = || [ss(1, 0, 0), ss(0, 1, 0), ss(0, 0, 1)];
        let v2 = || {
            [
                ss(2, 0, 0),
                ss(1, 1, 0),
                ss(1, 0, 1),
                ss(0, 2, 0),
                ss(0, 1, 1),
                ss(0, 0, 2),
            ]
        };

        // 1st Derivs
        if n_eval >= 1 {
            put(1, lift_first_derivative(at(&crv, 1), v1()));
        }

        // 2nd Derivs
        if n_eval >= 2 {
            put(
                2,
                lift_second_derivative([at(&crv, 1), at(&crv, 2)], v1(), v2()),
            );
        }

        // 3rd Derivs
        if n_eval >= 3 {
            let v3 = [
                ss(3, 0, 0),
                ss(2, 1, 0),
                ss(2, 0, 1),
                ss(1, 2, 0),
                ss(1, 1, 1),
                ss(1, 0, 2),
                ss(0, 3, 0),
                ss(0, 2, 1),
                ss(0, 1, 2),
                ss(0, 0, 3),
            ];
            put(
                3,
                lift_third_derivative([at(&crv, 1), at(&crv, 2), at(&crv, 3)], v1(), v2(), v3),
            );
        }

        // If too many requested, fill with zeros.
        for ii in MAX_DERIVS + 1..=num_derivs {
            put(ii, Vec3::new(0.0, 0.0, 0.0));
        }

        Ok(out)
    }

    /// The point at `t` (`EvaluatePoint`). Upstream maps the base curve point
    /// through `MapPoint` first in either case, then evaluates or maps it.
    ///
    /// # Errors
    ///
    /// Those of the base curve's evaluation; [`SolidError::Unported`] from the volume.
    pub fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        // Curve eval - Get ParamSpace point
        let p = self.curve.evaluate_point(k, t)?;

        // volume eval
        self.volume.map_point(p, true)?;

        if self.in_param_space {
            // Volume Evaluate - from Param to compounded last OutSpace
            volume_evaluate_point(&self.volume, p)
        } else {
            // Volume map - from InSpace to compounded last OutSpace
            self.volume.map_point(p, true)
        }
    }

    // ---- breaks ---------------------------------------------------------------------

    /// The cached breaks inside `interval` (`ContainsValue`, no tolerance) or all of
    /// them, with their multiplicities from [`Self::generate_mults`] (status
    /// ignored, as upstream: none when its state check fails, else partly filled).
    fn cached_knots(&self, interval: Option<&Extent1d>) -> (Vec<f64>, Vec<usize>) {
        let knots: Vec<f64> = match interval {
            None => self.breaks.clone(),
            Some(ivl) => self
                .breaks
                .iter()
                .copied()
                .filter(|&b| ivl.contains_value(b, 0.0))
                .collect(),
        };
        // GenerateMults sizes the output only after its state check
        let mut mults = if self.need_breaks || self.breaks.len() != self.conts.len() {
            Vec::new()
        } else {
            vec![0; knots.len()]
        };
        let _ = self.fill_mults(&knots, &mut mults);
        (knots, mults)
    }

    /// The uncached breaks: the base curve's knots with multiplicities adjusted to
    /// a cubic, plus every classification boundary not already a knot (as a C1
    /// knot, multiplicity 2). Returns the breaks, multiplicities and status, and the
    /// UVW tolerance; a failing classification ends it with the adjusted base
    /// knots written (upstream's `SER`).
    fn compute_breaks(&self, k: &Kernel) -> Result<(KnotsWithStatus, f64)> {
        // pick a UVWSpace tolerance
        let volume_param_size = self
            .volume
            .natural_param_domain()?
            .approximate_unbounded(None, BOUNDED_INFINITE_PARAM)
            .max_dimension();
        let curve_param_size = self
            .curve
            .natural_interval()
            .approximate_unbounded(0.0, BOUNDED_INFINITE_PARAM)
            .length();
        let tol_uvw = EFF_ZERO * (1.0 + max2(volume_param_size, curve_param_size));

        // get the Curve knots and multiplicities for the natural curve (status ignored)
        let (mut knots, mut mults) = ignored(self.curve.knots(k, None))?.unwrap_or_default();

        // modify the multiplicity list to act as if Curve is a degree 3 BSpline
        let crv_degree = self.curve.degree()?;
        if crv_degree != 3 {
            for m in &mut mults {
                *m = if crv_degree < *m + 3 {
                    *m + 3 - crv_degree
                } else {
                    1
                };
            }
        }

        // classify the BaseCurve against the volume knot isoParameter lines
        let classes = match classify_against_volume(
            k,
            &self.curve,
            &self.curve.natural_interval(),
            tol_uvw,
            &self.volume,
            self.in_param_space,
        ) {
            Ok(classes) => classes,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(e) => {
                let status = Err(e);
                return Ok((
                    KnotsWithStatus {
                        knots,
                        mults,
                        status,
                    },
                    tol_uvw,
                ));
            }
        };

        // add the classification boundaries uniquely and in order to the knots and mults
        let cnt = classes.len();
        let short = || {
            SolidError::invalid_input(OP_KNOTS, "no base curve knots or classification intervals")
        };
        let mut uvw_knot = *knots.first().ok_or_else(short)?;
        let mut jj = 0usize;
        for ii in 0..=cnt {
            let class_bndry = if ii < cnt {
                classes.get(ii).ok_or_else(short)?.min()
            } else {
                classes.get(ii.wrapping_sub(1)).ok_or_else(short)?.max()
            };

            // check the next UVWKnots until we get one bigger or equal to class_bndry
            while class_bndry > uvw_knot + tol_uvw {
                jj += 1;
                uvw_knot = if jj == knots.len() {
                    BIG_DOUBLE
                } else {
                    *knots.get(jj).ok_or_else(short)?
                };
            }

            // when class_bndry value is already in knots - move onto next ClassKnot
            if class_bndry > uvw_knot - tol_uvw {
                continue;
            }

            // add class boundary as if it were a C1 knot into the curve
            if jj > knots.len() {
                return Err(short());
            }
            knots.insert(jj, class_bndry);
            mults.insert(jj.min(mults.len()), 2);

            // skip to next UVWKnot and continue
            jj += 1;
            if jj >= knots.len() {
                jj = knots.len() - 1;
            }
            uvw_knot = *knots.get(jj).ok_or_else(short)?;
        }
        Ok((KnotsWithStatus::success(knots, mults), tol_uvw))
    }

    /// The continuity of each break from its multiplicity on a cubic (the ends are
    /// discontinuous).
    fn conts_of(mults: &[usize]) -> Vec<ContinuityType> {
        let cnt = mults.len();
        mults
            .iter()
            .enumerate()
            .map(|(ii, &m)| {
                if ii == 0 || ii == cnt - 1 {
                    ContinuityType::Discontinuous
                } else {
                    match m {
                        0 => ContinuityType::CInfinity,
                        1 => ContinuityType::C1C2,
                        2 => ContinuityType::C1,
                        3 => ContinuityType::C0,
                        _ => ContinuityType::Discontinuous,
                    }
                }
            })
            .collect()
    }

    /// Drops the breaks (and multiplicities) outside `interval` (with `tol`) when the
    /// first or last break lies outside it.
    fn cull(
        knots: &mut Vec<f64>,
        mults: &mut Vec<usize>,
        interval: Option<&Extent1d>,
        tol: f64,
    ) -> Result<()> {
        let Some(ivl) = interval else {
            return Ok(());
        };
        let short = || SolidError::invalid_input(OP_KNOTS, "no breaks to cull");
        let first = *knots.first().ok_or_else(short)?;
        let last = *knots.last().ok_or_else(short)?;
        if !(first < ivl.min() - tol || last > ivl.max() + tol) {
            return Ok(());
        }
        let min = ivl.min();
        let max = ivl.max();
        let mut jj = 0usize;
        for ii in 0..knots.len() {
            let class_bndry = *knots.get(ii).ok_or_else(short)?;
            if min > class_bndry + tol {
                // below the interval: don't increment jj
            } else if max < class_bndry - tol {
                break;
            } else {
                // when needed - compress output arrays
                if ii != jj {
                    if let Some(k) = knots.get_mut(jj) {
                        *k = class_bndry;
                    }
                    let m = mults.get(ii).copied().unwrap_or(0);
                    if let Some(d) = mults.get_mut(jj) {
                        *d = m;
                    }
                }
                jj += 1;
            }
        }
        knots.truncate(jj);
        mults.truncate(jj);
        Ok(())
    }

    /// The breaks ("knots") and their effective multiplicities, optionally limited
    /// to `interval` (`GetKnots`), computing and caching them when needed; on a
    /// failing classification, the adjusted base curve knots with that status
    /// (nothing cached or culled, as upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the volume and the curve classification;
    /// [`SolidError::InvalidInput`] where upstream would read outside its arrays.
    pub fn knots_with_status(
        &mut self,
        k: &Kernel,
        interval: Option<&Extent1d>,
    ) -> Result<KnotsWithStatus> {
        // low work - Knots and Conts already cached
        if !self.need_breaks {
            let (knots, mults) = self.cached_knots(interval);
            return Ok(KnotsWithStatus::success(knots, mults));
        }
        let (mut r, tol) = self.compute_breaks(k)?;
        if r.status.is_err() {
            return Ok(r);
        }

        // cache the knots
        self.breaks.clone_from(&r.knots);
        self.need_breaks = false;
        self.conts = Self::conts_of(&r.mults);

        // when given an interval - cull unwanted output values
        Self::cull(&mut r.knots, &mut r.mults, interval, tol)?;
        Ok(r)
    }

    /// [`Self::knots_with_status`] without storing the cache (for `&self` callers):
    /// the same answer as upstream's first call.
    ///
    /// # Errors
    ///
    /// As [`Self::knots_with_status`].
    pub fn knots_with_status_ref(
        &self,
        k: &Kernel,
        interval: Option<&Extent1d>,
    ) -> Result<KnotsWithStatus> {
        if !self.need_breaks {
            let (knots, mults) = self.cached_knots(interval);
            return Ok(KnotsWithStatus::success(knots, mults));
        }
        let (mut r, tol) = self.compute_breaks(k)?;
        if r.status.is_ok() {
            Self::cull(&mut r.knots, &mut r.mults, interval, tol)?;
        }
        Ok(r)
    }

    /// [`Self::knots_with_status`] with a failing status as the error.
    ///
    /// # Errors
    ///
    /// As [`Self::knots_with_status`], and its failing status.
    pub fn knots(
        &mut self,
        k: &Kernel,
        interval: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.knots_with_status(k, interval)?.into_result()
    }

    /// [`Self::knots_with_status_ref`] with a failing status as the error.
    ///
    /// # Errors
    ///
    /// As [`Self::knots`].
    pub fn knots_ref(
        &self,
        k: &Kernel,
        interval: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.knots_with_status_ref(k, interval)?.into_result()
    }

    /// Fills `mults` (one per knot, zero-initialised) from the cached continuities.
    fn fill_mults(&self, knots: &[f64], mults: &mut [usize]) -> Result<()> {
        // check state - cached breaks and conts are current
        if self.need_breaks || self.breaks.len() != self.conts.len() {
            return Err(SolidError::sm_err(OP_MULTS));
        }

        let mut bi = 0usize;
        for (ii, &param) in knots.iter().enumerate() {
            let mut found = false;

            // find the corresponding break value
            while let Some(&brk) = self.breaks.get(bi) {
                // when break matches current knot
                if is_zero_to_tol(param - brk, EFF_ZERO) {
                    found = true;
                    break;
                } else if brk > param {
                    // given knot value is not a discontinuity
                    if let Some(m) = mults.get_mut(ii) {
                        *m = 0;
                    }
                    break;
                }
                bi += 1;
            }

            // error condition - ran out of breaks to search
            if bi >= self.breaks.len() {
                return Err(SolidError::sm_err(OP_MULTS));
            }

            // when a match was not found move onto the next Knot
            if !found {
                continue;
            }

            // check cache currency (upstream indexes the continuities by the knot index)
            let cont = *self.conts.get(ii).ok_or_else(|| {
                SolidError::invalid_input(OP_MULTS, "more knots matched than continuities")
            })?;
            if cont == ContinuityType::Undefined {
                return Err(SolidError::sm_err(OP_MULTS));
            }

            // set an effective multiplicity
            let m = match cont {
                ContinuityType::Discontinuous => 3,
                c if c as i32 <= ContinuityType::G1G2G3 as i32 => 2,
                c if c as i32 <= ContinuityType::C1C2G3 as i32 => 1,
                _ => 0,
            };
            if let Some(d) = mults.get_mut(ii) {
                *d = m;
            }
        }
        Ok(())
    }

    /// The effective multiplicities of `knots` (ascending; expected among the cached
    /// breaks) from the cached continuities, as if the curve were a cubic
    /// (`GenerateMults`): 3 for a discontinuity, 2 up to G1-G2-G3, 1 up to
    /// C1-C2-G3, else 0; 0 for a knot that is not a break.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the cache is not current, a knot lies past the last break or a
    /// continuity is undefined; [`SolidError::InvalidInput`] when a matched knot's
    /// index has no continuity (upstream reads past the array).
    pub fn generate_mults(&self, knots: &[f64]) -> Result<Vec<usize>> {
        let mut mults = vec![0; knots.len()];
        self.fill_mults(knots, &mut mults)?;
        Ok(mults)
    }

    /// The sum of the break multiplicities (`GetNumberNaturalKnots`), caching the
    /// breaks. `GetKnots`' status is ignored upstream; here only an error before
    /// any break is known is reported.
    ///
    /// # Errors
    ///
    /// Those of [`Self::knots`].
    pub fn number_natural_knots(&mut self, k: &Kernel) -> Result<usize> {
        let mults = self.knots_with_status(k, None)?.mults;
        Ok(mults.iter().sum())
    }

    /// [`Self::number_natural_knots`] without storing the cache.
    ///
    /// # Errors
    ///
    /// Those of [`Self::knots_ref`].
    pub fn number_natural_knots_ref(&self, k: &Kernel) -> Result<usize> {
        let mults = self.knots_with_status_ref(k, None)?.mults;
        Ok(mults.iter().sum())
    }

    /// The continuities at `breaks`: the ends discontinuous, each interior break
    /// evaluated from both sides with `angle_tol`; and their minimum over the
    /// interior (`CInfinity` without interior breaks).
    fn continuities_at(
        &self,
        k: &Kernel,
        breaks: &[f64],
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        let n = breaks.len();
        if n == 0 {
            return Err(SolidError::invalid_input(OP_CONTS, "no breaks"));
        }
        let mut min_cont = ContinuityType::CInfinity;
        let mut conts = vec![ContinuityType::Undefined; n];
        if let Some(c) = conts.first_mut() {
            *c = ContinuityType::Discontinuous;
        }
        if let Some(c) = conts.last_mut() {
            *c = ContinuityType::Discontinuous;
        }
        for ii in 1..n - 1 {
            let t = *breaks
                .get(ii)
                .ok_or_else(|| SolidError::invalid_input(OP_CONTS, "no breaks"))?;
            // evaluate geometric continuities
            let c = evaluate_continuity(k, self, t, true, self, t, false, angle_tol)?.continuity;
            if let Some(d) = conts.get_mut(ii) {
                *d = c;
            }
            // save min continuity
            if min_cont as i32 > c as i32 {
                min_cont = c;
            }
        }
        Ok((min_cont, conts))
    }

    /// The minimum interior continuity and the continuity at every break
    /// (`CalculateContinuities`), caching the breaks.
    ///
    /// # Errors
    ///
    /// Those of [`Self::knots`] and of the evaluations;
    /// [`SolidError::InvalidInput`] without breaks.
    pub fn calculate_continuities(
        &mut self,
        k: &Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        let breaks = self.knots_with_status(k, None)?.knots;
        self.continuities_at(k, &breaks, angle_tol)
    }

    /// [`Self::calculate_continuities`] without storing the cache.
    ///
    /// # Errors
    ///
    /// As [`Self::calculate_continuities`].
    pub fn calculate_continuities_ref(
        &self,
        k: &Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        let breaks = self.knots_with_status_ref(k, None)?.knots;
        self.continuities_at(k, &breaks, angle_tol)
    }

    // ---- editing --------------------------------------------------------------------

    /// `EditParameterization`: rescales the cached breaks to `new_param` (or drops
    /// the cache when either interval has zero length), then reparameterises the
    /// base curve.
    ///
    /// # Errors
    ///
    /// Those of the base curve's edit (the breaks are rescaled already).
    pub fn edit_parameterization(&mut self, k: &Kernel, new_param: &Extent1d) -> Result<()> {
        // when appropriate scale breaks
        if !self.need_breaks {
            let ivl = self.curve.natural_interval();
            if ivl.length() > EFF_ZERO && new_param.length() > EFF_ZERO {
                // for every break
                for b in &mut self.breaks {
                    *b = ((*b - ivl.min()) / ivl.length()) * new_param.length() + new_param.min();
                }
            } else {
                // working with zero length intervals
                self.breaks.clear();
                self.conts.clear();
                self.need_breaks = true;
            }
        }

        // pass the call along to the curve
        self.curve.edit_parameterization(k, new_param)
    }

    /// `ReverseParameterization`: reverses the base curve; returns the new interval
    /// of `old_interval`. The break cache is kept, as upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the curve is not owned; those of the base curve's reversal.
    pub fn reverse_parameterization(
        &mut self,
        k: &Kernel,
        old_interval: &Extent1d,
    ) -> Result<Extent1d> {
        // Don't own curve can not reverse parameterization
        if !self.owns_curve() {
            return Err(SolidError::sm_err(OP));
        }
        self.curve.reverse_parameterization(k, old_interval)
    }

    /// `Trim`: trims the base curve (`trim_interval` may be snapped) and drops the
    /// break cache.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the curve is not owned; those of the base curve's trim.
    pub fn trim(&mut self, k: &Kernel, trim_interval: &mut Extent1d, mode: TrimMode) -> Result<()> {
        self.trim_with_zone_tol(
            k,
            trim_interval,
            mode,
            crate::geom::tol::zone_tol_3d(k, crate::geom::tol::SmallTopoType::NormalSize),
        )
    }

    /// Trims the owned curve with the caller-resolved owning object's tolerance.
    pub(crate) fn trim_with_zone_tol(
        &mut self,
        k: &Kernel,
        trim_interval: &mut Extent1d,
        mode: TrimMode,
        zone_tol_3d: f64,
    ) -> Result<()> {
        // Don't own curve can not trim the curve
        if !self.owns_curve() {
            return Err(SolidError::sm_err(OP));
        }

        // pass the call along to the Curve
        self.curve
            .trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d)?;

        // clear the knots
        if !self.need_breaks {
            self.breaks.clear();
            self.conts.clear();
            self.need_breaks = true;
        }
        Ok(())
    }

    /// `Transform`: compounds the rotate-and-move (after the optional scale) onto
    /// the volume; nothing for the identity.
    ///
    /// # Errors
    ///
    /// Those of [`Volume::transform`], including `InvalidInput` if appending a
    /// volume map would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    pub fn transform(&mut self, rotate_n_move: &Axis2Placement, scale: Option<Vec3>) -> Result<()> {
        self.transform_at_depth(rotate_n_move, scale, 0)
    }

    pub(crate) fn transform_at_depth(
        &mut self,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // compound the Transform onto the contained volume
        self.volume
            .transform_at_depth(rotate_n_move, scale, depth + 1)
    }

    // ---- exact B-spline -------------------------------------------------------------

    /// `MakeExactBSplineIfPossible`: the exact B-spline equivalent (upstream an
    /// `SmBSplineCurve` or a derived kind) of the base curve mapped through every
    /// compounded map of the volume to its final out-space; `None` (upstream `NULL`
    /// with `SM_SUCCESS`) when a map cannot produce one.
    ///
    /// # Errors
    ///
    /// Those of the curve copies (a twist copies any input curve; upstream then
    /// dereferences the `NULL` a failed `Copy` leaves); [`SolidError::Unported`] from
    /// the curve and volume services; `InvalidInput` for a volume chain deeper than
    /// `MAX_GEOMETRY_NESTING`.
    pub fn make_exact_b_spline_if_possible_with(&self, k: &Kernel) -> Result<Option<Curve>> {
        // Ask Volume to make exact BSpline if Possible
        if self.in_param_space {
            make_exact_b_spline_out_curve_from_param_space(k, &self.volume, &self.curve)
        } else {
            make_exact_b_spline_out_curve_from_in_space(k, &self.volume, &self.curve)
        }
    }

    // ---- persistence ----------------------------------------------------------------

    /// Reads a curve in volume from `db` (database version `version`) (`ReadFromDB`):
    /// the cache flag, the base curve (type header and data), the parameter-space
    /// flag, the volume (type header and data) and, when cached, the breaks and
    /// their continuities. The new object owns both parts (owner flag 3).
    ///
    /// # Errors
    ///
    /// Those of the reads and of the base curve's reader; [`SolidError::Unported`]
    /// from the volume reader; [`SolidError::InvalidInput`] for an unreadable type
    /// header, a count larger than the input or an unknown continuity code.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        let ascii = db.file_type() == FileType::Ascii;
        let no_stream = || SolidError::invalid_input(OP_READ, "no input stream");

        // cache flag
        let need_knots = if ascii {
            let v = db
                .in_stream_mut()
                .ok_or_else(no_stream)?
                .extract_int()
                .ok_or_else(|| SolidError::invalid_input(OP_READ, "cache flag not read"))?;
            db.go_to_next_line();
            v != 0
        } else {
            db.read_boolean()? != 0
        };

        // nested curve
        if ascii {
            db.go_to_next_line();
        }
        let header = db.read_type(true)?;
        let curve_type = header
            .type_code
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "curve type not read"))?;
        let curve_dim = header
            .dim
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "curve dimension not read"))?;
        let curve = Curve::read_from_db(k, curve_type, db, curve_dim, version)?;

        // bInParamSpace value (extracted as text in either file type; the empty
        // object's TRUE stays when the extraction is skipped)
        let in_param_space = db
            .in_stream_mut()
            .ok_or_else(no_stream)?
            .extract_int()
            .is_none_or(|v| v != 0);
        db.go_to_next_line();

        // compounding volume
        if ascii {
            db.go_to_next_line();
        }
        let volume_type = db
            .read_type(false)?
            .type_code
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "volume type not read"))?;
        let volume = volume_read_from_db(volume_type, db, version)?;

        // when appropriate - knots
        let mut breaks = Vec::new();
        let mut conts = Vec::new();
        if !need_knots {
            let count = if ascii {
                let n = db
                    .in_stream_mut()
                    .ok_or_else(no_stream)?
                    .extract_ulong()
                    .ok_or_else(|| SolidError::invalid_input(OP_READ, "break count not read"))?;
                db.go_to_next_line();
                n
            } else {
                db.read_ulong()?
            };
            let count = usize::try_from(count)
                .map_err(|_| SolidError::invalid_input(OP_READ, "break count too large"))?;
            db.ensure_available(count, if ascii { 1 } else { 8 }, OP_READ)?;

            // read every knot
            if ascii {
                let s = db.in_stream_mut().ok_or_else(no_stream)?;
                for _ in 0..count {
                    breaks.push(s.extract_double().unwrap_or(0.0));
                }
                if count > 0 {
                    db.go_to_next_line();
                }
            } else {
                breaks = db.read_doubles(count)?;
            }

            // read every Continuity
            let mut codes = Vec::new(); // grown as read: count comes from the file
            if ascii {
                let s = db.in_stream_mut().ok_or_else(no_stream)?;
                for _ in 0..count {
                    codes.push(s.extract_ulong().unwrap_or(0) as i64);
                }
                if count > 0 {
                    db.go_to_next_line();
                }
            } else {
                for _ in 0..count {
                    codes.push(db.read_ulong()? as i64);
                }
            }
            for c in codes {
                conts.push(continuity_from_code(c).ok_or_else(|| {
                    SolidError::invalid_input(OP_READ, "unknown continuity code")
                })?);
            }
        }

        // load obj
        Ok(Self {
            curve,
            in_param_space,
            volume,
            need_breaks: need_knots,
            breaks,
            conts,
            owner_flag: 3,
        })
    }
}

impl CurveEvaluate for CrvInVolume {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, k, t, num_derivs, from_left, non_zero_tangents)
    }

    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, k, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;
    use crate::surface::bend_volume::BendVolume;
    use crate::surface::volume::VolumeKind;
    use crate::topology::database_io::InputStream;

    /// A clamped cubic with one interior knot at 0.5 on `[0, 1]`.
    fn base() -> Curve {
        let mut cur = allocate_nurb_curve(4, 3, 8).unwrap();
        for (i, p) in cur.cpts_mut().iter_mut().enumerate() {
            p.x = i as f64;
            p.y = (i * i) as f64 * 0.1;
            p.z = 0.0;
            p.w = -f64::MAX;
        }
        cur.knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0]);
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    fn civ(owner: u64) -> CrvInVolume {
        CrvInVolume::new(
            base(),
            true,
            Volume::new(VolumeKind::Bend(BendVolume::new())),
            0,
            owner,
        )
        .unwrap()
    }

    fn cached(owner: u64) -> CrvInVolume {
        let mut c = civ(owner);
        c.set_breaks_cache(
            false,
            vec![0.0, 0.25, 0.5, 1.0],
            vec![
                ContinuityType::Discontinuous,
                ContinuityType::C1,
                ContinuityType::G1,
                ContinuityType::Discontinuous,
            ],
        );
        c
    }

    #[test]
    fn huge_derivative_orders_are_rejected() {
        // fuzz: `n = usize::MAX` overflowed the output size (capacity overflow panic)
        let c = civ(0);
        for n in [crate::curve::MAX_EVAL_DERIVS + 1, 1 << 31, usize::MAX] {
            assert!(matches!(
                c.evaluate(&Kernel::default(), 0.3, n, true, false),
                Err(SolidError::InvalidInput { .. })
            ));
        }
    }

    #[test]
    fn construct_and_flags() {
        let c = CrvInVolume::new(
            base(),
            false,
            Volume::new(VolumeKind::Bend(BendVolume::new())),
            3,
            0,
        )
        .unwrap();
        assert_eq!(c.owner_flag(), 3);
        assert!(c.owns_curve() && c.owns_volume());
        let c = CrvInVolume::new(
            base(),
            false,
            Volume::new(VolumeKind::Bend(BendVolume::new())),
            1,
            4,
        )
        .unwrap();
        assert_eq!(c.owner_flag(), 5);
        assert!(!c.owns_curve() && !c.owns_volume());
        assert_eq!(c.copy().unwrap().owner_flag(), 0);
        assert!(c.need_breaks());
        assert!(c.is_kind_of(CrvInVolume::TYPE) && c.is_kind_of(crate::curve::curve::p2::TYPE));
        assert_eq!(c.natural_interval().max(), 1.0);
        assert_eq!(c.type_string(), "SmCrvInVolume_TYPE");
    }

    #[test]
    fn generate_mults_from_cache() {
        let c = cached(3);
        assert_eq!(
            c.generate_mults(&[0.0, 0.25, 0.5, 1.0]).unwrap(),
            vec![3, 1, 2, 3]
        );
        // a knot that is not a break gets 0; conts indexed by the knot index
        assert_eq!(c.generate_mults(&[0.1, 0.5]).unwrap(), vec![0, 1]);
        // past the last break
        assert!(c.generate_mults(&[2.0]).is_err());
        let k = &Kernel::default();
        let (kn, m) = c
            .knots_ref(k, Some(&Extent1d::from_min_max(0.2, 0.6)))
            .unwrap();
        assert_eq!(kn, vec![0.25, 0.5]);
        assert_eq!(m, vec![3, 1]);
        assert_eq!(c.number_natural_knots_ref(k).unwrap(), 9);
    }

    #[test]
    fn uncached_knots_classify_against_the_volume() {
        let k = &Kernel::default();
        let mut c = civ(3);
        // GetKnots classifies the base curve (no longer deferred on ReSet)
        let r = c.knots(k, None);
        assert!(!matches!(r, Err(SolidError::Unported { .. })), "{r:?}");
        // the bend volume evaluates (a default bend divides by its zero neutral
        // distance)
        assert_eq!(c.evaluate(k, 0.5, 2, true, true).unwrap().len(), 3);
        assert!(c.evaluate_point(k, 0.5).is_ok());
    }

    #[test]
    fn edits() {
        let k = Kernel::default();
        let mut c = cached(3);
        c.edit_parameterization(&k, &Extent1d::from_min_max(0.0, 2.0))
            .unwrap();
        assert_eq!(c.breaks(), &[0.0, 0.5, 1.0, 2.0]);
        let mut ivl = Extent1d::from_min_max(0.25, 1.5);
        c.trim(&k, &mut ivl, TrimMode::Notify).unwrap();
        assert!(c.need_breaks() && c.breaks().is_empty());
        let mut d = cached(0);
        assert!(d.trim(&k, &mut ivl, TrimMode::Notify).is_err());
        assert!(d.reverse_parameterization(&k, &ivl).is_err());
        let (min, conts) = cached(3).continuities_at(&k, &[0.0, 1.0], 0.5).unwrap();
        assert_eq!(min, ContinuityType::CInfinity);
        assert_eq!(conts.len(), 2);
    }

    #[test]
    fn read_stops_at_the_volume() {
        let text = "1 Has knots flag\n Begin\nObject Type 14033\nCurve Dim 3\n";
        let mut db = DatabaseReader::with_stream(FileType::Ascii, InputStream::new(text.into()));
        assert!(CrvInVolume::read_from_db(&Kernel::default(), &mut db, 0).is_err());
    }
}
