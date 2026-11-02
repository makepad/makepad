//! The B-rep's own checks, its validate-only tolerance pass and the walk over its
//! topology.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp`, `SmTopologyTraverser.cpp`, `SmRegion.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `brep` | `SmBrep::AssertValid` |
//! | `tolerances` | `SmBrep::ValidateAndUpdateTolerances(TRUE, ...)` |
//! | `collect_regions` | `SmTopologyTraverser::CollectRegions` |
//! | `adjacent_regions` | `SmRegion::GetAdjacentRegions` |
//! | `sub_topology` | `SmBrep::AssertSubTopology` with the `SmBrep` branch of `SmTopologyTraverser::GetSubTopology` |
//!
//! Deviations: the validate-only tolerance pass keeps no maxima (its outputs are only
//! printed by a warning upstream, which is omitted) and does not compute the bounding
//! box (used only when fixing). Edge-use marks are a local set. `CollectRegions`'
//! linear stack and result searches use companion sets (same order).

use std::collections::BTreeSet;

use super::super::{contains_coincident_topology, ERROR};
use super::{base, brep_object, class, edge, face, loops, seams, shell, vertex, Cx, Test};
use crate::error::{EntityRef, Result};
use crate::kernel::Kernel;
use crate::math::UNDEF_DOUBLE;
use crate::query::brep::BrepQuery;
use crate::topology::brep::{p1, p2, p9};
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::{edge as tedge, vertexuse as tvu};
use crate::topology::{Brep, EdgeuseId, RegionId};

const C: i64 = class::BREP;

