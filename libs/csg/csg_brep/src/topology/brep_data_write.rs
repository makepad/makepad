//! Writing serialized B-rep data (`SmBrepData` and its record classes) to an SMLib
//! database stream (`.smb`, ASCII or binary): the write side of
//! [`crate::topology::brep_data`].
//!
//! Upstream: `source/SMLib/src/SmBrepData.cpp`, `source/SMLib/src/SmAttribute.cpp`,
//! `source/SMLib/inc/SmBrepData.h`.
//!
//! | Rust                                  | Upstream                                         |
//! |---------------------------------------|--------------------------------------------------|
//! | [`BrepData::write_to_file`]           | `SmBrepData::WriteToFile` (new file)             |
//! | [`BrepData::write_to_db`]             | `SmBrepData::WriteToDB`                          |
//! | [`write_curve_to_db`]                 | `SmBrepData::WriteCurveToDB`                     |
//! | [`write_surface_to_db`]               | `SmBrepData::WriteSurfaceToDB`                   |
//! | [`TopologyData::write_to_db`] ... [`VertexData::write_to_db`] | `SmTopologyData::WriteToDB` ... `SmVertexData::WriteToDB` |
//! | [`AttributeTable`]                    | `rAllAttributes` and `rAttrMap`                  |
//! | [`BrepData::from_brep`], [`FromBrepEnv`] | `SmBrepData::FromBrep`                        |
//! | [`write_brep_to_file`]                | `SmBrep::WriteToFile` (new file, native geometry) |
//! | `get_map`, `edgeuse_index` (private)  | `sm_GetMap`, `sm_GetEdgeuseIndex`                |
//! | `extract_attributes` (private)        | `sm_ExtractAttributes`, `sm_FindAttribute`       |
//! | `write_indexed_attributes` (private)  | `SmAttributeData::WriteIndexedAttributesToDB`    |
//! | [`DATABASE_VERSION`]                  | `SM_CURRENT_DATABASE_VERSION`                    |
//!
//! Deviations:
//! - **Files.** [`BrepData::write_to_file`] returns the file's bytes (no filesystem on
//!   wasm32); only a new file (`bNewFile = TRUE`) is supported.
//! - **Native formats only.** `bWriteAsBSplines = TRUE` approximates every non-B-spline
//!   curve and surface (`ApproximateCurve`, `ApproximateSurface`); it is not ported,
//!   and the writers always write native formats (the upstream default). The kinds
//!   with a ported writer are those of [`crate::topology::geometry_write`].
//! - **Edge use mate flags.** `SmEUData::WriteToDB` writes the edge use's own flags and
//!   user indices where `ReadFromDB` reads the mate's (`m_lFlags` twice, never
//!   `m_lMateFlags`), so the mate's values do not survive a write and read; the port
//!   writes what upstream writes.
//! - **UV curve dimension.** `WriteToDB` turns each UV curve whose control points all
//!   have `z == 0` into a 2D NURB in place before writing it, as upstream does (the
//!   write therefore takes `&mut self`).
//! - **`FromBrep` identities.** Upstream maps object addresses to record indices. The
//!   port maps typed ids, and reads each object's `SmTopology` record and attribute
//!   list under the identity [`FromBrepEnv::key_of`] gives it (geometry under the
//!   `*_key` closures). An object missing from the maps is `SM_NO_OBJECT`, as
//!   `sm_GetMap` returns (it also signals `SM_ERR`, which nothing reads). Upstream
//!   borrows the B-rep's curves and surfaces (`m_bIsGeomBorrowed`); the port copies
//!   them exactly (no copy-constructor knot fix-up). Each edge owns its curve, so the
//!   3D curve list has one curve per edge (upstream shares a curve between edges
//!   only when they share the object). `GetSurfaces` keeps the surfaces owned by
//!   their faces, as upstream. Dereferences of null objects (a loop without loop
//!   uses, an edge use without mate, next edge use, loop use or loop; an edge without
//!   primary edge use or curve) are `SM_ERR_NULL_POINTER`.
//! - **Statuses.** Upstream ignores the status of every record write and of the curve
//!   and surface writes; the port's record writes cannot fail, and a curve or surface
//!   that cannot be written (no ported writer, no NURB) is an error instead of a
//!   silently short file.

use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, EntityRef, Result, SolidError};
use crate::geom::attribute::{ids, AttributeHandle, AttributeStore, Behavior, ObjectKey};
use crate::geom::object::ObjectRef;
use crate::geom::vector3d::Vec3;
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::math::NOZ;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::brep::p1::{self as bl, surface_owners};
use crate::topology::brep::p10::HEALER_VERSION;
use crate::topology::brep::p2::is_manifold_solid;
use crate::topology::brep_data::{
    BrepData, DataSourceType, EdgeData, EuData, FaceData, FaceuseData, Keyed, LoopData, RegionData,
    ShellData, TopologyData, VertexData, NO_OBJECT, UNDEF_ULONG,
};
use crate::topology::database_io::{DatabaseWriter, FileType};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::geometry_write::{write_curve, write_surface};
use crate::topology::topology::Topology;
use crate::topology::{edge, face, loopuse, Brep, EdgeId, EdgeuseId, FaceId};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

/// `SM_CURRENT_DATABASE_VERSION`: the version [`BrepData::write_to_file`] writes.
pub const DATABASE_VERSION: u64 = 42;

const OP_CURVE: &str = "topology::brep_data_write::write_curve_to_db";
const OP_SURFACE: &str = "topology::brep_data_write::write_surface_to_db";
const OP_UV: &str = "topology::brep_data_write::BrepData::write_to_db";
const OP_FROM: &str = "topology::brep_data_write::BrepData::from_brep";

/// The attributes a write gathers (upstream `rAllAttributes`, with `rAttrMap`): every
/// attribute written, in index order, read from `store`.
#[derive(Debug)]
pub struct AttributeTable<'a> {
    /// The attribute lists of the objects written (by their keys).
    pub store: &'a AttributeStore,
    /// `rAllAttributes`: the attributes indexed so far.
    pub all: &'a mut Vec<AttributeHandle>,
    /// `rAttrMap`: the attributes this table indexed (an attribute in `all` but not
    /// here, indexed under another map, is appended again, as upstream).
    pub map: &'a mut BTreeSet<AttributeHandle>,
}

/// `sm_ExtractAttributes` (with `sm_FindAttribute`): the indices of the attributes of
/// `key` in `table.all`, appending those the map has not seen; attributes of
/// behaviour [`Behavior::Temp`] are skipped.
fn extract_attributes(table: &mut AttributeTable<'_>, key: ObjectKey) -> Vec<u64> {
    let mut indices = Vec::new();
    for h in table.store.attributes(key) {
        let temp = table
            .store
            .get(h)
            .is_some_and(|a| a.behavior() == Behavior::Temp);
        if temp {
            continue;
        }
        // sm_FindAttribute: in the map, then its position in the accumulation list
        let found = if table.map.contains(&h) {
            table.all.iter().position(|&x| x == h)
        } else {
            None
        };
        let index = if let Some(i) = found {
            i
        } else {
            table.all.push(h);
            table.map.insert(h);
            table.all.len() - 1
        };
        indices.push(index as u64);
    }
    indices
}

/// `SmAttributeData::WriteIndexedAttributesToDB`: the count and the indices.
fn write_indexed_attributes(indices: &[u64], db: &mut DatabaseWriter) {
    if db.file_type() == FileType::Ascii {
        if indices.is_empty() {
            db.out_str("            0 ");
        } else {
            // output total number of attributes, then each attribute index
            db.out_str("            ").out_u64(indices.len() as u64);
            db.out_str(" ");
            for &i in indices {
                db.out_i64(i as i64).out_str(" ");
            }
        }
        db.out_str("\n");
    } else {
        db.write_long(indices.len() as i64);
        if !indices.is_empty() {
            db.write_longs(indices);
        }
    }
}

fn is_ascii(db: &DatabaseWriter) -> bool {
    db.file_type() == FileType::Ascii
}

/// `version > 30 && bPersistAttribs`.
fn with_attributes(version: u64, persist_attribs: bool) -> bool {
    version > 30 && persist_attribs
}

impl TopologyData {
    /// `SmTopologyData::WriteToDB`: for `version > 31`, the flags and the two user
    /// indices (ASCII: each after a space; the small-topology flag of `version > 43`
    /// is `SM_USE_NEWTOL` only).
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter) {
        if version > 31 {
            if is_ascii(db) {
                db.out_str(" ").out_u64(self.flags);
                db.out_str(" ").out_u64(self.user_index1);
                db.out_str(" ").out_u64(self.user_index2);
            } else {
                db.write_long(self.flags as i64);
                db.write_long(self.user_index1 as i64);
                db.write_long(self.user_index2 as i64);
            }
        }
    }
}

