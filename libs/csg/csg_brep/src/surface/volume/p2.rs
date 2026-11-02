//! The second half of the volume base class `SmVolume`: compound domain checks of
//! lines and boxes, bounding boxes through the orient and next maps, compound point
//! evaluation, the inverse point solvers and the polymorphic database reader.
//!
//! Upstream: `source/SMLib/src/SmVolume.cpp`, `source/SMLib/inc/SmVolume.h`.
//!
//! | Rust                                         | Upstream                                  |
//! |----------------------------------------------|-------------------------------------------|
//! | [`Volume::is_point_in_param_domain`]         | `IsPointInParamDomain`                    |
//! | [`Volume::is_line_in_in_space_domain`]       | `IsLineInInSpaceDomain`                   |
//! | [`Volume::is_box_in_param_domain`]           | `IsBoxInParamDomain`                      |
//! | [`Volume::is_pseudo_box_in_in_space_domain`] | `IsPseudoBoxInInSpaceDomain`              |
//! | [`Volume::evaluate_bounding_box`]            | `EvaluateBoundingBox`                     |
//! | [`Volume::map_pseudo_box`]                   | `MapPseudoBox`                            |
//! | [`Volume::evaluate_point`]                   | `EvaluatePoint`                           |
//! | [`Volume::evaluate_1st_derivatives`]         | `Evaluate1stDerivatives`                  |
//! | [`Volume::inv_evaluate_point`]               | `InvEvaluatePoint`                        |
//! | [`Volume::inv_map_point`]                    | `InvMapPoint`                             |
//! | [`Volume::global_point_solve`]               | `GlobalPointSolve`                        |
//! | [`Volume::has_discontinuities`]              | `HasDiscontinuities`                      |
//! | [`Volume::evaluate_bounding_box_simple`]     | `EvaluateBoundingBoxSimple` (virtual)     |
//! | [`Volume::evaluate_pseudo_box_simple`]       | `EvaluatePseudoBoxSimple`                 |
//! | [`Volume::inv_evaluate_point_simple`]        | `InvEvaluatePointSimple`                  |
//! | [`Volume::global_point_solve_simple`]        | `GlobalPointSolveSimple` (virtual)        |
//! | [`Volume::local_point_solve_simple`]         | `LocalPointSolveSimple` (virtual)         |
//! | [`Volume::read_from_db`]                     | `ReadFromDB` (static, by type)            |
//! | [`Volume::read_base_from_db`]                | `ReadFromDB(SmVolume_TYPE, ..)` (base data of an existing volume) |
//!
//! Deviations:
//! - **Out-params** become return values ([`InvPoints`], tuples) or `Option<&mut _>`
//!   outputs where upstream reads the outputs' previous state (the boxes). Upstream's
//!   copies for aliased inputs and outputs are not needed: Rust borrows cannot alias.
//!   On an error the partial outputs upstream may leave are not returned.
//! - **Ignored statuses.** Where upstream ignores a status (`EvaluatePoint` in the
//!   box samplers and gap loops, `Evaluate1stDerivatives` for the box bases, the
//!   steps of `MapPseudoBox`, the nested reads of `ReadFromDB`, ...) the port does
//!   too: the value keeps what it held (a fresh `SmPoint3d` is [`Vec3::UNDEF`]),
//!   except that a failed `EvaluatePoint` of a twist, an unbend or a bend with
//!   next maps leaves that map's own image, which its `Evaluate` override writes
//!   before compounding.
//!   [`SolidError::Unported`] always propagates.
//! - **Undefined behaviour** upstream is [`SolidError::Unsupported`] here, which no
//!   caller ignores: a transform's `EvaluateBoundingBoxSimple` reached without a
//!   parameter pseudo box, a twist's with an unbounded box and no box output (both
//!   dereference null; see their units), and in `ReadFromDB` a top-level or
//!   next-map `SmVolume_TYPE` (null volume), an unreadable map type after a "has
//!   map" flag (uninitialised type), an orient map whose read fails or that is not a
//!   transform (null or mistyped `SmTransform`).
//! - **Solution objects.** The solvers take the volume's handle `this` (any
//!   [`SolutionObject`]); the inverse point maps, which drop the solutions, use a
//!   private handle.
//! - **Recursion.** The next-map chain is walked recursively as upstream (one level
//!   per map; chains are built by the caller). `ReadFromDB` recursion is input
//!   driven and bounded by [`DatabaseReader::enter_nested`] (32 levels); past the
//!   bound the read fails with [`SolidError::InvalidInput`], which the nested reads
//!   whose statuses upstream ignores propagate rather than drop.
//! - **Contexts** and `bCalcGeometric` (unused by the ported simple maps) are dropped.

use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::local_solve_nd::{BoundaryHandler, LocalSolveNd};
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType,
};
use crate::geom::transform::Transform;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQRT};
use crate::surface::b_spline_volume::BSplineVolume;
use crate::surface::bend_volume::BendVolume;
use crate::surface::twist_volume::TwistVolume;
use crate::surface::unbend_volume::UnbendVolume;
use crate::topology::database_io::{DatabaseReader, FileType, InputStream, NESTING_LIMIT_REASON};

use super::p1::{
    Discontinuities3d, EvalOptions, FindPvIntersect, Volume, VolumeKind, BEND_VOLUME_TYPE,
    B_SPLINE_VOLUME_TYPE, TRANSFORM_TYPE, TWIST_VOLUME_TYPE, UNBEND_VOLUME_TYPE, VOLUME_TYPE,
};

const OP_BOX: &str = "surface::volume::p2::Volume::evaluate_bounding_box";
const OP_MAP_BOX: &str = "surface::volume::p2::Volume::map_pseudo_box";
const OP_INV: &str = "surface::volume::p2::Volume::inv_evaluate_point";
const OP_SOLVE: &str = "surface::volume::p2::Volume::global_point_solve";
const OP_READ: &str = "surface::volume::p2::Volume::read_from_db";

/// Samples per box edge and per face iso line of the box samplers (`lSmpCnt`).
const SAMPLE_COUNT: usize = 5;

/// The edges of the box samplers as corner index pairs, over the corner order
/// `000 010 100 110 001 011 101 111`.
const SAMPLE_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 0),
    (4, 5),
    (5, 6),
    (6, 7),
    (7, 4),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// The faces of the box samplers as two corner index pairs (`i0 i1`, `j0 j1`).
const SAMPLE_FACES: [[usize; 4]; 6] = [
    [0, 1, 2, 3],
    [4, 5, 6, 7],
    [0, 4, 1, 5],
    [2, 6, 3, 7],
    [0, 4, 2, 6],
    [1, 5, 3, 7],
];

/// The result of an inverse point map (`InvEvaluatePoint`, `InvMapPoint`): the
/// success flag, the found points and their gaps (the distance from the target to
/// each point's image).
#[derive(Debug, Clone, PartialEq)]
pub struct InvPoints {
    /// `rbSuccess`.
    pub success: bool,
    /// The parameter-space (`InvEvaluatePoint`) or in-space (`InvMapPoint`) points.
    pub points: Vec<Point3>,
    /// `rdGaps`, one per point.
    pub gaps: Vec<f64>,
}

/// The solution handle of the inverse point maps, whose solutions never leave them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NoHandle;

impl SolutionObject for NoHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Volume
    }
}

/// A simple transform volume (the orient maps are simple `SmTransform`s).
const fn simple(t: &Transform) -> Volume {
    Volume::new(VolumeKind::Transform(*t))
}

/// `EvaluatePointSimple` of a simple transform.
fn transform_point(t: &Transform, p: Point3) -> Result<Point3> {
    simple(t).evaluate_point_simple(p)
}

/// `a * p + (1 - a) * q` as upstream writes it.
fn blend(a: f64, p: Point3, q: Point3) -> Point3 {
    a * p + (1.0 - a) * q
}

/// `b.x * dx + b.y * dy + b.z * dz` (a basis vector mapped by the first partials).
fn project_basis(b: Vec3, dx: Vec3, dy: Vec3, dz: Vec3) -> Vec3 {
    b.x * dx + b.y * dy + b.z * dz
}

/// Optional box outputs of the samplers.
struct BoxOuts<'a> {
    proj_box: Option<&'a mut Extent3d>,
    proj_pseudo_box: Option<&'a mut PseudoBox>,
}

impl BoxOuts<'_> {
    fn add(&mut self, p: Point3) {
        if let Some(b) = self.proj_box.as_deref_mut() {
            b.add_point(p);
        }
        if let Some(pb) = self.proj_pseudo_box.as_deref_mut() {
            pb.add_point(p);
        }
    }

    fn set_unbounded(&mut self) {
        if let Some(b) = self.proj_box.as_deref_mut() {
            b.set_unbounded();
        }
        if let Some(pb) = self.proj_pseudo_box.as_deref_mut() {
            pb.set_unbounded();
        }
    }
}

