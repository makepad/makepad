//! Copies of polygonal B-reps: the copy constructor `SmPolyBrep(const SmPolyBrep&)`,
//! which sequentializes the source into an `SmPolyBrepData` (`FromPolyBrep`) and
//! builds the new topology from it (`MakeTopologyFromData`), and `SmApiPolyBrepCopy`.
//!
//! Upstream: `source/SMLib/src/SmPoly.cpp`, `source/SMLib/inc/SmPoly.h`,
//! `source/SMLib/src/SmPolyBrepData.cpp`, `source/SMLib/inc/SmPolyBrepData.h`,
//! `source/SMLib/src/SmAttribute.cpp`, `source/SM_API/src/SmApiPolygons.cpp`.
//!
//! | Rust                                  | Upstream                                          |
//! |---------------------------------------|---------------------------------------------------|
//! | [`PolyBrep::copy`]                    | `SmPolyBrep::SmPolyBrep(const SmPolyBrep&, NULL, NULL)` |
//! | `PolyBrepData` (private)              | `SmPolyBrepData` and its region / shell / face / loop / edge / vertex records |
//! | `PolyBrepData::from_poly_brep` (private) | `SmPolyBrepData::FromPolyBrep` (no index map) |
//! | `PolyBrep::make_topology_from_data` (private) | `SmPolyBrep::MakeTopologyFromData` (no copy maps) |
//! | [`PolyCopyAttributes`]                | the poly B-rep's `sm_ExtractAttributes` / `AddAttribute` |
//! | [`poly_brep_copy`]                    | `SmApiPolyBrepCopy`                               |
//! | [`PolyBrep::edge_links`], [`PolyBrep::vertex_edge_list`], [`PolyEdgeLinks`] | (inspection: the ring links the copy sets) |
//!
//! Deviations:
//! - **Attributes.** Attribute lists live in an [`AttributeStore`] keyed by object, so
//!   the caller names the source and copy keys ([`PolyCopyAttributes`]); without them
//!   no attribute is copied. Only the poly B-rep's own attributes reach the copy (as
//!   upstream: `MakeTopologyFromData` adds the B-rep's list only); the other objects'
//!   lists, which upstream extracts and never uses, are not read.
//! - **Copy maps.** `SmCopyPolyBrepMap` outputs (`pOptCopyPolyBrepMap`,
//!   `pOptReverseMap`) are not ported: `SmApiPolyBrepCopy` passes none.
//! - **Source normals.** `FromPolyBrep` calls `GetNormal(FALSE, FALSE)` on each source
//!   face, which caches the normal on the `const` source; [`PolyBrep::copy`] takes
//!   `&mut self`.
//! - **Unresolvable indices.** An edge whose start vertex is missing or not on the
//!   vertex list maps to `INVALID_KEY`, which `MakeTopologyFromData` uses as an index
//!   into its vertex array (out of bounds upstream); the port returns
//!   [`SolidError::InvalidInput`].
//! - **Statuses.** The copy constructor ignores `MakeTopologyFromData`'s status, whose
//!   only failures are allocation failures (`NER`); the port's arena errors are
//!   returned. `sm_ExtractAttributes` cannot fail here.
//! - **`NULL` source.** [`poly_brep_copy`] takes a reference, so `SmApiPolyBrepCopy`'s
//!   `SM_ERR_INVALID_INPUT` for a null pointer has no counterpart.

use crate::error::{Result, SolidError};
use crate::geom::attribute::{AttributeStore, Behavior, ObjectKey};
use crate::geom::tol;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3};

use std::collections::BTreeMap;

use super::{
    push_slot, slot_mut, PolyBrep, PolyEdgeId, PolyFaceId, PolyLoop, PolyLoopId,
    PolyLoopOrientation, PolyShellId, PolyVertexId,
};

const OP_COPY: &str = "query::poly::copy::PolyBrep::copy";