impl RegionData {
    /// `SmRegionData::WriteToDB`: the topology data, the shell count, first shell and
    /// void flag, then the attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        self.topology.write_to_db(version, db);
        if is_ascii(db) {
            db.out_str(" ").out_u64(self.num_shells);
            db.out_str(" ").out_u64(self.start_shell);
            db.out_str(" ").out_i64(self.is_void.into());
            db.out_str("\n");
        } else {
            db.write_long(self.num_shells as i64);
            db.write_long(self.start_shell as i64);
            db.write_boolean(self.is_void);
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&self.topology.attributes, db);
        }
    }
}

impl ShellData {
    /// `SmShellData::WriteToDB`: the topology data and the shell by type (face use:
    /// the constant orientation `1`, count and first face use; wire edge: the edge;
    /// vertex: the vertex; another type writes nothing), then the attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        self.topology.write_to_db(version, db);
        // used to remove m_bOrientation - kept to keep the smb file structure constant
        let orientation: i32 = 1;
        if is_ascii(db) {
            match self.shell_type {
                0 => {
                    db.out_str(" ").out_u64(self.shell_type);
                    db.out_str(" ").out_i64(orientation.into());
                    db.out_str(" ").out_u64(self.num_faceuses);
                    db.out_str(" ").out_u64(self.faceuse_start);
                    db.out_str("\n");
                }
                1 => {
                    db.out_str(" ").out_u64(self.shell_type);
                    db.out_str(" ").out_u64(self.edge).out_str("\n");
                }
                2 => {
                    db.out_str(" ").out_u64(self.shell_type);
                    db.out_str(" ").out_u64(self.vertex).out_str("\n");
                }
                _ => {}
            }
        } else {
            match self.shell_type {
                0 => {
                    db.write_long(self.shell_type as i64);
                    db.write_long(self.num_faceuses as i64);
                    db.write_long(self.faceuse_start as i64);
                    db.write_boolean(orientation);
                }
                1 => {
                    db.write_long(self.shell_type as i64);
                    db.write_long(self.edge as i64);
                }
                2 => {
                    db.write_long(self.shell_type as i64);
                    db.write_long(self.vertex as i64);
                }
                _ => {}
            }
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&self.topology.attributes, db);
        }
    }
}

impl FaceuseData {
    /// `SmFaceuseData::WriteToDB`: the topology data, the face and the orientation,
    /// then the attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        self.topology.write_to_db(version, db);
        if is_ascii(db) {
            db.out_str(" ").out_u64(self.face);
            db.out_str(" ").out_i64(self.orientation.into());
            db.out_str("\n");
        } else {
            db.write_long(self.face as i64);
            db.write_boolean(self.orientation);
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&self.topology.attributes, db);
        }
    }
}

impl FaceData {
    /// `SmFaceData::WriteToDB`: the topology data, the zone tolerance, surface, first
    /// loop, loop count and rectangular-trim flag, then the UV domain as
    /// `umin umax vmin vmax` (binary: the domain first), then the attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        self.topology.write_to_db(version, db);
        let (lo, hi) = (self.uv_domain.min(), self.uv_domain.max());
        if is_ascii(db) {
            db.out_str(" ").out_f64(self.zone_tol_3d);
            db.out_str(" ").out_u64(self.surface);
            db.out_str(" ").out_u64(self.start_loop);
            db.out_str(" ").out_u64(self.num_loops);
            db.out_str(" ").out_i64(self.rectangular_trim.into());
            db.out_str("\n").out_f64(lo.x);
            db.out_str(" ").out_f64(hi.x);
            db.out_str(" ").out_f64(lo.y);
            db.out_str(" ").out_f64(hi.y);
            db.out_str("\n");
        } else {
            db.write_doubles(&[lo.x, hi.x, lo.y, hi.y, self.zone_tol_3d]);
            db.write_long(self.surface as i64);
            db.write_long(self.start_loop as i64);
            db.write_long(self.num_loops as i64);
            db.write_boolean(self.rectangular_trim);
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&self.topology.attributes, db);
        }
    }
}

impl LoopData {
    /// `SmLoopData::WriteToDB`: the topology data and the loop by type (edge uses:
    /// count and first edge use; vertex: the vertex; another type writes nothing),
    /// then the loop's and its two loop uses' attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        self.topology.write_to_db(version, db);
        if is_ascii(db) {
            match self.loop_type {
                0 => {
                    db.out_str(" ").out_u64(self.loop_type);
                    db.out_str(" ").out_u64(self.num_eu);
                    db.out_str(" ").out_u64(self.start_eu);
                    db.out_str("\n");
                }
                1 => {
                    db.out_str(" ").out_u64(self.loop_type);
                    db.out_str(" ").out_u64(self.vertex).out_str("\n");
                }
                _ => {}
            }
        } else {
            match self.loop_type {
                0 => {
                    db.write_long(self.loop_type as i64);
                    db.write_long(self.num_eu as i64);
                    db.write_long(self.start_eu as i64);
                }
                1 => {
                    db.write_long(self.loop_type as i64);
                    db.write_long(self.vertex as i64);
                }
                _ => {}
            }
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&self.topology.attributes, db);
            write_indexed_attributes(&self.lu_upper_attributes, db);
            write_indexed_attributes(&self.lu_lower_attributes, db);
        }
    }
}

impl EuData {
    /// `SmEUData::WriteToDB`: the topology data; for `version > 31` the edge use's
    /// flags and user indices again (where the reader expects the mate's: see the
    /// module notes); the edge use by type (edge: edge, loop, orientation, UV curve,
    /// next and mate's next edge use; vertex and vertex at pole: vertex, loop,
    /// orientation, UV curve; wire edge: edge, shell, orientation, UV curve), then its
    /// and its mate's attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        let t = &self.topology;
        t.write_to_db(version, db);
        let ascii = is_ascii(db);
        if version > 31 {
            if ascii {
                db.out_str(" ").out_u64(t.flags);
                db.out_str(" ").out_u64(t.user_index1);
                db.out_str(" ").out_u64(t.user_index2);
            } else {
                db.write_long(t.flags as i64);
                db.write_long(t.user_index1 as i64);
                db.write_long(t.user_index2 as i64);
            }
        }
        let orientation = i64::from(self.orientation);
        if ascii {
            let (a, b) = match self.eu_type {
                0 | 2 => (
                    self.edge,
                    if self.eu_type == 0 {
                        self.loop_
                    } else {
                        self.shell
                    },
                ),
                _ => (self.vertex, self.loop_),
            };
            if self.eu_type <= 3 {
                db.out_str(" ").out_u64(self.eu_type);
                db.out_str(" ").out_u64(a);
                db.out_str(" ").out_u64(b);
                db.out_str(" ").out_i64(orientation);
                db.out_str(" ").out_u64(self.uv_curve);
                if self.eu_type == 0 {
                    db.out_str(" ").out_i64(self.next_eu);
                    db.out_str(" ").out_i64(self.mate_next_eu);
                }
                db.out_str("\n");
            }
        } else {
            db.write_long(self.eu_type as i64);
            match self.eu_type {
                0 => {
                    db.write_long(self.edge as i64);
                    db.write_long(self.loop_ as i64);
                }
                1 | 3 => {
                    db.write_long(self.vertex as i64);
                    db.write_long(self.loop_ as i64);
                }
                2 => {
                    db.write_long(self.edge as i64);
                    db.write_long(self.shell as i64);
                }
                _ => {}
            }
            db.write_boolean(self.orientation);
            db.write_long(self.uv_curve as i64);
            if self.eu_type == 0 {
                db.write_long(self.next_eu);
                db.write_long(self.mate_next_eu);
            }
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&t.attributes, db);
            write_indexed_attributes(&self.mate_attributes, db);
        }
    }
}

impl EdgeData {
    /// `SmEdgeData::WriteToDB`: the topology data, the zone tolerance, curve, start
    /// and end vertex, primary edge use and the interval, then the attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        self.topology.write_to_db(version, db);
        let (lo, hi) = (self.interval.min(), self.interval.max());
        if is_ascii(db) {
            db.out_str(" ").out_f64(self.zone_tol_3d);
            db.out_str(" ").out_u64(self.curve);
            db.out_str(" ").out_u64(self.start_vertex);
            db.out_str(" ").out_u64(self.end_vertex);
            db.out_str(" ").out_i64(self.prim_eu);
            db.out_str(" ").out_f64(lo);
            db.out_str(" ").out_f64(hi);
            db.out_str("\n");
        } else {
            db.write_double(self.zone_tol_3d);
            db.write_long(self.curve as i64);
            db.write_long(self.start_vertex as i64);
            db.write_long(self.end_vertex as i64);
            db.write_long(self.prim_eu);
            db.write_doubles(&[lo, hi]);
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&self.topology.attributes, db);
        }
    }
}