/// `SmBrep::AssertValid`.
pub(super) fn brep(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>, walk: bool) -> Result<bool> {
    let this = brep_object();

    // Init rtn value, and invoke higher-level checks.
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::sag_object(b, cx)?;
    }

    // Old style checks: ValidatePointers() and ValidateAndUpdateTolerances().
    let status = super::validate_pointers(k, b, cx.rep.array.as_deref_mut(), 0, walk, cx.keys)?;
    if status != super::SUCCESS {
        return Ok(false);
    }

    // old style ValidateAndUpdateTolerances check (no updating is done)
    let status = tolerances(k, b, cx)?;

    // new style - remember failures and keep checking
    rtn &= status == super::SUCCESS;

    // Walk the Topology graph checks.
    let regions = p1::regions(b, None)?;
    let shells = p1::shells(k, b, None)?;
    let faces = p1::faces(b, None)?;
    let edges = p1::edges(b, None)?;
    let vertices = p1::vertices(b, None)?;
    let num_regions = regions.len();
    let num_faces = faces.len();
    let num_edges = edges.len();
    let num_vertices = vertices.len();

    // SmBrep and the objects it attaches to need to share common contexts (intrinsic);
    // the infinite region, the face, edge and vertex list heads and the deleted
    // surfaces list
    if b.infinite_region().is_some() {
        rtn &= cx.rep.this_bool(b, C, this, 0, true)?;
    }
    rtn &= cx.rep.this_bool(b, C, this, 1, true)?;
    rtn &= cx.rep.this_bool(b, C, this, 2, true)?;
    rtn &= cx.rep.this_bool(b, C, this, 5, true)?;
    rtn &= cx.rep.this_bool(b, C, this, 6, true)?;

    // Regions must be separated by closed shells, which must be Faceuse shells.
    // So if there's more than one region, we must have faces.
    if num_regions > 1 {
        let ok = !cx.rep.need(0) || num_faces >= 1;
        rtn &= cx.rep.this_bool(b, C, this, 7, ok)?;
    }

    // Check that the infinite region is actually the correct one.
    let query = BrepQuery::of_brep(b);
    let found = crate::error::ignored(p9::find_infinite_region(k, b, &query, None, None))?;
    rtn &= found.is_some();
    let found = found.flatten();
    let ok = !cx.rep.need(0) || found == b.infinite_region();
    rtn &= cx.rep.this_bool(b, C, this, 8, ok)?;

    // The infinite region must be first in the region list.
    let ok = !cx.rep.need(0) || b.infinite_region() == regions.first().copied();
    rtn &= cx.rep.this_bool(b, C, this, 9, ok)?;

    // for Non-empty Breps
    if num_faces > 0 || num_edges > 0 || num_vertices > 0 {
        // Check Region/Shell containment for every region.
        for (ii, &region) in regions.iter().enumerate() {
            let region_shells = b.region(region)?.shells.clone();
            let num_region_shells = region_shells.len();

            // Every region (except the infinite region) should have at least one shell
            let ok = !cx.rep.need(0) || num_region_shells >= 1 || num_regions <= 1;
            rtn &= cx.rep.this_bool(b, C, this, 16, ok)?;
            if num_region_shells < 1 && num_regions > 1 {
                continue;
            }

            // The infinite region has no outer shell. All other regions have one outer
            // shell, and it is first in the list.
            let outer = if ii == 0 {
                None
            } else {
                region_shells.first().copied()
            };

            // All regions other than the infinite region must be surrounded by Faces,
            // so its outer shell must be a closed Faceuse shell.
            if let Some(outer) = outer {
                let ok = !cx.rep.need(0) || b.shell(outer)?.is_faceuse_shell();
                rtn &= cx.rep.this_bool(b, C, this, 10, ok)?;
                let ok = !cx.rep.need(0) || b.shell(outer)?.is_closed(b, outer)?;
                rtn &= cx.rep.this_bool(b, C, this, 11, ok)?;
            }

            // for every inner shell - the infinite region has any number of inner shells
            let first_inner = usize::from(ii != 0);
            for &inner in region_shells.iter().skip(first_inner) {
                // ray casting makes sure that nonInfiniteRegion inner shells are
                // contained by outer shells
                let ok = !cx.rep.need(0)
                    || match outer {
                        None => true,
                        Some(outer) => seams::is_inner_shell_of(k, b, inner, outer)?,
                    };
                rtn &= cx.rep.this_bool(b, C, this, 12, ok)?;
            }
        }
    }

    // If more than one region, then the infinite region must contain at least one
    // shell that's a closed Faceuse shell.
    if num_regions > 1 {
        let mut has_closed_shell = false;
        if let Some(&infinite) = regions.first() {
            for &s in &b.region(infinite)?.shells {
                if !b.shell(s)?.is_faceuse_shell() {
                    continue;
                }
                // Check shell closure.
                if b.shell(s)?.is_closed(b, s)? {
                    has_closed_shell = true;
                    break;
                }
            }
        }
        let ok = !cx.rep.need(0) || has_closed_shell;
        rtn &= cx.rep.this_bool(b, C, this, 17, ok)?;
    }

    // All Regions must be connected to each other, across Faceuses: this Brep's list
    // of Regions against a list collected by a TopologyTraverser crawl (which does not
    // put the given region in the list).
    let mut connected = collect_regions(k, b, b.infinite_region())?;
    connected.push(b.infinite_region());

    // The number of regions found by a topology traversal should be the Brep Region
    // Count
    let ok = !cx.rep.need(0) || connected.len() == num_regions;
    rtn &= cx.rep.this_bool(b, C, this, 18, ok)?;

    // Regions found by topology traversal should be the same as the Brep's region list
    // (FindCommonElements: the connected regions that are brep regions)
    let region_set: BTreeSet<Option<RegionId>> = regions.iter().map(|&r| Some(r)).collect();
    let common = connected.iter().filter(|r| region_set.contains(r)).count();
    let ok = !cx.rep.need(0) || common == num_regions;
    rtn &= cx.rep.this_bool(b, C, this, 19, ok)?;

    // See about running coincident-topology checks (its default level 0, first pair).
    rtn &=
        contains_coincident_topology(k, b, -1.0, false, cx.rep.array.as_deref_mut(), 0, cx.keys)?
            .found;

    // Shell context checks (intrinsic).
    for _ in &shells {
        rtn &= cx.rep.this_bool(b, C, this, 13, true)?;
    }

    // Face context checks (intrinsic).
    for _ in &faces {
        rtn &= cx.rep.this_bool(b, C, this, 14, true)?;
    }

    // warn when Shell Count == Face Count in a multi-face NonManifold model; a warning,
    // so the return value is unchanged
    let mut faceuse_shell_count = 0;
    for &s in &shells {
        if b.shell(s)?.is_faceuse_shell() {
            faceuse_shell_count += 1;
        }
    }
    let ok = !cx.rep.need(0)
        || p2::is_manifold_solid(b)?
        || faces.len() != faceuse_shell_count
        || faces.len() <= 1;
    cx.rep.this_bool(b, C, this, 21, ok)?;

    // when asked - AssertValid for a topology graph traversal
    if walk {
        rtn &= sub_topology(k, b, cx)?;
    }

    // all done
    Ok(rtn)
}