/// `INVALID_KEY` (`SmPolyBrepData.cpp`): the index of an object `sm_GetMap` does not
/// know.
const INVALID_KEY: usize = 99_999_999;

/// The attribute lists of a poly B-rep copy: the store holding them and the keys of the
/// source poly B-rep and of its copy.
#[derive(Debug)]
pub struct PolyCopyAttributes<'a> {
    /// The attribute store.
    pub store: &'a mut AttributeStore,
    /// The source poly B-rep's key.
    pub source: ObjectKey,
    /// The copy's key.
    pub copy: ObjectKey,
}

/// The ring links of a poly edge ([`PolyBrep::edge_links`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolyEdgeLinks {
    /// `m_pNextRadialE`.
    pub next_radial: PolyEdgeId,
    /// `m_pLastRadialE`.
    pub last_radial: PolyEdgeId,
    /// `m_pNextEdgeAtV`.
    pub next_at_v: Option<PolyEdgeId>,
    /// `m_pPrevEdgeAtV`.
    pub prev_at_v: Option<PolyEdgeId>,
}

/// `SmPolyRegionData`.
#[derive(Debug, Clone, Copy)]
struct RegionData {
    /// `m_lStartPolyShell`.
    start_poly_shell: usize,
    /// `m_lNumPolyShells`.
    num_poly_shells: usize,
}

/// `SmPolyShellData`.
#[derive(Debug, Clone, Copy)]
struct ShellData {
    /// `m_lStartPolyFace`.
    start_poly_face: usize,
    /// `m_lNumPolyFaces`.
    num_poly_faces: usize,
}

/// `SmPolyFaceData`.
#[derive(Debug, Clone, Copy)]
struct FaceData {
    /// `m_sZoneTol3d`.
    zone_tol_3d: f64,
    /// `m_lStartPolyLoop`.
    start_poly_loop: usize,
    /// `m_lNumPolyLoops`.
    num_poly_loops: usize,
    /// `m_vNormal`.
    normal: Vec3,
    /// `m_dMaxDeviation`.
    max_deviation: f64,
}

/// `SmPolyLoopData`.
#[derive(Debug, Clone, Copy)]
struct LoopData {
    /// `m_bOuterLoop`.
    outer_loop: bool,
    /// `m_bClosed`.
    closed: bool,
    /// `m_lStartPolyEdge`.
    start_poly_edge: usize,
    /// `m_lNumPolyEdge`.
    num_poly_edge: usize,
}

/// `SmPolyEdgeData` (`m_lAuxData` stays `0`: `FromPolyBrep` never sets it).
#[derive(Debug, Clone, Copy)]
struct EdgeData {
    /// `m_sZoneTol3d`.
    zone_tol_3d: f64,
    /// `m_lVertex`.
    vertex: usize,
    /// `m_lNextRadialE`.
    next_radial_e: usize,
    /// `m_lNextEdgeAtV`.
    next_edge_at_v: usize,
}

/// `SmPolyVertexData`.
#[derive(Debug, Clone, Copy)]
struct VertexData {
    /// `m_sZoneTol3d`.
    zone_tol_3d: f64,
    /// `m_vPoint`.
    point: Point3,
    /// `m_lEdgeList`.
    edge_list: usize,
}

/// `SmPolyBrepData`: a poly B-rep as index-linked records.
#[derive(Debug, Default)]
struct PolyBrepData {
    /// `m_sZoneTol3d`.
    zone_tol_3d: f64,
    /// `m_vPolyRegions`.
    regions: Vec<RegionData>,
    /// `m_vPolyShells`.
    shells: Vec<ShellData>,
    /// `m_vPolyFaces`.
    faces: Vec<FaceData>,
    /// `m_vPolyLoops`.
    loops: Vec<LoopData>,
    /// `m_vPolyEdges`.
    edges: Vec<EdgeData>,
    /// `m_vPolyVertices`.
    vertices: Vec<VertexData>,
}