impl VertexData {
    /// `SmVertexData::WriteToDB`: the topology data, the zone tolerance and the point,
    /// then the attribute indices.
    pub fn write_to_db(&self, version: u64, db: &mut DatabaseWriter, persist_attribs: bool) {
        self.topology.write_to_db(version, db);
        let p = self.point;
        if is_ascii(db) {
            db.out_str(" ").out_f64(self.zone_tol_3d);
            db.out_str(" ").out_f64(p.x);
            db.out_str(" ").out_f64(p.y);
            db.out_str(" ").out_f64(p.z);
            db.out_str("\n");
        } else {
            db.write_doubles(&[self.zone_tol_3d, p.x, p.y, p.z]);
        }
        if with_attributes(version, persist_attribs) {
            write_indexed_attributes(&self.topology.attributes, db);
        }
    }
}

/// `SM_CAST_PTR(SmBSplineCurve, pCrv)`: the B-spline part of the kinds derived from
/// `SmBSplineCurve`, `None` for the others.
const fn bspline_curve_part(c: &Curve) -> Option<&BSplineCurve> {
    match c.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        CurveKind::Hermite(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::Conic(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => None,
    }
}

/// [`bspline_curve_part`], mutable.
fn bspline_curve_part_mut(c: &mut Curve) -> Option<&mut BSplineCurve> {
    match c.kind_mut() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => Some(&mut l.base),
        CurveKind::Ellipse(e) => Some(e.base_mut()),
        CurveKind::Hyperbola(h) => Some(h.base_mut()),
        CurveKind::Parabola(p) => Some(p.base_mut()),
        _ => None,
    }
}

/// `SM_CAST_PTR(SmBSplineSurface, pSrf)`: the B-spline part of the kinds derived from
/// `SmBSplineSurface` (a plane's made with its NURB when missing), `None` for the
/// others.
fn bspline_surface_part(s: &Surface) -> Result<Option<Cow<'_, BSplineSurface>>> {
    Ok(match s.kind() {
        SurfaceKind::BSpline(b) => Some(Cow::Borrowed(b)),
        SurfaceKind::Plane(p) => Some(p.base_with_nurb()?),
        SurfaceKind::Cone(c) => Some(Cow::Borrowed(c.rev().base())),
        SurfaceKind::Sphere(sp) => Some(Cow::Borrowed(sp.base().base())),
        SurfaceKind::Torus(t) => Some(Cow::Borrowed(t.base().base())),
        SurfaceKind::SurfOfRevolution(r) => Some(Cow::Borrowed(r.base())),
        SurfaceKind::SurfOfExtrusion(e) => Some(Cow::Borrowed(e.base())),
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => None,
    })
}

/// `SmBrepData::WriteCurveToDB` (native formats): for `version > 32` the curve's type
/// header and its own format; up to version 32 the B-spline type header and the
/// curve's B-spline part. With `version > 30` and `persist_attribs` the curve's
/// attribute indices follow (its attributes are added to `attrs`).
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` up to version 32 for a curve without B-spline part; those
/// of [`write_curve`].
pub fn write_curve_to_db(
    c: &Keyed<Curve>,
    version: u64,
    attrs: &mut AttributeTable<'_>,
    db: &mut DatabaseWriter,
    persist_attribs: bool,
) -> Result<()> {
    let curve = &c.object;
    if version > 32 {
        // preface Curve Write with Type and Dim data
        db.write_type(curve.type_code(), Some(curve.dim()));
        write_curve(curve, db, version)?;
    } else {
        // database versions 32 and before - limited to BSplineCurves
        db.write_type(crate::curve::curve::p2::BSPLINE_CURVE_TYPE, None);
        let b = bspline_curve_part(curve).ok_or_else(|| SolidError::null_pointer(OP_CURVE))?;
        b.write_to_db(db, version)?;
    }

    // gather and output curve attributes
    if with_attributes(version, persist_attribs) {
        let indices = extract_attributes(attrs, c.key);
        write_indexed_attributes(&indices, db);
    }
    Ok(())
}

/// `SmBrepData::WriteSurfaceToDB` (native formats): for `version > 32` the surface's
/// type header and its own format; up to version 32 an offset surface's type header,
/// offset distance and (extended) base's B-spline part, or the B-spline type header
/// and the surface's B-spline part. With `version > 30` and `persist_attribs` the
/// surface's attribute indices follow.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` up to version 32 for a surface (or offset base) without
/// B-spline part; those of [`write_surface`].
pub fn write_surface_to_db(
    s: &Keyed<Surface>,
    version: u64,
    attrs: &mut AttributeTable<'_>,
    db: &mut DatabaseWriter,
    persist_attribs: bool,
) -> Result<()> {
    use crate::surface::surface::p2::{B_SPLINE_SURFACE_TYPE, OFFSET_SURFACE_TYPE};
    let surface = &s.object;
    if version > 32 {
        // preface Surface Write with Type data
        db.write_type(surface.type_code(), None);
        write_surface(surface, db, version)?;
    } else if let SurfaceKind::OffsetSurface(off) = surface.kind() {
        db.write_type(OFFSET_SURFACE_TYPE, None);
        let distance = off.offset_distance();
        if is_ascii(db) {
            db.out_f64(distance).out_str(" Offset Distance \n");
        } else {
            db.write_double(distance);
        }
        let base = off.extended_base_surface().unwrap_or(off.base_surface());
        let b = bspline_surface_part(base)?.ok_or_else(|| SolidError::null_pointer(OP_SURFACE))?;
        b.write_to_db(db, version)?;
    } else {
        db.write_type(B_SPLINE_SURFACE_TYPE, None);
        let b =
            bspline_surface_part(surface)?.ok_or_else(|| SolidError::null_pointer(OP_SURFACE))?;
        b.write_to_db(db, version)?;
    }

    // add surface attributes to accumulation list and output attribute indices
    if with_attributes(version, persist_attribs) {
        let indices = extract_attributes(attrs, s.key);
        write_indexed_attributes(&indices, db);
    }
    Ok(())
}

/// The ASCII section header `"\n//<title>\n<count> <what>\n"` or the binary count.
fn section(db: &mut DatabaseWriter, title: &str, count: usize, what: &str) {
    if is_ascii(db) {
        db.out_str("\n//").out_str(title).out_str("\n");
        db.out_u64(count as u64).out_str(what);
    } else {
        db.write_long(count as i64);
    }
}

/// An ASCII comment line (nothing in binary).
fn comment(db: &mut DatabaseWriter, text: &str) {
    if is_ascii(db) {
        db.out_str(text);
    }
}

/// The ASCII record index (nothing in binary).
fn index(db: &mut DatabaseWriter, i: usize) {
    if is_ascii(db) {
        db.out_u64(i as u64);
    }
}

/// A composite face or edge section: `[count, n0, items.., n1, items.., ...]`.
fn write_composites(
    db: &mut DatabaseWriter,
    list: &[u64],
    title: &str,
    what: &str,
    columns: (&str, &str),
    version: u64,
) {
    let count = list.first().copied().unwrap_or(0);
    section(db, title, count as usize, what);
    if count == 0 {
        return;
    }
    comment(db, if version > 30 { columns.0 } else { columns.1 });
    let mut items = list.iter().skip(1).copied();
    for i in 0..count {
        let n = items.next().unwrap_or(0);
        if is_ascii(db) {
            db.out_u64(i).out_str(" ").out_u64(n);
            for _ in 0..n {
                db.out_str(" ").out_u64(items.next().unwrap_or(0));
            }
            db.out_str("\n");
        } else {
            db.write_long(n as i64);
            for _ in 0..n {
                db.write_long(items.next().unwrap_or(0) as i64);
            }
        }
    }
}