/// `SmBrep::ValidateAndUpdateTolerances(TRUE, ...)`: reports (list 2 of `SmBrep`, about
/// each edge use) the vertex/edge and vertex/face gaps at both ends of every face's
/// upward-faceuse edge use that exceed half the vertex tolerance (tests 0-3) and its
/// edge/face gap over the edge tolerance (test 4); an edge use and its mate are
/// checked once. Returns the native status (`SM_ERR` for an end without vertex).
fn tolerances(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>) -> Result<i32> {
    // Don't change the brep if we are just validating
    let validate_only = b.validate_only();
    b.set_validate_only(true);
    let status = tolerances_validate_only(k, b, cx);
    b.set_validate_only(validate_only);
    status
}

fn tolerances_validate_only(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>) -> Result<i32> {
    // get all faces
    let faces = p1::faces(b, None)?;

    // the marked edge uses (an unlocked mark)
    let mut marked: BTreeSet<EdgeuseId> = BTreeSet::new();
    let need = cx.rep.need(0);

    // for every face
    for f in faces {
        let Some(fu) = crate::topology::face::p2::upward_faceuse(b, f)? else {
            return Err(super::super::invalid("face without faceuse"));
        };
        // get all this face's edgeuses
        let edgeuses = eu1::faceuse_edgeuses(b, fu, false, None)?;

        // for every face->edgeuse
        for eu in edgeuses {
            let mate = eu1::mate(b, eu)?;

            // skip marked edgeuses
            if marked.contains(&eu) {
                continue;
            }

            // mark these edgeuses
            marked.insert(eu);
            let Some(mate) = mate else {
                return Err(super::super::invalid("edge use without mate"));
            };
            marked.insert(mate);

            let start_vu = super::super::required(eu1::vertexuse(b, eu)?)?;
            let Some(start_v) = tvu::vertex(b, start_vu)? else {
                return Ok(ERROR);
            };
            let end_vu = super::super::required(eu1::vertexuse(b, mate)?)?;
            let Some(end_v) = tvu::vertex(b, end_vu)? else {
                return Ok(ERROR);
            };

            // get all edgeuse start/end vertex gaps: upstream keeps pointers into the
            // vertex uses' caches and reads their lengths below, so a later refresh that
            // resets a sibling's cache (`CalcVertexFaceGap` / `CalcVertexEdgeGap` clear
            // every use of the vertex on the same face / edge, e.g. both ends of a
            // closed edge) shows in an earlier gap
            let start_edge_gap = tvu::vertex_edge_gap(k, b, start_vu, false)?.is_some();
            let start_face_gap = tvu::vertex_face_gap(k, b, start_vu, false)?.is_some();
            let end_edge_gap = tvu::vertex_edge_gap(k, b, end_vu, false)?.is_some();
            let end_face_gap = tvu::vertex_face_gap(k, b, end_vu, false)?.is_some();
            let start_tol = b.vertex(start_v)?.tolerance;
            let end_tol = b.vertex(end_v)?.tolerance;
            let (start, end) = (b.vertexuse(start_vu)?, b.vertexuse(end_vu)?);

            // report bad edgeuse start and end vertex/edge and vertex/face gaps
            let gaps = [
                (
                    0,
                    start_edge_gap.then_some(start.vertex_edge_gap.gap_3d),
                    start_tol,
                ),
                (
                    1,
                    start_face_gap.then_some(start.vertex_face_gap.gap_3d),
                    start_tol,
                ),
                (
                    2,
                    end_edge_gap.then_some(end.vertex_edge_gap.gap_3d),
                    end_tol,
                ),
                (
                    3,
                    end_face_gap.then_some(end.vertex_face_gap.gap_3d),
                    end_tol,
                ),
            ];
            for (index, gap, tol) in gaps {
                let ok = !need || gap.is_none_or(|g| g <= tol / 2.0);
                cx.rep
                    .add(b, ok, tolerance_test(index, eu, tol / 2.0), || {
                        gap.unwrap_or(UNDEF_DOUBLE)
                    })?;
            }

            // get edgeuse Edge/Surface Max gaps
            let max_gap = eu1::max_edge_face_gap(k, b, eu, false)?;
            let max_gap = max_gap.map_or(0.0, |g| g.gap_3d);

            // report unfixed bad edgeuse edge gaps
            let e = super::super::required(eu1::edge(b, eu)?)?;
            let eu_tol = tedge::tolerance(b, e)?;
            let ok = !need || max_gap <= eu_tol;
            cx.rep
                .add(b, ok, tolerance_test(4, eu, eu_tol), || max_gap)?;
        }
    }

    // all done
    Ok(super::SUCCESS)
}