/// `ADD_TO_MAP` over a list: each object's index.
fn index_map<T: Ord + Copy>(list: &[T]) -> BTreeMap<T, usize> {
    let mut m = BTreeMap::new();
    for (i, &x) in list.iter().enumerate() {
        m.insert(x, i);
    }
    m
}

/// `sm_GetMap`: the index of `x`, or [`INVALID_KEY`].
fn get_map<T: Ord>(map: &BTreeMap<T, usize>, x: &T) -> usize {
    map.get(x).copied().unwrap_or(INVALID_KEY)
}

impl PolyBrepData {
    /// `SmPolyBrepData::FromPolyBrep(crPolyBrep, rAttributes, NULL)`: the source's
    /// regions, shells, faces, loops, vertices and edges in list order, linked by index.
    fn from_poly_brep(k: &Kernel, src: &mut PolyBrep) -> Result<Self> {
        const OP: &str = OP_COPY;
        let mut d = Self {
            zone_tol_3d: src.tolerance(),
            ..Self::default()
        };

        // for every PolyRegion
        let mut all_shells: Vec<PolyShellId> = Vec::new();
        for r in src.poly_regions() {
            let shells = src.region_poly_shells(r)?;
            d.regions.push(RegionData {
                start_poly_shell: all_shells.len(),
                num_poly_shells: shells.len(),
            });
            all_shells.extend(shells);
        }

        // for every PolyShell
        let mut all_faces: Vec<PolyFaceId> = Vec::new();
        for &s in &all_shells {
            let faces = src.shell_poly_faces(s)?;
            d.shells.push(ShellData {
                start_poly_face: all_faces.len(),
                num_poly_faces: faces.len(),
            });
            all_faces.extend(faces);
        }

        // for every PolyFace
        let mut all_loops: Vec<PolyLoopId> = Vec::new();
        for &f in &all_faces {
            let loops = src.face_poly_loops(f)?;
            let zone_tol_3d = src.face(f)?.tolerance;
            let start_poly_loop = all_loops.len();
            let num_poly_loops = loops.len();
            let (normal, _) = src.face_normal(k, f, false, false)?;
            let max_deviation = src.face(f)?.max_deviation;
            d.faces.push(FaceData {
                zone_tol_3d,
                start_poly_loop,
                num_poly_loops,
                normal,
                max_deviation,
            });
            all_loops.extend(loops);
        }

        // for every PolyLoop
        let mut all_edges: Vec<PolyEdgeId> = Vec::new();
        for &l in &all_loops {
            let edges = src.loop_poly_edges(l)?;
            let outer_loop = src.lp(l)?.orientation != PolyLoopOrientation::Opposite;
            d.loops.push(LoopData {
                outer_loop,
                closed: true,
                start_poly_edge: all_edges.len(),
                num_poly_edge: edges.len(),
            });
            all_edges.extend(edges);
        }

        // for every PolyVertex
        let all_vertices = src.poly_vertices();
        let vertex_map = index_map(&all_vertices);
        for &v in &all_vertices {
            let x = src.vertex(v)?;
            d.vertices.push(VertexData {
                zone_tol_3d: x.tolerance,
                point: x.point,
                edge_list: INVALID_KEY,
            });
        }

        // for every PolyEdge (the aux data index is never stored)
        for &e in &all_edges {
            let x = src.edge(e)?;
            let vertex = match x.vertex {
                Some(v) => get_map(&vertex_map, &v),
                None => INVALID_KEY,
            };
            d.edges.push(EdgeData {
                zone_tol_3d: x.tolerance,
                vertex,
                next_radial_e: d.edges.len(),
                next_edge_at_v: INVALID_KEY,
            });
        }

        // Find radial ordering of edges
        let edge_map = index_map(&all_edges);
        for (ii, &e) in all_edges.iter().enumerate() {
            let next_at_v = src.edge(e)?.next_at_v;
            let radial = src.edge_radial(e)?;
            let rec = d.edges.get_mut(ii).ok_or(SolidError::sm_err(OP))?;
            // set NextRadialEdge index
            rec.next_radial_e = match radial {
                Some(r) if r != e => get_map(&edge_map, &r),
                _ => ii,
            };
            // set NextEdgeAtV index (sm_GetMap(NULL) is INVALID_KEY)
            rec.next_edge_at_v = match next_at_v {
                Some(n) if n == e => ii,
                Some(n) => get_map(&edge_map, &n),
                None => INVALID_KEY,
            };
        }

        // set PolyVertex back Edge indices now that all PolyEdges have an index
        for (ii, &v) in all_vertices.iter().enumerate() {
            let head = src.vertex(v)?.edge_list;
            let rec = d.vertices.get_mut(ii).ok_or(SolidError::sm_err(OP))?;
            rec.edge_list = match head {
                Some(h) => get_map(&edge_map, &h),
                None => INVALID_KEY,
            };
        }

        Ok(d)
    }
}