/// `GetOrCreateGwNurbPointer` of a surface about to be written: an analytic kind
/// without its NURB yet (cone, sphere, torus, surface of revolution or extrusion: the
/// recogniser's lazy surfaces) makes it with its virtual `MakeNurb`, status ignored as
/// upstream (a NURB that cannot be made stays missing and the writer reports it), and
/// a lazy line generator makes its own. The same make a `const` query of the surface runs;
/// here on the B-rep data's own copy. A surface (and generator) that has its NURB is
/// left as it is, so its written bytes do not change.
///
/// # Errors
///
/// [`SolidError::Unported`] from `MakeNurb`.
pub fn make_missing_nurb(k: &Kernel, surface: &mut Surface) -> Result<()> {
    let missing = match surface.kind() {
        SurfaceKind::Cone(s) => s.rev().base().gw_nurb().is_none(),
        SurfaceKind::Sphere(s) => s.base().base().gw_nurb().is_none(),
        SurfaceKind::Torus(s) => s.base().base().gw_nurb().is_none(),
        SurfaceKind::SurfOfRevolution(s) => s.base().gw_nurb().is_none(),
        SurfaceKind::SurfOfExtrusion(s) => s.base().gw_nurb().is_none(),
        _ => false,
    };
    if missing {
        match surface.kind_mut() {
            SurfaceKind::Cone(s) => ignore_status(s.make_nurb(k))?,
            SurfaceKind::Sphere(s) => ignore_status(s.make_nurb(k))?,
            SurfaceKind::Torus(s) => ignore_status(s.make_nurb(k))?,
            SurfaceKind::SurfOfRevolution(s) => ignore_status(s.make_nurb(k))?,
            SurfaceKind::SurfOfExtrusion(s) => ignore_status(s.make_nurb(k))?,
            _ => {}
        }
    }
    // the generator is written too: a lazy line generator (SmLine is an
    // SmBSplineCurve) makes its NURB the same way
    let lazy_line =
        |c: &Curve| matches!(c.kind(), CurveKind::Line(l) if l.base().gw_nurb().is_none());
    let has_lazy_line = match surface.kind() {
        SurfaceKind::Cone(s) => s.rev.gen_curve.as_ref().is_some_and(lazy_line),
        SurfaceKind::Sphere(s) => s.base.gen_curve.as_ref().is_some_and(lazy_line),
        SurfaceKind::Torus(s) => s.base.gen_curve.as_ref().is_some_and(lazy_line),
        SurfaceKind::SurfOfRevolution(s) => s.gen_curve.as_ref().is_some_and(lazy_line),
        SurfaceKind::SurfOfExtrusion(s) => s.gen_curve.as_ref().is_some_and(lazy_line),
        _ => false,
    };
    if !has_lazy_line {
        return Ok(());
    }
    let gen = match surface.kind_mut() {
        SurfaceKind::Cone(s) => s.rev.gen_curve.as_mut(),
        SurfaceKind::Sphere(s) => s.base.gen_curve.as_mut(),
        SurfaceKind::Torus(s) => s.base.gen_curve.as_mut(),
        SurfaceKind::SurfOfRevolution(s) => s.gen_curve.as_mut(),
        SurfaceKind::SurfOfExtrusion(s) => s.gen_curve.as_mut(),
        _ => None,
    };
    if let Some(CurveKind::Line(l)) = gen.map(Curve::kind_mut) {
        ignore_status(l.make_nurb())?;
    }
    Ok(())
}

impl BrepData {
    /// [`Self::write_to_file`] after [`make_missing_nurb`] on every surface: upstream's
    /// `SmBSplineSurface::WriteToDB` reaches the NURB through
    /// `GetOrCreateGwNurbPointer`, so a lazy surface is written with the NURB it makes.
    ///
    /// # Errors
    ///
    /// Those of [`make_missing_nurb`] and [`Self::write_to_file`].
    pub fn write_to_file_with_nurbs(&mut self, k: &Kernel, file_type: FileType) -> Result<Vec<u8>> {
        for s in &mut self.surfaces {
            make_missing_nurb(k, &mut s.object)?;
        }
        self.write_to_file(file_type)
    }

    /// `SmBrepData::WriteToFile(name, eType, TRUE)`: the `Version` and
    /// `Healer Version` header ([`DATABASE_VERSION`], `SM_HEALER_VERSION`; binary:
    /// `'V'`, a long, `'H'`, a long), then [`Self::write_to_db`] without attributes.
    /// Returns the file's contents.
    ///
    /// # Errors
    ///
    /// Those of [`Self::write_to_db`].
    pub fn write_to_file(&mut self, file_type: FileType) -> Result<Vec<u8>> {
        let mut db = DatabaseWriter::open_for_write(file_type);

        // virtual preWrite for derived classes - base class does nothing
        db.begin_writing();

        // starting with version 38 write out a header for this file
        if file_type == FileType::Ascii {
            db.out_str("Version ").out_u64(DATABASE_VERSION);
            db.out_str("\n");
            db.out_str("Healer Version ").out_u64(HEALER_VERSION);
            db.out_str("\n");
        } else {
            db.write_char(b'V');
            db.write_long(DATABASE_VERSION as i64);
            db.write_char(b'H');
            db.write_long(HEALER_VERSION as i64);
        }

        // output BrepData to stream, without attributes
        let store = AttributeStore::new();
        let mut all = Vec::new();
        let mut map = BTreeSet::new();
        let mut attrs = AttributeTable {
            store: &store,
            all: &mut all,
            map: &mut map,
        };
        self.write_to_db(DATABASE_VERSION, &mut attrs, &mut db, false)?;

        // virtual postWrite for derived classes - base class does nothing
        db.end_writing();
        Ok(db.into_bytes())
    }

