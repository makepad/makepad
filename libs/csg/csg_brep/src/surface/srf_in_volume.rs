//! Surfaces in volumes: a surface mapped through a (compound) volume, from the
//! volume's parameter space or in-space to its final out-space (the generated
//! surface `SmSrfInVolume`).
//!
//!
//! Upstream: `source/SMLib/src/SmSrfInVolume.cpp`, `source/SMLib/inc/SmSrfInVolume.h`.
//!
//! | Rust                                           | Upstream                                          |
//! |------------------------------------------------|---------------------------------------------------|
//! | [`SrfInVolume`]                                | `SmSrfInVolume`                                   |
//! | [`SrfInVolume::new`]                           | `SmSrfInVolume(SmSurface&, SmBoolean, SmVolume&, ULONG, ULONG, const SmContext*)` |
//! | [`SrfInVolume::copy`], `Clone`                 | copy constructor, `Copy`                          |
//! | `Drop` (implicit)                              | `~SmSrfInVolume`                                  |
//! | [`SrfInVolume::approx_eq`]                     | `operator==`                                      |
//! | [`SrfInVolume::TYPE`], [`SrfInVolume::type_code`], [`SrfInVolume::type_string`], [`SrfInVolume::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`SrfInVolume::is_kind_of`]                    | `IsKindOf`                                        |
//! | [`SrfInVolume::surface`], [`SrfInVolume::volume`], [`SrfInVolume::in_param_space`] | `GetSurface`, `GetVolume`, `GetInParamSpace` |
//! | [`SrfInVolume::natural_uv_domain`], [`SrfInVolume::step_uv_domain`], [`SrfInVolume::max_analytic_domain`] | `GetNaturalUVDomain`, `GetSTEPUVDomain`, `GetMaxAnalyticDomain` |
//! | [`SrfInVolume::degree`]                        | `GetDegree`                                       |
//! | [`SrfInVolume::convert_uv_from_step_to_nurbs`], [`SrfInVolume::convert_uv_from_nurbs_to_step`] | `ConvertUVFromSTEPToNURBS`, `ConvertUVFromNURBSToSTEP` |
//! | [`SrfInVolume::owns_surface`], [`SrfInVolume::owns_volume`] | `OwnsSurface`, `OwnsVolume`          |
//! | [`SrfInVolume::set_owner_flag`], [`SrfInVolume::make_owner`] | `SetOwnerFlag`, `MakeOwner`         |
//! | [`SrfInVolume::set_base_surface`], [`SrfInVolume::set_volume`] | `SetBaseSurface`, `SetVolume`     |
//! | [`SrfInVolume::knots`]                         | `GetKnots`                                        |
//! | [`SrfInVolume::evaluate`]                      | `Evaluate`                                        |
//! | [`SrfInVolume::evaluate_simple`]               | `EvaluateSimple`                                  |
//! | [`SrfInVolume::evaluate_point`]                | `EvaluatePoint`                                   |
//! | [`SrfInVolume::evaluate_normal`]               | `EvaluateNormal`                                  |
//! | [`SrfInVolume::step_inversion`]                | `STEPInversion`                                   |
//! | [`SrfInVolume::reverse`], [`SrfInVolume::swap_uv`], [`SrfInVolume::trim_with_domain`], [`SrfInVolume::transform`] | `Reverse`, `SwapUV`, `TrimWithDomain`, `Transform` |
//! | [`SrfInVolume::split_at`]                      | `SplitAt`                                         |
//! | [`SrfInVolume::create_mirror_surface`]         | `CreateMirrorSurface`                             |
//! | [`SrfInVolume::make_exact_b_spline_if_possible_with`] | `MakeExactBSplineIfPossible` |
//! | `make_exact_b_spline_out_surface_from_{in,param}_space` (private) | `SmVolume::MakeExactBSplineOutSurfaceFrom{In,Param}Space` |
//! | `make_exact_b_spline_1st_out_surface` (private) | `SmVolume`/`SmBSplineVolume`/`SmTransform::MakeExactBSpline1stOutSurface` |
//! | `transform_axis2_placement` (private) | `SmTransform::LoadAxis2Placement`, `IsUnitOrthonormal` |
//! | [`SrfInVolume::read_from_db`]                  | `ReadFromDB`                                      |
//!
//! Deviations:
//! - **Nesting.** `new`, `set_base_surface` and `set_volume` return `InvalidInput` when the surface or the volume would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`]
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream).
//! - Compound evaluation copies carry a depth counter checked by `Surface` against the same bound.
//! - **Volumes.** The volume is a [`Volume`] (unit `surface.volume_p1`).
//!   `InvEvaluatePoint`, `InvMapPoint` and `ReadFromDB` are wired to
//!   `surface.volume_p2`. `SmVolume::Mirror` is in no unit in `units.json`: co-ported
//!   here as [`Volume::mirror`] (with `MirrorSimple` of the base class, `SmTransform`
//!   and `SmBSplineVolume`). `MakeExactBSplineOutSurfaceFrom{Param,In}Space` are in no
//!   unit either (not `surface.volume_p2`): they are ported here with the
//!   `MakeExactBSpline1stOutSurface` of `SmVolume`, `SmBSplineVolume` (both `NULL`)
//!   and `SmTransform` (with `LoadAxis2Placement` / `IsUnitOrthonormal`), as
//!   [`SrfInVolume::make_exact_b_spline_if_possible_with`], which takes the kernel the
//!   surface copy and transform need. The bend, unbend and twist overrides (also in
//!   no unit) are ported beside their volumes
//!   ([`BendVolume::make_exact_b_spline_1st_out_surface`](crate::surface::bend_volume::BendVolume::make_exact_b_spline_1st_out_surface) and the unbend and twist
//!   counterparts). (There is no kernel-less variant: upstream's
//!   `MakeExactBSplineIfPossible` takes the context.) The compounding loops stop with
//!   `InvalidInput` past `MAX_GEOMETRY_NESTING` maps (upstream walks the whole chain);
//!   the result is a [`Surface`] (upstream an `SmBSplineSurface*` that may be a
//!   derived kind).
//! - **Ownership.** The base surface and the volume are held by value (the surface
//!   boxed: it is itself a [`Surface`]). Upstream either shares them with the caller
//!   or owns (and maybe copies) them, as the copy and owner flags say; here the copy
//!   flags only set the owner bits (the value moved in is already this object's
//!   copy) and the owner flag is kept as data: it decides
//!   [`SrfInVolume::owns_surface`], [`SrfInVolume::owns_volume`] and whether
//!   `Reverse`, `SwapUV` and `TrimWithDomain` are allowed, as upstream. `Clone` is the
//!   copy constructor: an owned base surface gets the ordinary copy constructor, which
//!   fixes near-coincident knots up, a borrowed one an exact copy (upstream shares it,
//!   so no copy fix-up ever reaches it), the volume is copied in either case (upstream
//!   also copies it for owner flag 1, whose result does not own it) and the owner flag
//!   is recomputed as upstream. The exact copy is infallible and does not re-check the
//!   nesting: the constructors, the setters and the database reader keep the graph
//!   within the bound it relies on. The owner links of the contained objects
//!   (`SetOwner(this)`), the notifications (`Notify`) and the `SmContext` arguments
//!   (they only choose where upstream allocates) are not modelled.
//! - **`Evaluate`.** Out-params become the matrix `D[u][v]` of the requested size.
//!   Upstream's flat arrays are reproduced index for index, including the stride
//!   quirks of the special cases (`bOnlyUpperHalf == FALSE`, unequal orders). The
//!   parameter is clamped into the base surface's domain. Orders above 3 overrun
//!   upstream's fixed arrays: the port returns [`SolidError::InvalidInput`].
//!   Entries upstream leaves unwritten are [`Vec3::UNDEF`] (the default constructor).
//!   `Evaluate(uv, 0, 0, .., bOnlyUpperHalf = FALSE)` also takes the two-derivative
//!   branch and writes `Duv` one slot past the single entry requested (the caller's
//!   array must be at least that long); the port skips that write and returns
//!   success with `D`, as upstream does.
//! - **Not in this unit.** `GetMemoryUsed`, `Draw`, `DrawUV`, `Dump`, `WriteToDB`
//!   and `AssertValid` are out of scope (no counterpart, as for `SmCrvInVolume`).
//! - **`ReadFromDB`** always builds a new object (the "fill the surface passed in"
//!   branch and its `IsKindOf` check have no counterpart; `lType` and the context
//!   are unused). The nested surface read is depth-limited
//!   ([`DatabaseReader::enter_nested`]). A parameter-space flag whose extraction was
//!   skipped (the stream had already failed) leaves upstream's member uninitialised;
//!   the port returns [`SolidError::InvalidInput`].

