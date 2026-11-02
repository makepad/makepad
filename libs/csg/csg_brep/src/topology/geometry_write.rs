//! Writing curves and surfaces to an SMLib database stream: the virtual `WriteToDB` of
//! `SmCurve` and `SmSurface` dispatched over the concrete kinds, and the writers of the
//! surfaces of revolution and extrusion and their analytic kinds.
//!
//! Upstream: `source/SMLib/src/SmSurfOfRevolution.cpp`, `SmSurfOfExtrusion.cpp`,
//! `SmCone.cpp`, `SmCylinder.cpp`, `SmSphere.cpp`, `SmTorus.cpp`, `SmCrvOnSurf.cpp`,
//! `SmTangentField.cpp` (and the `WriteToDB` overrides of the other curve and surface
//! classes, dispatched to).
//!
//! | Rust                         | Upstream                                         |
//! |------------------------------|--------------------------------------------------|
//! | [`write_curve`]              | `SmCurve::WriteToDB` (virtual)                   |
//! | [`write_surface`]            | `SmSurface::WriteToDB` (virtual)                 |
//! | `write_surf_of_revolution` (private) | `SmSurfOfRevolution::WriteToDB`          |
//! | `write_surf_of_extrusion` (private)  | `SmSurfOfExtrusion::WriteToDB`           |
//! | `write_cone` (private)       | `SmCone::WriteToDB`, `SmCylinder::WriteToDB`     |
//! | `write_sphere` (private)     | `SmSphere::WriteToDB`                            |
//! | `write_torus` (private)      | `SmTorus::WriteToDB`                             |
//! | `write_crv_on_surf` (private) | `SmCrvOnSurf::WriteToDB`                        |
//! | `write_tangent_field` (private) | `SmTangentField::WriteToDB`                   |
//!
//! Deviations:
//! - **Kinds without a writer.** The write side is not in the plan (see
//!   `plan/findings.md`); only the kinds B-rep files hold are ported: B-spline curves,
//!   lines, ellipses and circles, Hermite curves (the UV curves of tangent fields),
//!   curves on surfaces and tangent fields; B-spline
//!   surfaces, planes, cones and cylinders, spheres, tori, surfaces of revolution and
//!   extrusion, offset surfaces. Another
//!   kind is [`SolidError::Unported`] with its `WriteToDB` symbol and unit
//!   `(not in plan)`.
//! - **Missing NURB.** Upstream's analytic writers ignore the status of the B-spline
//!   part they end with; it cannot fail there, as the virtual
//!   `GetOrCreateGwNurbPointer` makes the NURB. The port cannot make a surface of
//!   revolution's NURB without the kernel, so a missing NURB is an error (`SM_ERR`)
//!   instead of a silently short record.
//! - **Recursion.** Writers nest through generator curves and offset bases. The depth
//!   is counted and bounded by [`MAX_GEOMETRY_NESTING`] (the bound every geometry
//!   constructor and the reader keep), past which the write is
//!   [`SolidError::InvalidInput`].

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::tangent_field::TangentFieldCurve;
use crate::error::{ignore_status, Result, SolidError};
use crate::surface::cone::Cone;
use crate::surface::sphere::Sphere;
use crate::surface::surf_of_extrusion::SurfOfExtrusion;
use crate::surface::surf_of_revolution::SurfOfRevolution;
use crate::surface::surface::p2::MAX_GEOMETRY_NESTING;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::torus::Torus;
use crate::topology::database_io::{DatabaseWriter, FileType};

const OP_WRITE: &str = "topology::geometry_write::write_surface";
const NOT_IN_PLAN: &str = "(not in plan)";

/// `pCrv->WriteToDB(rDB, lDBVersionNumber)`: writes `c` (without its type header) in
/// its own format.
///
/// # Errors
///
/// [`SolidError::Unported`] for a kind without a ported writer; those of the kind's
/// writer.
pub fn write_curve(c: &Curve, db: &mut DatabaseWriter, version: u64) -> Result<()> {
    write_curve_at(c, db, version, 0)
}