    /// `SmBrepData::WriteToDB` (native formats): the B-rep properties (attribute
    /// indices or colour, zone tolerance, manifold flag, the obsolete composites flag
    /// `0`, region count and start region), the region, shell, composite face, face
    /// use, face, loop, composite edge, edge use, edge and vertex sections, then the
    /// UV curves (each made 2D first when all its control points have `z == 0`), the
    /// 3D curves and the surfaces. ASCII streams carry the comment lines the reader
    /// skips.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a UV curve without NURB (upstream dereferences it),
    /// [`SolidError::InvalidInput`] for one that is not a B-spline kind (upstream
    /// casts it); those of [`write_curve_to_db`] and [`write_surface_to_db`].
    pub fn write_to_db(
        &mut self,
        version: u64,
        attrs: &mut AttributeTable<'_>,
        db: &mut DatabaseWriter,
        persist_attribs: bool,
    ) -> Result<()> {
        let ascii = is_ascii(db);
        let new_tol = version > 43;

        // output brep properties: color, tolerance, manifold flag, composites flag,
        // numRegions, startRegion
        let make_composites: i32 = 0;
        if ascii {
            db.out_str("//Brep Starts\n");
            if with_attributes(version, persist_attribs) {
                write_indexed_attributes(&self.attributes, db);
            } else {
                db.out_reals(&[self.color.x, self.color.y, self.color.z]);
                db.out_str(" - Color\n");
            }
            // (the SM_USE_NEWTOL lines of version > 43 are compiled out of the reference)
            if !new_tol {
                db.out_f64(self.zone_tol_3d).out_str(" - Brep ZoneTol3d\n");
            }
            db.out_i64(self.manifold.into());
            db.out_str(" - is Manifold:[1 = is, 0 = isn't]\n");
            db.out_i64(make_composites.into());
            db.out_str(" - has Composites:[1 = does, 0 = doesn't]\n");
            db.out_str("\n//Region Data\n");
            db.out_u64(self.regions.len() as u64);
            db.out_str(" Regions In Brep\n");
            db.out_u64(self.start_region).out_str(" - Start Region\n");
            if version > 30 {
                db.out_str(
                    "//Index, Flags, UserId1, UserId2, Number Of Shells, First Shell, \
                     Is void Region\n",
                );
            } else {
                db.out_str("//Index, Number Of Shells, First Shell, Is void Region\n");
            }
        } else {
            if with_attributes(version, persist_attribs) {
                write_indexed_attributes(&self.attributes, db);
            } else {
                db.write_doubles(&[self.color.x, self.color.y, self.color.z]);
            }
            if !new_tol {
                db.write_double(self.zone_tol_3d);
            }
            db.write_boolean(self.manifold);
            db.write_boolean(make_composites);
            db.write_long(self.num_regions as i64);
            db.write_long(self.start_region as i64);
        }

        // Region Data
        for (i, r) in self.regions.iter().enumerate() {
            index(db, i);
            r.write_to_db(version, db, persist_attribs);
        }

        // Shells
        let n = self.shells.len();
        section(db, "Shell Data", n, " Shells In Brep\n");
        if version > 30 {
            comment(
                db,
                "//Index, Flags, UserId1, UserId2, 0(Faceuse), Orientation, \
                 Number Of Faceuses, Start Faceuse\n",
            );
            comment(db, "//Index, Flags, UserId1, UserId2, 1(WireEdge), Edge\n");
            comment(db, "//Index, Flags, UserId1, UserId2, 2(Vertex), Vertex\n");
        } else {
            comment(
                db,
                "//Index, 0(Faceuse), Orientation, Number Of Faceuses, Start Faceuse\n",
            );
            comment(db, "//Index, 1(WireEdge), Edge\n");
            comment(db, "//Index, 2(Vertex), Vertex\n");
        }
        for (i, s) in self.shells.iter().enumerate() {
            index(db, i);
            s.write_to_db(version, db, persist_attribs);
        }

        // Composite Faces
        write_composites(
            db,
            &self.cfaces,
            "Composite Face Data",
            " Composite Faces In Brep\n",
            (
                "//Index, Flags, UserId1, UserId2, Face Count, Face1, Face2, ...\n",
                "//Index, Face Count, Face1, Face2, ...\n",
            ),
            version,
        );

        // Faceuse Data
        let n = self.faceuses.len();
        section(db, "Faceuse Data", n, " Faceuses In Brep\n");
        if version > 30 {
            comment(db, "//Index, Flags, UserId1, UserId2, Face, Orientation\n");
        } else {
            comment(db, "//Index, Face, Orientation\n");
        }
        for (i, f) in self.faceuses.iter().enumerate() {
            index(db, i);
            f.write_to_db(version, db, persist_attribs);
        }

        // Face Data
        let n = self.faces.len();
        section(db, "Face Data", n, " Faces In Brep\n");
        if version > 30 {
            comment(
                db,
                "//Index, Flags, UserId1, UserId2, Tol, Surface, Outer Loop, # of Loops, \
                 Naturally Trimmed\n",
            );
        } else {
            comment(
                db,
                "//Index, Tol, Surface, Outer Loop, # of Loops, Naturally Trimmed\n",
            );
        }
        comment(db, "//UMin, UMax, VMin, VMax\n");
        for (i, f) in self.faces.iter().enumerate() {
            index(db, i);
            f.write_to_db(version, db, persist_attribs);
        }

        // Loop Data
        let n = self.loops.len();
        section(db, "Loop Data", n, " Loops In Brep\n");
        if version > 30 {
            comment(
                db,
                "//Index, Flags, UserId1, UserId2, 0(Edgeuse), # Of Edgeuses, Start Edgeuse\n",
            );
            comment(db, "//Index, Flags, UserId1, UserId2, 1(Vertex), Vertex\n");
        } else {
            comment(db, "//Index, 0(Edgeuse), # Of Edgeuses, Start Edgeuse\n");
            comment(db, "//Index, 1(Vertex), Vertex\n");
        }
        for (i, l) in self.loops.iter().enumerate() {
            index(db, i);
            l.write_to_db(version, db, persist_attribs);
        }

        // Composite Edges
        write_composites(
            db,
            &self.cedges,
            "Composite Edge Data",
            " Composite Edges In Brep\n",
            (
                "//Index, Flags, UserId1, UserId2, Edge Count, Edge1, Edge2, ...\n",
                "//Index, Edge Count, Edge1, Edge2, ...\n",
            ),
            version,
        );

        // Edgeuse Data
        let n = self.edgeuses.len();
        section(db, "Edgeuse Data", n, " Edgeuses In Brep\n");
        if version > 30 {
            const HEAD: &str = "//Index, Flags, UserId1, UserId2, MateFlags, MateUserId1, \
                                MateUserId2, ";
            for tail in [
                "0(Edge), Edge, Loop, Orientation, UV Curve, Next EU, Mate Next EU\n",
                "1(Vertex), Vertex, Loop, Orientation, UV Curve\n",
                "2(Wire Edge), Edge, Shell, Orientation, UV Curve\n",
                "3(Vertex At Pole), Vertex, Loop, Orientation, UV Curve\n",
            ] {
                comment(db, HEAD);
                comment(db, tail);
            }
        } else {
            comment(
                db,
                "//Index, 0(Edge), Edge, Loop, Orientation, UV Curve, Next EU, Mate Next EU\n",
            );
            comment(
                db,
                "//Index, 1(Vertex), Vertex, Loop, Orientation, UV Curve\n",
            );
            comment(
                db,
                "//Index, 2(Wire Edge), Edge, Shell, Orientation, UV Curve\n",
            );
            comment(
                db,
                "//Index, 3(Vertex At Pole), Vertex, Loop, Orientation, UV Curve\n",
            );
        }
        for (i, e) in self.edgeuses.iter().enumerate() {
            index(db, i);
            e.write_to_db(version, db, persist_attribs);
        }

        // Edge Data
        let n = self.edges.len();
        section(db, "Edge Data", n, " Edges In Brep\n");
        if version > 30 {
            comment(
                db,
                "//Index, Flags, UserId1, UserId2, Tol, Curve, Start Vertex, End Vertex, \
                 Primary Edgeuse, Param Min, Param Max\n",
            );
        } else {
            comment(
                db,
                "//Index, Tol, Curve, Start Vertex, End Vertex, Primary Edgeuse, Param Min, \
                 Param Max\n",
            );
        }
        for (i, e) in self.edges.iter().enumerate() {
            index(db, i);
            e.write_to_db(version, db, persist_attribs);
        }

        // Vertex Data
        let n = self.vertices.len();
        section(db, "Vertex Data", n, " Vertices In Brep\n");
        if version > 30 {
            comment(db, "//Index, Flags, UserId1, UserId2, Tol, X, Y, Z\n");
        } else {
            comment(db, "//Index, Tol, X, Y, Z\n");
        }
        for (i, v) in self.vertices.iter().enumerate() {
            index(db, i);
            v.write_to_db(version, db, persist_attribs);
        }

        // UV-Curve Data
        let n = self.uv_curves.len();
        section(db, " UV-Curve Data", n, " UV-Curves In Brep\n");
        for (i, c) in self.uv_curves.iter_mut().enumerate() {
            if ascii {
                db.out_u64(i as u64).out_str(" - UV-Curve:\n");
            }
            // convert all 2d UV curves into x, y, NL_NOZ
            make_uv_curve_2d(&mut c.object)?;
            write_curve_to_db(c, version, attrs, db, persist_attribs)?;
        }

        // 3D-Curve Data
        let n = self.curves_3d.len();
        section(db, " 3D-Curve Data", n, " 3D-Curves In Brep\n");
        for (i, c) in self.curves_3d.iter().enumerate() {
            if ascii {
                db.out_u64(i as u64).out_str(" - 3D-Curve\n");
            }
            write_curve_to_db(c, version, attrs, db, persist_attribs)?;
        }

        // Surface Data
        let n = self.surfaces.len();
        section(db, "Surface Data", n, " Surfaces In Brep\n");
        for (i, s) in self.surfaces.iter().enumerate() {
            if ascii {
                db.out_u64(i as u64).out_str(" - Surface\n");
            }
            write_surface_to_db(s, version, attrs, db, persist_attribs)?;
        }
        Ok(())
    }
}

/// The UV curve conversion of `WriteToDB`: a NURB whose control points are 3D with
/// every `z == 0` is made 2D (`z = NL_NOZ`) in place.
fn make_uv_curve_2d(c: &mut Curve) -> Result<()> {
    let b = bspline_curve_part_mut(c)
        .ok_or_else(|| SolidError::invalid_input(OP_UV, "UV curve is not a B-spline curve"))?;
    let cur = b
        .nurb
        .as_mut()
        .ok_or_else(|| SolidError::null_pointer(OP_UV))?;
    let cpts = cur.cpts_mut();
    let first_z = cpts.first().map_or(NOZ, |p| p.z);
    if first_z != NOZ && cpts.iter().all(|p| p.z == 0.0) {
        for p in cpts.iter_mut() {
            p.z = NOZ;
        }
    }
    Ok(())
}

// ---- FromBrep -----------------------------------------------------------------------

/// The identities and records [`BrepData::from_brep`] reads (upstream: the objects'
/// addresses, their `SmTopology` members and their attribute lists).
pub struct FromBrepEnv<'a> {
    /// The attribute lists of the B-rep's objects (by identity).
    pub attributes: &'a AttributeStore,
    /// The identity of each topology entity (the B-rep's own is [`Brep::key`]).
    pub key_of: &'a dyn Fn(EntityRef) -> ObjectRef,
    /// The `SmTopology` records (flags, user indices) by identity; a missing record is
    /// a fresh `SmTopology()`.
    pub records: &'a crate::topology::topology::NotifyRecords,
    /// The identity of an edge's curve.
    pub curve_key: &'a dyn Fn(EdgeId) -> ObjectKey,
    /// The identity of the UV trim curve held by an edge use.
    pub uv_curve_key: &'a dyn Fn(EdgeuseId) -> ObjectKey,
    /// The identity of a face's surface.
    pub surface_key: &'a dyn Fn(FaceId) -> ObjectKey,
}

