//! Curve-sweep, swung, skinned and blended primitive orchestration.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`,
//! `source/SMLib/src/SmBSplineSurfaceNLib.cpp` (`CreateSwungSurface`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_curve_sweep`] | `CreateCurveSweep` |
//! | [`PrimitiveCreation::create_curve_sweep_with_placements`], `profile_motions` (private) | `CreateCurveSweep` with the source fork's `pOptPlacements` |
//! | [`PrimitiveCreation::create_swung_primitive`] | `CreateSwungPrimitive` |
//! | [`PrimitiveCreation::create_skin_from_faces`] | `CreateSkinFromFaces` |
//! | [`PrimitiveCreation::create_blend_primitive`] | `CreateBlendPrimitive` |
//! | `swung_curve_scale` (private) | `CreateSwungPrimitive` scale initialization |
//! | `swung_surface` (private) | `CreateSwungSurface` virtual NURB acquisition |
//!
//! Deviations: typed service resolution replaces pointers. Output face lists
//! preserve partial results; owned geometry handles cleanup. Missing analytic
//! NURBs are generated on owned copies with caller context/owner. No recursion.
//! Not upstream (WR-404, source issue #411, opt-in
//! `Kernel::fixes.curve_sweep_caps_at_sweep_ends`): the curve sweep's caps
//! come from the side faces at the sweep's ends (`sweep_end_curves`), not the first
//! and last `count` side faces.
//! Not upstream (the source fork's source fork 5d3939e, WR-523, #1450):
//! [`PrimitiveCreation::create_curve_sweep_with_placements`] reports the rigid motions
//! carrying the profile onto the first, middle and last sections.