impl PolyBrep {
    /// A copy of this poly B-rep (`SmPolyBrep(const SmPolyBrep&)`): new regions,
    /// shells, faces (tolerance, cached normal and deviation), loops (orientation,
    /// closed), edges (tolerance, start vertex, radial and vertex rings) and vertices
    /// (point, tolerance, edge ring head), in list order. The copy does not own its
    /// context, its back pointers are not valid, and composite faces, aux data, vertex
    /// normals and original object keys are not copied. With `attributes`, the copy
    /// gets the source's non-temporary attributes.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an edge without a start vertex on the vertex
    /// list (out of bounds upstream); those of the walks and of the arena.
    pub fn copy(
        &mut self,
        k: &Kernel,
        attributes: Option<PolyCopyAttributes<'_>>,
    ) -> Result<PolyBrep> {
        // m_bOwnsContext = m_bOKBackPtrs = FALSE; list heads allocated
        let mut out = PolyBrep::new(k, 0.0);
        out.owns_context = false;
        out.ok_back_ptrs = false;

        // sequentialize the PolyBrep data into PolyBrepData
        let data = PolyBrepData::from_poly_brep(k, self)?;

        // Create the topology (status ignored upstream; only allocation can fail)
        out.make_topology_from_data(k, &data, attributes)?;
        Ok(out)
    }

