//! Tapered (drafted) extrusions of a chain of curves.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_taper_extrude`] | `CreateTaperExtrude` |
//! | [`PrimitiveCreation::create_taper_extrude_with_refusal`] | `CreateTaperExtrude` (fork: `pOptRefusal`, `pOptRefusedCorner`) |
//!
//! Deviations: the input curves are consumed, as upstream on success. Upstream's
//! curve objects are shared between the split input list, the composite and the
//! ruled surfaces, which mutate them; here the composite owns its segments, the
//! ruled surface step edits their B-splines (`generators`), and the first two split
//! curves (whose intervals and rationality the sliver and weight tests read) are
//! found among the segments by their exact interval and end points. Duplicates are
//! removed before splitting, so that match is unique. A single split curve gives
//! an empty second interval where upstream reads past its array (UB-112). Offset
//! map attributes travel with their curves through the chain and gap steps by
//! their input indices, and sit in the access's attribute store on faces. An
//! analytic offset continues as its B-spline after its ruled surface is built.
//! Ignored statuses keep their outputs. No recursion. source changes
//! (source fork 54148db, e8f23e0, in `taper_walls`):
//! [`OffsetCornerType::CurveExtension`] and, under opt-in
//! `fixes.taper_tangent_corner_planes` (WR-521), linear-extension corners of a profile
//! of lines and arcs build their walls exactly and may refuse a corner; the refusal
//! is a return value, not out-params.

use super::taper_walls::{
    curvature_keeping_walls, has_corner, lines_and_arcs, CornerRefusal, WallsInput,
};
use super::{as_bspline, PrimitiveCreationAccess};
use crate::curve::composite_curve::{
    CompositeCurve, OffsetCornerType, OffsetDirection, OffsetMapAttribute, OffsetParams,
};
use crate::curve::curve::p3::{compute_crvs_normal, crvs_fix_gaps};
use crate::curve::curve::p4::fix_chain_curves_list;
use crate::curve::curve::p5::crvs_cleanup;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::tol::ANG_TOL_DEG;
use crate::kernel::Kernel;
use crate::math::{Vec3, PI};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::b_spline_surface::p1::BSplineSurface;
use crate::primitive::primitive_creation::p1::PrimitiveCreation;
use crate::primitive::primitive_creation::p3::{ruled_surface_from_offset, OffsetRuleOptions};
use crate::surface::surface::Surface;
use crate::topology::BrepId;

const OP: &str = "primitive::primitive_creation::p2::taper";

/// Whether two B-splines are the same split curve: equal natural intervals and end
/// points, bit for bit.
fn same_curve(a: &BSplineCurve, b: &BSplineCurve) -> bool {
    let (ia, ib) = (a.natural_interval(), b.natural_interval());
    if ia.min() != ib.min() || ia.max() != ib.max() {
        return false;
    }
    match (a.ends(), b.ends()) {
        (Ok(x), Ok(y)) => x.start == y.start && x.end == y.end,
        _ => false,
    }
}

/// The interval and rationality of split curve `slot`, read from its composite
/// segment (which the ruled surfaces may have changed) when it has one.
fn current_curve(
    split: &[BSplineCurve],
    links: [Option<usize>; 2],
    generators: &[BSplineCurve],
    slot: usize,
) -> Result<Option<(Extent1d, bool)>> {
    let linked = links
        .get(slot)
        .copied()
        .flatten()
        .and_then(|g| generators.get(g));
    match linked.or_else(|| split.get(slot)) {
        Some(curve) => Ok(Some((curve.natural_interval(), curve.is_rational()?))),
        None => Ok(None),
    }
}

/// Write the edited B-spline generators back into the composite's segments, in
/// place (the segment curves keep their owners).
fn sync_generators(composite: &mut CompositeCurve, generators: &[BSplineCurve]) {
    for (index, generator) in generators.iter().enumerate() {
        if let Some(curve) = composite.segment_b_spline_mut(index) {
            *curve = generator.clone();
        }
    }
}

