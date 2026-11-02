//! `SmBrep::Mirror`: mirrors a B-rep about the XY plane of an axis placement.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp` (`SmBrep::Mirror`, `sm_ReverseReflectionEdgeuses`,
//! `sm_SwapReflectionShells`), `source/SMLib/inc/SmBrep.h`.
//!
//! | Rust                                            | Upstream                                   |
//! |-------------------------------------------------|--------------------------------------------|
//! | [`mirror`], [`mirror_with_notifications`]       | `SmBrep::Mirror`                           |
//! | `Axis2Placement::mirror_point`                  | `SmAxis2Placement::MirrorPoint`            |
//! | `Axis2Placement::mirror_vector`                 | `SmAxis2Placement::MirrorVector`           |
//! | `Curve::create_mirror_curve`                    | `SmCurve::CreateMirrorCurve` (virtual)     |
//! | `Surface::create_mirror_surface`                | `SmSurface::CreateMirrorSurface` (virtual) |
//! | `BSplineSurface::create_mirror_surface`         | `SmBSplineSurface::CreateMirrorSurface`    |
//! | `OffsetSurface::create_mirror_surface`          | `SmOffsetSurface::CreateMirrorSurface`     |
//! | `CrvOnSurf::create_mirror_curve`                | `SmCrvOnSurf::CreateMirrorCurve`           |
//! | `CrvInVolume::create_mirror_curve`              | `SmCrvInVolume::CreateMirrorCurve`         |
//!
//! (`BSplineCurve::create_mirror_curve`, `SrfInVolume::create_mirror_surface`,
//! `topology::brep::p1::reverse_reflection_edgeuses` and `swap_reflection_shells` were
//! ported before this unit.)
//!
//! Deviations: the steps run in upstream's order with its statuses: the editing
//! notifications of the vertices, edges and faces (`Notify` statuses ignored), then
//! `SER` on every curve and surface creation, `CopyAndAddAnalytics`, `ReplaceSurface`,
//! `RemoveUVTrimCurves` and `CreateUVTrimCurves`. An edge's old curve and a face's
//! temporary mirror surface are dropped where upstream deletes them. A curve or surface
//! wrapping a volume (`SmCrvInVolume`, `SmSrfInVolume`) mirrors its own copy of the
//! volume (parts are held by value), where upstream also edits a volume the object does
//! not own. Composite edges do not exist in the arena model (`sEdges` is the edge
//! alone). The NURBs of analytic surfaces without one are made before the infinite region
//! search (`make_missing_nurbs`), where upstream's bounding box makes them lazily. The
//! debug drawing code is omitted. No recursion is introduced here.

use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::kernel::Kernel;
use crate::query::brep::BrepQuery;
use crate::surface::surface::p2::SurfaceKind;
use crate::topology::brep::p1::{
    curve_owners, reverse_reflection_edgeuses, surface_owners, swap_reflection_shells,
    SetRegionVoidType,
};
use crate::topology::brep::p5::{
    brep_faces, brep_regions, brep_vertices, find_and_set_infinite_region, notify_edge_pre_edit,
    notify_face_pre_edit, notify_pre_edit, notify_vertex_pre_edit, plain_edit, replace_surface,
    set_edge_curve_owner, set_joined_edge_curve, set_vertex_point, transform_owner_key,
    with_notifications, EditNotifyEnv,
};
use crate::topology::Brep;
use crate::EntityRef;

const OP: &str = "topology::brep::mirror::mirror";

/// `SmBrep::Mirror(crMirrorPlane)`: mirrors every vertex point, edge curve and face
/// surface about the XY plane of `mirror_plane`, reverses the radial order of the edge
/// uses and swaps the shells of the face uses (a reflection turns the B-rep inside out),
/// rebuilds the UV trim curves and, with more than one region, finds the infinite
/// region again.
///
/// # Errors
///
/// `SM_ERR` for an edge curve or face surface of a kind without `CreateMirrorCurve` /
/// `CreateMirrorSurface` (see [`crate::curve::curve::Curve::create_mirror_curve`],
/// [`crate::surface::surface::Surface::create_mirror_surface`]); `InvalidInput` where
/// upstream dereferences null (an edge without curve, a face without surface or
/// face uses); those of the callees.
pub fn mirror(k: &mut Kernel, b: &mut Brep, mirror_plane: &Axis2Placement) -> Result<()> {
    mirror_with_notifications(k, b, mirror_plane, None)
}