use crate::error::{ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::determinate_4x4;
use crate::geom::tol::is_zero;
use crate::geom::transform::Transform;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::b_spline_surface::{LocationType, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::{
    check_geometry_nesting, is_kind_of as surface_is_kind_of, NestedGeometry, Surface, SurfaceKind,
    B_SPLINE_SURFACE_TYPE, MAX_GEOMETRY_NESTING, SRF_IN_VOLUME_TYPE,
};
use crate::surface::volume::p1::{
    lift_first_derivative_srf, lift_second_derivative_srf, lift_third_derivative_srf,
};
use crate::surface::volume::{EvalOptions, Volume, VolumeKind};
use crate::topology::database_io::{DatabaseReader, FileType};

const OP: &str = "surface::srf_in_volume::SrfInVolume";
const OP_EVAL: &str = "surface::srf_in_volume::SrfInVolume::evaluate";
const OP_NORMAL: &str = "surface::srf_in_volume::SrfInVolume::evaluate_normal";
const OP_STEP: &str = "surface::srf_in_volume::SrfInVolume::step_inversion";
const OP_EXACT: &str = "surface::srf_in_volume::SrfInVolume::make_exact_b_spline_if_possible_with";
const OP_READ: &str = "surface::srf_in_volume::SrfInVolume::read_from_db";

/// `MAX_DERIVS` of `Evaluate`: the volume lifts go up to third derivatives.
const MAX_DERIVS: usize = 3;

// ---- back-edges ----------------------------------------------------------------------

/// `SmVolume::Mirror` (in no unit in units.json; co-ported as [`Volume::mirror`]).
fn volume_mirror(volume: &mut Volume, plane: &Axis2Placement) -> Result<()> {
    volume.mirror(plane)
}

/// `SmTransform::LoadAxis2Placement`: the placement of a right-handed, unit,
/// orthonormal transform (`IsUnitOrthonormal`), else `None` (`SM_ERR`).
pub(crate) fn transform_axis2_placement(t: &Transform) -> Option<Axis2Placement> {
    // init output
    let mut placement = Axis2Placement::default();
    placement.init();

    // check for right-handed coordinate system (IsUnitOrthonormal)
    // Determinate must be 1
    let det = determinate_4x4(t.matrix());
    if !is_zero(det - 1.0) {
        return None;
    }
    // Each axis length must be one - need only to check two
    let x = t.to_x_axis();
    if !is_zero(x.dot(x) - 1.0) {
        return None;
    }
    let y = t.to_y_axis();
    if !is_zero(y.dot(y) - 1.0) {
        return None;
    }

    // set output (SetCanonical's status is ignored)
    let _ = placement.set_canonical(t.to_disp(), x, y);
    Some(placement)
}

/// `MakeExactBSpline1stOutSurface(eInputSpace, rInputSurface, rpNewSurface)` of one map
/// (no compounding), from in-space when `in_space` (`SM_VS_IN_SPACE`), else from
/// parameter space: `None` when the map cannot produce an exact B-spline.
///
/// `SmVolume` and `SmBSplineVolume` never can (header bodies); `SmTransform` copies a
/// B-spline-derived surface and transforms it by its placement when the matrix is a
/// right-handed unit orthonormal one (upstream asserts the orient map is NULL for a
/// parameter-space input and uses the same mapping either way); the bend, unbend and
/// twist overrides are [`BendVolume::make_exact_b_spline_1st_out_surface`](crate::surface::bend_volume::BendVolume::make_exact_b_spline_1st_out_surface) and its
/// unbend and twist counterparts.
fn make_exact_b_spline_1st_out_surface(
    k: &Kernel,
    map: &Volume,
    in_space: bool,
    surface: &Surface,
) -> Result<Option<Surface>> {
    match map.kind() {
        VolumeKind::Base | VolumeKind::BSpline(_) => Ok(None),
        VolumeKind::Transform(t) => {
            // for geometry derived from SmBSpline
            if surface.is_kind_of(B_SPLINE_SURFACE_TYPE)? {
                // for Transforms which can be represented by SmAxis2Placement mappings
                if let Some(placement) = transform_axis2_placement(t) {
                    // copy and transform the surface (statuses ignored)
                    let mut new_surface = surface.copy(k)?;
                    ignored(new_surface.transform(k, &placement, None))?;
                    return Ok(Some(new_surface));
                }
            }
            Ok(None)
        }
        VolumeKind::Bend(b) => b.make_exact_b_spline_1st_out_surface(k, map, in_space, surface),
        VolumeKind::Unbend(u) => u.make_exact_b_spline_1st_out_surface(k, map, in_space, surface),
        VolumeKind::Twist(t) => t.make_exact_b_spline_1st_out_surface(k, map, in_space, surface),
    }
}

/// `SmVolume::MakeExactBSplineOutSurfaceFromInSpace`: `surface` projected exactly
/// through every compounded map, or `None` when one cannot.
fn make_exact_b_spline_out_surface_from_in_space(
    k: &Kernel,
    volume: &Volume,
    in_space_surface: &Surface,
) -> Result<Option<Surface>> {
    // init output and locals
    let mut this_map = Some(volume);
    let mut this_in_space: Option<Surface> = None;

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
        // see if the current InSpaceSurface can project exactly to the Next OutSpace
        let input = this_in_space.as_ref().unwrap_or(in_space_surface);
        let Some(new_surface) = make_exact_b_spline_1st_out_surface(k, map, true, input)? else {
            // return NULL when an exact projection was not possible
            return Ok(None);
        };

        // set up for the next iteration
        this_in_space = Some(new_surface);
        this_map = map.next_map();
    }
    Ok(this_in_space)
}

/// `SmVolume::MakeExactBSplineOutSurfaceFromParamSpace`: the first map from parameter
/// space, then every compounded map from in-space; `None` when one cannot.
fn make_exact_b_spline_out_surface_from_param_space(
    k: &Kernel,
    volume: &Volume,
    param_space_surface: &Surface,
) -> Result<Option<Surface>> {
    // for 1st volume mapping - Get 1stOutSpace project if possible
    let Some(mut new_surface) =
        make_exact_b_spline_1st_out_surface(k, volume, false, param_space_surface)?
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
        let Some(next) = make_exact_b_spline_1st_out_surface(k, map, true, &new_surface)? else {
            return Ok(None);
        };
        new_surface = next;
        this_map = map.next_map();
    }
    Ok(Some(new_surface))
}