/// One ruled surface between an offset curve and its generating curve
/// (`sm_CreateRuledSurfaceFromOffsetCurve`), with the split curves' current state.
fn ruled(
    k: &Kernel,
    offset_curve: &mut BSplineCurve,
    map: Option<OffsetMapAttribute>,
    state: &mut TaperState,
    options: OffsetRuleOptions,
) -> Result<(BSplineSurface, Option<OffsetMapAttribute>)> {
    let first = current_curve(&state.split, state.links, &state.generators, 0)?;
    let second = current_curve(&state.split, state.links, &state.generators, 1)?;
    let (first_interval, first_rational) = first.ok_or_else(|| SolidError::sm_err(OP))?;
    let second_interval = second.map_or_else(Extent1d::new, |(interval, _)| interval);
    ruled_surface_from_offset(
        k,
        offset_curve,
        map,
        &mut state.generators,
        [first_interval, second_interval],
        first_rational,
        &mut state.unmarried,
        &mut state.missing_faces,
        options,
    )
}

/// The split curves and their marriage state, shared by both ruled-surface passes.
struct TaperState {
    /// `sCurves`: the input curves split at G1 discontinuities.
    split: Vec<BSplineCurve>,
    /// The composite segment of the first two split curves.
    links: [Option<usize>; 2],
    /// The composite's segment curves (`m_pParentCurve`), as the ruled surfaces edit them.
    generators: Vec<BSplineCurve>,
    /// `sUnMarriedCurves`: whether each split curve is still unpaired.
    unmarried: Vec<bool>,
    /// `bMissingFaces`.
    missing_faces: bool,
}

impl PrimitiveCreation {
    /// Extrude the chain of `curves` by `height` along `normal` with a draft of
    /// `draft_angle_deg` (`CreateTaperExtrude`): 0 is along the normal; only the sign
    /// picks the side (positive inward, negative outward) and the in-plane offset is
    /// `|height * tan(|draft_angle_deg|)|`, so 90 to 180 is not the outward taper the
    /// upstream parameter comment promises (`DOC-5`). A negative height puts the taper on
    /// the other side. The curves must form one chain; like the release C++, this does not
    /// check it. `end_caps` is 0 none, 1 at the curves, 2 at the offset end, 3 both.
    /// A zero `normal` is computed from the curves (and written back); a given one
    /// reverses the curves when they wind the other way. `corner` applies to outer
    /// corners. The result is shrunk, stitched and oriented into the target B-rep.
    ///
    /// # Errors
    ///
    /// A null pointer for no target region; `InvalidInput` for a curve that is not
    /// a B-spline; `SM_ERR` when the curves form no composite or the surface
    /// intersection fallback finds no offsets; those of the chaining, merge and
    /// topology steps that upstream does not ignore; `InvalidInput` for a corner
    /// refused under [`OffsetCornerType::CurveExtension`] or
    /// `fixes.taper_tangent_corner_planes` (see
    /// [`Self::create_taper_extrude_with_refusal`]).
    pub fn create_taper_extrude(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        curves: Vec<Curve>,
        height: f64,
        draft_angle_deg: f64,
        end_caps: i32,
        normal: &mut Vec3,
        corner: OffsetCornerType,
    ) -> Result<()> {
        match self.create_taper_extrude_with_refusal(
            a,
            brep,
            curves,
            height,
            draft_angle_deg,
            end_caps,
            normal,
            corner,
        )? {
            None => Ok(()),
            Some(_) => Err(SolidError::invalid_input(OP, "draft corner refused")),
        }
    }

    /// [`Self::create_taper_extrude`] reporting a refused corner: `Ok(Some(refusal))`
    /// is the fork's `SM_ERR_INVALID_INPUT` with `pOptRefusal` and
    /// `pOptRefusedCorner` set (source change, source fork 54148db): with
    /// [`OffsetCornerType::CurveExtension`], or linear-extension corners under
    /// `fixes.taper_tangent_corner_planes` (WR-521), a profile of lines and circular
    /// arcs with a sharp corner gets walls that continue as themselves (or as their
    /// tangent planes) to meet along their exact seams, and a corner they cannot
    /// close is refused, the target left as it was. `CurveExtension` on a profile
    /// without a sharp corner is `LinearExtension`.
    ///
    /// # Errors
    ///
    /// As [`Self::create_taper_extrude`], except a refusal.
    pub fn create_taper_extrude_with_refusal(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        curves: Vec<Curve>,
        height: f64,
        draft_angle_deg: f64,
        end_caps: i32,
        normal: &mut Vec3,
        corner: OffsetCornerType,
    ) -> Result<Option<CornerRefusal>> {
        // init SmPrimitiveCreation memory
        self.init_entity_maps();
        if self.region().is_none() {
            return Err(SolidError::null_pointer(OP));
        }

        // make output temporary for now
        let scratch = a.new_brep(brep)?;
        // ready breps for modification (the result's flag is restored on exit)
        let editing = match a.set_editing(brep, true) {
            Ok(previous) => previous,
            Err(e) => {
                a.delete_brep(scratch);
                return Err(e);
            }
        };
        let result = taper(
            a,
            brep,
            scratch,
            curves,
            height,
            draft_angle_deg,
            end_caps,
            normal,
            corner,
        );
        a.delete_brep(scratch);
        a.set_editing(brep, editing)?;
        result
    }
}