    /// `SmPolyBrep::MakeTopologyFromData(pPolyBrepData, rAllAttributes, NULL, NULL,
    /// NULL)` on a new poly B-rep.
    fn make_topology_from_data(
        &mut self,
        k: &Kernel,
        d: &PolyBrepData,
        attributes: Option<PolyCopyAttributes<'_>>,
    ) -> Result<()> {
        const OP: &str = OP_COPY;

        // Add attributes (the B-rep's: sm_ExtractAttributes skips SM_AB_TEMP ones)
        if let Some(a) = attributes {
            for h in a.store.attributes(a.source) {
                let temp = a
                    .store
                    .get(h)
                    .is_some_and(|x| x.behavior() == Behavior::Temp);
                if temp {
                    continue;
                }
                a.store.add_attribute(a.copy, h)?;
            }
        }

        // tolerance
        self.tolerance = d.zone_tol_3d;

        // PolyVertices
        let mut vertices: Vec<PolyVertexId> = Vec::new();
        for v in &d.vertices {
            vertices.push(self.add_vertex(k, v.point, v.zone_tol_3d)?);
        }

        let mut all_edges: Vec<PolyEdgeId> = Vec::new();
        for r in &d.regions {
            let new_region = self.add_region()?;

            // Create SmPolyShells
            for jj in 0..r.num_poly_shells {
                let new_shell = self.add_shell(new_region)?;
                let s = d
                    .shells
                    .get(r.start_poly_shell + jj)
                    .ok_or(SolidError::sm_err(OP))?;

                // Create SmPolyFaces
                for kk in 0..s.num_poly_faces {
                    let f = d
                        .faces
                        .get(s.start_poly_face + kk)
                        .ok_or(SolidError::sm_err(OP))?;
                    let new_face = self.add_face(k, new_shell, f.zone_tol_3d, None)?;
                    {
                        let x = slot_mut(&mut self.face_slots, new_face.0, OP)?;
                        x.normal = f.normal;
                        x.max_deviation = f.max_deviation;
                    }

                    // Create SmPolyLoops
                    for mm in 0..f.num_poly_loops {
                        let l = d
                            .loops
                            .get(f.start_poly_loop + mm)
                            .ok_or(SolidError::sm_err(OP))?;
                        let new_loop = self.start_poly_edge_loop(new_face)?;
                        {
                            let x: &mut PolyLoop = slot_mut(&mut self.loop_slots, new_loop.0, OP)?;
                            x.orientation = if l.outer_loop {
                                PolyLoopOrientation::Same
                            } else {
                                PolyLoopOrientation::Opposite
                            };
                            x.closed = l.closed;
                        }

                        // Create SmPolyEdges
                        for nn in 0..l.num_poly_edge {
                            let e = d
                                .edges
                                .get(l.start_poly_edge + nn)
                                .ok_or(SolidError::sm_err(OP))?;
                            let edge_tol = tol::zone_tol3d(k, e.zone_tol_3d);
                            let new_edge = PolyEdgeId(push_slot(
                                &mut self.edge_slots,
                                Self::blank_edge(edge_tol),
                                OP,
                            )?);
                            // Link to vertex (sVertices[INVALID_KEY] is out of bounds upstream)
                            let vert = *vertices.get(e.vertex).ok_or(SolidError::InvalidInput {
                                operation: OP,
                                reason: "edge without a start vertex on the vertex list",
                            })?;
                            {
                                let x = slot_mut(&mut self.edge_slots, new_edge.0, OP)?;
                                x.next_radial = new_edge;
                                x.last_radial = new_edge;
                                x.vertex = Some(vert);
                            }
                            all_edges.push(new_edge);
                            self.loop_post_insert(new_loop, new_edge)?;
                        } // end for (nn) each Edge
                    } // end for (mm) each Loop
                } // end for (kk) each Face
            } // end for (jj) each Shell
        } // end for (ii) each Region

        // Link up vertices to edge list elements
        for (m, v) in d.vertices.iter().enumerate() {
            let new_vertex = *vertices.get(m).ok_or(SolidError::sm_err(OP))?;
            let head = if v.edge_list == INVALID_KEY {
                None
            } else {
                Some(*all_edges.get(v.edge_list).ok_or(SolidError::sm_err(OP))?)
            };
            slot_mut(&mut self.vertex_slots, new_vertex.0, OP)?.edge_list = head;
        }

        // Link up edges to edges
        for (m2, e) in d.edges.iter().enumerate() {
            let new_edge = *all_edges.get(m2).ok_or(SolidError::sm_err(OP))?;
            if e.next_radial_e != INVALID_KEY {
                let next = *all_edges
                    .get(e.next_radial_e)
                    .ok_or(SolidError::sm_err(OP))?;
                slot_mut(&mut self.edge_slots, new_edge.0, OP)?.next_radial = next;
                slot_mut(&mut self.edge_slots, next.0, OP)?.last_radial = new_edge;
            }
            if e.next_edge_at_v != INVALID_KEY {
                let next = *all_edges
                    .get(e.next_edge_at_v)
                    .ok_or(SolidError::sm_err(OP))?;
                slot_mut(&mut self.edge_slots, new_edge.0, OP)?.next_at_v = Some(next);
                slot_mut(&mut self.edge_slots, next.0, OP)?.prev_at_v = Some(new_edge);
            }
        }

        Ok(())
    }
}