/// A `SM_ASSERT_OBJ_VALUE_REPORT(this, SM_LIST_2, index, ...)` of the tolerance pass.
fn tolerance_test(index: u64, eu: EdgeuseId, tol: f64) -> Test<'static> {
    Test {
        class: C,
        list: 2,
        index,
        reporter: brep_object(),
        owner: EntityRef::Edgeuse(eu).into(),
        owner_type: class::EDGEUSE,
        tol,
        message: None,
    }
}

/// `SmRegion::GetAdjacentRegions`: the regions across the mated shells of `region`'s
/// face-use shells, each once, in first-seen order.
fn adjacent_regions(k: &mut Kernel, b: &Brep, region: RegionId) -> Result<Vec<Option<RegionId>>> {
    let mut out: Vec<Option<RegionId>> = Vec::new();
    for &s in &b.region(region)?.shells {
        if !b.shell(s)?.is_faceuse_shell() {
            continue;
        }
        // increments an unused Mark value
        for mate in b.shell(s)?.mated_shells(k, b)? {
            let region_mate = b.shell(mate)?.region;
            if region_mate != Some(region) && !out.contains(&region_mate) {
                out.push(region_mate);
            }
        }
    }
    Ok(out)
}

/// `SmTopologyTraverser::CollectRegions(pStartRegion, rAllRegions)`: the regions
/// connected to `start` (without it), in native order.
fn collect_regions(
    k: &mut Kernel,
    b: &Brep,
    start: Option<RegionId>,
) -> Result<Vec<Option<RegionId>>> {
    let mut all: Vec<Option<RegionId>> = Vec::new();
    let mut in_all: BTreeSet<Option<RegionId>> = BTreeSet::new();
    let Some(start_region) = start else {
        return Err(super::super::invalid("CollectRegions: no start region"));
    };

    // Init stack to adjacent Regions of pStartRegion.
    let mut stack = adjacent_regions(k, b, start_region)?;

    // Retrieves and removes Last.
    while let Some(this) = stack.pop() {
        if this == start {
            continue;
        }
        let Some(this) = this else {
            return Err(super::super::invalid(
                "CollectRegions: shell without region",
            ));
        };
        let shells = b.region(this)?.shells.clone();
        for s in shells {
            let region_mate = b.shell(s)?.region;
            if region_mate.is_none() || region_mate == start {
                continue;
            }
            // Already processed.
            if in_all.contains(&region_mate) {
                continue;
            }
            all.push(region_mate);
            in_all.insert(region_mate);
            let Some(region_mate) = region_mate else {
                continue;
            };
            for mate in adjacent_regions(k, b, region_mate)? {
                if mate != start && !stack.contains(&mate) {
                    stack.push(mate);
                }
            }
        }
    }
    Ok(all)
}

/// `SmBrep::AssertSubTopology(*this)`: `AssertValid(SM_NO_WALK)` of every region,
/// shell, face, face loop, edge and vertex of the B-rep, in that order.
fn sub_topology(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>) -> Result<bool> {
    let regions = p1::regions(b, None)?;
    let shells = p1::shells(k, b, None)?;
    let faces = p1::faces(b, None)?;
    let mut face_loops = Vec::new();
    for &f in &faces {
        face_loops.extend(crate::topology::face::p2::loops(b, f, false, None)?);
    }
    let edges = p1::edges(b, None)?;
    let vertices = p1::vertices(b, None)?;

    let shell_index = shell::ShellIndex::new(b)?;
    let mut rtn = true;
    for r in regions {
        rtn &= shell::region(b, cx, r)?;
    }
    for s in shells {
        rtn &= shell::shell(k, b, cx, &shell_index, s)?;
    }
    for f in faces {
        rtn &= face::face(k, b, cx, f)?;
    }
    for l in face_loops {
        rtn &= loops::loop_(k, b, cx, l)?;
    }
    for e in edges {
        rtn &= edge::edge(k, b, cx, e)?;
    }
    for v in vertices {
        rtn &= vertex::vertex(k, b, cx, v)?;
    }
    Ok(rtn)
}