impl Volume {
    // ---- domains --------------------------------------------------------------------

    /// Whether the parameter point lies in the natural parameter domain and, with
    /// `with_compounding`, its image in every next map's domain
    /// (`IsPointInParamDomain`). Upstream maps the point with the compounding
    /// `EvaluatePoint` (to the last out-space) before asking the next map; kept.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind.
    pub fn is_point_in_param_domain(
        &self,
        param_point: Point3,
        with_compounding: bool,
    ) -> Result<bool> {
        // pass the call along
        let mut is_in = self.is_point_in_param_domain_simple(param_point)?;

        // when asked - check compounded volumes as well
        if let (true, true, Some(next)) = (is_in, with_compounding, self.next_map.as_deref()) {
            // map crParamPoint to 1stOutSpace (status ignored)
            let next_in_space_point = self
                .evaluate_point_written(param_point, true)?
                .unwrap_or(Vec3::UNDEF);
            // Check the NextInSpacePoint
            is_in &= next
                .is_point_in_in_space_domain(next_in_space_point, true)?
                .0;
        }
        Ok(is_in)
    }

    /// Whether the in-space segment maps through the inverse orient map to a
    /// parameter segment in the domain (`IsLineInInSpaceDomain`), with that
    /// segment's ends.
    ///
    /// # Errors
    ///
    /// Those of [`Volume::is_line_in_param_domain`].
    pub fn is_line_in_in_space_domain(
        &self,
        k: &Kernel,
        start: Point3,
        end: Point3,
        with_compounding: bool,
    ) -> Result<(bool, Point3, Point3)> {
        // Lines map to lines through the Orient and InvOrient Maps (statuses ignored)
        let (start_param, end_param) = match &self.inv_orient_map {
            Some(inv) => (
                ignored(transform_point(inv, start))?.unwrap_or(Vec3::UNDEF),
                ignored(transform_point(inv, end))?.unwrap_or(Vec3::UNDEF),
            ),
            None => (start, end),
        };
        // pass the check along
        let is_in = self.is_line_in_param_domain(k, start_param, end_param, with_compounding)?;
        Ok((is_in, start_param, end_param))
    }