/// `SmBrep::WriteToFile(name, eType, TRUE, FALSE, 0.0)`: `b` flattened into a
/// temporary `SmBrepData` ([`BrepData::from_brep`]) and written as a new file in
/// native formats ([`BrepData::write_to_file_with_nurbs`]). Returns the file's
/// contents. Upstream ignores `FromBrep`'s status; its failures here are the null
/// dereferences upstream would crash on, so they are returned.
///
/// # Errors
///
/// Those of [`BrepData::from_brep`] and [`BrepData::write_to_file_with_nurbs`].
pub fn write_brep_to_file(
    k: &Kernel,
    b: &Brep,
    env: &FromBrepEnv<'_>,
    file_type: FileType,
) -> Result<Vec<u8>> {
    // (the attribute pointers FromBrep collects; the file is written without them)
    let mut attributes = Vec::new();
    let mut data = BrepData::from_brep(b, env, &mut attributes)?;
    data.write_to_file_with_nurbs(k, file_type)
}

/// `SM_ERR_NULL_POINTER` of [`BrepData::from_brep`].
fn null() -> SolidError {
    SolidError::null_pointer(OP_FROM)
}

/// `sm_GetMap`: the record index of `e`, `SM_NO_OBJECT` when it is not mapped (or
/// null).
fn get_map(map: &BTreeMap<EntityRef, u64>, e: Option<EntityRef>) -> u64 {
    e.and_then(|e| map.get(&e).copied()).unwrap_or(NO_OBJECT)
}

/// `sm_GetEdgeuseIndex`: `-(index + 1)` for a mate of a recorded edge use, else the
/// edge use's own index (`SM_NO_OBJECT` when unmapped).
fn edgeuse_index(
    map: &BTreeMap<EntityRef, u64>,
    mate_map: &BTreeMap<EdgeuseId, u64>,
    eu: EdgeuseId,
) -> i64 {
    match mate_map.get(&eu) {
        Some(&v) if v != 0 => -(v as i64),
        _ => get_map(map, Some(EntityRef::Edgeuse(eu))) as i64,
    }
}

/// The `SmTopology` record of `e` (a fresh one when there is none).
fn record_of(env: &FromBrepEnv<'_>, e: EntityRef) -> Topology {
    let key = (env.key_of)(e).key;
    env.records.get(&key).copied().unwrap_or_default()
}

/// "for backward compatibility: map `SM_UNDEF_ULONG` flag and index values to zero".
const fn defined(v: u64) -> u64 {
    if v == UNDEF_ULONG {
        0
    } else {
        v
    }
}

/// The topology part of a record: flags and user indices mapped by [`defined`], and
/// the attribute indices.
fn topology_data(
    env: &FromBrepEnv<'_>,
    table: &mut AttributeTable<'_>,
    e: EntityRef,
) -> TopologyData {
    let t = record_of(env, e);
    let mut data = TopologyData::zeroed();
    data.flags = defined(t.all_flags());
    data.user_index1 = defined(t.user_index1());
    data.user_index2 = defined(t.user_index2());
    data.attributes = extract_attributes(table, (env.key_of)(e).key);
    data
}

fn same(o: OrientType) -> i32 {
    i32::from(o == OrientType::Same)
}