/// `CreateTaperExtrude` once its scratch B-rep exists.
fn taper(
    a: &mut impl PrimitiveCreationAccess,
    brep: BrepId,
    scratch: BrepId,
    curves: Vec<Curve>,
    height: f64,
    draft_angle_deg: f64,
    end_caps: i32,
    normal: &mut Vec3,
    corner: OffsetCornerType,
) -> Result<Option<CornerRefusal>> {
    a.set_editing(scratch, true)?;

    // Local tolerance uses: the B-rep's own tolerance (SmTol::GetZoneTol3d)
    let brep_tol = a.brep(brep)?.tolerance();
    let zone_tol = brep_tol;
    let approx_tol = 0.001 * a.brep(brep)?.tolerance();
    let same_point_tol = 10.0 * approx_tol;
    a.set_brep_tolerance(scratch, brep_tol)?;

    // Enforce head-tail order for input curves
    let mut input = fix_chain_curves_list(
        a.kernel(brep)?,
        curves.into_iter().map(Some).collect(),
        2.0 * zone_tol,
        true,
    )?
    .curves;

    if normal.length_squared() < 0.00001 {
        // compute the normal from curve sample points (zeroed before it can fail)
        *normal = ignored(compute_crvs_normal(a.kernel(brep)?, &input))?.unwrap_or(Vec3::ZERO);
        // snap curves to Z axis when close
        if normal.z > 0.98 {
            normal.x = 0.0;
            normal.y = 0.0;
            normal.z = 1.0;
        }
        if normal.z < -0.98 {
            normal.x = 0.0;
            normal.y = 0.0;
            normal.z = -1.0;
        }
    } else {
        // check that the curves order conforms to the specified input normal
        let computed = ignored(compute_crvs_normal(a.kernel(brep)?, &input))?.unwrap_or(Vec3::ZERO);
        let angle = ignored(computed.angle_between(*normal))?.unwrap_or(0.0);
        if (angle - PI).abs() < 0.0001 {
            // reverse all the curves, then rechain them
            for curve in &mut input {
                let interval = curve.natural_interval();
                ignore_status(curve.reverse_parameterization(a.kernel(brep)?, &interval))?;
            }
            input = fix_chain_curves_list(
                a.kernel(brep)?,
                input.into_iter().map(Some).collect(),
                2.0 * zone_tol,
                true,
            )?
            .curves;
        }
    }

    // build translation to generate the offset curves
    let mut movement = Axis2Placement::IDENTITY;
    let translation = height * *normal;
    movement.translate(translation);

    // select Left or Right hand offset
    let direction = if draft_angle_deg < 0.0 {
        OffsetDirection::RightHandSide
    } else {
        OffsetDirection::LeftHandSide
    };

    // This is the offset distance in the plane of the input curves
    let mut offset = height * (draft_angle_deg.abs() * PI / 180.0).tan();
    if offset < 0.0 {
        offset = -offset;
    }

    // Break up all input curves at G1 discontinuities into local copies
    let mut split = Vec::new();
    for curve in &input {
        let bspline = as_bspline(curve)
            .ok_or_else(|| SolidError::invalid_input(OP, "taper curve is not a B-spline"))?;
        split.extend(
            ignored(bspline.subdivide_at_discontinuity(
                a.kernel(brep)?,
                ContinuityType::G1,
                ANG_TOL_DEG,
            ))?
            .unwrap_or_default(),
        );
    }

    // connect the split curves into composite curves
    let built = ignored(CompositeCurve::build_composites_from_curves(
        a.kernel(brep)?,
        split.iter().map(|c| Curve::from(c.clone())).collect(),
        same_point_tol,
        0.0,
    ))?
    .ok_or_else(|| SolidError::sm_err(OP))?;
    // expect one composite curve
    let mut composite = built
        .composites
        .into_iter()
        .next()
        .ok_or_else(|| SolidError::sm_err(OP))?;
    let mut generators = Vec::with_capacity(composite.segments().len());
    for segment in composite.segments() {
        generators.push(
            as_bspline(&segment.curve)
                .ok_or_else(|| SolidError::invalid_input(OP, "segment is not a B-spline"))?,
        );
    }
    let link = |slot: usize| {
        split
            .get(slot)
            .and_then(|curve| generators.iter().position(|g| same_curve(curve, g)))
    };
    let links = [link(0), link(1)];
    let unmarried = vec![true; split.len()];
    let mut state = TaperState {
        split,
        links,
        generators,
        unmarried,
        missing_faces: false,
    };

    // source change (source fork 54148db, e8f23e0): walls that keep their
    // curvature at the corners. Without a sharp corner the three corner rules give the
    // same walls, which the offset method builds. The tangent rule too, for lines and
    // arcs (opt-in `fixes.taper_tangent_corner_planes`, WR-521): their walls continue
    // exactly as their tangent planes. Other curves keep the offset method.
    let mut corner = corner;
    let tangent_rule = a.kernel(brep)?.fixes.taper_tangent_corner_planes
        && corner == OffsetCornerType::LinearExtension;
    let has_corner = (corner == OffsetCornerType::CurveExtension || tangent_rule)
        && has_corner(a.kernel(brep)?, &composite, *normal);
    let mut exact = false;
    if has_corner && tangent_rule {
        exact = lines_and_arcs(a.kernel(brep)?, &composite, &input, brep_tol)?;
    }
    let keep_curvature = has_corner && corner == OffsetCornerType::CurveExtension;
    let tangent_planes = exact && corner == OffsetCornerType::LinearExtension;
    if corner == OffsetCornerType::CurveExtension {
        corner = OffsetCornerType::LinearExtension;
    }

    let mut empty_offsets = false;
    let mut trim_curves: Vec<Curve> = Vec::new();

    if keep_curvature || tangent_planes {
        let walls = curvature_keeping_walls(
            a,
            brep,
            scratch,
            &WallsInput {
                tangent: tangent_planes,
                composite: &composite,
                inputs: &input,
                normal: *normal,
                height,
                offset,
                side: direction,
                tol: brep_tol,
            },
        )?;
        match walls {
            Err(refusal) => return Ok(Some(refusal)),
            Ok(far_curves) => trim_curves = far_curves,
        }
        a.stitch_faces(scratch, brep_tol)?;
        let faces = a.faces(scratch)?;
        a.copy_faces_into(scratch, &faces, brep)?;
    } else {
        // offset the composite curve - extend convex joints and intersect concave ones
        let params = OffsetParams {
            xsect_tol_3d: 2.0 * zone_tol,
            approx_tol_3d: approx_tol,
            plane_normal: *normal,
            corner,
            trim_results: true,
            offset_distance: offset,
            match_parameterization: false,
        };
        let trimmed =
            ignored(composite.create_trimmed_offsets(a.kernel(brep)?, direction, &params))?
                .unwrap_or_default();

        let rule_options = |surface_intersection| OffsetRuleOptions {
            tolerance: zone_tol,
            same_point_tolerance: same_point_tol,
            offset,
            movement,
            surface_intersection,
        };

        if trimmed.is_empty() {
            state.missing_faces = true;
            empty_offsets = true;
        } else {
            // reorder the trimmed offset curves back into a chain; their maps follow them
            let (curves, mut maps): (Vec<_>, Vec<_>) = trimmed
                .into_iter()
                .map(|piece| (Some(piece.curve), piece.attribute))
                .unzip();
            let chained = fix_chain_curves_list(a.kernel(brep)?, curves, 2.0 * zone_tol, true)?;
            let mut chain_maps: Vec<Option<OffsetMapAttribute>> = chained
                .origin
                .iter()
                .map(|&i| maps.get_mut(i).and_then(Option::take))
                .collect();

            // move end points to close small gaps in the head to toe curve sequence
            let fixed = crvs_fix_gaps(a.kernel(brep)?, chained.curves, 2.0 * zone_tol)?;
            let fixed_maps: Vec<Option<OffsetMapAttribute>> = fixed
                .chained
                .origin
                .iter()
                .map(|&i| chain_maps.get_mut(i).and_then(Option::take))
                .collect();
            trim_curves = fixed.chained.curves;

            // for every offset curve - find its original partner and build a ruled surface
            for (curve, map) in trim_curves.iter_mut().zip(fixed_maps) {
                let mut offset_curve = as_bspline(curve)
                    .ok_or_else(|| SolidError::invalid_input(OP, "offset is not a B-spline"))?;
                let result = ruled(
                    a.kernel(brep)?,
                    &mut offset_curve,
                    map,
                    &mut state,
                    rule_options(false),
                );
                *curve = Curve::from(offset_curve);
                match result {
                    Err(e) if e.is_unported() => return Err(e),
                    // no ruled surface: skip this offset curve
                    Err(_) => {}
                    Ok((surface, _)) => {
                        // create a face from the ruled surface in the scratch B-rep
                        let surface = Surface::from(surface);
                        // (a B-spline surface just made: never lazy)
                        let domain = surface.natural_uv_domain_if_built();
                        let mut face = None;
                        ignore_status(a.surface_face(scratch, surface, domain, &mut face))?;
                    }
                }
            }

            // unmarried curves exist when the offset curve was completely trimmed away
            if !state.missing_faces {
                state.missing_faces = state.unmarried.iter().any(|&unpaired| unpaired);
            }

            // Stitch the potential result to get the topology
            ignore_status(a.stitch_faces(scratch, brep_tol))?;

            // Each face should be lamina along input curve and offset curve, manifold otherwise
            for face in a.faces(scratch)? {
                if state.missing_faces {
                    break;
                }
                state.missing_faces = a.has_consecutive_lamina_edges((scratch, face))?;
            }
        }

        if state.missing_faces {
            // redo with the surface intersection method
            let surfaces = a.new_brep(brep)?;
            let intersections = a.new_brep(brep)?;
            let result = surface_intersection_method(
                a,
                brep,
                [surfaces, intersections],
                &mut composite,
                &mut state,
                direction,
                params,
                &rule_options(true),
                brep_tol,
                same_point_tol,
            );
            a.delete_brep(surfaces);
            result?;
        } else {
            let faces = a.faces(scratch)?;
            ignore_status(a.copy_faces_into(scratch, &faces, brep))?;
        }
    } // end the offset method

    // Optional end caps: cap construction fails for curves that are not planar and closed
    let region = (brep, a.infinite_region(brep)?);
    if end_caps == 1 || end_caps == 3 {
        crvs_cleanup(a.kernel(brep)?, &mut input, brep_tol)?;
        let mut face = None;
        match a.planar_face(region, input, brep_tol, &mut face) {
            Err(e) if e.is_unported() => return Err(e),
            _ => {}
        }
    }
    if (end_caps == 2 || end_caps == 3) && !empty_offsets {
        // order the offsets into one or more head-tail chains (there may be two loops)
        let chained = fix_chain_curves_list(
            a.kernel(brep)?,
            trim_curves.into_iter().map(Some).collect(),
            2.0 * zone_tol,
            true,
        )?;
        let mut rest = chained.curves;
        let mut loops = Vec::new();
        for &start in chained.chain_starts.iter().rev() {
            loops.push(rest.split_off(start.min(rest.len())));
        }
        for mut chain in loops.into_iter().rev() {
            // remove short curves and close resulting gaps from chain list
            crvs_cleanup(a.kernel(brep)?, &mut chain, brep_tol)?;
            let mut face = None;
            match a.planar_face(region, chain, brep_tol, &mut face) {
                Err(e) if e.is_unported() => return Err(e),
                _ => {}
            }
        }
    }

    // clean up the Brep
    ignore_status(a.shrink_geometry(brep))?;
    ignore_status(a.stitch_and_orient(brep))?;
    Ok(None)
}