/// [`write_curve`] for a curve nested `depth` levels deep.
///
/// # Errors
///
/// Those of [`write_curve`]; [`SolidError::InvalidInput`] past
/// [`MAX_GEOMETRY_NESTING`].
pub fn write_curve_at(
    c: &Curve,
    db: &mut DatabaseWriter,
    version: u64,
    depth: usize,
) -> Result<()> {
    check_depth(depth)?;
    match c.kind() {
        CurveKind::BSpline(b) => b.write_to_db(db, version),
        CurveKind::Line(l) => l.write_to_db(db, version),
        CurveKind::Ellipse(e) => e.write_to_db(db, version),
        CurveKind::Hermite(h) => h.write_to_db(db, version),
        CurveKind::Hyperbola(_) => Err(unported("SmHyperbola::WriteToDB")),
        CurveKind::Projected(_) => Err(unported("SmProjectedCurve::WriteToDB")),
        CurveKind::Iso(_) => Err(unported("SmIsoCurve::WriteToDB")),
        CurveKind::Offset(_) => Err(unported("SmOffsetCurve::WriteToDB")),
        CurveKind::Conic(_) => Err(unported("SmConic::WriteToDB")),
        CurveKind::CrvInVolume(_) => Err(unported("SmCrvInVolume::WriteToDB")),
        CurveKind::CrvOnSurf(c) => write_crv_on_surf(c, db, version, depth),
        CurveKind::TangentField(t) => write_tangent_field(t, db, version, depth),
        CurveKind::Parabola(_) => Err(unported("SmParabola::WriteToDB")),
        CurveKind::Composite(_) => Err(unported("SmCompositeCurve::WriteToDB")),
    }
}

/// `pSrf->WriteToDB(rDB, lDBVersionNumber)`: writes `s` (without its type header) in
/// its own format.
///
/// # Errors
///
/// [`SolidError::Unported`] for a kind without a ported writer; those of the kind's
/// writer.
pub fn write_surface(s: &Surface, db: &mut DatabaseWriter, version: u64) -> Result<()> {
    write_surface_at(s, db, version, 0)
}

/// [`write_surface`] for a surface nested `depth` levels deep.
///
/// # Errors
///
/// Those of [`write_surface`]; [`SolidError::InvalidInput`] past
/// [`MAX_GEOMETRY_NESTING`].
pub fn write_surface_at(
    s: &Surface,
    db: &mut DatabaseWriter,
    version: u64,
    depth: usize,
) -> Result<()> {
    check_depth(depth)?;
    match s.kind() {
        SurfaceKind::BSpline(b) => b.write_to_db(db, version),
        SurfaceKind::Plane(p) => p.write_to_db(db, version),
        SurfaceKind::Cone(c) => write_cone(c, db, version, depth),
        SurfaceKind::Sphere(sp) => write_sphere(sp, db, version, depth),
        SurfaceKind::Torus(t) => write_torus(t, db, version, depth),
        SurfaceKind::SurfOfRevolution(r) => write_surf_of_revolution(r, db, version, depth),
        SurfaceKind::SurfOfExtrusion(e) => write_surf_of_extrusion(e, db, version, depth),
        SurfaceKind::OffsetSurface(o) => o.write_to_db(db, version, depth),
        SurfaceKind::CurveBounded(_) => Err(unported("SmCurveBoundedSurface::WriteToDB")),
        SurfaceKind::Step(_) => Err(unported("SmSTEPSurface::WriteToDB")),
        SurfaceKind::SrfInVolume(_) => Err(unported("SmSrfInVolume::WriteToDB")),
    }
}