/// [`mirror`] notifying through the caller's environment `notifications` (a plain edit
/// under the kernel's identities when `None`).
///
/// # Errors
///
/// Those of [`mirror`].
pub fn mirror_with_notifications(
    k: &mut Kernel,
    b: &mut Brep,
    mirror_plane: &Axis2Placement,
    notifications: Option<&mut EditNotifyEnv<'_>>,
) -> Result<()> {
    if let Some(env) = notifications {
        mirror_in(k, b, mirror_plane, env)
    } else {
        plain_edit(k, b, |k, b, env| mirror_in(k, b, mirror_plane, env))
    }
}

fn null() -> SolidError {
    SolidError::null_pointer(OP)
}

fn mirror_in(
    k: &mut Kernel,
    b: &mut Brep,
    mirror_plane: &Axis2Placement,
    env: &mut EditNotifyEnv<'_>,
) -> Result<()> {
    let owner_keys = env.key_of;
    let mut notifications = Some(env);

    // notify Brep of editing operation
    ignore_status(notify_pre_edit(k, b, notifications.as_deref_mut()))?;

    // mirror every vertex->Point
    for vertex in brep_vertices(b)? {
        let point = b.vertex(vertex)?.point;
        ignore_status(notify_vertex_pre_edit(
            k,
            b,
            vertex,
            notifications.as_deref_mut(),
        ))?;
        set_vertex_point(
            k,
            b,
            vertex,
            mirror_plane.mirror_point(point),
            notifications.as_deref_mut(),
        )?;
    }

    // mirror every edge - reverse radial orderings: for every Brep->Curve
    for edge in curve_owners(b, None, &|e| {
        transform_owner_key(b, EntityRef::Edge(e), owner_keys(EntityRef::Edge(e)))
    })? {
        ignore_status(notify_edge_pre_edit(
            k,
            b,
            edge,
            notifications.as_deref_mut(),
        ))?;
        let new_curve = b
            .edge(edge)?
            .curve
            .as_ref()
            .ok_or_else(null)?
            .create_mirror_curve(k, mirror_plane)?;
        // SetCurve(pNewCurve, FALSE, FALSE) (status ignored), then SetOwner(pEdge); the
        // old curve is dropped here (upstream deletes it after the edge use reversal)
        set_joined_edge_curve(
            k,
            b,
            edge,
            Some(new_curve),
            false,
            notifications.as_deref_mut(),
        )?;
        set_edge_curve_owner(b, edge, notifications.as_deref_mut())?;
        reverse_reflection_edgeuses(b, edge)?;
    }

    // mirror every face: for every Brep->Surface
    for face in surface_owners(b, None, &|f| {
        transform_owner_key(b, EntityRef::Face(f), owner_keys(EntityRef::Face(f)))
    })? {
        ignore_status(notify_face_pre_edit(
            k,
            b,
            face,
            notifications.as_deref_mut(),
        ))?;
        let tmp_surface = b
            .face(face)?
            .surface
            .as_ref()
            .ok_or_else(null)?
            .create_mirror_surface(k, mirror_plane)?;

        // Copy pTmpSurface for new face, when possible as an analytic surface
        let new_surface = tmp_surface.copy_and_add_analytics(k)?;
        drop(tmp_surface);
        let mut modified = Vec::new();
        replace_surface(
            k,
            b,
            face,
            new_surface,
            false,
            Some(&mut modified),
            true,
            notifications.as_deref_mut(),
        )?;

        // for every Face using pSurface - remove UVTrimCurves, swap Faceuse shells
        for f in modified {
            with_notifications(b, notifications.as_deref_mut(), None, |b, env, _| {
                crate::topology::face::p1::remove_uv_trim_curves_with_notifications(b, f, env)
            })?;
            swap_reflection_shells(b, f)?;
        }
    }

    // Brep->Faces - rebuild UVTrimCurves
    for f in brep_faces(b)? {
        with_notifications(b, notifications.as_deref_mut(), None, |b, env, _| {
            crate::topology::face::p3::create_uv_trim_curves_notified(
                k,
                b,
                f,
                true,
                None,
                Some(env),
            )
            .map(|_| ())
        })?;
    }

    // Brep->Regions - FindAndSetInfiniteRegion
    if brep_regions(b)?.len() > 1 {
        make_missing_nurbs(k, b)?;
        // SmTemporaryChangeValue<SmBoolean> sChange(m_bEditingEnabled, FALSE)
        let previous = b.editing_enabled();
        b.set_editing_enabled(false);
        let query = BrepQuery::of_brep(b);
        let result = find_and_set_infinite_region(
            k,
            b,
            &query,
            None,
            SetRegionVoidType::InfiniteRegionSwap,
            None,
        );
        b.set_editing_enabled(previous);
        result?;
    }
    Ok(())
}