impl BrepData {
    /// `SmBrepData::FromBrep`: flattens `b` into records indexed as upstream does: the
    /// regions (infinite region first), the vertices, the wire edges' primary edge
    /// uses, the shells, the face uses, the faces' surfaces (those owned by their
    /// faces), the faces, the loops (with the edge uses of each loop's upper loop use),
    /// the edge uses (next and mate's next as `sm_GetEdgeuseIndex` codes; UV trim
    /// curves gathered), then the edges and their curves. Attributes are indexed into
    /// `all_attributes` in that order (under a map of this call). The colour is the
    /// B-rep's `SM_AI_COLOR` vector attribute, red without one.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for the null objects upstream dereferences (see the module
    /// notes); [`SolidError::InvalidInput`] for a vertex loop use without vertex; those
    /// of the topology queries and the geometry copies.
    pub fn from_brep(
        b: &Brep,
        env: &FromBrepEnv<'_>,
        all_attributes: &mut Vec<AttributeHandle>,
    ) -> Result<Self> {
        let mut attr_map = BTreeSet::new();
        let mut table = AttributeTable {
            store: env.attributes,
            all: all_attributes,
            map: &mut attr_map,
        };
        // map ObjectPointers to indices; [Edge->primaryEdgeUse->Mate, edgeuseIndex+1]
        let mut map: BTreeMap<EntityRef, u64> = BTreeMap::new();
        let mut mate_map: BTreeMap<EdgeuseId, u64> = BTreeMap::new();

        // init output
        let mut bd = Self::new(0, DataSourceType::Unknown);

        // set Brep properties
        bd.data_source_type = DataSourceType::SmLib;
        bd.manifold = i32::from(is_manifold_solid(b)?);
        bd.zone_tol_3d = b.tolerance();
        bd.start_region = 0;

        // Set the brep color if it exists. Otherwise default to red
        let color = env
            .attributes
            .find_attribute(b.key(), ids::COLOR)
            .and_then(|h| env.attributes.get(h))
            .and_then(crate::geom::attribute::Attribute::vector3d_value);
        bd.color = color.unwrap_or(Vec3::new(1.0, 0.0, 0.0));

        // get Brep attributes (not attributes of objects in the Brep structure)
        bd.attributes = extract_attributes(&mut table, b.key());

        // Regions: place infinite region at head of list
        let mut regions = bl::regions(b, None)?;
        let infinite = b.infinite_region();
        if regions.len() > 1 && regions.first().copied() != infinite {
            if let Some(i) = regions.iter().skip(1).position(|&r| Some(r) == infinite) {
                let r = regions.remove(i + 1);
                regions.insert(0, r);
            }
        }
        bd.num_regions = regions.len() as u64;
        let mut all_shells = Vec::new();
        for (i, &r) in regions.iter().enumerate() {
            map.insert(EntityRef::Region(r), i as u64);
            let region = b.region(r)?;
            let mut rd = RegionData::zeroed();
            rd.start_shell = all_shells.len() as u64;
            rd.num_shells = region.shells.len() as u64;
            rd.is_void = region.is_void;
            rd.topology = topology_data(env, &mut table, EntityRef::Region(r));
            all_shells.extend(region.shells.iter().copied());
            bd.regions.push(rd);
        }

        // Vertices
        for (i, v) in bl::vertices(b, None)?.into_iter().enumerate() {
            map.insert(EntityRef::Vertex(v), i as u64);
            let vertex = b.vertex(v)?;
            let mut vd = VertexData::zeroed();
            vd.zone_tol_3d = vertex.tolerance;
            vd.point = vertex.point;
            vd.topology = topology_data(env, &mut table, EntityRef::Vertex(v));
            bd.vertices.push(vd);
        }

        // Edges: the wire edges' primary edge uses come first
        let edges = bl::edges(b, None)?;
        let mut all_edgeuses: Vec<EdgeuseId> = Vec::new();
        for (i, &e) in edges.iter().enumerate() {
            map.insert(EntityRef::Edge(e), i as u64);
            if edge::is_wire(b, e)? {
                let prim = eu1::primary_edgeuse(b, e)?.ok_or_else(null)?;
                let n = all_edgeuses.len() as u64;
                map.insert(EntityRef::Edgeuse(prim), n);
                if let Some(m) = eu1::mate(b, prim)? {
                    mate_map.insert(m, n + 1);
                }
                all_edgeuses.push(prim);
            }
        }

        // Shells
        let mut all_faceuses = Vec::new();
        for (i, &s) in all_shells.iter().enumerate() {
            map.insert(EntityRef::Shell(s), i as u64);
            let shell = b.shell(s)?;
            let mut sd = ShellData::zeroed();
            if shell.is_vertex_shell() {
                sd.shell_type = 2;
                sd.vertex = get_map(&map, shell.vertex(b)?.map(EntityRef::Vertex));
            } else if shell.is_wire_shell() {
                sd.shell_type = 1;
                sd.edge = get_map(&map, shell.wire_edge(b)?.map(EntityRef::Edge));
            } else {
                // Normal (Faceuse)
                let faceuses = shell.get_faceuses()?;
                sd.shell_type = 0;
                sd.faceuse_start = all_faceuses.len() as u64;
                sd.num_faceuses = faceuses.len() as u64;
                all_faceuses.extend(faceuses);
            }
            sd.topology = topology_data(env, &mut table, EntityRef::Shell(s));
            bd.shells.push(sd);
        }

        // Faces (mapped before the face uses refer to them)
        let faces = bl::faces(b, None)?;
        for (i, &f) in faces.iter().enumerate() {
            map.insert(EntityRef::Face(f), i as u64);
        }

        // Faceuses
        for (i, &fu) in all_faceuses.iter().enumerate() {
            map.insert(EntityRef::Faceuse(fu), i as u64);
            let mut fd = FaceuseData::zeroed();
            fd.orientation = same(eu1::faceuse_orientation(b, fu)?);
            fd.face = get_map(&map, eu1::faceuse_face(b, fu)?.map(EntityRef::Face));
            fd.topology = topology_data(env, &mut table, EntityRef::Faceuse(fu));
            bd.faceuses.push(fd);
        }

        // Surfaces owned by their faces
        let face_ref = |f: FaceId| (env.key_of)(EntityRef::Face(f));
        let mut surface_index: BTreeMap<FaceId, u64> = BTreeMap::new();
        for f in surface_owners(b, None, &face_ref)? {
            let surface = b.face(f)?.surface.as_ref().ok_or_else(null)?;
            surface_index.insert(f, bd.surfaces.len() as u64);
            bd.surfaces.push(Keyed {
                key: (env.surface_key)(f),
                object: surface.clone_for_evaluation()?,
            });
        }

        // Faces
        let mut all_loops = Vec::new();
        for &f in &faces {
            let face_rec = b.face(f)?;
            let loops = face::p2::loops(b, f, false, None)?;
            let mut fd = FaceData::zeroed();
            fd.surface = surface_index.get(&f).copied().unwrap_or(NO_OBJECT);
            fd.zone_tol_3d = face_rec.tolerance;
            fd.rectangular_trim = face_rec.rectangular_trim;
            fd.uv_domain = face_rec.uv_domain;
            fd.start_loop = all_loops.len() as u64;
            fd.num_loops = loops.len() as u64;
            fd.topology = topology_data(env, &mut table, EntityRef::Face(f));
            all_loops.extend(loops);
            bd.faces.push(fd);
        }

        // Loops
        for (i, &l) in all_loops.iter().enumerate() {
            map.insert(EntityRef::Loop(l), i as u64);

            // set pLUT1 = topFU->loopuse (pLUT2 stays the other loop use)
            let (lu1, lu2) = loopuse::loop_loopuses(b, l)?;
            let lu2 = lu2.ok_or_else(null)?;
            let fu = loopuse::faceuse(b, lu1)?.ok_or_else(null)?;
            let lut1 = if eu1::faceuse_orientation(b, fu)? == OrientType::Opposite {
                lu2
            } else {
                lu1
            };

            let mut ld = LoopData::zeroed();
            if loopuse::is_vertex_loopuse(b, lut1)? {
                let vertices = loopuse::vertices(b, lut1, None)?;
                let v = vertices.first().copied().ok_or_else(|| {
                    SolidError::invalid_input(OP_FROM, "vertex loop use without vertex")
                })?;
                ld.loop_type = 1;
                ld.vertex = get_map(&map, v.map(EntityRef::Vertex));
            } else {
                let edgeuses = loopuse::edgeuses(b, lut1, None, None)?;
                ld.loop_type = 0;
                ld.start_eu = all_edgeuses.len() as u64;
                ld.num_eu = edgeuses.len() as u64;
                for eu in edgeuses {
                    let n = all_edgeuses.len() as u64;
                    map.insert(EntityRef::Edgeuse(eu), n);
                    if let Some(m) = eu1::mate(b, eu)? {
                        mate_map.insert(m, n + 1);
                    }
                    all_edgeuses.push(eu);
                }
            }
            ld.topology = topology_data(env, &mut table, EntityRef::Loop(l));
            ld.lu_upper_attributes =
                extract_attributes(&mut table, (env.key_of)(EntityRef::Loopuse(lut1)).key);
            ld.lu_lower_attributes =
                extract_attributes(&mut table, (env.key_of)(EntityRef::Loopuse(lu2)).key);
            bd.loops.push(ld);
        }

        // Edgeuses (top face use edge uses and wire primary edge uses, not mates)
        for &eu in &all_edgeuses {
            let mut ed = EuData::zeroed();
            let next = eu1::edgeuse_next(b, eu)?.ok_or_else(null)?;
            ed.next_eu = edgeuse_index(&map, &mate_map, next);
            let mate = eu1::mate(b, eu)?.ok_or_else(null)?;
            let mate_next = eu1::edgeuse_next(b, mate)?.ok_or_else(null)?;
            ed.mate_next_eu = edgeuse_index(&map, &mate_map, mate_next);
            ed.edge = get_map(&map, eu1::edge(b, eu)?.map(EntityRef::Edge));

            if eu1::is_shell_edgeuse(b, eu)? {
                // connects to a wire edge
                ed.eu_type = 2;
                ed.orientation = same(eu1::orientation(b, eu)?);
                ed.shell = get_map(&map, eu1::shell(b, eu)?.map(EntityRef::Shell));
            } else {
                // an edge in a loop connecting edges to a face
                let lu = eu1::loopuse(b, eu)?.ok_or_else(null)?;
                let l = loopuse::loop_(b, lu)?;
                ed.eu_type = 0;
                ed.loop_ = get_map(&map, l.map(EntityRef::Loop));
                ed.orientation = same(eu1::orientation(b, eu)?);
                let (uv, slot) = eu1::uv_trim_curve(b, eu)?;
                if let Some(c) = uv {
                    ed.uv_curve = bd.uv_curves.len() as u64;
                    bd.uv_curves.push(Keyed {
                        key: (env.uv_curve_key)(slot.unwrap_or(eu)),
                        object: c.clone_for_evaluation()?,
                    });
                } else {
                    ed.uv_curve = NO_OBJECT;
                }
            }

            let t = record_of(env, EntityRef::Edgeuse(eu));
            ed.topology.flags = t.all_flags();
            ed.topology.user_index1 = t.user_index1();
            ed.topology.user_index2 = t.user_index2();
            ed.topology.attributes =
                extract_attributes(&mut table, (env.key_of)(EntityRef::Edgeuse(eu)).key);

            let mt = record_of(env, EntityRef::Edgeuse(mate));
            ed.mate_flags = mt.all_flags();
            ed.mate_user_index1 = mt.user_index1();
            ed.mate_user_index2 = mt.user_index2();
            ed.mate_attributes =
                extract_attributes(&mut table, (env.key_of)(EntityRef::Edgeuse(mate)).key);
            bd.edgeuses.push(ed);
        }

        // Edges - set EdgeData objects and store Curves
        for &e in &edges {
            let prim = eu1::primary_edgeuse(b, e)?.ok_or_else(null)?;
            let mut ed = EdgeData::zeroed();
            ed.prim_eu = edgeuse_index(&map, &mate_map, prim);

            let (start, end) = if eu1::orientation(b, prim)? == OrientType::Same {
                let start = edge::vertex(b, e)?;
                (start, edge::other_vertex(b, e, start)?)
            } else {
                let end = edge::vertex(b, e)?;
                (edge::other_vertex(b, e, end)?, end)
            };
            ed.start_vertex = get_map(&map, start.map(EntityRef::Vertex));
            ed.end_vertex = get_map(&map, end.map(EntityRef::Vertex));

            // every edge owns its curve: a new 3D curve each
            let edge_rec = b.edge(e)?;
            let curve = edge_rec.curve.as_ref().ok_or_else(null)?;
            ed.curve = bd.curves_3d.len() as u64;
            bd.curves_3d.push(Keyed {
                key: (env.curve_key)(e),
                object: curve.clone_for_evaluation()?,
            });

            ed.zone_tol_3d = edge_rec.tolerance;
            ed.interval = edge_rec.interval;
            ed.topology = topology_data(env, &mut table, EntityRef::Edge(e));
            bd.edges.push(ed);
        }

        // All curves & surfaces are borrowed from Brep (copied here)
        bd.is_geom_borrowed = true;
        Ok(bd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::Kernel;

    /// The ASCII bytes `write_surface` writes for `s`.
    fn surface_bytes(s: &Surface) -> Result<Vec<u8>> {
        let mut db = DatabaseWriter::open_for_write(FileType::Ascii);
        db.begin_writing();
        write_surface(s, &mut db, DATABASE_VERSION)?;
        db.end_writing();
        Ok(db.into_bytes())
    }

    /// A lazy surface of extrusion and a lazy cone (no NURB yet) are written with the
    /// NURB their `MakeNurb` gives: the bytes equal those of the same surface with its
    /// NURB made first (upstream's `GetOrCreateGwNurbPointer`). A surface that has its
    /// NURB is written byte for byte as before.
    #[test]
    fn lazy_surfaces_write_the_nurb_they_make() {
        use crate::geom::extent2d::Extent2d;
        use crate::nurbs::crv_basic::Curve as NurbCurve;
        use crate::nurbs::math::{CPoint, NOW};
        use crate::surface::cone::Cone;
        use crate::surface::surf_of_extrusion::SurfOfExtrusion;
        let k = Kernel::default();

        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 1.0, 0.0, NOW),
            CPoint::new(2.0, 0.0, 0.0, NOW),
            CPoint::new(3.0, 0.5, 0.0, NOW),
        ];
        let gen = NurbCurve::new(pw, 3, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]).unwrap();
        let gen = Curve::from(BSplineCurve::from_nurb(gen, 3, false));
        let extrusion = SurfOfExtrusion::new(
            &k,
            gen,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            false,
        )
        .unwrap();
        let cone = Cone::new_bounded(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            2.0,
            1.0,
            0.0,
            270.0,
            3.0,
            false,
            false,
            false,
        );
        for lazy in [Surface::from(extrusion), Surface::from(cone)] {
            assert!(
                surface_bytes(&lazy).is_err(),
                "a lazy surface has no NURB to write"
            );
            // the same make on a separate copy: what upstream writes
            let mut made = lazy.clone_for_evaluation().unwrap();
            match made.kind_mut() {
                SurfaceKind::SurfOfExtrusion(s) => s.make_nurb(&k).unwrap(),
                SurfaceKind::Cone(s) => s.make_nurb(&k).unwrap(),
                _ => unreachable!(),
            }
            let want = surface_bytes(&made).unwrap();
            let mut written = lazy.clone_for_evaluation().unwrap();
            make_missing_nurb(&k, &mut written).unwrap();
            assert_eq!(surface_bytes(&written).unwrap(), want);
            // a surface with its NURB: unchanged by make_missing_nurb
            let mut again = made.clone_for_evaluation().unwrap();
            make_missing_nurb(&k, &mut again).unwrap();
            assert_eq!(surface_bytes(&again).unwrap(), want);
        }
    }
    use crate::topology::database_io::fixed16;

    /// A version 42 file as `WriteToFile` writes it (topology only).
    const V42: &str = "Version 42