use super::{
    geometry::{prepare_curve_sweep, GeometryHandle, PreparedCurveSweep},
    PrimitiveAccess,
};
use crate::curve::composite_curve::CompositeCurve;
use crate::curve::curve::p2::CurvePropertyType;
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::math::{Vec3, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::b_spline_surface::p1::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::SkinnedSurfaceOptions;
use crate::nurbs::knots::Side;
use crate::primitive::primitive_creation::{p1::PrimitiveCreation, SweepOptions};
use crate::surface::surface::{
    p2::{BlendEndType, CurvatureType, DerivSurfDefinition},
    Surface,
};
use crate::sweep::topology_sweep::access::{FaceRef, RegionRef};
use crate::topology::{BrepId, EdgeuseId, FaceId};

const OP: &str = "primitive::primitive_creation::p3";

pub(super) fn target(primitive: &PrimitiveCreation, brep: BrepId) -> Result<RegionRef> {
    Ok((
        brep,
        primitive
            .region()
            .ok_or_else(|| SolidError::null_pointer(OP))?,
    ))
}
fn face_surface(a: &impl PrimitiveAccess, face: FaceRef) -> Result<&Surface> {
    crate::topology::face::p2::surface(a.brep(face.0)?, face.1)?
        .ok_or_else(|| SolidError::null_pointer(OP))
}

/// Parameters for `CreateBlendPrimitive`.
#[derive(Debug, Clone, Copy)]
pub struct BlendPrimitiveOptions {
    /// Requested blend continuity.
    pub curvature: CurvatureType,
    /// Smooth or cusped ends.
    pub ends: BlendEndType,
    /// 0 guesses directions; 1 matches starts; 2 matches start to end.
    pub curves_direction: u64,
    /// Starting takeoff length.
    pub start_tangent_length: f64,
    /// Ending takeoff length (divided by the start, including zero, upstream).
    pub end_tangent_length: f64,
}

// Keep the derived curve dispatch here: the upstream SM_CAST_PTR checks the
// inheritance chain without slicing SmLine/conics to their cached NURB base.
fn swung_curve_scale(
    kernel: &crate::kernel::Kernel,
    path: &Curve,
    profile: &Curve,
    approx_tol: f64,
) -> Result<f64> {
    let mut point = profile.evaluate_point(kernel, profile.natural_interval().min())?;
    let normal = Vec3::Y;
    let d = -point.dot(normal);
    let property = path.global_property_analysis(
        kernel,
        &profile.natural_interval(),
        CurvePropertyType::PlaneIntersection,
        approx_tol,
        &PropertyAnalysisOptions {
            property_value: Some(d),
            opt_vectors: &[normal],
            handle: Some(GeometryHandle(path.natural_interval())),
            ..Default::default()
        },
    )?;
    if let Some(solution) = property.solutions.get(0) {
        point = path.evaluate_point(kernel, solution.start.param(0)?)?;
    }
    Ok((1.0 / point.x).abs())
}

// CreateSwungSurface queries GetOrCreateGwNurbPointer on each original
// derived curve. Copying a lazy SmLine's empty base would lose its MakeNurb.
fn swung_nurb(kernel: &crate::kernel::Kernel, curve: &Curve) -> Result<BSplineCurve> {
    let base = super::geometry::bspline(curve)
        .ok_or_else(|| SolidError::invalid_input(OP, "non B-spline swung input"))?;
    if base.gw_nurb().is_some() {
        return Ok(base);
    }
    // GetOrCreateGwNurbPointer ignores MakeNurb's ordinary status. Keep the
    // caller's owner/context for analytic generation and propagate Unported.
    match curve.kind() {
        CurveKind::Line(line) => {
            let mut line = line.clone();
            ignore_status(line.make_nurb_owned(curve.owner()))?;
            Ok(line.base().clone())
        }
        CurveKind::Ellipse(ellipse) => {
            let mut ellipse = ellipse.clone();
            ignore_status(ellipse.make_nurb_owned_with_edge_tol(
                kernel,
                curve.owner(),
                curve.owner_zone_tol_3d(),
            ))?;
            Ok(ellipse.base().clone())
        }
        CurveKind::Parabola(parabola) => {
            let mut parabola = parabola.clone();
            ignore_status(parabola.make_nurb(curve.owner()))?;
            Ok(parabola.base().clone())
        }
        CurveKind::Hyperbola(hyperbola) => {
            let mut hyperbola = hyperbola.clone();
            ignore_status(hyperbola.make_nurb(curve.owner()))?;
            Ok(hyperbola.base().clone())
        }
        _ => Ok(base),
    }
}

fn swung_surface(
    kernel: &crate::kernel::Kernel,
    path: &Curve,
    profile: &Curve,
    scale: f64,
) -> Result<BSplineSurface> {
    // The native builder obtains section data before trajectory data.
    let profile = swung_nurb(kernel, profile)?;
    let path = swung_nurb(kernel, path)?;
    BSplineSurface::create_swung_surface(&profile, &path, scale)
}

impl PrimitiveCreation {
    /// Build swung surfaces for every B-spline-compatible path/profile pair.
    /// Only pair (0,0) initializes the scale, even if earlier slots were skipped.
    pub fn create_swung_primitive(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        paths: &[Option<Curve>],
        profiles: &[Option<Curve>],
        scale: f64,
        approx_tol: f64,
    ) -> Result<()> {
        self.init_entity_maps();
        let _ = target(self, brep)?;
        let mut curve_scale = 0.0;
        let mut faces = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            let Some(path) = path.as_ref() else {
                continue;
            };
            if !path.is_kind_of(BSplineCurve::TYPE) {
                continue;
            }
            for (j, profile) in profiles.iter().enumerate() {
                let Some(profile) = profile.as_ref() else {
                    continue;
                };
                if !profile.is_kind_of(BSplineCurve::TYPE) {
                    continue;
                }
                if i == 0 && j == 0 {
                    curve_scale = swung_curve_scale(a.kernel(brep)?, path, profile, approx_tol)?;
                }
                let surface = swung_surface(a.kernel(brep)?, path, profile, scale * curve_scale)?;
                let surface = Surface::from(surface);
                // (a B-spline surface just made: never lazy)
                let domain = surface.natural_uv_domain_if_built();
                a.surface_faces(brep, surface, domain, &mut faces)?;
            }
        }
        Ok(())
    }

    /// Loft matching loops of faces, or the supplied sequence of profile curves.
    /// `new_faces` is reset before any failing call, as upstream.
    pub fn create_skin_from_faces(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        faces: &[FaceRef],
        profiles: &[BSplineCurve],
        approx_tol: f64,
        degree: usize,
        new_faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        self.init_entity_maps();
        new_faces.clear();
        let _ = target(self, brep)?;
        let mut skin_curves = Vec::new();
        let mut layers = Vec::new();
        let mut layer_count = 0;
        for (i, &face) in faces.iter().enumerate() {
            let domain = crate::topology::face::p2::uv_domain(a.brep(face.0)?, face.1)?;
            face_surface(a, face)?.evaluate_normal(
                a.kernel(brep)?,
                domain.evaluate(0.5, 0.5),
                Side::Left,
                Side::Left,
            )?;
            let composites = a.face_composites(face)?;
            if i == 0 {
                layer_count = composites.len();
            } else if layer_count != composites.len() {
                return Err(SolidError::sm_err(OP));
            }
            for (j, mut composite) in composites.into_iter().enumerate() {
                skin_curves.push(composite.make_composite_nurb()?);
                layers.push(j);
            }
        }
        if faces.is_empty() {
            skin_curves.extend_from_slice(profiles);
            layer_count = 1;
            layers.resize(skin_curves.len(), 0);
        }
        let mut made = Vec::new();
        for layer in 0..layer_count {
            let curves: Vec<_> = skin_curves
                .iter()
                .zip(&layers)
                .filter(|(_, l)| **l == layer)
                .map(|(c, _)| c.clone())
                .collect();
            let surface = BSplineSurface::create_skinned_surface(
                a.kernel(brep)?,
                &curves,
                SkinnedSurfaceOptions {
                    approx_tol,
                    degree,
                    deriv_surfs: Some([None, None]),
                    ..Default::default()
                },
            )?;
            let surface = Surface::from(surface);
            // (a B-spline surface just made: never lazy)
            let domain = surface.natural_uv_domain_if_built();
            ignore_status(a.surface_faces(brep, surface, domain, &mut made))?;
            new_faces.extend_from_slice(&made);
        }
        Ok(())
    }

    /// Construct a blend and add its face, preserving the ignored face status.
    pub fn create_blend_primitive(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        first: Option<EdgeuseId>,
        second: Option<EdgeuseId>,
        output: &mut Option<FaceId>,
        options: BlendPrimitiveOptions,
    ) -> Result<()> {
        self.init_entity_maps();
        let _ = target(self, brep)?;
        let flag = match options.curvature {
            CurvatureType::NoCurvature => 1,
            CurvatureType::G3FromSurface => 3,
            _ => 2,
        };
        let mut derivatives = DerivSurfDefinition {
            curvature_type: options.curvature,
            blend_end_type: options.ends,
            start_tan_length: options.start_tangent_length,
            end_tan_scale: options.end_tangent_length / options.start_tangent_length,
            ..Default::default()
        };
        let surface = a.with_primitive_brep(brep, |kernel, model| {
            crate::topology::brep::p1::create_edge_edge_blend(
                kernel,
                flag,
                model,
                first,
                None, // Both edge uses belong to this primitive's B-rep.
                second,
                options.curves_direction,
                Some(&mut derivatives),
            )
        })?;
        a.set_editing(brep, true)?;
        // (a B-spline surface just made: never lazy)
        let domain = surface.natural_uv_domain_if_built();
        ignore_status(a.surface_face(brep, surface, domain, output))?;
        Ok(())
    }

    /// Sweep copies of the profiles along the path, optionally scaling and capping.
    /// Only `side_faces` is reset; cap output slots retain their upstream lifetime.
    /// The scale reference retains its derived curve dispatch; path and scale use
    /// B-spline copies, as in the upstream constructor.
    pub fn create_curve_sweep(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        profiles: &[Option<Curve>],
        path: &BSplineCurve,
        scale_reference: Option<&Curve>,
        scale_curve: Option<&BSplineCurve>,
        approx_tol: f64,
        options: SweepOptions,
        test_continuity: bool,
        start_faces: &mut Vec<FaceId>,
        side_faces: &mut Vec<FaceId>,
        end_faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        self.curve_sweep(
            a,
            brep,
            profiles,
            path,
            scale_reference,
            scale_curve,
            approx_tol,
            options,
            test_continuity,
            start_faces,
            side_faces,
            end_faces,
            None,
        )
    }

    /// [`create_curve_sweep`](Self::create_curve_sweep) that also reports where it
    /// placed the profile (the source fork's `pOptPlacements`, source fork
    /// 5d3939e, WR-523, #1450): the rigid motions carrying the profile curves as given
    /// onto the sweep's first, middle and last sections (of the first composite; all
    /// share the path and its start frame). The B-rep and faces are the same as
    /// [`create_curve_sweep`](Self::create_curve_sweep)'s.
    ///
    /// # Errors
    ///
    /// `InvalidInput` with a scale curve (its sections are no rigid images of the
    /// profile); otherwise those of [`create_curve_sweep`](Self::create_curve_sweep).
    pub fn create_curve_sweep_with_placements(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        profiles: &[Option<Curve>],
        path: &BSplineCurve,
        scale_reference: Option<&Curve>,
        scale_curve: Option<&BSplineCurve>,
        approx_tol: f64,
        options: SweepOptions,
        test_continuity: bool,
        start_faces: &mut Vec<FaceId>,
        side_faces: &mut Vec<FaceId>,
        end_faces: &mut Vec<FaceId>,
    ) -> Result<[Axis2Placement; 3]> {
        let mut placements = [Axis2Placement::IDENTITY; 3];
        self.curve_sweep(
            a,
            brep,
            profiles,
            path,
            scale_reference,
            scale_curve,
            approx_tol,
            options,
            test_continuity,
            start_faces,
            side_faces,
            end_faces,
            Some(&mut placements),
        )?;
        Ok(placements)
    }

    /// `CreateCurveSweep` with the fork's optional `pOptPlacements` output.
    fn curve_sweep(
        &mut self,
        a: &mut impl PrimitiveAccess,
        brep: BrepId,
        profiles: &[Option<Curve>],
        path: &BSplineCurve,
        scale_reference: Option<&Curve>,
        scale_curve: Option<&BSplineCurve>,
        approx_tol: f64,
        options: SweepOptions,
        test_continuity: bool,
        start_faces: &mut Vec<FaceId>,
        side_faces: &mut Vec<FaceId>,
        end_faces: &mut Vec<FaceId>,
        mut opt_placements: Option<&mut [Axis2Placement; 3]>,
    ) -> Result<()> {
        self.init_entity_maps();
        side_faces.clear();
        if profiles.is_empty() {
            return Err(SolidError::sm_err(OP));
        }
        // source fork (5d3939e): a scaled sweep's sections are no rigid images of the
        // profile.
        if opt_placements.is_some() && scale_curve.is_some() {
            return Err(SolidError::invalid_input(
                OP,
                "no placements of a scaled sweep",
            ));
        }
        let region = target(self, brep)?;
        let previous = a.kernel(brep)?.context.doing_boolean();
        a.kernel_mut(brep)?.context.set_doing_boolean(true);
        let result = (|| {
            let mut prepared = prepare_curve_sweep(
                a.kernel(brep)?,
                profiles,
                path,
                scale_reference,
                scale_curve,
                options,
                test_continuity,
            )?;
            let count = prepared.profiles.len();
            let curves = core::mem::take(&mut prepared.profiles)
                .into_iter()
                .map(Curve::from)
                .collect();
            let composites = CompositeCurve::build_composites_from_curves(
                a.kernel(brep)?,
                curves,
                a.brep(brep)?.tolerance(),
                0.0,
            )?;
            // source fork (5d3939e): where the sweep places the profile, in the space
            // it sweeps in (the same for every composite: they share the path and its
            // start frame).
            let mut placed = [Axis2Placement::IDENTITY; 3];
            // each composite's side faces (WR-404's caps)
            let mut groups = Vec::new();
            for (index, mut composite) in composites.composites.into_iter().enumerate() {
                let profile = composite.make_composite_nurb()?;
                let surface = if index == 0 && opt_placements.is_some() {
                    let (surface, at) = BSplineSurface::create_sweep_surface_with_placements(
                        &profile,
                        &prepared.path,
                        prepared.scale.as_ref(),
                        options.translational_sweep,
                        approx_tol,
                    )?;
                    placed = at;
                    surface
                } else {
                    BSplineSurface::create_sweep_surface(
                        &profile,
                        &prepared.path,
                        prepared.scale.as_ref(),
                        options.translational_sweep,
                        approx_tol,
                    )?
                };
                let surface = Surface::from(surface);
                // (a B-spline surface just made: never lazy)
                let domain = surface.natural_uv_domain_if_built();
                let mut made = Vec::new();
                ignore_status(a.surface_faces(brep, surface, domain, &mut made))?;
                let first = side_faces.len();
                side_faces.extend(made);
                groups.push(first..side_faces.len());
            }
            ignore_status(a.transform_primitive(brep, &prepared.rotation_back))?;
            let placement = if options.move_to_profile {
                &prepared.profile_back
            } else {
                &prepared.path_back
            };
            ignore_status(a.transform_primitive(brep, placement))?;
            // source fork (5d3939e): the motions carrying the profile as given onto
            // its first, middle and last sections. A profile point went forward into the
            // sweep's space (to the origin, the path turned onto x, oriented with the path
            // when asked), the sweep placed it, and the result came back: each motion is
            // that chain, applied to the global system.
            if let Some(out) = opt_placements.as_deref_mut() {
                *out = profile_motions(&prepared, placement, &placed);
            }
            if options.cap_ends && a.kernel(brep)?.fixes.curve_sweep_caps_at_sweep_ends {
                // not upstream (WR-404, #411, opt-in)
                let (mut starts, mut ends) = sweep_end_curves(
                    &*a,
                    brep,
                    side_faces,
                    &groups,
                    options.translational_sweep,
                    approx_tol,
                )?;
                a.planar_faces(region, &mut starts, approx_tol, start_faces)?;
                a.planar_faces(region, &mut ends, approx_tol, end_faces)?;
            } else if options.cap_ends {
                let mut starts = Vec::new();
                let mut ends = Vec::new();
                let mut direction = SurfParamType::U;
                let total = side_faces.len();
                for (i, &face) in side_faces.iter().enumerate() {
                    let surface = face_surface(a, (brep, face))?;
                    let domain = surface.natural_uv_domain_with_nurb(a.kernel(brep)?)?;
                    if i < count {
                        direction = if options.translational_sweep {
                            SurfParamType::V
                        } else {
                            SurfParamType::U
                        };
                        let param = if options.translational_sweep {
                            domain.min().y
                        } else {
                            domain.min().x
                        };
                        let curve = surface
                            .create_iso_parametric_curve(
                                a.kernel(brep)?,
                                direction,
                                param,
                                approx_tol / 10.0,
                                None,
                                false,
                            )?
                            .curve;
                        starts.push(Some(curve));
                    }
                    if count > total || i >= total - count {
                        let param = if options.translational_sweep {
                            direction = SurfParamType::V;
                            domain.max().y
                        } else {
                            domain.max().x
                        };
                        let curve = surface
                            .create_iso_parametric_curve(
                                a.kernel(brep)?,
                                direction,
                                param,
                                approx_tol / 10.0,
                                None,
                                false,
                            )?
                            .curve;
                        ends.push(Some(curve));
                    }
                }
                a.planar_faces(region, &mut starts, approx_tol, start_faces)?;
                a.planar_faces(region, &mut ends, approx_tol, end_faces)?;
            }
            ignore_status(a.stitch_and_orient(brep))?;
            Ok(())
        })();
        a.kernel_mut(brep)?.context.set_doing_boolean(previous);
        result
    }
}