/// `SmSurface::operator==` (the base comparison: the same type; the same-object test
/// is the caller's).
fn base_surface_eq(a: &SrfInVolume, b: &SrfInVolume) -> bool {
    a.type_code() == b.type_code()
}

// ---- helpers ------------------------------------------------------------------------

/// Entry `i` of an evaluation array, or `UNDEF` (an unwritten `SmVector3d`).
fn at(v: &[Vec3], i: usize) -> Vec3 {
    v.get(i).copied().unwrap_or(Vec3::UNDEF)
}

/// Writes entry `i` of an evaluation array.
fn put(v: &mut [Vec3], i: usize, x: Vec3) -> Result<()> {
    let slot = v
        .get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(OP_EVAL, "evaluation array too short"))?;
    *slot = x;
    Ok(())
}

/// A copy of `volume` by its virtual `Copy`; when that fails (upstream ignores the
/// status and keeps the shared volume) the volume itself, cloned.
fn copy_volume(volume: &Volume) -> Result<Volume> {
    Ok(ignored(volume.copy(false))?.unwrap_or_else(|| volume.clone()))
}

// ---- the surface --------------------------------------------------------------------

/// A surface mapped through a volume (`SmSrfInVolume`): the base surface lives in
/// the volume's parameter space or in-space and is mapped to the volume's final
/// out-space.
#[derive(Debug)]
pub struct SrfInVolume {
    /// `m_pSurface`: the base surface.
    surface: Box<Surface>,
    /// `m_bInParamSpace`: the base surface is in the volume's parameter space (else
    /// in its in-space).
    in_param_space: bool,
    /// `m_pVolume`: the mapping volume.
    volume: Volume,
    /// `m_lOwnerFlag`: bit 1 owns the surface, bit 2 owns the volume (only the
    /// values 1 and 3, and 2 and 3, count as owning).
    owner_flag: u64,
}

/// The copy constructor: an owned base surface gets the ordinary copy constructor, a
/// borrowed one is an exact copy (upstream shares it, so the knot fix-up of a copy never
/// reaches it), the volume is always copied (upstream copies it whenever the original
/// owns either part, also for owner flag 1, whose result does not own that copy), and
/// the owner flag becomes 3, 1, 2 or 0 from what the original owns.
impl Clone for SrfInVolume {
    fn clone(&self) -> Self {
        let owns_surface = self.owns_surface();
        let owns_volume = self.owns_volume();
        Self {
            surface: Box::new(if owns_surface {
                (*self.surface).clone()
            } else {
                // infallible: the constructors, setters and database reader bounded it
                self.surface.clone_exact_bounded()
            }),
            in_param_space: self.in_param_space,
            volume: self.volume.clone(),
            owner_flag: if owns_surface && owns_volume {
                3
            } else if owns_surface {
                1
            } else if owns_volume {
                2
            } else {
                0
            },
        }
    }
}