/// The missing-face fallback: ruled surfaces of the untrimmed offsets in
/// `surfaces`, piecewise merged into `intersections`, the faces not touching their
/// generating curve's ends removed, and the rest united with the result.
fn surface_intersection_method(
    a: &mut impl PrimitiveCreationAccess,
    brep: BrepId,
    [surfaces, intersections]: [BrepId; 2],
    composite: &mut CompositeCurve,
    state: &mut TaperState,
    direction: OffsetDirection,
    params: OffsetParams,
    rule_options: &OffsetRuleOptions,
    brep_tol: f64,
    same_point_tol: f64,
) -> Result<()> {
    // Prepare breps for appropriate editing
    a.set_editing(surfaces, true)?;
    a.set_editing(intersections, true)?;
    a.set_brep_tolerance(surfaces, brep_tol)?;
    a.set_brep_tolerance(intersections, brep_tol)?;

    // offset the composite curve without trimming
    sync_generators(composite, &state.generators);
    let untrimmed = ignored(composite.create_trimmed_offsets(
        a.kernel(brep)?,
        direction,
        &OffsetParams {
            trim_results: false,
            ..params
        },
    ))?
    .unwrap_or_default();
    if untrimmed.is_empty() {
        return Err(SolidError::sm_err(OP));
    }

    // for every offset curve - find its original partner and build a ruled surface
    for piece in untrimmed {
        let mut offset_curve = as_bspline(&piece.curve)
            .ok_or_else(|| SolidError::invalid_input(OP, "offset is not a B-spline"))?;
        match ruled(
            a.kernel(brep)?,
            &mut offset_curve,
            piece.attribute,
            state,
            *rule_options,
        ) {
            Err(e) if e.is_unported() => return Err(e),
            // no ruled surface: skip this offset curve
            Err(_) => {}
            Ok((surface, map)) => {
                let surface = Surface::from(surface);
                // (a B-spline surface just made: never lazy)
                let domain = surface.natural_uv_domain_if_built();
                let mut face = None;
                ignore_status(a.surface_face(surfaces, surface, domain, &mut face))?;
                // the ruled surface carries its offset map
                if let (Some(face), Some(map)) = (face, map) {
                    a.attach_offset_map((surfaces, face), map)?;
                }
            }
        }
    }

    // Piecewise Merge each face into the Brep
    let intersections =
        a.with_merge(brep, intersections, surfaces, brep_tol, 0.0, |a, merge| {
            a.piecewise_merge(merge, true, true)
        })?;

    // Iterate over Faces to remove extraneous faces
    for face in a.faces(intersections)? {
        // the map attribute of the curve that generated the face
        let map = a
            .offset_map((intersections, face))?
            .ok_or_else(|| SolidError::null_pointer(OP))?;
        let index = usize::try_from(map.curve).map_err(|_| SolidError::null_pointer(OP))?;
        let corner_offset = map.corner_offset;

        // Parent curve that generated this surface
        let ends = state
            .generators
            .get(index)
            .ok_or_else(|| SolidError::null_pointer(OP))?
            .ends()?;

        // Compare each face vertex with the parent curve ends
        let mut connect_start = false;
        let mut connect_end = false;
        for vertex in a.face_vertices((intersections, face))? {
            if connect_start && connect_end {
                break;
            }
            let point = a.brep(intersections)?.vertex(vertex)?.point;
            if ends.start.distance(point) < same_point_tol {
                connect_start = true;
            }
            if ends.end.distance(point) < same_point_tol {
                connect_end = true;
            }
        }

        // A side generated from a corner connects to one vertex, from a curve to both
        if !(corner_offset && (connect_start || connect_end))
            && !(!corner_offset && (connect_start && connect_end))
        {
            ignore_status(a.delete_face_and_free_topology((intersections, face)))?;
        }
    }

    // Boolean new extruded face to the original Brep
    a.with_merge(brep, brep, intersections, brep_tol, 0.0, |a, merge| {
        ignore_status(a.union(merge))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_curves_are_found_among_the_segments_by_interval_and_ends() -> Result<()> {
        let a = BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::X)?;
        let b = BSplineCurve::create_line_segment(3, Vec3::X, Vec3::X + Vec3::Y)?;
        let reversed = BSplineCurve::create_line_segment(3, Vec3::X, Vec3::ZERO)?;
        assert!(same_curve(&a, &a.clone()));
        assert!(!same_curve(&a, &b));
        assert!(!same_curve(&a, &reversed));

        // the composite holds the segments in its own order; the link reads the
        // segment's current state, else the split curve's
        let split = vec![a.clone(), b.clone()];
        let mut generators = vec![b.clone(), a.clone()];
        let links = [Some(1), Some(0)];
        let before = current_curve(&split, links, &generators, 0)?;
        assert_eq!(before.map(|(i, _)| (i.min(), i.max())), Some((0.0, 1.0)));
        let mut edited = BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::X)?;
        edited.edit_parameterization(&Extent1d::from_min_max(2.0, 5.0))?;
        if let Some(target) = generators.get_mut(1) {
            *target = edited;
        }
        let after = current_curve(&split, links, &generators, 0)?;
        assert_eq!(after.map(|(i, _)| (i.min(), i.max())), Some((2.0, 5.0)));
        assert!(current_curve(&split, [None, None], &generators, 2)?.is_none());
        Ok(())
    }
}