/// The rigid motions carrying the profile as given onto the sweep's sections placed
/// on `placed` (in the sweep's space), with `back_move` the move back from the path
/// (the source fork's `pOptPlacements` of `CreateCurveSweep`, source fork
/// 5d3939e): the global system carried forward (profile to the origin, path turned onto
/// x, oriented when asked), placed, and carried back.
fn profile_motions(
    prepared: &PreparedCurveSweep,
    back_move: &Axis2Placement,
    placed: &[Axis2Placement; 3],
) -> [Axis2Placement; 3] {
    let forward = |q: Vec3| {
        let a = prepared.profile_to_origin.transform_point(q);
        let b = prepared.rotation.transform_point(a);
        match &prepared.orientation {
            Some(orientation) => orientation.transform_point(b),
            None => b,
        }
    };
    let back = |q: Vec3| {
        let a = prepared.rotation_back.transform_point(q);
        back_move.transform_point(a)
    };
    let turned = |at: &Axis2Placement, v: Vec3| {
        let a = prepared.rotation.transform_vector(v);
        let b = match &prepared.orientation {
            Some(orientation) => orientation.transform_vector(a),
            None => a,
        };
        let c = at.transform_vector(b);
        let d = prepared.rotation_back.transform_vector(c);
        back_move.transform_vector(d)
    };
    placed.map(|at| {
        let origin = at.transform_point(forward(Vec3::ZERO));
        Axis2Placement::new(back(origin), turned(&at, Vec3::X), turned(&at, Vec3::Y))
    })
}

