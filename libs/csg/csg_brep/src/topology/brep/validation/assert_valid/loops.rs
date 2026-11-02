//! The loop and loop-use checks.
//!
//! Upstream: `source/SMLib/src/SmLoop.cpp`, `SmLoopuse.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `loop_` | `SmLoop::AssertValid` |
//! | `loopuse` | `SmLoopuse::AssertValid` |
//! | `c_fixed` | `smos_sprintf("%W.Plf")` |
//!
//! Deviations: the single-use edge search counts edges with one map instead of
//! comparing every pair (same first index). A mate loop use with fewer edge uses than
//! the primary returns [`crate::SolidError::InvalidInput`] at test 6 (upstream reads
//! past the array). The test 5 message names the edge by its report identity in place
//! of its address.

use std::collections::{BTreeMap, BTreeSet};

use super::{base, class, seams, Cx};
use crate::error::{EntityRef, Result};
use crate::kernel::Kernel;
use crate::topology::loopuse as tlu;
use crate::topology::{Brep, EdgeuseId, LoopId, LoopuseId, LoopuseStart};

/// `printf("%W.Plf", x)`: fixed notation with `precision` digits, right-aligned in
/// `width` (glibc spells infinities `inf` and NaNs `nan`).
pub(super) fn c_fixed(x: f64, width: usize, precision: usize) -> String {
    let body = if x.is_nan() {
        if x.is_sign_negative() {
            "-nan".to_string()
        } else {
            "nan".to_string()
        }
    } else if x.is_infinite() {
        if x < 0.0 {
            "-inf".to_string()
        } else {
            "inf".to_string()
        }
    } else {
        format!("{x:.precision$}")
    };
    format!("{body:>width$}")
}

/// The edge of each edge use.
fn edges_of(b: &Brep, eus: &[EdgeuseId]) -> Result<Vec<Option<crate::topology::EdgeId>>> {
    eus.iter().map(|&eu| Ok(b.edgeuse(eu)?.edge)).collect()
}

/// `SmLoop::AssertValid(SM_NO_WALK)`.
pub(super) fn loop_(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>, l: LoopId) -> Result<bool> {
    const C: i64 = class::LOOP;
    let this = EntityRef::Loop(l);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::topology(b, cx, this)?;
    }

    // 0: Loop and the objects it attaches to need to share common contexts
    let primary = b.loop_(l)?.loopuse;
    if let Some(lu) = primary {
        rtn &= cx.rep.this_bool(b, C, this, 0, true)?;
        let ok = !cx.rep.need(0) || seams::is_loopuse_side_in_surface(k, b, lu)?;
        rtn &= cx.rep.this_bool(b, C, this, 2, ok)?;
    }

    // the trimming checks of the loop on its face's surface (status ignored)
    let face = b.loop_(l)?.face;
    if let Some(surface) = b.face(face)?.surface.clone() {
        let tol = b.face(face)?.tolerance;
        crate::error::ignored(crate::topology::trimming_tools::check_loop(
            k,
            b,
            &surface,
            l,
            tol,
            cx.rep.array.as_deref_mut(),
        ))?;
    }

    // GetLoopuses
    let mate = match primary {
        Some(lu) => tlu::other_loopuse(b, lu)?,
        None => None,
    };
    if let (Some(lu1), Some(lu2)) = (primary, mate) {
        // the edgeuses in CCW order
        let edgeuses1 = tlu::edgeuses(b, lu1, None, None)?;
        let edgeuses2 = tlu::edgeuses(b, lu2, None, None)?;
        let n1 = edgeuses1.len();
        let need = cx.rep.need(0);
        rtn &= cx
            .rep
            .this_bool(b, C, this, 5, !need || n1 == edgeuses2.len())?;
        let ok = !need
            || (tlu::is_edge_loopuse(b, lu1)? && n1 > 0)
            || (tlu::is_vertex_loopuse(b, lu1)? && n1 == 0);
        rtn &= cx.rep.this_bool(b, C, this, 3, ok)?;

        if n1 >= 1 {
            let edges1 = edges_of(b, &edgeuses1)?;
            let edges2 = edges_of(b, &edgeuses2)?;

            // look for an edge used just once in the loop
            let mut count: BTreeMap<Option<crate::topology::EdgeId>, usize> = BTreeMap::new();
            for &e in &edges1 {
                *count.entry(e).or_default() += 1;
            }
            let single = edges1.iter().position(|e| count.get(e) == Some(&1));
            let has_single_use_edge = single.is_some();
            let idx1 = single.unwrap_or(n1);
            let edge1 = edges1
                .get(if has_single_use_edge { idx1 } else { 0 })
                .copied()
                .flatten();

            // look for the common edge
            let idx2 = edges2
                .iter()
                .position(|&e| e == edge1)
                .unwrap_or(edges2.len());

            // every primary loopuse edge is an edge of the mate
            let loop_edges2: BTreeSet<Option<crate::topology::EdgeId>> =
                tlu::edges(b, lu2, None)?.into_iter().collect();
            let remaining = tlu::edges(b, lu1, None)?
                .into_iter()
                .filter(|e| !loop_edges2.contains(e))
                .count();
            rtn &= cx.rep.this_bool(b, C, this, 4, !need || remaining == 0)?;

            if n1 > 1 {
                let off = (idx1 + idx2) % n1;
                for i1 in 1..n1 {
                    let i2 = (n1 - i1 + off) % n1;
                    if has_single_use_edge {
                        let (Some(&a), Some(&c)) = (edges1.get(i1), edges2.get(i2)) else {
                            return Err(super::super::invalid(
                                "loop mate has fewer edge uses than the loop use",
                            ));
                        };
                        rtn &= cx.rep.this_bool(b, C, this, 6, !need || a == c)?;
                    }
                }
            }
        }
    }

    if let Some(lu) = primary {
        rtn &= loopuse(k, b, cx, lu)?;
    }
    if let Some(lu) = mate {
        rtn &= loopuse(k, b, cx, lu)?;
    }

    // all done
    Ok(rtn)
}