impl From<SrfInVolume> for Surface {
    fn from(s: SrfInVolume) -> Self {
        Self::new(SurfaceKind::SrfInVolume(Box::new(s)))
    }
}

impl SrfInVolume {
    /// Exact evaluation copy; the caller validates the complete geometry graph first.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            surface: Box::new(self.surface.clone_exact_bounded()),
            in_param_space: self.in_param_space,
            volume: self.volume.clone(),
            owner_flag: self.owner_flag,
        }
    }

    /// The base surface, mutable, for [`Surface::detach_nurb_cells`].
    pub(crate) fn nested_surfaces_mut(&mut self) -> Vec<&mut Surface> {
        vec![self.surface.as_mut()]
    }

    /// An evaluation copy retaining exact basis data, with Surface's `MAX_GEOMETRY_NESTING` bound.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        Surface::check_volume_evaluation_copy_depth(&self.volume, depth)?;
        Ok(Self {
            surface: Box::new(self.surface.clone_for_evaluation_depth(depth)?),
            in_param_space: self.in_param_space,
            volume: self.volume.clone(),
            owner_flag: self.owner_flag,
        })
    }

    /// `SmSrfInVolume_TYPE`.
    pub const TYPE: i64 = SRF_IN_VOLUME_TYPE;

    /// A surface in `volume` (the constructor). `copy_flag` bit 1 (surface) and
    /// bit 2 (volume) ask for copies (by the virtual `Copy`), which this object then
    /// owns. `owner_flag` is the initial owner flag.
    ///
    /// # Errors
    ///
    /// The failure of copying the surface (upstream ignores the status and keeps the
    /// shared surface; the port propagates it only for [`SolidError::Unported`]),
    /// [`SolidError::Unported`] from copying an unported volume kind; `InvalidInput`
    /// when `surface` or `volume` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`] - 1.
    pub fn new(
        k: &Kernel,
        surface: Surface,
        in_param_space: bool,
        volume: Volume,
        copy_flag: u64,
        owner_flag: u64,
    ) -> Result<Self> {
        check_geometry_nesting(NestedGeometry::Surface(&surface), 1, OP)?;
        check_geometry_nesting(NestedGeometry::Volume(&volume), 1, OP)?;
        let mut owner_flag = owner_flag;
        let mut surface = surface;
        let mut volume = volume;
        // When Surface is to be copied - copy it
        if copy_flag == 1 || copy_flag == 3 {
            if let Some(copy) = ignored(surface.copy(k))? {
                surface = copy;
            }
            owner_flag |= 1;
        }
        // when Volume is to be copied - copy it
        if copy_flag == 2 || copy_flag == 3 {
            volume = copy_volume(&volume)?;
            owner_flag |= 2;
        }
        Ok(Self {
            surface: Box::new(surface),
            in_param_space,
            volume,
            owner_flag,
        })
    }

    /// The copy constructor (`Copy`): copies both parts and sets the owner flag to
    /// 3, 1, 2 or 0 from what this object owns. A surface this object does not own is
    /// shared upstream, so here it is an exact copy that keeps its owner
    /// (`Surface::clone` would fix its knots up).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from copying an unported volume kind; those of
    /// [`Surface::clone_for_evaluation`] on a surface not owned (geometry nested too
    /// deeply).
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        let owns_surface = self.owns_surface();
        let owns_volume = self.owns_volume();
        let owner_flag = if owns_surface && owns_volume {
            3
        } else if owns_surface {
            1
        } else if owns_volume {
            2
        } else {
            0
        };
        // copy Volume when the target owns the Surface or the Volume
        let volume = if owns_surface || owns_volume {
            copy_volume(&self.volume)?
        } else {
            self.volume.clone()
        };
        // copy the Surface when the target owns it
        let surface = if owns_surface {
            ignored(self.surface.copy(k))?.unwrap_or_else(|| (*self.surface).clone())
        } else {
            // shared upstream: the same surface
            self.surface.clone_for_evaluation()?
        };
        Ok(Self {
            surface: Box::new(surface),
            in_param_space: self.in_param_space,
            volume,
            owner_flag,
        })
    }

    // ---- simple access --------------------------------------------------------------

    /// The base surface (`GetSurface`).
    #[must_use]
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// The mapping volume (`GetVolume`).
    #[must_use]
    pub const fn volume(&self) -> &Volume {
        &self.volume
    }

    /// Whether the base surface is in the volume's parameter space
    /// (`GetInParamSpace`).
    #[must_use]
    pub const fn in_param_space(&self) -> bool {
        self.in_param_space
    }

    /// The owner flag (`m_lOwnerFlag`).
    #[must_use]
    pub const fn owner_flag(&self) -> u64 {
        self.owner_flag
    }

    /// `OwnsSurface`: owner flag 1 or 3.
    #[must_use]
    pub const fn owns_surface(&self) -> bool {
        self.owner_flag == 1 || self.owner_flag == 3
    }

    /// `OwnsVolume`: owner flag 2 or 3.
    #[must_use]
    pub const fn owns_volume(&self) -> bool {
        self.owner_flag == 2 || self.owner_flag == 3
    }

    /// `GetType`.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `GetTypeString`.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmSrfInVolume_TYPE"
    }

    /// `GetClassString`.
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmSrfInVolume"
    }

    /// `IsKindOf`: `SmSrfInVolume_TYPE` or a base type.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || surface_is_kind_of(t)
    }

    /// `GetNaturalUVDomain`: the base surface's.
    #[must_use]
    pub fn natural_uv_domain(&self) -> Extent2d {
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        self.surface.natural_uv_domain_if_built()
    }

    /// `GetSTEPUVDomain`: the base surface's.
    #[must_use]
    pub fn step_uv_domain(&self) -> Extent2d {
        self.surface.step_uv_domain()
    }

    /// `GetMaxAnalyticDomain`: the base surface's.
    ///
    /// # Errors
    ///
    /// Those of the base surface's query.
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent2d> {
        self.surface.max_analytic_domain(k)
    }

    /// `GetDegree`: always 3.
    #[must_use]
    pub const fn degree(&self, _param: SurfParamType) -> usize {
        3
    }

    /// `ConvertUVFromSTEPToNURBS`: the base surface's.
    ///
    /// # Errors
    ///
    /// Those of the base surface's conversion.
    pub fn convert_uv_from_step_to_nurbs(&self, k: &Kernel, step_uv: Point2) -> Result<Point2> {
        self.surface.convert_uv_from_step_to_nurbs(k, step_uv)
    }

    /// `ConvertUVFromNURBSToSTEP`: the base surface's.
    ///
    /// # Errors
    ///
    /// Those of the base surface's conversion.
    pub fn convert_uv_from_nurbs_to_step(&self, k: &Kernel, nurbs_uv: Point2) -> Result<Point2> {
        self.surface.convert_uv_from_nurbs_to_step(k, nurbs_uv)
    }

    /// `operator==`: after the base comparison, the same parameter-space flag, equal
    /// base surfaces and equal volumes.
    ///
    /// # Errors
    ///
    /// Those of the base surface's and the volume's `operator==`.
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // first check the base
        if !base_surface_eq(self, other) {
            return Ok(false);
        }

        // check equivalence of these objects
        Ok(self.in_param_space == other.in_param_space
            && self.surface.approx_eq(k, &other.surface)?
            && self.volume.approx_eq(&other.volume)?)
    }

    // ---- owners and parts -----------------------------------------------------------

    /// `SetOwnerFlag`: sets which contained objects this surface owns.
    pub fn set_owner_flag(&mut self, owner_flag: u64) {
        self.owner_flag = owner_flag;
    }

    /// `MakeOwner`: ensures this surface owns its base surface and volume, copying
    /// any unowned one.
    ///
    /// # Errors
    ///
    /// Those of copying the base surface (`SM_ERR` for a kind without `Copy`) and the
    /// volume ([`SolidError::Unported`] for an unported kind).
    pub fn make_owner(&mut self, k: &Kernel) -> Result<()> {
        // copy and own unowned surfaces
        if !self.owns_surface() {
            *self.surface = self.surface.copy(k)?;
            self.owner_flag |= 1;
        }
        // copy and own unowned volumes
        if !self.owns_volume() {
            self.volume = self.volume.copy(false)?;
            self.owner_flag |= 2;
        }
        Ok(())
    }

    /// `SetBaseSurface`: replaces the base surface. With `copy_surface` the surface
    /// is copied (`Copy`, status ignored: a failed copy keeps the surface as is);
    /// `owns_surface` or a copy makes this object own it.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from copying an unported surface kind; `InvalidInput`
    /// when `surface` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`] - 1 (nothing is changed).
    pub fn set_base_surface(
        &mut self,
        k: &Kernel,
        surface: Surface,
        in_param_space: bool,
        copy_surface: bool,
        owns_surface: bool,
    ) -> Result<()> {
        check_geometry_nesting(NestedGeometry::Surface(&surface), 1, OP)?;
        *self.surface = surface;
        self.in_param_space = in_param_space;
        self.owner_flag &= 2; // clear owns surface bit
        if copy_surface {
            if let Some(copy) = ignored(self.surface.copy(k))? {
                *self.surface = copy;
            }
            self.owner_flag |= 1;
        }
        // when asked - remember to own this BaseSurface
        if owns_surface {
            self.owner_flag |= 1;
        }
        Ok(())
    }

    /// `SetVolume`: replaces the compounding volume, as [`Self::set_base_surface`].
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from copying an unported volume kind; `InvalidInput`
    /// when `volume` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`] - 1 (nothing is changed).
    pub fn set_volume(&mut self, volume: Volume, copy: bool, owns_volume: bool) -> Result<()> {
        check_geometry_nesting(NestedGeometry::Volume(&volume), 1, OP)?;
        self.volume = volume;
        self.owner_flag &= 1; // clear owns volume bit
        if copy {
            self.volume = copy_volume(&self.volume)?;
            self.owner_flag |= 2;
        }
        // when asked - remember to own this Volume
        if owns_volume {
            self.owner_flag |= 2;
        }
        Ok(())
    }

    // ---- exact B-spline and mirror --------------------------------------------------

    /// `MakeExactBSplineIfPossible`: the exact B-spline equivalent (upstream an
    /// `SmBSplineSurface` or a derived kind) of the base surface mapped through every
    /// compounded map of the volume to its final out-space; `None` (upstream `NULL`
    /// with `SM_SUCCESS`) when a map cannot produce one.
    ///
    /// # Errors
    ///
    /// Those of the surface copy and kind query; [`SolidError::Unported`] from the
    /// surface and volume services; `InvalidInput` for a volume chain deeper than
    /// `MAX_GEOMETRY_NESTING`.
    pub fn make_exact_b_spline_if_possible_with(&self, k: &Kernel) -> Result<Option<Surface>> {
        // Ask Volume to make exact BSpline if Possible - handles compounding
        if self.in_param_space {
            make_exact_b_spline_out_surface_from_param_space(k, &self.volume, &self.surface)
        } else {
            make_exact_b_spline_out_surface_from_in_space(k, &self.volume, &self.surface)
        }
    }

    /// `CreateMirrorSurface`: a copy of this surface whose volume is mirrored about
    /// `mirror_plane`.
    ///
    /// # Errors
    ///
    /// Those of the copy and of [`Volume::mirror`].
    pub fn create_mirror_surface(&self, k: &Kernel, mirror_plane: &Axis2Placement) -> Result<Self> {
        // copy this surface
        let mut new_surface = self.copy(k)?;
        // mirror the the volume
        volume_mirror(&mut new_surface.volume, mirror_plane)?;
        Ok(new_surface)
    }

    // ---- knots ----------------------------------------------------------------------

    /// `GetKnots`: the base surface's knots and multiplicities, optionally limited
    /// to `interval`.
    ///
    /// # Errors
    ///
    /// Those of the base surface's query.
    pub fn knots(
        &self,
        param: SurfParamType,
        interval: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.surface.knots(param, interval)
    }

    // ---- evaluation -----------------------------------------------------------------

    /// Position and derivatives at `uv` (`Evaluate`): `D[u][v]` is the derivative `u`
    /// times in `u` and `v` times in `v`, of the base surface evaluated (up to third
    /// derivatives) and lifted through the volume. Clamps `uv` to the base surface's
    /// domain. Unequal orders are supported when one of them is 0, and a full matrix
    /// (`only_upper_half == false`) for orders up to 1.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an unsupported combination of orders; [`SolidError::InvalidInput`]
    /// for orders above 3 (upstream overruns its arrays); those of the base surface's
    /// evaluation and the volume's [`Volume::evaluate`] / [`Volume::map`].
    pub fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        let EvaluateFlags {
            u_side,
            v_side,
            only_upper_half,
            non_zero_tangents,
        } = flags;
        let u_from_left = u_side == Side::Left;
        let (hu, hv) = (highest_u_deriv, highest_v_deriv);

        // check input
        // bOnlyUpperHalf == FALSE is supported for lHighestUDerv == 1 only
        // lHighestUDeriv != lHighestVDeriv is supported for cases lHighestUDeriv == 0 || lHighestVDeriv == 0.
        let supported = if hu == hv {
            only_upper_half || hu <= 1
        } else {
            hu == 0 || hv == 0
        };
        if !supported {
            return Err(SolidError::sm_err(OP_EVAL));
        }

        // limit the number of derivatives to 3
        let highest = hu.max(hv);
        if highest > MAX_DERIVS {
            return Err(SolidError::invalid_input(
                OP_EVAL,
                "derivative order above 3",
            ));
        }
        let n_eval = highest;
        let mut n_temp = n_eval;

        // when bOnlyUpperHalf == FALSE && lHighestDeriv == 1, to compute Duv compute all 2nd Derivatives
        if !only_upper_half && hu <= 1 && hu == hv {
            n_temp = 2;
        }

        // For indexing into the surface pt/deriv array and the volume pt/deriv array
        let stride = n_temp + 1;

        // Check for evaluating outside of BaseSurface domain: clamp the UV point
        let domain = self.surface.natural_uv_domain_with_nurb(k)?;
        let s_uv = if domain.contains_point(uv, 0.0) {
            uv
        } else {
            domain.clamp_point(uv)
        };

        // Surface eval
        let srf = self.surface.evaluate(
            k,
            s_uv,
            n_temp,
            n_temp,
            EvaluateFlags {
                u_side,
                v_side,
                only_upper_half: true,
                non_zero_tangents,
            },
        )?;
        let mut srf_derivs = vec![Vec3::UNDEF; stride * stride];
        for (i, row) in srf.iter().take(stride).enumerate() {
            for (j, d) in row.iter().take(stride).enumerate() {
                put(&mut srf_derivs, i * stride + j, *d)?;
            }
        }
        let su = |u: usize, v: usize| at(&srf_derivs, u * stride + v);

        // Volume eval
        //  gwc: something smarter needs to be worked out for the FromLeft values
        let tangent = at(&srf_derivs, n_eval + 1);
        let side = |c: f64| if c > 0.0 { u_from_left } else { !u_from_left };
        let opts = EvalOptions {
            from_left: [side(tangent.x), side(tangent.y), side(tangent.z)],
            non_zero_tangents,
            ..EvalOptions::default()
        };
        let vol = if self.in_param_space {
            self.volume.evaluate(su(0, 0), n_temp, &opts)?
        } else {
            self.volume.map(su(0, 0), n_temp, &opts)?
        };
        let sv = |u: usize, v: usize, w: usize| at(&vol, (u * stride + v) * stride + w);

        // set outputs: out[u * (hv + 1) + v], temp[u * (n + 1) + v]
        let ovs = hv + 1;
        let mut out = vec![Vec3::UNDEF; (hu + 1) * ovs];
        let mut temp = vec![Vec3::UNDEF; (1 + MAX_DERIVS) * (1 + MAX_DERIVS)];
        let vol1 = [sv(1, 0, 0), sv(0, 1, 0), sv(0, 0, 1)];
        let vol2 = [
            sv(2, 0, 0),
            sv(1, 1, 0),
            sv(1, 0, 1),
            sv(0, 2, 0),
            sv(0, 1, 1),
            sv(0, 0, 2),
        ];

        // Position
        put(&mut out, 0, sv(0, 0, 0))?;

        // 1st Derivs
        if n_eval >= 1 {
            // When managing the special cases of UpperHalf == FALSE or
            // lHighestUDeriv != lHighestVDeriv evals use temporary memory
            let [du, dv] = lift_first_derivative_srf([su(1, 0), su(0, 1)], vol1);
            let dest = if hu == hv { &mut out } else { &mut temp };
            put(dest, n_eval + 1, du)?;
            put(dest, 1, dv)?;
            if hu != hv {
                if hu > 0 {
                    put(&mut out, ovs, at(&temp, n_eval + 1))?;
                }
                if hv > 0 {
                    put(&mut out, 1, at(&temp, 1))?;
                }
            }
        }

        // 2nd Derivs
        if n_eval >= 2 || n_temp >= 2 {
            let [duu, duv, dvv] = lift_second_derivative_srf(
                [su(1, 0), su(0, 1)],
                [su(2, 0), su(1, 1), su(0, 2)],
                vol1,
                vol2,
            );
            let direct = n_eval == n_temp && hu == hv;
            let dest = if direct { &mut out } else { &mut temp };
            put(dest, 2 * stride, duu)?;
            put(dest, stride + 1, duv)?;
            put(dest, 2, dvv)?;

            // When managing the special cases move results from temporary memory to
            // output memory
            if n_eval != n_temp {
                // copy the final DUV derivative to the output (for (0, 0) with
                // bOnlyUpperHalf == FALSE the slot lies past the requested matrix:
                // upstream writes into the caller's oversized array, the port skips
                // the copy and still succeeds)
                if let Some(slot) = out.get_mut(ovs + 1) {
                    *slot = at(&temp, stride + 1);
                }
            } else if hu != hv {
                if hu > 1 {
                    put(&mut out, 2 * ovs, at(&temp, 2 * stride))?;
                }
                if hv > 1 {
                    put(&mut out, 2, at(&temp, 2))?;
                }
            }
        }

        // 3rd Derivs
        if n_eval >= 3 {
            let vol3 = [
                sv(3, 0, 0),
                sv(2, 1, 0),
                sv(2, 0, 1),
                sv(1, 2, 0),
                sv(1, 1, 1),
                sv(1, 0, 2),
                sv(0, 3, 0),
                sv(0, 2, 1),
                sv(0, 1, 2),
                sv(0, 0, 3),
            ];
            let [duuu, duuv, duvv, dvvv] = lift_third_derivative_srf(
                [su(1, 0), su(0, 1)],
                [su(2, 0), su(1, 1), su(0, 2)],
                [su(3, 0), su(2, 1), su(1, 2), su(0, 3)],
                vol1,
                vol2,
                vol3,
            );
            // when managing the special case of lHighestUDeriv != lHighestVDeriv is
            // when one or the other is 0.
            let dest = if hu == hv { &mut out } else { &mut temp };
            put(dest, 3 * stride, duuu)?;
            put(dest, 2 * stride + 1, duuv)?;
            put(dest, stride + 2, duvv)?;
            put(dest, 3, dvvv)?;
            if hu != hv {
                if hu > 2 {
                    put(&mut out, 3 * ovs, at(&temp, 3 * stride))?;
                }
                if hv > 2 {
                    put(&mut out, 3, at(&temp, 3))?;
                }
            }
        }

        // upstream fills the derivatives above MAX_DERIVS with zeros: none are asked
        Ok(out.chunks(ovs).map(<[Vec3]>::to_vec).collect())
    }

    /// `EvaluateSimple`: [`Self::evaluate`] without correcting zero-length first
    /// derivatives.
    ///
    /// # Errors
    ///
    /// Those of [`Self::evaluate`].
    pub fn evaluate_simple(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        u_side: Side,
        v_side: Side,
        only_upper_half: bool,
    ) -> Result<Vec<Vec<Vec3>>> {
        // pass the call along
        self.evaluate(
            k,
            uv,
            highest_u_deriv,
            highest_v_deriv,
            EvaluateFlags {
                u_side,
                v_side,
                only_upper_half,
                non_zero_tangents: false,
            },
        )
    }

    /// The point at `uv` (`EvaluatePoint`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::evaluate`].
    pub fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        // pass the call along
        let d = self.evaluate(
            k,
            uv,
            0,
            0,
            EvaluateFlags {
                only_upper_half: true,
                non_zero_tangents: false,
                ..EvaluateFlags::default()
            },
        )?;
        d.first()
            .and_then(|row| row.first())
            .copied()
            .ok_or_else(|| SolidError::invalid_input(OP, "empty evaluation"))
    }

    /// The unit normal at `uv` (`EvaluateNormal`): `Du x Dv`, unitized.
    ///
    /// # Errors
    ///
    /// Those of [`Self::evaluate`]; [`SolidError::InvalidInput`] for a zero-length
    /// normal (`Unitize`'s status).
    pub fn evaluate_normal(
        &self,
        k: &Kernel,
        uv: Point2,
        u_side: Side,
        v_side: Side,
    ) -> Result<Vec3> {
        // evaluate 1st derivs with nonZeroTangents
        let d = self.evaluate(
            k,
            uv,
            1,
            1,
            EvaluateFlags {
                u_side,
                v_side,
                only_upper_half: true,
                non_zero_tangents: true,
            },
        )?;
        let get = |u: usize, v: usize| {
            d.get(u)
                .and_then(|row| row.get(v))
                .copied()
                .ok_or_else(|| SolidError::invalid_input(OP_NORMAL, "short evaluation"))
        };

        // compute the normal = Du cross Dv
        let normal = get(1, 0)?.cross(get(0, 1)?);

        // normalize the output
        normal.unitize()
    }

    // ---- inversion ------------------------------------------------------------------

    /// The parameter of `point_on_surf` in `anal_domain` and its location
    /// (`STEPInversion`): the point is mapped back through the volume, then through
    /// the base surface.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the volume finds no inverse; the volume inversions' failures
    /// ([`Volume::inv_evaluate_point`], [`Volume::inv_map_point`]); those of the base
    /// surface's inversion.
    pub fn step_inversion(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        distance_tolerance: f64,
    ) -> Result<(Point2, LocationType)> {
        // map the point back through the volume
        let inv = if self.in_param_space {
            self.volume
                .inv_evaluate_point(k, point_on_surf, None, None, true)?
        } else {
            self.volume
                .inv_map_point(k, point_on_surf, None, None, true)?
        };
        if !inv.success {
            return Err(SolidError::sm_err(OP_STEP));
        }
        let first = inv
            .points
            .first()
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP_STEP))?;

        // Project the inverted volume point back through the base surface
        self.surface
            .step_inversion(k, anal_domain, first, distance_tolerance, None)
    }

    // ---- edits ----------------------------------------------------------------------

    /// Reverses the base surface's parameterisation in `param`'s direction
    /// (`Reverse`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when this object does not own the surface; those of the base
    /// surface's `Reverse`.
    pub fn reverse(&mut self, k: &Kernel, param: SurfParamType) -> Result<()> {
        // check input - Can not Reverse an unowned surface
        self.require_owned_surface()?;
        self.surface.reverse(k, param)
    }

    /// Swaps the base surface's `u` and `v` directions (`SwapUV`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when this object does not own the surface; those of the base
    /// surface's `SwapUV`.
    pub fn swap_uv(&mut self, k: &Kernel) -> Result<()> {
        // check input - Can not swap UV on an unowned surface
        self.owned_surface_mut()?.swap_uv(k)
    }

    /// The base surface, for an edit of it this object makes (`SM_ERR` when this
    /// object does not own it). `Surface::swap_uv_notifying` notifies this object
    /// around the base surface's swap, as upstream `SwapUV` does.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when this object does not own the surface.
    pub(crate) fn owned_surface_mut(&mut self) -> Result<&mut Surface> {
        self.require_owned_surface()?;
        Ok(&mut self.surface)
    }

    /// Trims the base surface to `trim_domain` when possible (`TrimWithDomain`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when this object does not own the surface; those of the base
    /// surface's `TrimWithDomain`.
    pub fn trim_with_domain(&mut self, k: &Kernel, trim_domain: &mut Extent2d) -> Result<()> {
        // check input - Can not trim an unowned surface
        self.require_owned_surface()?;
        self.surface.trim_with_domain(k, trim_domain)
    }

    /// `SM_ERR` unless this object owns its base surface.
    fn require_owned_surface(&self) -> Result<()> {
        if self.owns_surface() {
            Ok(())
        } else {
            Err(SolidError::sm_err(OP))
        }
    }

    /// Scales (optional) then rotates and moves the surface (`Transform`): the
    /// compounding volume is transformed.
    ///
    /// # Errors
    ///
    /// Those of [`Volume::transform`].
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

        // compounding volume transform
        self.volume
            .transform_at_depth(rotate_n_move, scale, depth + 1)
    }

    /// The two halves of the surface split at `param` in `surf_param`'s direction
    /// (`SplitAt`): each half is a new surface in (a copy of) the same volume.
    ///
    /// # Errors
    ///
    /// Those of the base surface's split.
    pub fn split_at(
        &self,
        k: &Kernel,
        param: f64,
        surf_param: SurfParamType,
    ) -> Result<(Self, Self)> {
        // split Base Surface
        let (left, right) = self.surface.split_at(k, param, surf_param)?;

        // build left and right split surfaces from the base surface splits
        let left = Self::new(k, left, self.in_param_space, self.volume.clone(), 2, 3)?;
        let right = Self::new(k, right, self.in_param_space, self.volume.clone(), 2, 3)?;
        Ok((left, right))
    }

    // ---- database -------------------------------------------------------------------

    /// Reads a surface in a volume from `db` (`ReadFromDB`).
    ///
    /// # Errors
    ///
    /// Those of the base surface's reader and of [`Volume::read_from_db`];
    /// [`SolidError::InvalidInput`] for
    /// a header whose extraction was skipped or objects nested too deeply.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        let ascii = db.file_type() == FileType::Ascii;
        let no_stream = || SolidError::invalid_input(OP_READ, "no input stream");

        // base surface
        if ascii {
            db.go_to_next_line();
        }
        let base_type = db
            .read_type(false)?
            .type_code
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "surface type not read"))?;
        db.enter_nested(OP_READ)?;
        let surface = Surface::read_from_db(k, base_type, db, version);
        db.leave_nested();
        let surface = surface?;

        // bInParamSpace value
        let in_param_space = db
            .in_stream_mut()
            .ok_or_else(no_stream)?
            .extract_int()
            .map(|v| v != 0)
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "parameter space flag not read"))?;
        db.go_to_next_line();

        // volume
        if ascii {
            db.go_to_next_line();
        }
        let volume_type = db
            .read_type(false)?
            .type_code
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "volume type not read"))?;
        let volume = Volume::read_from_db(volume_type, db, version)?;

        // load the object
        Ok(Self {
            surface: Box::new(surface),
            in_param_space,
            volume,
            owner_flag: 3,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::plane::Plane;
    use crate::surface::twist_volume::TwistVolume;

    /// `Evaluate(uv, 0, 0, .., bOnlyUpperHalf = FALSE)` runs the second-derivative
    /// branch, writes `Duv` past the requested matrix (skipped here) and succeeds.
    #[test]
    fn point_evaluation_without_upper_half_succeeds() {
        let k = &Kernel::default();
        let plane = Plane::from_normal(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0));
        let volume = TwistVolume::create(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            4.0,
            1e-3,
        );
        let siv =
            SrfInVolume::new(&Kernel::default(), Surface::from(plane), true, volume, 0, 3).unwrap();
        let uv = Point2::new(0.3, 0.7);
        let flags = EvaluateFlags {
            only_upper_half: false,
            ..EvaluateFlags::default()
        };
        let d = siv.evaluate(k, uv, 0, 0, flags).unwrap();
        let p = siv.evaluate_point(k, uv).unwrap();
        assert_eq!(d.first().and_then(|r| r.first()).copied(), Some(p));
    }

    /// `SmSrfInVolume::ConvertUVFromSTEPToNURBS` / `ConvertUVFromNURBSToSTEP`
    /// (`SmSrfInVolume.h`) forward to the base surface: through the `Surface`
    /// dispatch, a surface in volume over a cone converts as the cone does, not as the
    /// identity.
    #[test]
    fn surface_dispatch_converts_uv_like_the_base_surface() {
        let k = &Kernel::default();
        let cone = crate::surface::cone::Cone::create_canonical(
            k,
            &Axis2Placement::IDENTITY,
            2.0,
            1.0,
            3.0,
            false,
            false,
        )
        .unwrap();
        let base = Surface::from(cone);
        let volume = TwistVolume::create(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            4.0,
            1e-3,
        );
        let siv = Surface::from(SrfInVolume::new(k, base.clone(), true, volume, 0, 3).unwrap());
        let uv = Point2::new(1.1, 0.4);
        let to_nurbs = base.convert_uv_from_step_to_nurbs(k, uv).unwrap();
        assert_ne!(to_nurbs, uv);
        assert_eq!(siv.convert_uv_from_step_to_nurbs(k, uv).unwrap(), to_nurbs);
        let to_step = base.convert_uv_from_nurbs_to_step(k, uv).unwrap();
        assert_ne!(to_step, uv);
        assert_eq!(siv.convert_uv_from_nurbs_to_step(k, uv).unwrap(), to_step);
    }
}