    /// Whether every edge of the parameter box lies in the domain
    /// (`IsBoxInParamDomain`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::is_line_in_param_domain`].
    pub fn is_box_in_param_domain(
        &self,
        k: &Kernel,
        param_box: &Extent3d,
        with_compounding: bool,
    ) -> Result<bool> {
        // some volumes have internal discontinuities - don't check vertices check edges
        for e in param_box.edges().chunks_exact(2) {
            if let [a, b] = e {
                // when any ParamBoxEdge line is not completely within ParamSpace domain
                if !self.is_line_in_param_domain(k, *a, *b, with_compounding)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Whether every edge of the in-space pseudo box maps into the domain
    /// (`IsPseudoBoxInInSpaceDomain`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::is_line_in_in_space_domain`].
    pub fn is_pseudo_box_in_in_space_domain(
        &self,
        k: &Kernel,
        in_space_pseudo_box: &PseudoBox,
        with_compounding: bool,
    ) -> Result<bool> {
        for e in in_space_pseudo_box.edges().chunks_exact(2) {
            if let [a, b] = e {
                if !self
                    .is_line_in_in_space_domain(k, *a, *b, with_compounding)?
                    .0
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    // ---- bounding boxes -------------------------------------------------------------

    /// The in-space and out-space boxes of the parameter box (`EvaluateBoundingBox`):
    /// the requested outputs among `out_box` / `out_pseudo_box` (the last out-space
    /// with `with_compounding`, else the first) and `in_box` / `in_pseudo_box`.
    /// `param_pseudo_box` orients the output pseudo boxes. Nothing happens without an
    /// output.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the box is not in the parameter domain
    /// ([`Volume::is_box_in_param_domain`]); the failures of the simple maps' box
    /// mappings and of [`Volume::map_pseudo_box`].
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_bounding_box(
        &self,
        k: &Kernel,
        param_box: &Extent3d,
        param_pseudo_box: Option<&PseudoBox>,
        mut out_box: Option<&mut Extent3d>,
        out_pseudo_box: Option<&mut PseudoBox>,
        in_box: Option<&mut Extent3d>,
        in_pseudo_box: Option<&mut PseudoBox>,
        with_compounding: bool,
    ) -> Result<()> {
        // no work - no output requests
        if in_box.is_none()
            && in_pseudo_box.is_none()
            && out_box.is_none()
            && out_pseudo_box.is_none()
        {
            return Ok(());
        }

        // check input - ParamBox must be maintained within Natural ParamDomain
        if !self.is_box_in_param_domain(k, param_box, true)? {
            return Err(SolidError::sm_err(OP_BOX));
        }

        // when asked for InSpace Boxes
        if in_box.is_some() || in_pseudo_box.is_some() {
            if let Some(orient) = &self.orient_map {
                // project ParamDomain boxes to InSpace boxes through OrientMap()
                simple(orient).evaluate_bounding_box_simple(
                    k,
                    param_box,
                    param_pseudo_box,
                    in_box,
                    in_pseudo_box,
                )?;
            } else {
                // without an OrientMap, ParamSpace == InSpace
                if let Some(b) = in_box {
                    *b = *param_box;
                }
                if let Some(ipb) = in_pseudo_box {
                    Self::param_pseudo_box_in_space(param_box, param_pseudo_box, ipb);
                }
            }
        }

        // when asked for OutSpace boxes with compounding
        if out_box.is_some() || out_pseudo_box.is_some() {
            let mut this_out = PseudoBox::new();

            // project ParamDomain Boxes to ProjectSpace Boxes
            if let Some(orient) = &self.orient_map {
                let mut proj_box = Extent3d::new();
                let mut proj_pseudo_box = PseudoBox::new();
                // map from Param to Proj space
                self.evaluate_bounding_box_simple(
                    k,
                    param_box,
                    param_pseudo_box,
                    Some(&mut proj_box),
                    Some(&mut proj_pseudo_box),
                )?;
                // map from Proj to Out space
                simple(orient).evaluate_bounding_box_simple(
                    k,
                    &proj_box,
                    Some(&proj_pseudo_box),
                    out_box.as_deref_mut(),
                    Some(&mut this_out),
                )?;
            } else {
                // projSpace == OutSpace, map from Param to Out space
                self.evaluate_bounding_box_simple(
                    k,
                    param_box,
                    param_pseudo_box,
                    out_box.as_deref_mut(),
                    Some(&mut this_out),
                )?;
            }

            // when asked - Map 1st OutSpace boxes to last OutSpace
            if let (Some(next), true) = (self.next_map.as_deref(), with_compounding) {
                let first = this_out;
                next.map_pseudo_box(
                    k,
                    &first,
                    out_box,
                    Some(&mut this_out),
                    with_compounding,
                    false,
                )?;
            }

            // set output
            if let Some(o) = out_pseudo_box {
                *o = this_out;
            }
        }
        Ok(())
    }

    /// The in-space pseudo box of `param_box` without an orient map (the branch of
    /// `EvaluateBoundingBox` building `pInSpacePseudoBox`).
    fn param_pseudo_box_in_space(
        param_box: &Extent3d,
        param_pseudo_box: Option<&PseudoBox>,
        ipb: &mut PseudoBox,
    ) {
        let Some(ppb) = param_pseudo_box else {
            // let pInSpacePseudoBox = crParamBox
            ipb.init_basis();
            ipb.set_intervals(
                param_box.u_interval(),
                param_box.v_interval(),
                param_box.w_interval(),
            );
            return;
        };
        // let pInSpacePseudoBox = pOptParamPseudoBox (for orientation)
        *ipb = *ppb;

        if param_box.is_bounded() {
            // add ParamDomain Corners to PseudoBox
            for (i, j, k) in [
                (0.0, 0.0, 0.0),
                (0.0, 0.0, 1.0),
                (0.0, 1.0, 0.0),
                (0.0, 1.0, 1.0),
                (1.0, 0.0, 0.0),
                (1.0, 0.0, 1.0),
                (1.0, 1.0, 0.0),
                (1.0, 1.0, 1.0),
            ] {
                ipb.add_point(param_box.evaluate(i, j, k));
            }
            return;
        }

        // Unbounded paramDomains - set PseudoBox intervals to unbounded
        match ipb.is_axis_aligned() {
            None => ipb.set_unbounded(),
            Some(axis_map) => {
                // Set the intervals directly from the BBox intervals (allows
                // partially unbounded boxes)
                for m in axis_map {
                    let lx = (m.unsigned_abs() as usize).wrapping_sub(1);
                    let length = ipb.basis_vector(lx).map_or(0.0, Vec3::length);
                    // don't scale here because of infinite values
                    let mut ivl =
                        Extent1d::from_min_max(param_box.min_at(lx), param_box.max_at(lx));
                    ivl.scale(length);
                    // lx is in 0..3 for an aligned box
                    let _ = ipb.set_interval(lx, ivl);
                }
            }
        }
    }

    /// The out-space boxes of the in-space pseudo box (`MapPseudoBox`): mapped
    /// through the inverse orient map, the simple map and the orient map, then (with
    /// `with_compounding`) through the next maps. With `trim_to_legal_param_space`
    /// the parameter boxes are trimmed to the natural domain; otherwise a box not
    /// mapping inside it is an error. Upstream ignores the steps' statuses.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a box outside the domain (without trimming);
    /// [`SolidError::Unsupported`] for the undefined cases (module docs);
    /// [`SolidError::Unported`] from an unported kind.
    pub fn map_pseudo_box(
        &self,
        k: &Kernel,
        in_space_pseudo_box: &PseudoBox,
        mut out_box: Option<&mut Extent3d>,
        out_pseudo_box: Option<&mut PseudoBox>,
        with_compounding: bool,
        trim_to_legal_param_space: bool,
    ) -> Result<()> {
        // when asked check input - InSpacePseudoBox must map within Natural ParamDomain
        if !trim_to_legal_param_space
            && !self.is_pseudo_box_in_in_space_domain(k, in_space_pseudo_box, true)?
        {
            return Err(SolidError::sm_err(OP_MAP_BOX));
        }

        let next = self.next_map.as_deref().filter(|_| with_compounding);

        // when asked for compound maps - force a OutSpacePseudoBox to pass along
        let has_this = out_pseudo_box.is_some() || next.is_some();
        let mut this_out = match &out_pseudo_box {
            Some(pb) => **pb,
            None => PseudoBox::new(),
        };

        if let Some(inv) = &self.inv_orient_map {
            let orient = self.orient_map.as_ref().ok_or(SolidError::invalid_input(
                OP_MAP_BOX,
                "inverse orient map without the orient map",
            ))?;
            // intermediate ParamSpace Boxes
            let mut param_box = Extent3d::new();
            let mut param_pseudo_box = PseudoBox::new();

            // map inSpace box to ParamBoxes
            ignore_status(simple(inv).evaluate_pseudo_box_simple(
                in_space_pseudo_box,
                Some(&mut param_box),
                has_this.then_some(&mut param_pseudo_box),
            ))?;

            // when asked - trim ParamBoxes to legal space
            if trim_to_legal_param_space {
                let src = param_box;
                ignore_status(self.trim_param_bounding_box_simple(&src, &mut param_box))?;
            }

            // map ParamBoxes to ProjSpace
            let mut proj_box = Extent3d::new();
            let mut proj_pseudo_box = PseudoBox::new();
            ignore_defined(self.evaluate_bounding_box_simple(
                k,
                &param_box,
                has_this.then_some(&param_pseudo_box),
                Some(&mut proj_box),
                Some(&mut proj_pseudo_box),
            ))?;

            // map ProjBox to OutSpace
            ignore_defined(simple(orient).evaluate_bounding_box_simple(
                k,
                &proj_box,
                Some(&proj_pseudo_box),
                out_box.as_deref_mut(),
                has_this.then_some(&mut this_out),
            ))?;
        } else if trim_to_legal_param_space {
            // no orient map, DomainSpace == ParamSpace and ProjSpace == OutSpace
            let mut param_box = Extent3d::new();
            ignore_status(self.trim_param_bounding_box_simple(
                &Extent3d::from_pseudo_box(in_space_pseudo_box),
                &mut param_box,
            ))?;
            // map trimmed InBoxes to OutBoxes directly
            ignore_defined(self.evaluate_bounding_box_simple(
                k,
                &param_box,
                None,
                out_box.as_deref_mut(),
                has_this.then_some(&mut this_out),
            ))?;
        } else {
            // map InBoxes to OutBoxes directly
            ignore_status(self.evaluate_pseudo_box_simple(
                in_space_pseudo_box,
                out_box.as_deref_mut(),
                has_this.then_some(&mut this_out),
            ))?;
        }

        // when asked for compound maps - pass the call along to next mapping
        if let Some(next) = next {
            let first = this_out;
            ignore_defined(next.map_pseudo_box(
                k,
                &first,
                out_box,
                Some(&mut this_out),
                with_compounding,
                trim_to_legal_param_space,
            ))?;
        }

        if let Some(pb) = out_pseudo_box {
            *pb = this_out;
        }
        Ok(())
    }

    /// The projection-space boxes of the parameter box (`EvaluateBoundingBoxSimple`,
    /// virtual): a transform maps it exactly (sampling the compound map at the
    /// corners for an unaligned pseudo box), a B-spline volume boxes its control
    /// points, a twist sweeps its corners; the base class samples the compound map
    /// (`EvaluatePoint`) at the corners, 5 points per edge and 5 x 5 per face, and
    /// orients the pseudo box by the mapped basis at the box centre.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unsupported`] where upstream dereferences null: a transform
    /// without `param_pseudo_box`, a twist with an unbounded box and no
    /// `proj_box`. The derived classes' failures (see
    /// [`Transform::evaluate_bounding_box_simple`],
    /// [`BSplineVolume::evaluate_bounding_box_simple`],
    /// [`TwistVolume::evaluate_bounding_box_simple`],
    /// [`UnbendVolume::evaluate_bounding_box_simple`]; a bend's analytic box cannot
    /// fail), including [`SolidError::Unported`] from an unbend's edge boxes.
    pub fn evaluate_bounding_box_simple(
        &self,
        k: &Kernel,
        param_box: &Extent3d,
        param_pseudo_box: Option<&PseudoBox>,
        proj_box: Option<&mut Extent3d>,
        proj_pseudo_box: Option<&mut PseudoBox>,
    ) -> Result<()> {
        let has_out = proj_box.is_some() || proj_pseudo_box.is_some();
        match &self.kind {
            VolumeKind::Transform(_) if has_out && param_pseudo_box.is_none() => {
                return Err(undefined_box(
                    "transform box without a parameter pseudo box (upstream dereferences null)",
                ))
            }
            // Reached through this simple entry point, and through the compound entry
            // points for a twist without an orient map (the component constructor
            // installs one, but a twist read by ReadFromDB with orient flag 0 has
            // none), which hand it a null box output.
            VolumeKind::Twist(_) if has_out && proj_box.is_none() && !param_box.any_bounds() => {
                return Err(undefined_box(
                    "unbounded twist box without a box output (upstream dereferences null)",
                ))
            }
            _ => {}
        }
        match &self.kind {
            VolumeKind::Transform(t) => t.evaluate_bounding_box_simple(
                param_box,
                param_pseudo_box,
                proj_box,
                proj_pseudo_box,
                |p| {
                    self.evaluate_point_written(p, true)?
                        .ok_or(SolidError::sm_err(OP_BOX))
                },
            ),
            VolumeKind::BSpline(b) => b.evaluate_bounding_box_simple(
                param_box,
                param_pseudo_box,
                proj_box,
                proj_pseudo_box,
            ),
            VolumeKind::Twist(t) => {
                t.evaluate_bounding_box_simple(param_box, proj_box, proj_pseudo_box)
            }
            VolumeKind::Unbend(u) => {
                u.evaluate_bounding_box_simple(k, self, param_box, proj_box, proj_pseudo_box)
            }
            VolumeKind::Bend(b) => {
                b.evaluate_bounding_box_simple(param_box, proj_box, proj_pseudo_box);
                Ok(())
            }
            VolumeKind::Base => {
                // no work - no output
                if proj_box.is_none() && proj_pseudo_box.is_none() {
                    return Ok(());
                }
                let mut outs = BoxOuts {
                    proj_box,
                    proj_pseudo_box,
                };
                let mut proj_point = Vec3::UNDEF;
                if let Some(b) = outs.proj_box.as_deref_mut() {
                    b.init();
                }
                if outs.proj_pseudo_box.is_some() {
                    let basis = param_pseudo_box.map(|p| *p.basis());
                    self.orient_sample_pseudo_box(
                        param_box.evaluate(0.5, 0.5, 0.5),
                        basis,
                        &mut proj_point,
                        &mut outs,
                    )?;
                }
                if param_box.is_bounded() {
                    self.sample_box(param_box.corners(), &mut proj_point, &mut outs)?;
                } else {
                    // just return an infinite box
                    outs.set_unbounded();
                }
                Ok(())
            }
        }
    }

    /// The projection-space boxes of the parameter pseudo box
    /// (`EvaluatePseudoBoxSimple`, not overridden): the base sampler of
    /// [`Volume::evaluate_bounding_box_simple`] over the pseudo box, its basis mapped
    /// to orient the output pseudo box.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind (the statuses of the
    /// evaluations are ignored).
    pub fn evaluate_pseudo_box_simple(
        &self,
        param_pseudo_box: &PseudoBox,
        proj_box: Option<&mut Extent3d>,
        proj_pseudo_box: Option<&mut PseudoBox>,
    ) -> Result<()> {
        // no work - no output
        if proj_box.is_none() && proj_pseudo_box.is_none() {
            return Ok(());
        }
        let mut outs = BoxOuts {
            proj_box,
            proj_pseudo_box,
        };
        let mut proj_point = Vec3::UNDEF;
        let centre = param_pseudo_box.evaluate(0.5, 0.5, 0.5);
        if let Some(b) = outs.proj_box.as_deref_mut() {
            b.init();
        }
        if outs.proj_pseudo_box.is_some() {
            self.orient_sample_pseudo_box(
                centre,
                Some(*param_pseudo_box.basis()),
                &mut proj_point,
                &mut outs,
            )?;
        }
        if param_pseudo_box.is_bounded() {
            self.sample_box(param_pseudo_box.corners(), &mut proj_point, &mut outs)?;
        } else {
            outs.set_unbounded();
        }
        Ok(())
    }

    /// Orients the output pseudo box of the samplers: the parameter `basis` (else
    /// the axes) mapped by the compound first partials at `centre`, unitized, and
    /// "inside out" intervals.
    fn orient_sample_pseudo_box(
        &self,
        centre: Point3,
        basis: Option<[Vec3; 3]>,
        proj_point: &mut Point3,
        outs: &mut BoxOuts<'_>,
    ) -> Result<()> {
        let [bx, by, bz] = basis.unwrap_or([Vec3::X, Vec3::Y, Vec3::Z]);
        // map ParamSpace basis vectors to ProjectSpace (status ignored)
        let (mut dx, mut dy, mut dz) = (Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF);
        if let Some([p, du, dv, dw]) =
            ignored(self.evaluate_1st_derivatives(centre, [true; 3], true, true))?
        {
            *proj_point = p;
            (dx, dy, dz) = (du, dv, dw);
        }
        // unitize the projected directions
        let proj_dx = project_basis(bx, dx, dy, dz).unitize_or_z();
        let proj_dy = project_basis(by, dx, dy, dz).unitize_or_z();
        let proj_dz = project_basis(bz, dx, dy, dz).unitize_or_z();
        if let Some(pb) = outs.proj_pseudo_box.as_deref_mut() {
            // upstream ignores the status
            let _ = pb.set_basis(proj_dx, proj_dy, proj_dz);
            pb.init();
        }
        Ok(())
    }

    /// Adds the compound images of the corners, edge samples and face samples of
    /// the box with `corners` (order `000 010 100 110 001 011 101 111` for an
    /// [`Extent3d`]) to the outputs.
    fn sample_box(
        &self,
        corners: [Point3; 8],
        proj_point: &mut Point3,
        outs: &mut BoxOuts<'_>,
    ) -> Result<()> {
        let inc = 1.0 / (SAMPLE_COUNT as f64 + 1.0);
        let corner = |i: usize| corners.get(i).copied().unwrap_or(Vec3::UNDEF);
        let mut add = |p: Point3, proj_point: &mut Point3| -> Result<()> {
            // map the sample to ProjectSpace (status ignored)
            if let Some(q) = self.evaluate_point_written(p, true)? {
                *proj_point = q;
            }
            outs.add(*proj_point);
            Ok(())
        };

        // corners
        for c in corners {
            add(c, proj_point)?;
        }

        // Edges
        for (i0, i1) in SAMPLE_EDGES {
            let mut a = inc;
            for _ in 0..SAMPLE_COUNT {
                add(blend(a, corner(i0), corner(i1)), proj_point)?;
                a += inc;
            }
        }

        // faces
        for [i0, i1, j0, j1] in SAMPLE_FACES {
            let mut a = inc;
            for _ in 0..SAMPLE_COUNT {
                let pi = blend(a, corner(i0), corner(i1));
                let pj = blend(a, corner(j0), corner(j1));
                let mut b = inc;
                for _ in 0..SAMPLE_COUNT {
                    add(blend(b, pi, pj), proj_point)?;
                    b += inc;
                }
                a += inc;
            }
        }
        Ok(())
    }

    // ---- compound evaluation --------------------------------------------------------

    /// The out-space image of the parameter point (`EvaluatePoint`): position only,
    /// through the next maps with `with_compounding`.
    ///
    /// # Errors
    ///
    /// Those of [`Volume::evaluate`].
    pub fn evaluate_point(&self, param_point: Point3, with_compounding: bool) -> Result<Point3> {
        let opts = EvalOptions {
            with_compounding,
            ..EvalOptions::default()
        };
        let d = self.evaluate(param_point, 0, &opts)?;
        d.first()
            .copied()
            .ok_or(SolidError::invalid_input(OP_INV, "empty evaluation"))
    }

    /// The point `EvaluatePoint` writes when its caller ignores the status: the image
    /// on success; on failure nothing (`None`), except that a twist's, an unbend's or a
    /// bend's `Evaluate` override writes its own (first out-space) image before its next map
    /// fails.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind.
    fn evaluate_point_written(
        &self,
        param_point: Point3,
        with_compounding: bool,
    ) -> Result<Option<Point3>> {
        match self.evaluate_point(param_point, with_compounding) {
            Ok(q) => Ok(Some(q)),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => match self.kind {
                VolumeKind::Twist(_) | VolumeKind::Unbend(_) | VolumeKind::Bend(_)
                    if with_compounding && self.next_map.is_some() =>
                {
                    ignored(self.evaluate_point(param_point, false))
                }
                _ => Ok(None),
            },
        }
    }

    /// Out-space position and first partials `[P, Du, Dv, Dw]` of the parameter
    /// point (`Evaluate1stDerivatives`).
    ///
    /// # Errors
    ///
    /// Those of [`Volume::evaluate`].
    pub fn evaluate_1st_derivatives(
        &self,
        param_point: Point3,
        from_left: [bool; 3],
        non_zero_tangents: bool,
        with_compounding: bool,
    ) -> Result<[Vec3; 4]> {
        let opts = EvalOptions {
            from_left,
            non_zero_tangents,
            do_zero_sampling: true,
            with_compounding,
        };
        // sMat[2][2][2]
        let m = self.evaluate(param_point, 1, &opts)?;
        let at = |i: usize| {
            m.get(i)
                .copied()
                .ok_or(SolidError::invalid_input(OP_INV, "evaluation too short"))
        };
        Ok([at(0)?, at(4)?, at(2)?, at(1)?])
    }

    /// Whether this map or a compounded one has internal discontinuities
    /// (`HasDiscontinuities`). With `disconts` the details are written there: this
    /// map's, made "unaligned" (keeping the lower minimum continuity) when a next map
    /// has discontinuities.
    ///
    /// `calc_geometric` (upstream `bCalcGeometric`) is passed to the simple map and
    /// down the chain (without `disconts` the next maps get `false`, as upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from an unported kind.
    pub fn has_discontinuities(
        &self,
        k: &Kernel,
        mut disconts: Option<&mut Discontinuities3d>,
        calc_geometric: bool,
    ) -> Result<bool> {
        // Pass the call along to the simple map
        let mut this_has =
            self.has_discontinuities_simple(k, disconts.as_deref_mut(), calc_geometric)?;

        if let Some(next) = self.next_map.as_deref() {
            if let Some(d) = disconts {
                let mut next_disconts = Discontinuities3d::new();
                let next_has =
                    next.has_discontinuities(k, Some(&mut next_disconts), calc_geometric)?;
                // when NextMap has discontinuities - assume unaligned discontinuities
                if next_has {
                    let a = d.min_continuity;
                    let b = next_disconts.min_continuity;
                    let min_cont: ContinuityType = if (a as i32) < (b as i32) { a } else { b };
                    d.set_unaligned();
                    d.has_discontinuities |= next_has;
                    d.min_continuity = min_cont;
                }
                // combine results
                this_has |= next_has;
                d.has_discontinuities = this_has;
            } else {
                this_has |= next.has_discontinuities(k, None, false)?;
            }
        }
        Ok(this_has)
    }

    // ---- inverse point solving ------------------------------------------------------

    /// The parameter points mapping to the out-space point (`InvEvaluatePoint`):
    /// with `with_compounding` the point is first mapped back through the next maps
    /// ([`Volume::inv_map_point`], from the image of `param_guess` and inside the
    /// image of `param_domain`), then through the inverse orient map and
    /// [`Volume::inv_evaluate_point_simple`]. The gaps are the distances from the
    /// target to the points' images. `success` is `true` on return, as upstream.
    ///
    /// # Errors
    ///
    /// The failures of the inversions; [`SolidError::Unported`] from an unported
    /// kind.
    pub fn inv_evaluate_point(
        &self,
        k: &Kernel,
        out_point: Point3,
        param_guess: Option<Point3>,
        param_domain: Option<&Extent3d>,
        with_compounding: bool,
    ) -> Result<InvPoints> {
        let mut gaps = Vec::new();
        let mut points = Vec::new();

        // invert all compound maps from last to first with recursion
        let first_out = match self.next_map.as_deref().filter(|_| with_compounding) {
            Some(next) => {
                // map pOptParamDomain to this map outSpace to get Next InSpaceDomain
                let next_in_pseudo_box = match param_domain {
                    Some(d) => {
                        let mut b = Extent3d::new();
                        let mut pb = PseudoBox::new();
                        ignore_defined(self.evaluate_bounding_box(
                            k,
                            d,
                            None,
                            Some(&mut b),
                            Some(&mut pb),
                            None,
                            None,
                            false,
                        ))?;
                        Some(pb)
                    }
                    None => None,
                };
                // map pOptParamGuess to this map 1stOutSpace
                let next_in_guess = match param_guess {
                    Some(g) => Some(
                        self.evaluate_point_written(g, false)?
                            .unwrap_or(Vec3::UNDEF),
                    ),
                    None => None,
                };
                let r = next.inv_map_point(
                    k,
                    out_point,
                    next_in_guess,
                    next_in_pseudo_box.as_ref(),
                    true,
                )?;
                gaps = r.gaps;
                r.points
            }
            None => vec![out_point],
        };

        // map pOutSpace back to ParamSpace
        for p in first_out {
            let proj = match self.orient_inv()? {
                Some(inv) => transform_point(inv, p)?,
                None => p,
            };
            let (_, this_points, this_gaps) =
                self.inv_evaluate_point_simple(proj, param_guess, param_domain)?;
            gaps = this_gaps;
            points.extend(this_points);
        }

        // Get map found rParamPoint back to OutSpace
        let mut out_space = Vec3::UNDEF;
        gaps.resize(points.len(), 0.0);
        for (gap, p) in gaps.iter_mut().zip(&points) {
            if let Some(q) = self.evaluate_point_written(*p, with_compounding)? {
                out_space = q;
            }
            *gap = out_point.distance(out_space);
        }
        Ok(InvPoints {
            success: true,
            points,
            gaps,
        })
    }

    /// The in-space points mapping to the out-space point (`InvMapPoint`): with
    /// `with_compounding` mapped back through the next maps first, then through the
    /// inverse orient map, [`Volume::inv_evaluate_point_simple`] and the orient map.
    /// `in_guess` and `in_domain` (an in-space pseudo box) guide the search.
    /// `success` is `true` on return, as upstream.
    ///
    /// # Errors
    ///
    /// The failures of the inversions; [`SolidError::Unported`] from an unported
    /// kind.
    pub fn inv_map_point(
        &self,
        k: &Kernel,
        out_point: Point3,
        in_guess: Option<Point3>,
        in_domain: Option<&PseudoBox>,
        with_compounding: bool,
    ) -> Result<InvPoints> {
        let mut gaps = Vec::new();
        let mut in_points = Vec::new();

        // map crOutSpacePoint from LastOutSpace back to 1stOutSpace
        let first_out = match self.next_map.as_deref().filter(|_| with_compounding) {
            Some(next) => {
                let next_in_box = match in_domain {
                    Some(d) => {
                        let mut pb = PseudoBox::new();
                        ignore_defined(self.map_pseudo_box(
                            k,
                            d,
                            None,
                            Some(&mut pb),
                            false,
                            true,
                        ))?;
                        Some(pb)
                    }
                    None => None,
                };
                let next_in_guess = match in_guess {
                    Some(g) => Some(ignored(self.map_point(g, false))?.unwrap_or(Vec3::UNDEF)),
                    None => None,
                };
                let r =
                    next.inv_map_point(k, out_point, next_in_guess, next_in_box.as_ref(), true)?;
                gaps = r.gaps;
                r.points
            }
            None => vec![out_point],
        };

        if let Some(inv) = self.orient_inv()? {
            let orient = self.orient_map.as_ref().ok_or(SolidError::invalid_input(
                OP_INV,
                "inverse orient map without the orient map",
            ))?;
            // Map pOptInSpaceDomain to ParamSpace
            let param_box = match in_domain {
                Some(d) => {
                    let mut b = Extent3d::new();
                    ignore_status(simple(inv).evaluate_pseudo_box_simple(d, Some(&mut b), None))?;
                    let src = b;
                    ignore_status(self.trim_param_bounding_box_simple(&src, &mut b))?;
                    Some(b)
                }
                None => None,
            };
            // map pOptInPointGuess to ParamSpace (status ignored)
            let param_guess = match in_guess {
                Some(g) => Some(ignored(transform_point(inv, g))?.unwrap_or(Vec3::UNDEF)),
                None => None,
            };
            for p in first_out {
                let proj = transform_point(inv, p)?;
                let (_, this_points, this_gaps) =
                    self.inv_evaluate_point_simple(proj, param_guess, param_box.as_ref())?;
                gaps = this_gaps;
                for q in this_points {
                    in_points.push(transform_point(orient, q)?);
                }
            }
        } else {
            let param_box = match in_domain {
                Some(d) => {
                    let mut b = Extent3d::from_pseudo_box(d);
                    let src = b;
                    ignore_status(self.trim_param_bounding_box_simple(&src, &mut b))?;
                    Some(b)
                }
                None => None,
            };
            for p in first_out {
                let (_, this_points, this_gaps) =
                    self.inv_evaluate_point_simple(p, in_guess, param_box.as_ref())?;
                gaps = this_gaps;
                in_points.extend(this_points);
            }
        }

        // map found rInSpacePoints back to OutSpace to calc gaps
        let mut out = Vec3::UNDEF;
        gaps.resize(in_points.len(), 0.0);
        for (gap, p) in gaps.iter_mut().zip(&in_points) {
            if let Some(q) = ignored(self.map_point(*p, with_compounding))? {
                out = q;
            }
            *gap = out_point.distance(out);
        }
        Ok(InvPoints {
            success: true,
            points: in_points,
            gaps,
        })
    }

    /// The parameter-space solutions of the out-space point without a guess
    /// (`GlobalPointSolve`): mapped back through the next maps (inside the image of
    /// `param_domain`) and the inverse orient map, then solved by
    /// [`Volume::global_point_solve_simple`] for each first out-space point. Returns
    /// the last solve's found flag and all solutions (object `this`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the next maps cannot map the point back; the solvers' failures;
    /// [`SolidError::Unported`] from an unported kind.
    pub fn global_point_solve<O: SolutionObject>(
        &self,
        k: &Kernel,
        out_point: Point3,
        param_domain: Option<&Extent3d>,
        with_compounding: bool,
        this: O,
    ) -> Result<(bool, SolutionArray<O>)> {
        let mut found = false;
        let mut solutions = SolutionArray::new();

        let first_out = match self.next_map.as_deref().filter(|_| with_compounding) {
            Some(next) => {
                // map pOptParamDomain from ParamSpace to 1stOutSpace
                let next_in_pseudo_box = match param_domain {
                    Some(d) => {
                        let mut b = Extent3d::new();
                        let mut pb = PseudoBox::new();
                        ignore_defined(self.evaluate_bounding_box(
                            k,
                            d,
                            None,
                            Some(&mut b),
                            Some(&mut pb),
                            None,
                            None,
                            false,
                        ))?;
                        Some(pb)
                    }
                    None => None,
                };
                let r =
                    next.inv_map_point(k, out_point, None, next_in_pseudo_box.as_ref(), true)?;
                // quit when mapping back to 1st OutSpace fails
                if !r.success {
                    return Err(SolidError::sm_err(OP_SOLVE));
                }
                r.points
            }
            None => vec![out_point],
        };

        let mut this_solutions = SolutionArray::new();
        for p in first_out {
            let proj = match self.orient_inv()? {
                Some(inv) => transform_point(inv, p)?,
                None => p,
            };
            found =
                self.global_point_solve_simple(proj, &mut this_solutions, param_domain, this)?;
            solutions.append(&this_solutions);
        }
        Ok((found, solutions))
    }

    /// The parameter points mapping to the projection-space point
    /// (`InvEvaluatePointSimple`): [`Volume::local_point_solve_simple`] from the
    /// guess, else (or when that fails) [`Volume::global_point_solve_simple`].
    /// Returns the success flag, the points and their gaps (the solution values);
    /// a failed local solve forces the flag to `false` even when the global solve
    /// succeeds, as upstream.
    ///
    /// # Errors
    ///
    /// The global solve's failure; [`SolidError::Unported`] from an unported kind.
    pub fn inv_evaluate_point_simple(
        &self,
        proj_point: Point3,
        param_guess: Option<Point3>,
        param_domain: Option<&Extent3d>,
    ) -> Result<(bool, Vec<Point3>, Vec<f64>)> {
        let mut status_ok = true;
        let mut success = false;
        let mut solutions: SolutionArray<NoHandle> = SolutionArray::new();

        // Try LocalPointSolve if a guess was supplied
        if let Some(g) = param_guess {
            solutions.set_size(1);
            if let Some(sol) = solutions.get_mut(0) {
                match self.local_point_solve_simple(proj_point, g, sol, param_domain, NoHandle) {
                    Ok(f) => success = f,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => {
                        status_ok = false;
                        success = false;
                    }
                }
            }
        }

        // When LocalPointSolve() can't be used or fails, try GlobalPointSolve
        if !success {
            success =
                self.global_point_solve_simple(proj_point, &mut solutions, param_domain, NoHandle)?;
            if !status_ok {
                success = false;
            }
        }

        let mut points = Vec::new();
        let mut gaps = Vec::new();
        if success {
            for s in solutions.as_slice() {
                let [u, v, w, ..] = s.start.parameters;
                points.push(Vec3::new(u, v, w));
                gaps.push(s.start.solution_value);
            }
        }
        Ok((success, points, gaps))
    }

    /// The parameter-space solutions of the projection-space point without a guess
    /// (`GlobalPointSolveSimple`, virtual), written to `solutions`; returns
    /// `rbFoundAnswer`. A transform inverts its matrix, a twist its rotation; the
    /// base class runs [`Volume::local_point_solve_simple`] from every
    /// [`Volume::inv_evaluate_guess_point_simple`] guess (none when that fails).
    ///
    /// # Errors
    ///
    /// A singular transform's `SM_ERR`; the local solves' failures (a bend's
    /// solves cannot fail).
    pub fn global_point_solve_simple<O: SolutionObject>(
        &self,
        proj_point: Point3,
        solutions: &mut SolutionArray<O>,
        param_domain: Option<&Extent3d>,
        this: O,
    ) -> Result<bool> {
        match &self.kind {
            VolumeKind::Transform(t) => t
                .global_point_solve_simple(proj_point, solutions, this)
                .map(|()| true),
            VolumeKind::Twist(t) => {
                Ok(t.global_point_solve_simple(proj_point, solutions, param_domain, this))
            }
            VolumeKind::Unbend(u) => {
                u.global_point_solve_simple(self, proj_point, solutions, param_domain, this)
            }
            VolumeKind::Bend(b) => Ok(b.global_point_solve_simple(proj_point, solutions, this)),
            VolumeKind::Base | VolumeKind::BSpline(_) => {
                // init output
                let mut found = false;
                solutions.reset();
                // Get sGuessPoint (status ignored)
                let guesses =
                    ignored(self.inv_evaluate_guess_point_simple(proj_point))?.unwrap_or_default();
                let mut this_solution = Solution::new();
                for g in guesses {
                    let this_found = self.local_point_solve_simple(
                        proj_point,
                        g,
                        &mut this_solution,
                        param_domain,
                        this,
                    )?;
                    found |= this_found;
                    if this_found {
                        solutions.add(&this_solution);
                    }
                }
                Ok(found)
            }
        }
    }

    /// The parameter-space solution of the projection-space point from the guess
    /// (`LocalPointSolveSimple`, virtual), written to `solution`; returns
    /// `rbFoundAnswer`. The base class runs a Newton solve of
    /// [`FindPvIntersect`] (tolerance `SM_EFF_ZERO^2`, clamped to `param_domain` or
    /// the natural domain, three boundary hits, acceptable accuracy
    /// `100 SM_EFF_ZERO_SQRT`); its solution value is the distance from the target
    /// to the compound image of the found point.
    ///
    /// # Errors
    ///
    /// The solver's and the evaluation's failures; a singular transform's `SM_ERR`
    /// (a bend's solves cannot fail).
    pub fn local_point_solve_simple<O: SolutionObject>(
        &self,
        proj_point: Point3,
        param_guess: Point3,
        solution: &mut Solution<O>,
        param_domain: Option<&Extent3d>,
        this: O,
    ) -> Result<bool> {
        match &self.kind {
            VolumeKind::Transform(t) => {
                *solution = t.local_point_solve_simple(proj_point, this)?;
                Ok(true)
            }
            VolumeKind::Twist(t) => {
                Ok(t.local_point_solve_simple(proj_point, solution, param_domain, this))
            }
            VolumeKind::Unbend(u) => {
                u.local_point_solve_simple(self, proj_point, solution, param_domain, this)
            }
            VolumeKind::Bend(b) => {
                Ok(b.local_point_solve_simple(proj_point, param_guess, solution, this))
            }
            VolumeKind::Base | VolumeKind::BSpline(_) => self.base_local_point_solve_simple(
                proj_point,
                param_guess,
                solution,
                param_domain,
                this,
            ),
        }
    }

    /// `SmVolume::LocalPointSolveSimple`.
    fn base_local_point_solve_simple<O: SolutionObject>(
        &self,
        proj_point: Point3,
        param_guess: Point3,
        solution: &mut Solution<O>,
        param_domain: Option<&Extent3d>,
        this: O,
    ) -> Result<bool> {
        // init output
        solution.solution_type = SolutionType::Unknown;

        let domain = match param_domain {
            Some(d) => *d,
            None => self.natural_param_domain()?,
        };

        // SolveIt Interval locals (SetMinMax statuses ignored)
        let mut intervals = ExtentNd::new(3);
        for (i, (lo, hi)) in [
            (domain.u_min(), domain.u_max()),
            (domain.v_min(), domain.v_max()),
            (domain.w_min(), domain.w_max()),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(ivl) = intervals.get_mut(i) {
                let _ = ivl.set_min_max(lo, hi);
            }
        }
        // for now - don't check for periodicity in Volumes
        let periodicities = [false; 3];
        let guess = [param_guess.x, param_guess.y, param_guess.z];

        // 1. construct NR Solver
        let mut eval_fun = FindPvIntersect::new(proj_point, self, EFF_ZERO * EFF_ZERO);
        let mut ls = LocalSolveNd::new(&mut eval_fun, Some(&intervals), Some(&periodicities));
        ls.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 3);

        // 2. find answer with NR solver
        let r = ls.solve_it(&guess, EFF_ZERO_SQRT * 100.0)?;

        // 3. when SolveIt failed to converge
        if !r.found {
            return Ok(false);
        }
        let x = |i: usize| r.x.get(i).copied().unwrap_or(0.0);
        let uvw = Vec3::new(x(0), x(1), x(2));
        let pnt1 = self.evaluate_point(uvw, true)?;

        solution.solution_type = SolutionType::SingleValue;
        solution.start.solution_value = pnt1.distance(proj_point);
        solution.start.parameters[0] = x(0);
        solution.start.parameters[1] = x(1);
        solution.start.parameters[2] = x(2);
        solution.num_variables = 3;
        solution.num_objects = 1;
        solution.objects[0] = Some(this);
        Ok(true)
    }

    /// The inverse orient map when there is an orient map (upstream tests
    /// `m_pOrientMap` and uses `m_pInvOrientMap`).
    fn orient_inv(&self) -> Result<Option<&Transform>> {
        if self.orient_map.is_none() {
            return Ok(None);
        }
        self.inv_orient_map
            .as_ref()
            .map(Some)
            .ok_or(SolidError::invalid_input(
                OP_INV,
                "orient map without its inverse (upstream dereferences null)",
            ))
    }

    // ---- persistence ----------------------------------------------------------------

    /// Reads a volume of type `volume_type` from a database stream (`ReadFromDB`):
    /// the derived class reads its data and then the base data
    /// ([`Volume::read_base_from_db`]).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an unknown type and a B-spline volume (not readable in
    /// upstream's build); the derived readers' failures; [`SolidError::Unsupported`]
    /// for `SmVolume_TYPE` (upstream fills a null volume) and the undefined base
    /// reads; [`SolidError::InvalidInput`] past the nesting bound.
    pub fn read_from_db(volume_type: i64, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        db.enter_nested(OP_READ)?;
        let r = Self::read_dispatch(volume_type, db, version);
        db.leave_nested();
        r
    }

    /// The type switch of `ReadFromDB`.
    fn read_dispatch(volume_type: i64, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        match volume_type {
            B_SPLINE_VOLUME_TYPE => {
                BSplineVolume::read_from_db(version).map(|b| Self::new(VolumeKind::BSpline(b)))
            }
            BEND_VOLUME_TYPE => BendVolume::read_from_db(db, version),
            TRANSFORM_TYPE => Transform::read_from_db(db, version),
            UNBEND_VOLUME_TYPE => UnbendVolume::read_from_db(db, version),
            TWIST_VOLUME_TYPE => TwistVolume::read_from_db(db, version),
            VOLUME_TYPE => Err(unsupported("SmVolume_TYPE without a volume to fill")),
            _ => Err(SolidError::sm_err(OP_READ)),
        }
    }

    /// Reads the base-class data into this (derived) volume
    /// (`ReadFromDB(SmVolume_TYPE, ..)`): the next-map flag and next map (owned,
    /// owner flag 1), then the orient-map flag and orient map (whose inverse is
    /// rebuilt). Upstream ignores the nested reads' statuses: a next map that fails
    /// to read is left out.
    ///
    /// # Errors
    ///
    /// A failed binary flag read; [`SolidError::Unsupported`] for the undefined
    /// cases (module docs); [`SolidError::Unported`] from an unported kind.
    pub fn read_base_from_db(&mut self, db: &mut DatabaseReader, version: u64) -> Result<()> {
        // check for NextMap
        let has_next = read_flag(db)?;
        self.next_map = None;
        if has_next != 0 {
            let next_type = read_map_type(db)?;
            // read NextMap - recursion gets all NextMaps in the linked list
            let next = ignore_read(Self::read_from_db(next_type, db, version))?;
            self.next_map = next.map(Box::new);
            self.next_owner_flag = 1;
        }

        // check for OrientMap
        let has_orient = read_flag(db)?;
        self.orient_map = None;
        if has_orient != 0 {
            let orient_type = read_map_type(db)?;
            let orient = ignore_read(Self::read_from_db(orient_type, db, version))?;
            match orient.map(|v| v.kind) {
                Some(VolumeKind::Transform(t)) => self.orient_map = Some(t),
                Some(_) => return Err(unsupported("orient map read as a non-transform volume")),
                None => return Err(unsupported("orient map read failed (null orient map)")),
            }
            // set the inverse orient map (cannot fail with an orient map)
            self.refresh_inv_orient_map()?;
        }
        Ok(())
    }
}

/// [`SolidError::Unsupported`] marking undefined behaviour upstream in the reader.
const fn unsupported(reason: &'static str) -> SolidError {
    SolidError::Unsupported {
        operation: OP_READ,
        reason,
    }
}

/// The result of a read whose status upstream ignores: `None` on failure, but
/// [`SolidError::Unported`], the undefined cases ([`SolidError::Unsupported`]) and
/// the nesting bound of [`DatabaseReader::enter_nested`] (not upstream: swallowing
/// it would return a shorter chain instead of an error) propagate.
pub(crate) fn ignore_read<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (SolidError::Unported { .. } | SolidError::Unsupported { .. })) => Err(e),
        Err(
            e
            @ (SolidError::InvalidInput { reason, .. } | SolidError::PortInvariant { reason, .. }),
        ) if reason == NESTING_LIMIT_REASON => Err(e),
        Err(_) => Ok(None),
    }
}

/// The status of a call upstream ignores, except that [`SolidError::Unported`] and
/// the undefined cases ([`SolidError::Unsupported`]) propagate.
fn ignore_defined(r: Result<()>) -> Result<()> {
    ignore_read(r).map(|_| ())
}

/// [`SolidError::Unsupported`] for a box mapping upstream reaches through a null
/// pointer.
const fn undefined_box(reason: &'static str) -> SolidError {
    SolidError::Unsupported {
        operation: OP_BOX,
        reason,
    }
}

/// A `SmBoolean` flag: `>>` then `GoToNextLine` in ASCII (an extraction that cannot
/// start leaves `FALSE`), `ReadBoolean` in binary.
fn read_flag(db: &mut DatabaseReader) -> Result<i32> {
    if db.file_type() == FileType::Ascii {
        let v = db
            .in_stream_mut()
            .and_then(InputStream::extract_int)
            .unwrap_or(0);
        db.go_to_next_line();
        Ok(v)
    } else {
        db.read_boolean()
    }
}

/// A map type after a "has map" flag (`ReadType`, status ignored); a type that is
/// never assigned is undefined upstream.
fn read_map_type(db: &mut DatabaseReader) -> Result<i64> {
    match db.read_type(false) {
        Ok(h) => h
            .type_code
            .ok_or_else(|| unsupported("map type not read (uninitialised type upstream)")),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Err(unsupported(
            "map type not read (uninitialised type upstream)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Me;

    impl SolutionObject for Me {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Volume
        }
    }

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-12
    }

    fn affine(o: Vec3, x: Vec3, y: Vec3, z: Vec3) -> Transform {
        Transform::new(o, x, y, Some(z))
    }

    fn shift(o: Vec3) -> Transform {
        affine(o, Vec3::X, Vec3::Y, Vec3::Z)
    }

    fn scaled() -> Transform {
        affine(
            v(1.0, 2.0, 3.0),
            v(2.0, 0.0, 0.0),
            v(0.0, 3.0, 0.0),
            v(0.0, 0.0, 4.0),
        )
    }

    fn tv(t: Transform) -> Volume {
        Volume::new(VolumeKind::Transform(t))
    }

    fn unit_box() -> Extent3d {
        Extent3d::from_points(v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0))
    }

    #[test]
    fn evaluate_point_compounds() {
        let vol = Volume::with_maps(
            VolumeKind::Transform(scaled()),
            Some(shift(v(10.0, 0.0, 0.0))),
            Some(tv(shift(v(0.0, 0.0, 5.0)))),
        )
        .unwrap();
        let p = vol.evaluate_point(v(1.0, 1.0, 1.0), true).unwrap();
        let q = vol.evaluate_point(v(1.0, 1.0, 1.0), false).unwrap();
        assert!(close(p - q, v(0.0, 0.0, 5.0)));
        let [p1, du, dv, dw] = vol
            .evaluate_1st_derivatives(v(1.0, 1.0, 1.0), [true; 3], true, true)
            .unwrap();
        assert!(close(p1, p));
        assert!(close(du, v(2.0, 0.0, 0.0)));
        assert!(close(dv, v(0.0, 3.0, 0.0)));
        assert!(close(dw, v(0.0, 0.0, 4.0)));
        // a plain volume cannot evaluate
        assert!(Volume::new(VolumeKind::Base)
            .evaluate_point(v(0.0, 0.0, 0.0), true)
            .is_err());
    }

    #[test]
    fn domain_checks_of_unbounded_maps() {
        let vol = tv(scaled());
        assert!(vol
            .is_point_in_param_domain(v(5.0, 5.0, 5.0), true)
            .unwrap());
        let k = Kernel::default();
        assert!(vol.is_box_in_param_domain(&k, &unit_box(), true).unwrap());
        let (inside, a, b) = vol
            .is_line_in_in_space_domain(&k, v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0), true)
            .unwrap();
        assert!(inside);
        assert!(close(a, v(0.0, 0.0, 0.0)) && close(b, v(1.0, 1.0, 1.0)));
        let pb = PseudoBox::from_extent3d(&unit_box());
        assert!(vol.is_pseudo_box_in_in_space_domain(&k, &pb, true).unwrap());
    }

    #[test]
    fn bounding_boxes_of_a_transform() {
        let vol = Volume::with_maps(VolumeKind::Transform(scaled()), None, None).unwrap();
        let ppb = PseudoBox::new();
        let mut out = Extent3d::new();
        let mut in_box = Extent3d::new();
        let k = Kernel::default();
        vol.evaluate_bounding_box(
            &k,
            &unit_box(),
            Some(&ppb),
            Some(&mut out),
            None,
            Some(&mut in_box),
            None,
            true,
        )
        .unwrap();
        assert_eq!(in_box, unit_box());
        assert!(out.is_bounded());
        // no outputs - no work
        vol.evaluate_bounding_box(&k, &unit_box(), None, None, None, None, None, true)
            .unwrap();
        // a transform without a parameter pseudo box dereferences null upstream
        assert!(vol
            .evaluate_bounding_box(
                &k,
                &unit_box(),
                None,
                Some(&mut out),
                None,
                None,
                None,
                true
            )
            .is_err());
    }

    #[test]
    fn base_sampler_samples_the_compound_map() {
        let k = &Kernel::default();
        // a plain volume with a transform next map: the sampler's evaluations fail
        // (the plain simple map), so every sample is the undefined point
        let vol = Volume::with_maps(VolumeKind::Base, None, Some(tv(scaled()))).unwrap();
        let mut b = Extent3d::new();
        let mut pb = PseudoBox::new();
        vol.evaluate_bounding_box_simple(k, &unit_box(), None, Some(&mut b), Some(&mut pb))
            .unwrap();
        assert_eq!(b.min(), Vec3::UNDEF);
        // unbounded input boxes give unbounded output boxes
        let mut unb = Extent3d::new();
        unb.set_unbounded();
        vol.evaluate_bounding_box_simple(k, &unb, None, Some(&mut b), None)
            .unwrap();
        assert!(!b.is_bounded());
        // the pseudo-box sampler of a transform volume maps exactly at the corners
        let t = tv(scaled());
        let mut pbox = Extent3d::new();
        t.evaluate_pseudo_box_simple(
            &PseudoBox::from_extent3d(&unit_box()),
            Some(&mut pbox),
            None,
        )
        .unwrap();
        assert!(close(pbox.min(), v(1.0, 2.0, 3.0)));
        assert!(close(pbox.max(), v(3.0, 5.0, 7.0)));
    }

    #[test]
    fn inverse_points_of_transforms() {
        let vol = Volume::with_maps(
            VolumeKind::Transform(scaled()),
            Some(shift(v(10.0, 0.0, 0.0))),
            Some(tv(shift(v(0.0, 0.0, 5.0)))),
        )
        .unwrap();
        let target = vol.evaluate_point(v(0.25, 0.5, 0.75), true).unwrap();
        let k = Kernel::default();
        let r = vol
            .inv_evaluate_point(&k, target, None, None, true)
            .unwrap();
        assert!(r.success);
        assert_eq!(r.points.len(), 1);
        assert!(close(r.points[0], v(0.25, 0.5, 0.75)));
        assert!(r.gaps[0] < 1e-12);
        let m = vol
            .inv_map_point(&k, target, Some(v(0.0, 0.0, 0.0)), None, true)
            .unwrap();
        assert_eq!(m.points.len(), 1);
        let (found, sols) = vol.global_point_solve(&k, target, None, true, Me).unwrap();
        assert!(found);
        assert_eq!(sols.len(), 1);
        // a singular transform fails
        let flat = tv(affine(v(0.0, 0.0, 0.0), Vec3::X, Vec3::Y, Vec3::ZERO));
        assert!(flat
            .inv_evaluate_point(&k, v(1.0, 1.0, 1.0), None, None, true)
            .is_err());
    }

    #[test]
    fn base_solvers_without_an_evaluation() {
        let vol = Volume::new(VolumeKind::Base);
        let mut sols: SolutionArray<Me> = SolutionArray::new();
        // the base guess fails: no guesses, nothing found
        assert!(!vol
            .global_point_solve_simple(v(1.0, 2.0, 3.0), &mut sols, None, Me)
            .unwrap());
        assert!(sols.is_empty());
        let (ok, pts, gaps) = vol
            .inv_evaluate_point_simple(v(1.0, 2.0, 3.0), None, None)
            .unwrap();
        assert!(!ok && pts.is_empty() && gaps.is_empty());
    }

    #[test]
    fn discontinuities_of_chains() {
        let k = &Kernel::default();
        let vol =
            Volume::with_maps(VolumeKind::Transform(scaled()), None, Some(tv(scaled()))).unwrap();
        let mut d = Discontinuities3d::new();
        d.set_unaligned();
        assert!(!vol.has_discontinuities(k, Some(&mut d), true).unwrap());
        assert_eq!(d, Discontinuities3d::new());
        assert!(!vol.has_discontinuities(k, None, true).unwrap());
    }

    fn ascii(s: &str) -> DatabaseReader {
        let mut db = DatabaseReader::new();
        db.open_for_read(s.as_bytes().to_vec(), FileType::Ascii);
        db
    }

    #[test]
    fn reads_chains() {
        let text = format!(
            "1 0 0 0\n0 1 0 0\n0 0 1 0\n1 2 3 1\n1\nObject Type {TRANSFORM_TYPE}\n\
             2 0 0 0\n0 2 0 0\n0 0 2 0\n0 0 0 1\n0\n0\n1\nObject Type {TRANSFORM_TYPE}\n\
             1 0 0 0\n0 1 0 0\n0 0 1 0\n5 0 0 1\n0\n0\n"
        );
        let mut db = ascii(&text);
        let vol = Volume::read_from_db(TRANSFORM_TYPE, &mut db, 30).unwrap();
        assert!(vol.next_map().is_some());
        assert!(vol.orient_map().is_some() && vol.inv_orient_map().is_some());
        // a top-level plain volume has nothing to fill
        let mut db = ascii("0\n0\n");
        assert!(matches!(
            Volume::read_from_db(VOLUME_TYPE, &mut db, 30),
            Err(SolidError::Unsupported { .. })
        ));
        // unknown types and B-spline volumes fail
        assert!(Volume::read_from_db(12345, &mut ascii(""), 30).is_err());
        assert!(Volume::read_from_db(B_SPLINE_VOLUME_TYPE, &mut ascii(""), 30).is_err());
        // a failed next map is left out
        let text = format!(
            "1 0 0 0\n0 1 0 0\n0 0 1 0\n0 0 0 1\n1\nObject Type {B_SPLINE_VOLUME_TYPE}\n0\n"
        );
        let vol = Volume::read_from_db(TRANSFORM_TYPE, &mut ascii(&text), 30).unwrap();
        assert!(vol.next_map().is_none());
    }

    #[test]
    fn reads_unbend_volumes() {
        let text = format!("2.5\n0.01\n0\n1\nObject Type {TRANSFORM_TYPE}\n1 0 0 0\n0 1 0 0\n0 0 1 0\n1 2 3 1\n0\n0\n");
        let vol = Volume::read_from_db(UNBEND_VOLUME_TYPE, &mut ascii(&text), 30).unwrap();
        match vol.kind() {
            VolumeKind::Unbend(u) => {
                assert_eq!(u.unbend_neutral_dist(), 2.5);
                assert_eq!(u.singularity_tol(), 0.01);
            }
            k => panic!("not an unbend: {k:?}"),
        }
        assert!(vol.next_map().is_none());
        assert!(close(vol.orient_origin(), v(1.0, 2.0, 3.0)));
    }

    #[test]
    fn unbend_overrides_are_dispatched() {
        let k = &Kernel::default();
        let vol = UnbendVolume::create(
            v(0.0, 0.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(1.0, 0.0, 0.0),
            2.0,
            0.01,
        );
        // the unbend's solvers invert exactly
        let target = vol.evaluate_point(v(1.5, 0.5, 0.25), true).unwrap();
        let mut sols = SolutionArray::new();
        assert!(vol
            .global_point_solve_simple(target, &mut sols, None, Me)
            .unwrap());
        let p = sols.get(0).unwrap().start.parameters;
        assert!(close(v(p[0], p[1], p[2]), v(1.5, 0.5, 0.25)));
        // its box covers the mapped corners
        let mut b = Extent3d::new();
        vol.evaluate_bounding_box_simple(k, &unit_box(), None, Some(&mut b), None)
            .unwrap();
        assert!(b.contains_point(vol.evaluate_point(v(1.0, 1.0, 1.0), true).unwrap(), 1e-9));
        // a zero-length box edge is refused (upstream reads a null line)
        let flat = Extent3d::from_points(v(0.5, 0.5, 0.5), v(0.5, 1.0, 1.0));
        assert!(vol
            .evaluate_bounding_box_simple(k, &flat, None, Some(&mut b), None)
            .is_err());
    }

    #[test]
    fn unbend_leaves_its_own_image_when_the_next_map_fails() {
        let mut vol = UnbendVolume::create(
            v(0.0, 0.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(1.0, 0.0, 0.0),
            2.0,
            0.01,
        );
        vol.add_next_map(Volume::new(VolumeKind::Base), 2).unwrap();
        let p = v(1.5, 0.5, 0.25);
        assert!(vol.evaluate_point(p, true).is_err());
        let own = vol.evaluate_point(p, false).unwrap();
        assert_eq!(vol.evaluate_point_written(p, true).unwrap(), Some(own));
    }

    #[test]
    fn nesting_bound_is_not_an_ignored_status() {
        // MAX_READ_NESTING + 1 (33) twists chained by next maps, one past the bound;
        // the twist data and base flags are zeros, so a swallowed bound error would
        // leave a stream that still reads as a shorter (32-map) chain
        let max = crate::topology::database_io::MAX_READ_NESTING as usize;
        assert_eq!(max, 32);
        let twist = format!("0\n0\n1\nObject Type {TWIST_VOLUME_TYPE}\n");
        let mut text = twist.repeat(max);
        text.push_str("0\n0\n0\n0\n");
        text.push_str(&"0\n".repeat(max));
        assert!(matches!(
            Volume::read_from_db(TWIST_VOLUME_TYPE, &mut ascii(&text), 30),
            Err(SolidError::InvalidInput { reason, .. }) if reason == NESTING_LIMIT_REASON
        ));
        // MAX_READ_NESTING (32) maps read
        let mut text = twist.repeat(max - 1);
        text.push_str("0\n0\n0\n0\n");
        text.push_str(&"0\n".repeat(max - 1));
        let vol = Volume::read_from_db(TWIST_VOLUME_TYPE, &mut ascii(&text), 30).unwrap();
        let mut n = 0;
        let mut p = Some(&vol);
        while let Some(q) = p {
            n += 1;
            p = q.next_map();
        }
        assert_eq!(n, max);
    }

    #[test]
    fn nested_reads_are_bounded() {
        let text = format!("1 0 0 0\n0 1 0 0\n0 0 1 0\n0 0 0 1\n1\nObject Type {TRANSFORM_TYPE}\n")
            .repeat(1000);
        assert!(Volume::read_from_db(TRANSFORM_TYPE, &mut ascii(&text), 30).is_err());
    }
}