/// Makes the NURB of every analytic face surface that has none. Upstream's first
/// `SmBSplineSurface::CalculateBoundingBox` makes it through the virtual
/// `GetOrCreateGwNurbPointer` (`FindAndSetInfiniteRegion` calls it); the port's bounding
/// box is a `const` method that returns `SM_ERR` instead (finding
/// `query.brep--boolean-results-need-their-analytic-nurbs`), so the NURBs are made here,
/// where upstream makes them, just before that call.
fn make_missing_nurbs(k: &Kernel, b: &mut Brep) -> Result<()> {
    for f in brep_faces(b)? {
        let Some(surface) = b.face_mut(f)?.surface.as_mut() else {
            continue;
        };
        match surface.kind_mut() {
            SurfaceKind::Plane(s) if s.base().gw_nurb().is_none() => s.make_nurb()?,
            SurfaceKind::Cone(s) if s.rev().base().gw_nurb().is_none() => s.make_nurb(k)?,
            SurfaceKind::Sphere(s) if s.base().base().gw_nurb().is_none() => s.make_nurb(k)?,
            SurfaceKind::Torus(s) if s.base().base().gw_nurb().is_none() => s.make_nurb(k)?,
            SurfaceKind::SurfOfRevolution(s) if s.base().gw_nurb().is_none() => s.make_nurb(k)?,
            SurfaceKind::SurfOfExtrusion(s) if s.base().gw_nurb().is_none() => s.make_nurb(k)?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec3;
    use crate::primitive::api_primitives::create_box;
    use crate::topology::brep::p1;

    #[test]
    fn mirrors_a_box_in_a_plane_through_its_side() {
        let mut k = Kernel::default();
        let mut b = create_box(&mut k, Vec3::new(1.0, 2.0, 3.0), 2.0, 3.0, 4.0).unwrap();
        let vertices = p1::vertices(&b, None).unwrap();
        let before: Vec<Vec3> = vertices
            .iter()
            .map(|&v| b.vertex(v).unwrap().point)
            .collect();
        // the plane x = 2 (normal +X)
        let plane = Axis2Placement::new(Vec3::new(2.0, 0.0, 0.0), Vec3::Y, Vec3::Z);
        mirror(&mut k, &mut b, &plane).unwrap();
        for (&v, p) in vertices.iter().zip(&before) {
            let q = b.vertex(v).unwrap().point;
            assert!((q.x - (4.0 - p.x)).abs() < 1e-12 && q.y == p.y && q.z == p.z);
        }
        // every edge curve follows its vertices
        for e in p1::edges(&b, None).unwrap() {
            let edge = b.edge(e).unwrap();
            let curve = edge.curve.as_ref().unwrap();
            let start = curve.evaluate_point(&k, edge.interval.min()).unwrap();
            let end = curve.evaluate_point(&k, edge.interval.max()).unwrap();
            let on_vertex = |p: Vec3| {
                p1::vertices(&b, None)
                    .unwrap()
                    .iter()
                    .any(|&v| (b.vertex(v).unwrap().point - p).length() < 1e-9)
            };
            assert!(on_vertex(start) && on_vertex(end));
        }
        // a second mirror restores the box
        mirror(&mut k, &mut b, &plane).unwrap();
        for (&v, p) in vertices.iter().zip(&before) {
            assert!((b.vertex(v).unwrap().point - *p).length() < 1e-12);
        }
    }
}