impl PolyBrep {
    /// The radial and vertex ring links stored on edge `e`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_links(&self, e: PolyEdgeId) -> Result<PolyEdgeLinks> {
        let x = self.edge(e)?;
        Ok(PolyEdgeLinks {
            next_radial: x.next_radial,
            last_radial: x.last_radial,
            next_at_v: x.next_at_v,
            prev_at_v: x.prev_at_v,
        })
    }

    /// The head of vertex `v`'s ring of starting edges (`m_pPolyEdgeList`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted vertex.
    pub fn vertex_edge_list(&self, v: PolyVertexId) -> Result<Option<PolyEdgeId>> {
        Ok(self.vertex(v)?.edge_list)
    }
}

/// A deep copy of a poly B-rep (`SmApiPolyBrepCopy`): [`PolyBrep::copy`].
///
/// # Errors
///
/// Those of [`PolyBrep::copy`].
pub fn poly_brep_copy(
    k: &Kernel,
    poly_brep: &mut PolyBrep,
    attributes: Option<PolyCopyAttributes<'_>>,
) -> Result<PolyBrep> {
    poly_brep.copy(k, attributes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::attribute::Attribute;

    /// Two triangles sharing an edge, linked radially.
    fn two_triangles(k: &Kernel) -> PolyBrep {
        let mut b = PolyBrep::new(k, 1e-5);
        let r = b.add_region().unwrap();
        let s = b.add_shell(r).unwrap();
        let vs: Vec<_> = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ]
        .iter()
        .map(|p| b.add_vertex(k, Vec3::new(p[0], p[1], p[2]), 1e-5).unwrap())
        .collect();
        let mut edges = Vec::new();
        for tri in [[0, 1, 2], [1, 3, 2]] {
            let f = b.add_face(k, s, 1e-5, None).unwrap();
            let lp = b.start_poly_edge_loop(f).unwrap();
            for j in 0..3 {
                let (a, c) = (vs[tri[j]], vs[tri[(j + 1) % 3]]);
                let (pa, pc) = (b.vertex_point(a).unwrap(), b.vertex_point(c).unwrap());
                edges.push(b.add_poly_edge(k, lp, 1e-5, pa, pc, a).unwrap());
            }
            b.finish_poly_edge_loop(lp).unwrap();
        }
        // edge 1 (1->2) and edge 5 (2->1)
        b.link_radials(&[edges[1], edges[5]]).unwrap();
        b
    }

    #[test]
    fn copy_keeps_the_topology() {
        let k = Kernel::default();
        let mut b = two_triangles(&k);
        let c = b.copy(&k, None).unwrap();
        assert_eq!(c.poly_faces(None).unwrap().len(), 2);
        assert_eq!(c.poly_vertices().len(), 4);
        let edges = c.poly_edges(None).unwrap();
        assert_eq!(edges.len(), 6);
        let l1 = c.edge_links(edges[1]).unwrap();
        assert_eq!(l1.next_radial, edges[5]);
        assert_eq!(c.edge_links(edges[5]).unwrap().next_radial, edges[1]);
        // the source normals were cached by FromPolyBrep
        let f = b.poly_faces(None).unwrap()[0];
        assert!(b.face_max_deviation(f).unwrap() > -1.0);
        assert!(!c.ok_back_ptrs());
    }

    #[test]
    fn copy_takes_the_lasting_attributes() {
        let k = Kernel::default();
        let mut b = two_triangles(&k);
        let mut store = AttributeStore::new();
        let keep = store.insert(Attribute::long(7, 3, Behavior::Copy)).unwrap();
        let temp = store.insert(Attribute::long(8, 4, Behavior::Temp)).unwrap();
        store.add_attribute(ObjectKey(1), keep).unwrap();
        store.add_attribute(ObjectKey(1), temp).unwrap();
        let attrs = PolyCopyAttributes {
            store: &mut store,
            source: ObjectKey(1),
            copy: ObjectKey(2),
        };
        poly_brep_copy(&k, &mut b, Some(attrs)).unwrap();
        assert_eq!(store.attributes(ObjectKey(2)), vec![keep]);
    }
}