/// `SmLoopuse::AssertValid(SM_NO_WALK)`.
pub(super) fn loopuse(
    k: &mut Kernel,
    b: &mut Brep,
    cx: &mut Cx<'_>,
    lu: LoopuseId,
) -> Result<bool> {
    const C: i64 = class::LOOPUSE;
    let this = EntityRef::Loopuse(lu);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::topology(b, cx, this)?;
    }

    // 0, 1, 2: SmLoopuse and the objects it attaches to need to share common contexts
    let record = *b.loopuse(lu)?;
    let has_start = match record.start {
        LoopuseStart::Edgeuse(eu) => eu.is_some(),
        LoopuseStart::Vertexuse(vu) => vu.is_some(),
        LoopuseStart::Unknown => false,
    };
    if record.loop_.is_some() {
        rtn &= cx.rep.this_bool(b, C, this, 0, true)?;
    }
    if record.mate.is_some() {
        rtn &= cx.rep.this_bool(b, C, this, 1, true)?;
    }
    if has_start {
        rtn &= cx.rep.this_bool(b, C, this, 2, true)?;
    }

    // 3: When Loopuse is used in a Faceuse, it must have a Vertexuse or Edgeuse
    let ok = !cx.rep.need(0) || record.faceuse.is_none() || has_start;
    rtn &= cx.rep.this_bool(b, C, this, 3, ok)?;

    // 5: a Loopuse should form a closed loop in 3d (owned by the primary loopuse)
    if let Some(l) = record.loop_ {
        if b.loop_(l)?.loopuse == Some(lu) {
            let closed = tlu::is_closed_3d(k, b, lu, true)?;
            let message = if closed.closed {
                String::new()
            } else {
                let eu = super::super::required(closed.edgeuse)?;
                let e = super::super::required(b.edgeuse(eu)?.edge)?;
                let (key, _, _) = super::super::object_info(b, EntityRef::Edge(e).into())?;
                format!(
                    "SmLoopuse::AssertValid - found loop with out of tol of Gap/XSectTol3d:[{}/{}] starting on Edge:[0x{:#x}] ",
                    c_fixed(closed.max_gap, 16, 16),
                    c_fixed(closed.xsect_tol_3d, 16, 16),
                    key.0
                )
            };
            let ok = !cx.rep.need(0) || closed.closed;
            rtn &= cx.rep.add(
                b,
                ok,
                super::Test {
                    class: C,
                    list: 0,
                    index: 5,
                    reporter: this.into(),
                    owner: this.into(),
                    owner_type: C,
                    tol: closed.xsect_tol_3d,
                    message: Some(message.into()),
                },
                || closed.max_gap,
            )?;
        }
    }

    // all done
    Ok(rtn)
}