/// `SmCrvOnSurf::WriteToDB`: the domain of interest and the breaks flag, the UV curve
/// (type header with its dimension, then its record), the surface (type header, then
/// its record) and, when the breaks are cached, their count and values.
fn write_crv_on_surf(
    c: &CrvOnSurf,
    db: &mut DatabaseWriter,
    version: u64,
    depth: usize,
) -> Result<()> {
    let dom = c.uv_domain();
    let need_breaks = i32::from(c.need_breaks());
    if is_ascii(db) {
        db.out_reals(&[dom.u_min(), dom.v_min(), dom.u_max(), dom.v_max()]);
        db.out_str(" SmCrvOnSurf Domain of interest \n");
        db.out_i64(need_breaks.into());
        db.out_str(" SmCrvOnSurf Has breaks flag \n");
    } else {
        db.write_double(dom.u_min());
        db.write_double(dom.v_min());
        db.write_double(dom.u_max());
        db.write_double(dom.v_max());
        db.write_boolean(need_breaks);
    }

    // parameter space curve
    let uv = c.uv_curve();
    if is_ascii(db) {
        db.out_str(" CrvOnSurf->UVCurve \n");
    }
    db.write_type(uv.type_code(), Some(uv.dim()));
    write_curve_at(uv, db, version, depth + 1)?;

    // compounding surface
    if is_ascii(db) {
        db.out_str(" CrvOnSurf->Surface \n");
    }
    db.write_type(c.surface().type_code(), None);
    write_surface_at(c.surface(), db, version, depth + 1)?;

    // when appropriate - breaks
    if need_breaks == 0 {
        let breaks = c.breaks();
        // number of breaks
        if is_ascii(db) {
            db.out_u64(breaks.len() as u64);
            db.out_str(" Number of Curve internal discontinuities\n");
        } else {
            db.write_long(breaks.len() as i64);
        }

        // for every break
        if is_ascii(db) {
            for &b in &breaks {
                db.out_f64(b);
                db.out_str(" ");
            }
            if !breaks.is_empty() {
                db.out_str(" \n");
            }
        } else {
            for &b in &breaks {
                db.write_double(b);
            }
        }
    }

    // all done
    Ok(())
}

/// `SmTangentField::WriteToDB`: the interpolation type and the start and end tangents,
/// then the `SmCrvOnSurf` part (whose status upstream ignores: the write succeeds).
fn write_tangent_field(
    t: &TangentFieldCurve,
    db: &mut DatabaseWriter,
    version: u64,
    depth: usize,
) -> Result<()> {
    let (s, e) = (t.start_tangent(), t.end_tangent());
    if is_ascii(db) {
        db.out_u64(t.dir_type_code());
        db.out_str(" SmTangentField Tangent Dir Interpolation Type \n");
        db.out_reals(&[s.x, s.y, s.z]);
        db.out_str(" SmTangentField Start Point CrossTangent vector \n");
        db.out_reals(&[e.x, e.y, e.z]);
        db.out_str(" SmTangentField End Point CrossTangent vector \n");
    } else {
        db.write_long(t.dir_type_code() as i64);

        db.write_double(s.x);
        db.write_double(s.y);
        db.write_double(s.z);

        db.write_double(e.x);
        db.write_double(e.y);
        db.write_double(e.z);
    }

    // Write the base object (status ignored; an unported part still propagates)
    ignore_status(write_crv_on_surf(t.base(), db, version, depth))?;

    // all done
    Ok(())
}

fn unported(symbol: &'static str) -> SolidError {
    SolidError::unported(symbol, NOT_IN_PLAN)
}

fn check_depth(depth: usize) -> Result<()> {
    if depth > MAX_GEOMETRY_NESTING {
        return Err(SolidError::invalid_input(
            OP_WRITE,
            "geometry nested too deeply",
        ));
    }
    Ok(())
}

fn is_ascii(db: &DatabaseWriter) -> bool {
    db.file_type() == FileType::Ascii
}