/// The 3D curves of one cap's loops, as `planar_faces` takes them.
type CapCurves = Vec<Option<Curve>>;

/// The cap curves of a curve sweep (not upstream, WR-404, #411): the iso curves at the
/// sweep's start and end of each composite's side faces that reach them. A path that
/// is not G1 splits every swept surface into one face per piece, so upstream's first
/// and last `count` side faces are not the faces at the ends.
fn sweep_end_curves(
    a: &impl PrimitiveAccess,
    brep: BrepId,
    side_faces: &[FaceId],
    groups: &[core::ops::Range<usize>],
    translational: bool,
    approx_tol: f64,
) -> Result<(CapCurves, CapCurves)> {
    // the sweep runs along v for a translational sweep, else along u
    let direction = if translational {
        SurfParamType::V
    } else {
        SurfParamType::U
    };
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for group in groups {
        let mut spans = Vec::new();
        for &face in side_faces.get(group.clone()).unwrap_or_default() {
            let domain =
                face_surface(a, (brep, face))?.natural_uv_domain_with_nurb(a.kernel(brep)?)?;
            let span = if translational {
                (domain.min().y, domain.max().y)
            } else {
                (domain.min().x, domain.max().x)
            };
            spans.push((face, span));
        }
        let first = spans.iter().map(|(_, s)| s.0).fold(f64::INFINITY, f64::min);
        let last = spans
            .iter()
            .map(|(_, s)| s.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let tol = EFF_ZERO_SQRT * (1.0 + first.abs().max(last.abs()));
        for &(face, (low, high)) in &spans {
            let surface = face_surface(a, (brep, face))?;
            for (at, keep, out) in [
                (low, low - first <= tol, &mut starts),
                (high, last - high <= tol, &mut ends),
            ] {
                if keep {
                    let curve = surface
                        .create_iso_parametric_curve(
                            a.kernel(brep)?,
                            direction,
                            at,
                            approx_tol / 10.0,
                            None,
                            false,
                        )?
                        .curve;
                    out.push(Some(curve));
                }
            }
        }
    }
    Ok((starts, ends))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::geom::extent1d::Extent1d;
    use crate::kernel::Kernel;

    // An imported line may have last-bit differences between its analytic data
    // and cached NURB. Virtual evaluations must continue to use the analytic data.
    fn imported_line(point: Vec3, vector: Vec3, scale: f64) -> Curve {
        let cached_point = Vec3::new(f64::from_bits(point.x.to_bits() + 2), point.y, point.z);
        let base =
            BSplineCurve::create_line_segment(3, cached_point, cached_point + scale * vector)
                .unwrap();
        Curve::from(Line::from_parts(
            base,
            point,
            vector,
            Extent1d::from_min_max(0.0, 1.0),
            scale,
            false,
        ))
    }

    #[test]
    fn swung_builder_materializes_lazy_line_nurbs() -> Result<()> {
        let kernel = Kernel::default();
        let p0 = Vec3::new(1.0, 0.0, 0.0);
        let p1 = Vec3::new(1.0, 0.0, 1.0);
        let t0 = Vec3::new(2.0, -1.0, 0.0);
        let t1 = Vec3::new(2.0, 1.0, 0.0);
        let profile = Line::create_line_segment(3, p0, p1, None)?;
        let path = Line::create_line_segment(3, t0, t1, None)?;
        let expected = Surface::from(BSplineSurface::create_swung_surface(
            profile.base(),
            path.base(),
            0.5,
        )?);
        let lazy_profile = Line::from_points(p0, p1, 3);
        let lazy_path = Line::from_points(t0, t1, 3);
        assert!(lazy_profile.base().gw_nurb().is_none());
        assert!(lazy_path.base().gw_nurb().is_none());
        // Each input must reach its own virtual MakeNurb, including when both
        // caches are empty. The old base copies fail before building a surface.
        for (profile, path) in [
            (lazy_profile.clone(), path.clone()),
            (profile, lazy_path.clone()),
            (lazy_profile, lazy_path),
        ] {
            let surface = Surface::from(swung_surface(
                &kernel,
                &Curve::from(path),
                &Curve::from(profile),
                0.5,
            )?);
            let domain = surface.natural_uv_domain_if_built();
            assert_eq!(domain, expected.natural_uv_domain_if_built());
            for i in 0..=4 {
                for j in 0..=4 {
                    let uv = domain.evaluate(f64::from(i) / 4.0, f64::from(j) / 4.0);
                    assert_eq!(
                        surface.evaluate_point(&Kernel::default(), uv)?,
                        expected.evaluate_point(&Kernel::default(), uv)?,
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn swung_builder_keeps_an_existing_cached_nurb() -> Result<()> {
        let kernel = Kernel::default();
        let profile = imported_line(Vec3::new(2.0, 0.0, 0.0), Vec3::Z, 1.0);
        let path = Curve::from(Line::from_points(Vec3::X, Vec3::X + Vec3::Y, 3));
        let surface = Surface::from(swung_surface(&kernel, &path, &profile, 1.0)?);
        let uv = surface.natural_uv_domain_if_built().min();
        let point = surface.evaluate_point(&kernel, uv)?;
        // Surface construction uses the existing NURB, even though scale
        // initialization above must use the original analytic evaluations.
        assert_eq!(point.x, f64::from_bits(2.0_f64.to_bits() + 2));
        assert_ne!(point.x, 2.0);
        Ok(())
    }

    #[test]
    fn swung_scale_keeps_analytic_profile_when_no_plane_intersection_exists() {
        let kernel = Kernel::default();
        let path = Curve::from(BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::X).unwrap());
        let profile = imported_line(Vec3::new(2.0, 2.0, 0.0), Vec3::Z, 1.0);
        assert_eq!(
            swung_curve_scale(&kernel, &path, &profile, 1e-5).unwrap(),
            0.5
        );
        let sliced = Curve::from(super::super::geometry::bspline(&profile).unwrap());
        assert_ne!(
            swung_curve_scale(&kernel, &path, &sliced, 1e-5).unwrap(),
            0.5
        );
    }

    #[test]
    fn swung_scale_keeps_analytic_path_after_plane_intersection() {
        let kernel = Kernel::default();
        let path = imported_line(Vec3::new(2.0, -1.0, 0.0), Vec3::Y, 2.0);
        let profile =
            Curve::from(BSplineCurve::create_line_segment(3, Vec3::X, Vec3::X + Vec3::Z).unwrap());
        assert_eq!(
            swung_curve_scale(&kernel, &path, &profile, 1e-5).unwrap(),
            0.5
        );
        let sliced = Curve::from(super::super::geometry::bspline(&path).unwrap());
        assert_ne!(
            swung_curve_scale(&kernel, &sliced, &profile, 1e-5).unwrap(),
            0.5
        );
    }

    /// An annulus swept along a line and a tangent quarter arc gets its two caps
    /// with the opt-in: each wall is two faces, one per path piece (WR-404).
    #[test]
    fn issue_411_holed_profile_sweeps_along_a_curved_path() {
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::mass::integrator::properties::{brep_volume, PropertyAttributes};
        use crate::nurbs::b_spline_curve::p1::NurbCircleParam;
        use crate::primitive::api_curves::create_circle;
        use crate::sweep::api_brep::create_curve_sweep;

        let o = Vec3::new(0.0, 0.0, 0.0);
        let line = BSplineCurve::create_line_segment(3, o, Vec3::new(0.0, 0.0, 10.0)).unwrap();
        let arc = BSplineCurve::create_arc_from_points(
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, 20.0),
            NurbCircleParam::Quadratic,
        )
        .unwrap();
        let k0 = Kernel::default();
        let mut built = CompositeCurve::build_composites_from_curves(
            &k0,
            vec![Curve::from(line), Curve::from(arc)],
            1e-6,
            0.0,
        )
        .unwrap();
        let path = Curve::from(built.composites[0].make_composite_nurb().unwrap());
        let outer = Curve::from(create_circle(o, 2.0).unwrap());
        let inner = Curve::from(create_circle(o, 1.0).unwrap());
        let options = SweepOptions {
            cap_ends: true,
            ..SweepOptions::default()
        };

        let store = AttributeStore::default();
        let key_of = |_| ObjectKey(0);
        let attributes = PropertyAttributes {
            store: &store,
            key_of: &key_of,
        };
        let mut k = Kernel::default();
        let sweep = |k: &mut Kernel, profile: &[Option<&Curve>]| {
            let swept =
                create_curve_sweep(k, profile, Some(&path), None, None, Some(&options)).unwrap();
            assert_eq!(swept.start_faces.len(), 1);
            assert_eq!(swept.end_faces.len(), 1);
            assert!(crate::topology::brep::p2::is_manifold_solid(&swept.brep).unwrap());
            swept
        };
        let mut outer_solid = sweep(&mut k, &[Some(&outer)]).brep;
        let solid_outer = brep_volume(&k, &mut outer_solid, 1e-6, &attributes).unwrap();

        k.fixes.curve_sweep_caps_at_sweep_ends = true;
        let mut pipe = sweep(&mut k, &[Some(&outer), Some(&inner)]);
        // each cap is the annulus: the outer loop and the hole
        for cap in [pipe.start_faces[0], pipe.end_faces[0]] {
            let loops = crate::topology::face::p2::loops(&pipe.brep, cap, false, None).unwrap();
            assert_eq!(loops.len(), 2);
        }
        // (the precise integration is not trusted to 1e-3 across this path's C1 joint,
        // source issue #384, so only the hole's removal is checked)
        let volume = brep_volume(&k, &mut pipe.brep, 1e-6, &attributes).unwrap();
        assert!(
            volume > 0.0 && volume < 0.9 * solid_outer,
            "volume {volume}, outer {solid_outer}"
        );
    }
}