Healer Version 10
//Brep Starts
1.0000000000000000 0.0000000000000000 0.0000000000000000 - Color
0.0000010000000000 - Brep ZoneTol3d
1 - is Manifold:[1 = is, 0 = isn't]
0 - has Composites:[1 = does, 0 = doesn't]

//Region Data
2 Regions In Brep
0 - Start Region
//Index, Flags, UserId1, UserId2, Number Of Shells, First Shell, Is void Region
0 0 0 0 1 0 0
1 4 7 0 0 1 1

//Shell Data
1 Shells In Brep
//Index, Flags, UserId1, UserId2, 0(Faceuse), Orientation, Number Of Faceuses, Start Faceuse
//Index, Flags, UserId1, UserId2, 1(WireEdge), Edge
//Index, Flags, UserId1, UserId2, 2(Vertex), Vertex
0 0 0 0 0 1 2 0

//Composite Face Data
0 Composite Faces In Brep

//Faceuse Data
2 Faceuses In Brep
//Index, Flags, UserId1, UserId2, Face, Orientation
0 0 0 0 0 1
1 0 0 0 0 0

//Face Data
1 Faces In Brep
//Index, Flags, UserId1, UserId2, Tol, Surface, Outer Loop, # of Loops, Naturally Trimmed
//UMin, UMax, VMin, VMax
0 0 0 0 0.0000010000000000 0 0 1 0
-5.0000000000000000 5.0000000000000000 -1.5000000000000000 3.5000000000000000

//Loop Data
1 Loops In Brep
//Index, Flags, UserId1, UserId2, 0(Edgeuse), # Of Edgeuses, Start Edgeuse
//Index, Flags, UserId1, UserId2, 1(Vertex), Vertex
0 0 0 0 0 2 0

//Composite Edge Data
0 Composite Edges In Brep

//Edgeuse Data
2 Edgeuses In Brep
//Index, Flags, UserId1, UserId2, MateFlags, MateUserId1, MateUserId2, 0(Edge), Edge, Loop, Orientation, UV Curve, Next EU, Mate Next EU
//Index, Flags, UserId1, UserId2, MateFlags, MateUserId1, MateUserId2, 1(Vertex), Vertex, Loop, Orientation, UV Curve
//Index, Flags, UserId1, UserId2, MateFlags, MateUserId1, MateUserId2, 2(Wire Edge), Edge, Shell, Orientation, UV Curve
//Index, Flags, UserId1, UserId2, MateFlags, MateUserId1, MateUserId2, 3(Vertex At Pole), Vertex, Loop, Orientation, UV Curve
0 3 0 0 3 0 0 0 0 0 1 9999999 1 -1
1 0 0 0 0 0 0 0 0 0 0 9999999 0 -2

//Edge Data
1 Edges In Brep
//Index, Flags, UserId1, UserId2, Tol, Curve, Start Vertex, End Vertex, Primary Edgeuse, Param Min, Param Max
0 0 0 0 0.0000010000000000 0 0 0 0 0.0000000000000000 1.0000000000000000

//Vertex Data
1 Vertices In Brep
//Index, Flags, UserId1, UserId2, Tol, X, Y, Z
0 0 0 0 0.0000010000000000 5.0000000000000000 0.0000000000000000 -0.2500000000000000

// UV-Curve Data
0 UV-Curves In Brep

// 3D-Curve Data
0 3D-Curves In Brep

//Surface Data
0 Surfaces In Brep
";

    #[test]
    fn fixed16_matches_percent_16f() {
        assert_eq!(fixed16(0.1), "0.1000000000000000");
        assert_eq!(fixed16(-0.0), "-0.0000000000000000");
        assert_eq!(fixed16(1e-17), "0.0000000000000000");
        assert_eq!(fixed16(123_456_789.123), "123456789.1229999959468842");
        assert_eq!(fixed16(f64::INFINITY), "inf");
        assert_eq!(fixed16(f64::NEG_INFINITY), "-inf");
        assert_eq!(fixed16(f64::NAN), "nan");
    }

    #[test]
    fn ascii_write_reproduces_the_file() -> Result<()> {
        let k = Kernel::default();
        let mut bd = BrepData::read_from_file(&k, V42.as_bytes().to_vec(), FileType::Ascii)?;
        assert_eq!(bd.regions.get(1).map(|r| r.topology.user_index1), Some(7));
        let out = bd.write_to_file(FileType::Ascii)?;
        assert_eq!(String::from_utf8_lossy(&out), V42);
        Ok(())
    }

    #[test]
    fn binary_write_reads_back() -> Result<()> {
        let k = Kernel::default();
        let mut bd = BrepData::read_from_file(&k, V42.as_bytes().to_vec(), FileType::Ascii)?;
        let bin = bd.write_to_file(FileType::Binary)?;
        let mut back = BrepData::read_from_file(&k, bin.clone(), FileType::Binary)?;
        assert_eq!(back.healer_version(), HEALER_VERSION);
        assert_eq!(back.edgeuses.len(), 2);
        assert_eq!(back.edgeuses.get(1).map(|e| e.mate_next_eu), Some(-2));
        assert_eq!(back.vertices.first().map(|v| v.point.z), Some(-0.25));
        assert_eq!(back.write_to_file(FileType::Binary)?, bin);
        let text = back.write_to_file(FileType::Ascii)?;
        assert_eq!(String::from_utf8_lossy(&text), V42);
        Ok(())
    }

    #[test]
    fn indexed_attributes_line() {
        let mut db = DatabaseWriter::open_for_write(FileType::Ascii);
        write_indexed_attributes(&[], &mut db);
        write_indexed_attributes(&[3, 0], &mut db);
        assert_eq!(db.bytes(), b"            0 \n            2 3 0 \n");
    }
}