/// `SmCone::WriteToDB` (`SmCylinder::WriteToDB` passes the call on): the base radius,
/// the semi-angle (degrees) and the inside-out flag, then the surface of revolution.
fn write_cone(c: &Cone, db: &mut DatabaseWriter, version: u64, depth: usize) -> Result<()> {
    let inside_out = i32::from(c.inside_out);
    if is_ascii(db) {
        db.out_f64(c.base_radius);
        db.out_str(" SmCone Radius of the cone in the plane passing through origin \n");
        db.out_f64(c.semi_angle_deg);
        db.out_str(" SmCone SemiAngleDeg between -90 and 90 \n");
        db.out_i64(inside_out.into());
        db.out_str(
            " SmCone InsideOutFlag: FALSE= m_vPosition.ZAxis points from bot to top of Nurb, \
             TRUE=Inverted \n",
        );
    } else {
        db.write_double(c.base_radius);
        db.write_double(c.semi_angle_deg);
        db.write_boolean(inside_out);
    }
    write_surf_of_revolution(&c.rev, db, version, depth)
}

/// `SmSphere::WriteToDB`: the radius and the inside-out flag, then the surface of
/// revolution.
fn write_sphere(s: &Sphere, db: &mut DatabaseWriter, version: u64, depth: usize) -> Result<()> {
    let inside_out = i32::from(s.inside_out);
    if is_ascii(db) {
        db.out_f64(s.radius).out_str(" SmSphere Sphere Radius \n");
        db.out_i64(inside_out.into());
        db.out_str(" SmSphere Sphere InsideOut: TRUE=outward pointing normal, FALSE=inverted \n");
    } else {
        db.write_double(s.radius);
        db.write_boolean(inside_out);
    }
    write_surf_of_revolution(&s.base, db, version, depth)
}

/// `SmTorus::WriteToDB`: the major and minor radii and the inside-out flag, then the
/// surface of revolution.
fn write_torus(t: &Torus, db: &mut DatabaseWriter, version: u64, depth: usize) -> Result<()> {
    let inside_out = i32::from(t.inside_out);
    if is_ascii(db) {
        db.out_f64(t.major_radius);
        db.out_str(" SmTorus Torus Major Radius \n");
        db.out_f64(t.minor_radius);
        db.out_str(" SmTorus Torus Minor Radius \n");
        db.out_i64(inside_out.into());
        db.out_str(" SmTorus Torus InsideOut: TRUE=outward pointing normal, FALSE=inverted \n");
    } else {
        db.write_double(t.major_radius);
        db.write_double(t.minor_radius);
        db.write_boolean(inside_out);
    }
    write_surf_of_revolution(&t.base, db, version, depth)
}

/// `SmSurfOfRevolution::WriteToDB`: the flags, the placement, the analytic UV domain
/// and the swap flag; then the generator curve with its type header, the polar
/// converter and the B-spline surface part.
fn write_surf_of_revolution(
    r: &SurfOfRevolution,
    db: &mut DatabaseWriter,
    version: u64,
    depth: usize,
) -> Result<()> {
    let ascii = is_ascii(db);
    let o = r.position.origin();
    let x = r.position.x_axis();
    let y = r.position.y_axis();
    let (lo, hi) = (r.anal_uv_domain.min(), r.anal_uv_domain.max());
    let make_nurb_gen_curve = i32::from(r.make_nurb_gen_curve);
    let planar_generator = i32::from(r.planar_generator);
    let swap_uv = i32::from(r.swap_uv);
    if ascii {
        db.out_i64(make_nurb_gen_curve.into());
        db.out_str(
            " SmSurfOfRevolution Make Nurb GenCurve Flag, FALSE=Unbounded analytic, \
             TRUE=bounded \n",
        );
        db.out_i64(planar_generator.into());
        db.out_str(" SmSurfOfRevolution Planar Generator Curve \n");
        db.out_reals(&[o.x, o.y, o.z]);
        db.out_str(" SmSurfOfRevolution Placement Origin \n");
        db.out_reals(&[x.x, x.y, x.z]);
        db.out_str(" SmSurfOfRevolution Placement X Axis \n");
        db.out_reals(&[y.x, y.y, y.z]);
        db.out_str(" SmSurfOfRevolution Placement Y Axis \n");
        db.out_reals(&[lo.x, lo.y, hi.x, hi.y]);
        db.out_str(" SmSurfOfRevolution AnalUVDomain \n");
        db.out_i64(swap_uv.into());
        db.out_str(" SmSurfOfRevolution SwapUV Boolean \n");
    } else {
        db.write_boolean(make_nurb_gen_curve);
        db.write_boolean(planar_generator);
        db.write_doubles(&[o.x, o.y, o.z, x.x, x.y, x.z, y.x, y.y, y.z]);
        db.write_doubles(&[lo.x, lo.y, hi.x, hi.y]);
        db.write_boolean(swap_uv);
    }

    // the generator curve (upstream dereferences a null one)
    let gen = r
        .gen_curve
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP_WRITE))?;
    if ascii {
        db.out_str(" SmSurfOfRevolution->GenCurve \n");
    }
    db.write_type(gen.type_code(), Some(gen.dim()));
    write_curve_at(gen, db, version, depth + 1)?;

    // the polar converter (with its own copy of the curve, if it has one)
    if ascii {
        db.out_str(" SmSurfOfRevolution->PolarConverter \n");
    }
    r.polar
        .write_to_db(db, version, r.polar_curve.as_ref(), depth + 1)?;

    r.base.write_to_db(db, version)
}

/// `SmSurfOfExtrusion::WriteToDB`: the origin, the extrusion vector, the analytic UV
/// domain, the generator's orientation type, the swap flag and the generator's point
/// and vector; then the generator curve with its type header and the B-spline surface
/// part.
fn write_surf_of_extrusion(
    e: &SurfOfExtrusion,
    db: &mut DatabaseWriter,
    version: u64,
    depth: usize,
) -> Result<()> {
    let ascii = is_ascii(db);
    let (o, v) = (e.origin, e.extrusion_vec);
    let (lo, hi) = (e.anal_uv_domain.min(), e.anal_uv_domain.max());
    let orientation = e.orientation as i64;
    let swap_uv = i32::from(e.swap_uv);
    let (cp, cv) = (e.curve_point, e.curve_vec);
    if ascii {
        db.out_reals(&[o.x, o.y, o.z]);
        db.out_str(" SmSurfOfExtrusion Origin \n");
        db.out_reals(&[v.x, v.y, v.z]);
        db.out_str(" SmSurfOfExtrusion Extrusion Vector \n");
        db.out_reals(&[lo.x, lo.y, hi.x, hi.y]);
        db.out_str(" SmSurfOfExtrusion AnalUVDomain \n");
        db.out_i64(orientation);
        db.out_str(" SmSurfOfExtrusion Curve Orientation Type \n");
        db.out_i64(swap_uv.into());
        db.out_str(" SmSurfOfExtrusion SwapUV Boolean \n");
        db.out_reals(&[cp.x, cp.y, cp.z]);
        db.out_str(
            " SmSurfOfExtrusion When GenCurve is planar or linear - point on plane or line\n",
        );
        db.out_reals(&[cv.x, cv.y, cv.z]);
        db.out_str(
            " SmSurfOfExtrusion When GenCurve is planar or linear - plane normal or line vec \n",
        );
    } else {
        db.write_doubles(&[o.x, o.y, o.z, v.x, v.y, v.z]);
        db.write_doubles(&[lo.x, lo.y, hi.x, hi.y]);
        db.write_long(orientation);
        db.write_boolean(swap_uv);
        db.write_doubles(&[cp.x, cp.y, cp.z, cv.x, cv.y, cv.z]);
    }

    // the generator curve (upstream dereferences a null one)
    let gen = e
        .gen_curve
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP_WRITE))?;
    if ascii {
        db.out_str(" SmSurfOfExtrusion->GenCurve \n");
    }
    db.write_type(gen.type_code(), Some(gen.dim()));
    write_curve_at(gen, db, version, depth + 1)?;

    e.base.write_to_db(db, version)
}
