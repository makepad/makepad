//! Pointer checks in native report order.
//!
//! Upstream: `source/SMLib/src/SmBrep.cpp` (`ValidatePointers`,
//! `sm_CheckEdgeuseGeometry`), `source/SMLib/src/SmEdge.cpp` (`ValidateGeometry`),
//! `source/SMLib/src/NL_CrvBasic.cpp` (`N_CrvIsNotReversed`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`validate_pointers`] | `SmBrep::ValidatePointers` |
//! | `fix_reversed` | `N_CrvIsNotReversed(Fixit=TRUE)` |
//! | `fix_reversed_on`, `end_turns_back`, `first_fundamental_form` | the source fork's `PassesReverseDirectionCheckOn`, `nl_EndTurnsBack`, `sm_FirstFundamentalForm` (WR-406) |
//!
//! Deviations: no recursion; CCW walks are bounded at the native 100000 steps.
//! Native const geometry repairs use mutable curve access. Warning-only checks
//! retain their evaluations and repairs; diagnostic log strings are omitted.
//! Not upstream (WR-406, source issue #891, opt-in
//! `Kernel::fixes.uv_trim_reversal_on_surface`): a UV trim's turn at its
//! ends is judged on its face's surface, so the check no longer straightens a
//! circle's exact trim on a face whose parameters run at different speeds.
use super::{invalid, required, Object, Reports, ERROR, SUCCESS};
use crate::error::{EntityRef, Result};
use crate::geom::assert_array::AssertArray;
use crate::kernel::Kernel;
use crate::math::{Point2, Vec3, EFF_ZERO, UNDEF_DOUBLE};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::brep::p1;
use crate::topology::edge;
use crate::topology::edgeuse::{p1 as eu1, p3 as eu3};
use crate::topology::face::p2 as face2;
use crate::topology::shell::{ShellMembers, ShellType};
use crate::topology::{
    Brep, BrepId, EdgeId, EdgeuseId, EdgeuseOwner, FaceuseId, LoopuseId, LoopuseStart, VertexuseId,
    VertexuseOwner,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_WALK: usize = 100_000;

#[derive(Clone, Copy)]
struct Links {
    next: EdgeuseId,
    last: EdgeuseId,
}
struct Lists {
    links: BTreeMap<EdgeuseId, Links>,
    shell_members: BTreeSet<(crate::topology::ShellId, FaceuseId)>,
    walks: BTreeMap<LoopuseId, Vec<EdgeuseId>>,
}
impl Lists {
    fn new(b: &Brep) -> Self {
        let mut links = BTreeMap::new();
        for (_, e) in b.edges().iter() {
            for (i, &eu) in e.edgeuses.iter().enumerate() {
                if let (Some(&next), Some(&last)) = (
                    e.edgeuses.get((i + 1) % e.edgeuses.len()),
                    e.edgeuses
                        .get((i + e.edgeuses.len() - 1) % e.edgeuses.len()),
                ) {
                    // Membership is only valid when GetEdge points back to this list.
                    links.entry(eu).or_insert(Links { next, last });
                }
            }
        }
        let mut shell_members = BTreeSet::new();
        for (s, sh) in b.shells().iter() {
            if let ShellMembers::Faceuses(fus) = &sh.members {
                for &fu in fus {
                    shell_members.insert((s, fu));
                }
            }
        }
        Self {
            links,
            shell_members,
            walks: BTreeMap::new(),
        }
    }
    fn mate(&self, b: &Brep, eu: EdgeuseId, radial: bool) -> Result<Option<EdgeuseId>> {
        let Some(links) = self.links.get(&eu) else {
            return Ok(None);
        };
        let data = b.edgeuse(eu)?;
        if matches!(data.owner, EdgeuseOwner::Shell(_)) {
            return Ok(Some(links.next));
        }
        let e = b.edge(required(data.edge)?)?;
        let primary = b
            .edgeuse(required(e.edgeuses.first().copied())?)?
            .orientation;
        let next = (primary == data.orientation) != radial;
        Ok(Some(if next { links.next } else { links.last }))
    }
    fn walk(&mut self, b: &Brep, lu: LoopuseId) -> Result<Vec<EdgeuseId>> {
        if let Some(cached) = self.walks.get(&lu) {
            return Ok(cached.clone());
        }
        let mut out = Vec::new();
        if let LoopuseStart::Edgeuse(Some(start)) = b.loopuse(lu)?.start {
            let mut current = start;
            loop {
                if out.len() >= MAX_WALK {
                    return Err(invalid("loop walk exceeds 100000 edge uses"));
                }
                out.push(current);
                current = required(b.edgeuse(current)?.ccw)?;
                if current == start {
                    break;
                }
            }
        }
        self.walks.insert(lu, out.clone());
        Ok(out)
    }
}

fn req_fu(b: &Brep, r: &mut Reports<'_>, fu: FaceuseId) -> Result<bool> {
    let f = b.faceuse(fu)?;
    let owner = EntityRef::Faceuse(fu);
    for (i, ok) in [
        (123, f.face.is_some()),
        (124, f.mate.is_some()),
        (125, f.shell.is_some()),
        (126, !f.loopuses.is_empty()),
    ] {
        if !r.check(b, i, ok, owner)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn req_vu(b: &Brep, r: &mut Reports<'_>, vu: VertexuseId) -> Result<bool> {
    let v = b.vertexuse(vu)?;
    if !r.check(b, 144, v.vertex.is_some(), EntityRef::Vertexuse(vu))? {
        return Ok(false);
    }
    let linked = matches!(
        v.owner,
        VertexuseOwner::Edgeuse(Some(_))
            | VertexuseOwner::Loopuse(Some(_))
            | VertexuseOwner::Shell(Some(_))
    );
    r.check(b, 145, linked, EntityRef::Vertexuse(vu))
}
fn req_eu(b: &Brep, r: &mut Reports<'_>, cache: &Lists, eu: EdgeuseId) -> Result<bool> {
    let e = b.edgeuse(eu)?;
    let linked = matches!(
        e.owner,
        EdgeuseOwner::Loopuse(Some(_)) | EdgeuseOwner::Shell(Some(_))
    );
    for (i, ok) in [
        (135, e.edge.is_some()),
        (136, e.vertexuse.is_some()),
        (137, linked),
    ] {
        if !r.check(b, i, ok, EntityRef::Edgeuse(eu))? {
            return Ok(false);
        }
    }
    if matches!(e.owner, EdgeuseOwner::Loopuse(_)) {
        for (i, ok) in [(138, e.ccw.is_some()), (139, e.cw.is_some())] {
            if !r.check(b, i, ok, EntityRef::Edgeuse(eu))? {
                return Ok(false);
            }
        }
    }
    for i in [140, 141] {
        if !r.check(b, i, cache.links.contains_key(&eu), EntityRef::Edgeuse(eu))? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn req_lu(b: &Brep, r: &mut Reports<'_>, cache: &mut Lists, lu: LoopuseId) -> Result<bool> {
    let l = b.loopuse(lu)?;
    let start = matches!(
        l.start,
        LoopuseStart::Edgeuse(Some(_)) | LoopuseStart::Vertexuse(Some(_))
    );
    for (i, ok) in [
        (128, l.loop_.is_some()),
        (129, l.mate.is_some()),
        (130, l.faceuse.is_some()),
        (131, start),
    ] {
        if !r.check(b, i, ok, EntityRef::Loopuse(lu))? {
            return Ok(false);
        }
    }
    match l.start {
        LoopuseStart::Edgeuse(Some(first)) => {
            let mut current = first;
            let mut visited = Vec::new();
            loop {
                if !req_eu(b, r, cache, current)? {
                    return Ok(false);
                }
                visited.push(current);
                current = required(b.edgeuse(current)?.ccw)?;
                if current == first || visited.len() >= MAX_WALK {
                    break;
                }
            }
            if !r.check(b, 146, current == first, EntityRef::Loopuse(lu))? {
                return Ok(false);
            }
            cache.walks.insert(lu, visited);
        }
        LoopuseStart::Vertexuse(Some(vu)) if !req_vu(b, r, vu)? => {
            return Ok(false);
        }
        _ => {}
    }
    Ok(true)
}

fn preflight(b: &Brep, r: &mut Reports<'_>, cache: &mut Lists) -> Result<bool> {
    let Some(infinite) = b.infinite_region() else {
        return r.check(b, 113, false, EntityRef::Brep(BrepId::from_index(0)));
    };
    if !r.check(
        b,
        117,
        b.region(infinite)?.in_brep,
        EntityRef::Region(infinite),
    )? {
        return Ok(false);
    }
    for region in p1::regions(b, None)? {
        let data = b.region(region)?;
        if region != infinite && !r.check(b, 117, data.in_brep, EntityRef::Region(region))? {
            return Ok(false);
        }
        for &s in &data.shells {
            let sh = b.shell(s)?;
            if !r.check(b, 118, sh.region.is_some(), EntityRef::Shell(s))? {
                return Ok(false);
            }
            if sh.shell_type != ShellType::Unknown
                && !r.check(
                    b,
                    119,
                    match &sh.members {
                        ShellMembers::Empty => false,
                        ShellMembers::Faceuses(fus) => !fus.is_empty(),
                        _ => true,
                    },
                    EntityRef::Shell(s),
                )?
            {
                return Ok(false);
            }
            match &sh.members {
                ShellMembers::Faceuses(fus) if sh.shell_type == ShellType::Faceuse => {
                    for &fu in fus {
                        if !req_fu(b, r, fu)? {
                            return Ok(false);
                        }
                    }
                }
                ShellMembers::Edgeuse(eu) if sh.shell_type == ShellType::Wire => {
                    let e = b.edge(required(b.edgeuse(*eu)?.edge)?)?;
                    for &eu in &e.edgeuses {
                        if !req_eu(b, r, cache, eu)? {
                            return Ok(false);
                        }
                    }
                }
                ShellMembers::Vertexuse(vu)
                    if sh.shell_type == ShellType::Vertex && !req_vu(b, r, *vu)? =>
                {
                    return Ok(false);
                }
                _ => {}
            }
        }
    }
    for &f in b.face_list() {
        let face = b.face(f)?;
        // Arena list ownership is the native owner pointer.
        for (i, ok) in [
            (120, true),
            (121, face.surface.is_some()),
            (122, face.faceuse.is_some()),
        ] {
            if !r.check(b, i, ok, EntityRef::Face(f))? {
                return Ok(false);
            }
        }
        let fu = required(face.faceuse)?;
        if !req_fu(b, r, fu)? {
            return Ok(false);
        }
        let mate = required(b.faceuse(fu)?.mate)?;
        if !req_fu(b, r, mate)? {
            return Ok(false);
        }
        for fu in [fu, mate] {
            for &lu in &b.faceuse(fu)?.loopuses {
                if !req_lu(b, r, cache, lu)? {
                    return Ok(false);
                }
                if let Some(l) = b.loopuse(lu)?.loop_ {
                    if !r.check(b, 127, b.loop_(l)?.loopuse.is_some(), EntityRef::Loop(l))? {
                        return Ok(false);
                    }
                }
            }
        }
    }
    for &e in b.edge_list() {
        let edge = b.edge(e)?;
        for (i, ok) in [
            (132, true),
            (133, edge.curve.is_some()),
            (134, !edge.edgeuses.is_empty()),
        ] {
            if !r.check(b, i, ok, EntityRef::Edge(e))? {
                return Ok(false);
            }
        }
        for &eu in &edge.edgeuses {
            if !req_eu(b, r, cache, eu)? {
                return Ok(false);
            }
        }
    }
    for &v in b.vertex_list() {
        let vertex = b.vertex(v)?;
        for (i, ok) in [(142, true), (143, !vertex.vertexuses.is_empty())] {
            if !r.check(b, i, ok, EntityRef::Vertex(v))? {
                return Ok(false);
            }
        }
        for &vu in &vertex.vertexuses {
            if !req_vu(b, r, vu)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Runs `SmBrep::ValidatePointers`, appending its failed reports in native order.
/// `walk` skips `CheckFace`'s loop checks, as `SM_WALK` does. The returned integer
/// preserves native `SM_SUCCESS` (1000), required-pointer failure (1001), and the
/// early boolean return (0). Ordinary failed checks are reports, not an error status.
/// `keys` supplies the caller's topology identities used by curve-owner checks.
/// `level` is the native `SM_LEVEL_0` through `SM_LEVEL_GIVEN` (0 through 4).
pub fn validate_pointers(
    k: &Kernel,
    b: &mut Brep,
    reports: Option<&mut AssertArray>,
    level: u8,
    walk: bool,
    keys: &dyn Fn(EntityRef) -> crate::geom::object::ObjectRef,
) -> Result<i32> {
    if level > 4 {
        return Err(invalid("invalid assert test level"));
    }
    let mut r = Reports {
        array: reports,
        level,
    };
    let mut cache = Lists::new(b);
    if !preflight(b, &mut r, &mut cache)? {
        return Ok(ERROR);
    }
    r.check(
        b,
        0,
        b.tolerance() > EFF_ZERO,
        EntityRef::Brep(BrepId::from_index(0)),
    )?;
    let mut live = BTreeSet::new();
    for region in p1::regions(b, None)? {
        for &s in &b.region(region)?.shells {
            live.insert((16013, s.index()));
            if let ShellMembers::Faceuses(fus) = &b.shell(s)?.members {
                for &fu in fus {
                    for &lu in &b.faceuse(fu)?.loopuses {
                        live.insert((16004, lu.index()));
                    }
                }
            }
        }
    }
    for &e in b.edge_list() {
        for &eu in &b.edge(e)?.edgeuses {
            let ok = match b.edgeuse(eu)?.owner {
                EdgeuseOwner::Shell(Some(s)) => live.contains(&(16013, s.index())),
                EdgeuseOwner::Loopuse(Some(lu)) => {
                    live.contains(&(16004, lu.index())) && eu1::shell(b, eu)?.is_some()
                }
                _ => false,
            };
            if !ok {
                r.check(b, 98, false, EntityRef::Edgeuse(eu))?;
                return Ok(ERROR);
            }
        }
    }
    for region in p1::regions(b, None)? {
        for &s in &b.region(region)?.shells {
            let sh = b.shell(s)?;
            if sh.shell_type == ShellType::Wire {
                if let Some(e) = sh.wire_edge(b)? {
                    r.check(b, 2, !b.edge(e)?.edgeuses.is_empty(), EntityRef::Shell(s))?;
                } else {
                    r.check(b, 1, true, EntityRef::Shell(s))?;
                }
            }
        }
    }
    let faces = p1::faces(b, None)?;
    for &f in &faces {
        let tol = b.face(f)?.tolerance;
        let checked =
            crate::topology::trimming_tools::check_face(k, b, f, tol, r.array.as_deref_mut(), walk);
        if let Err(e) = checked {
            if e.is_unported() {
                return Err(e);
            }
        }
        let face = b.face(f)?;
        for (i, ok) in [
            (4, true),
            (5, face.tolerance < b.tolerance() * 1000.0),
            (6, face.tolerance > b.tolerance() * 0.99),
            (7, face.surface.is_some()),
        ] {
            r.check(b, i, ok, EntityRef::Face(f))?;
        }
        let surface = required(face.surface.as_ref())?;
        r.check(b, 8, true, Object::Surface(f))?;
        r.check(b, 9, surface.owner().is_some(), Object::Surface(f))?;
        if !face
            .uv_domain
            .is_contained_by(&surface.natural_uv_domain_with_nurb(k)?, EFF_ZERO)
        {
            r.check(b, 11, false, Object::Surface(f))?;
        }
        let Some(fu) = face.faceuse else {
            r.check(b, 12, false, EntityRef::Face(f))?;
            return Ok(0);
        };
        r.check(b, 13, true, EntityRef::Faceuse(fu))?;
        r.check(
            b,
            14,
            b.faceuse(fu)?.orientation == OrientType::Same,
            EntityRef::Faceuse(fu),
        )?;
        let Some(fum) = b.faceuse(fu)?.mate else {
            r.check(b, 15, false, EntityRef::Faceuse(fu))?;
            return Ok(0);
        };
        for (i, ok, owner) in [
            (16, true, EntityRef::Faceuse(fum)),
            (
                17,
                b.faceuse(fu)?.orientation != b.faceuse(fum)?.orientation,
                EntityRef::Faceuse(fu),
            ),
            (18, true, EntityRef::Face(f)),
            (19, b.faceuse(fu)?.face == Some(f), EntityRef::Faceuse(fu)),
            (20, b.faceuse(fum)?.face == Some(f), EntityRef::Faceuse(fum)),
            (21, b.faceuse(fu)?.mate == Some(fum), EntityRef::Faceuse(fu)),
            (
                22,
                b.faceuse(fum)?.mate == Some(fu),
                EntityRef::Faceuse(fum),
            ),
        ] {
            r.check(b, i, ok, owner)?;
        }
        let Some(s1) = b.faceuse(fu)?.shell else {
            r.check(b, 23, false, EntityRef::Faceuse(fu))?;
            return Ok(0);
        };
        let Some(s2) = b.faceuse(fum)?.shell else {
            r.check(b, 24, false, EntityRef::Faceuse(fum))?;
            return Ok(0);
        };
        for (i, ok, s) in [
            (25, true, s1),
            (26, true, s2),
            (27, b.shell(s1)?.shell_type == ShellType::Faceuse, s1),
            (28, b.shell(s2)?.shell_type == ShellType::Faceuse, s2),
        ] {
            r.check(b, i, ok, EntityRef::Shell(s))?;
        }
        let loops = face2::loops(b, f, false, None)?;
        for (j, &l) in loops.iter().enumerate() {
            // SmBrep.cpp:31357 `pL->GetFace() == pF`; SmLoop::GetFace is
            // m_pLU->GetFaceuse()->GetFace() (SmLoop.h:61), not the port's Loop::face cache
            let loop_face =
                crate::topology::integrity::derived_loop_face(b, l).unwrap_or(b.loop_(l)?.face);
            r.check(b, 29, loop_face == f, EntityRef::Loop(l))?;
            let Some(lu) = b.loop_(l)?.loopuse else {
                r.check(b, 30, false, EntityRef::Loop(l))?;
                return Ok(0);
            };
            let Some(lum) = b.loopuse(lu)?.mate else {
                r.check(b, 31, false, EntityRef::Loop(l))?;
                return Ok(0);
            };
            for (i, owner) in [
                (32, EntityRef::Loop(l)),
                (33, EntityRef::Loopuse(lu)),
                (34, EntityRef::Loopuse(lum)),
            ] {
                r.check(b, i, true, owner)?;
            }
            if j == 0 {
                r.check(
                    b,
                    35,
                    b.loopuse(lu)?.orientation == OrientType::Same,
                    EntityRef::Loopuse(lu),
                )?;
                r.check(
                    b,
                    36,
                    b.loopuse(lum)?.orientation == OrientType::Same,
                    EntityRef::Loopuse(lum),
                )?;
                r.check(
                    b,
                    37,
                    !matches!(b.loopuse(lu)?.start, LoopuseStart::Vertexuse(_)),
                    EntityRef::Loopuse(lu),
                )?;
                r.check(
                    b,
                    38,
                    !matches!(b.loopuse(lum)?.start, LoopuseStart::Vertexuse(_)),
                    EntityRef::Loopuse(lum),
                )?;
            } else {
                r.check(
                    b,
                    39,
                    b.loopuse(lu)?.orientation == OrientType::Opposite,
                    EntityRef::Loopuse(lu),
                )?;
                r.check(
                    b,
                    40,
                    b.loopuse(lum)?.orientation == OrientType::Opposite,
                    EntityRef::Loopuse(lum),
                )?;
            }
            for (i, ok, owner) in [
                (41, b.loopuse(lu)?.loop_ == Some(l), lu),
                (42, b.loopuse(lum)?.loop_ == Some(l), lum),
                (43, b.loopuse(lu)?.mate == Some(lum), lu),
                (44, b.loopuse(lum)?.mate == Some(lu), lum),
            ] {
                r.check(b, i, ok, EntityRef::Loopuse(owner))?;
            }
            if matches!(b.loopuse(lu)?.start, LoopuseStart::Vertexuse(_)) {
                continue;
            }
            for (this, offset, missing) in [(lu, 46, 45), (lum, 52, 51)] {
                if matches!(b.loopuse(this)?.start, LoopuseStart::Vertexuse(_)) {
                    break;
                }
                if !matches!(b.loopuse(this)?.start, LoopuseStart::Edgeuse(Some(_))) {
                    r.check(b, missing, false, EntityRef::Loopuse(this))?;
                    return Ok(0);
                }
                for eu in cache.walk(b, this)? {
                    r.check(b, offset, true, EntityRef::Edgeuse(eu))?;
                    let result = check_edgeuse_geometry(k, b, eu);
                    let good = match result {
                        Ok(()) => true,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => false,
                    };
                    r.check(b, offset + 1, good, EntityRef::Edgeuse(eu))?;
                    r.check(
                        b,
                        offset + 2,
                        matches!(b.edgeuse(eu)?.owner, EdgeuseOwner::Loopuse(_)),
                        EntityRef::Edgeuse(eu),
                    )?;
                    r.check(
                        b,
                        offset + 3,
                        b.edgeuse(eu)?.owner == EdgeuseOwner::Loopuse(Some(this)),
                        EntityRef::Edgeuse(eu),
                    )?;
                    let next = required(b.edgeuse(eu)?.ccw)?;
                    r.check(
                        b,
                        offset + 4,
                        b.edgeuse(next)?.cw == Some(eu),
                        EntityRef::Edgeuse(next),
                    )?;
                }
            }
            let uses = cache.walk(b, lu)?;
            let mates = cache.walk(b, lum)?;
            let mut mated = uses.len() == mates.len();
            if mated {
                for eu in uses {
                    match cache.mate(b, eu, false)? {
                        Some(m) if b.edgeuse(m)?.owner == EdgeuseOwner::Loopuse(Some(lum)) => {}
                        _ => {
                            mated = false;
                            break;
                        }
                    }
                }
            }
            r.check(b, 111, mated, EntityRef::Loopuse(lu))?;
        }
    }
    check_edges(k, b, &mut r, &cache, keys)?;
    check_vertices(b, &mut r)?;
    for region in p1::regions(b, None)? {
        for &s in &b.region(region)?.shells {
            r.check(
                b,
                94,
                b.shell(s)?.region == Some(region),
                EntityRef::Shell(s),
            )?;
            if b.shell(s)?.shell_type == ShellType::Faceuse {
                for fu in b.shell(s)?.get_faceuses()? {
                    r.check(
                        b,
                        95,
                        b.faceuse(fu)?.shell == Some(s),
                        EntityRef::Faceuse(fu),
                    )?;
                }
            }
        }
    }
    for &e in b.edge_list() {
        let uses = &b.edge(e)?.edgeuses;
        for j in (1..uses.len()).step_by(2) {
            let eu0 = *uses.get(j - 1).ok_or_else(|| invalid("edge list"))?;
            let eu1 = *uses.get(j).ok_or_else(|| invalid("edge list"))?;
            let next = *uses
                .get((j + 1) % uses.len())
                .ok_or_else(|| invalid("edge list"))?;
            r.check(
                b,
                97,
                cache.mate(b, eu1, true)? == Some(next),
                EntityRef::Edgeuse(eu1),
            )?;
            let s1 = required(eu1::shell(b, eu1)?)?;
            let sn = required(eu1::shell(b, next)?)?;
            if b.shell(s1)?.region != b.shell(sn)?.region {
                r.check(b, 98, false, EntityRef::Edgeuse(eu1))?;
            }
            r.check(
                b,
                99,
                b.edgeuse(eu0)?.orientation != b.edgeuse(eu1)?.orientation,
                EntityRef::Edgeuse(eu0),
            )?;
            r.check(
                b,
                100,
                b.edgeuse(eu0)?.orientation
                    == b.edgeuse(required(uses.first().copied())?)?.orientation,
                EntityRef::Edgeuse(eu0),
            )?;
            r.check(
                b,
                101,
                b.edgeuse(eu1)?.orientation
                    == b.edgeuse(required(uses.get(1).copied())?)?.orientation,
                EntityRef::Edgeuse(eu1),
            )?;
        }
    }
    for f in faces {
        let fu = required(b.face(f)?.faceuse)?;
        let fm = required(b.faceuse(fu)?.mate)?;
        for (this, outer, inner, missing) in [(fu, 102, 103, 109), (fm, 104, 105, 110)] {
            let list = &b.faceuse(this)?.loopuses;
            let Some(&first) = list.first() else {
                r.check(b, missing, false, EntityRef::Faceuse(this))?;
                return Ok(0);
            };
            r.check(
                b,
                outer,
                b.loopuse(first)?.orientation == OrientType::Same,
                EntityRef::Loopuse(first),
            )?;
            for &lu in list.iter().skip(1) {
                r.check(
                    b,
                    inner,
                    b.loopuse(lu)?.orientation == OrientType::Opposite,
                    EntityRef::Loopuse(lu),
                )?;
            }
        }
        for l in face2::loops(b, f, false, None)? {
            let lu = required(b.loop_(l)?.loopuse)?;
            let lm = required(b.loopuse(lu)?.mate)?;
            r.check(
                b,
                106,
                b.loopuse(lu)?.faceuse == Some(fu),
                EntityRef::Loopuse(lu),
            )?;
            r.check(
                b,
                106,
                b.loopuse(lm)?.faceuse == Some(fm),
                EntityRef::Loopuse(lm),
            )?;
        }
        for (this, index) in [(fu, 107), (fm, 108)] {
            let ok = b
                .faceuse(this)?
                .shell
                .is_some_and(|s| cache.shell_members.contains(&(s, this)));
            r.check(b, index, ok, EntityRef::Faceuse(this))?;
        }
    }
    Ok(SUCCESS)
}

fn check_edges(
    k: &Kernel,
    b: &mut Brep,
    r: &mut Reports<'_>,
    cache: &Lists,
    keys: &dyn Fn(EntityRef) -> crate::geom::object::ObjectRef,
) -> Result<()> {
    for e in p1::edges(b, None)? {
        r.check(b, 57, true, EntityRef::Edge(e))?;
        let primary = required(b.edge(e)?.edgeuses.first().copied())?;
        let orientation = b.edgeuse(primary)?.orientation;
        let v1 = required(edge::vertex(b, e)?)?;
        let v2 = required(edge::other_vertex(b, e, Some(v1))?)?;
        let start = edge::start_vertex(b, e)?;
        let ok = (orientation == OrientType::Same && start == Some(v1))
            || (orientation == OrientType::Opposite && start == Some(v2));
        if !ok {
            r.check(b, 58, false, EntityRef::Edge(e))?;
        }
        let tol = b.edge(e)?.tolerance;
        r.value(
            b,
            59,
            tol < b.tolerance() * 1000.0,
            EntityRef::Edge(e),
            b.tolerance() * 1000.0,
            tol,
        )?;
        r.value(
            b,
            60,
            tol > b.tolerance() * 0.99,
            EntityRef::Edge(e),
            b.tolerance() * 0.99,
            tol,
        )?;
        let good = match check_edge_geometry(k, b, e, cache, keys) {
            Ok(()) => true,
            Err(err) if err.is_unported() => return Err(err),
            Err(_) => false,
        };
        r.check(b, 61, good, EntityRef::Edge(e))?;
        let uses = b.edge(e)?.edgeuses.clone();
        r.check(b, 62, uses.len() % 2 == 0, EntityRef::Edge(e))?;
        let mut last = b.edgeuse(required(uses.last().copied())?)?.orientation;
        for eu in uses {
            let data = b.edgeuse(eu)?;
            r.check(b, 63, data.orientation != last, EntityRef::Edgeuse(eu))?;
            last = data.orientation;
            r.check(b, 64, data.edge == Some(e), EntityRef::Edgeuse(eu))?;
            if let EdgeuseOwner::Shell(s) = data.owner {
                let s = required(s)?;
                r.check(
                    b,
                    65,
                    b.shell(s)?.shell_type != ShellType::Vertex,
                    EntityRef::Shell(s),
                )?;
            } else {
                let ccw = required(data.ccw)?;
                r.check(
                    b,
                    67,
                    b.edgeuse(ccw)?.cw == Some(eu),
                    EntityRef::Edgeuse(eu),
                )?;
                let cw = required(data.cw)?;
                r.check(
                    b,
                    69,
                    b.edgeuse(cw)?.ccw == Some(eu),
                    EntityRef::Edgeuse(eu),
                )?;
                if let Some(trim) = eu1::uv_trim_curve(b, eu)?.0 {
                    let f = required(eu1::face(b, eu)?)?;
                    let domain = b.face(f)?.uv_domain;
                    let ivl = b.edge(e)?.interval;
                    let mut point = Vec3::UNDEF;
                    for (index, t) in [
                        (70, ivl.min()),
                        (71, ivl.max()),
                        (72, (ivl.min() + ivl.max()) / 2.0),
                    ] {
                        match trim.evaluate_point(k, t) {
                            Ok(p) => point = p,
                            Err(err) if err.is_unported() => return Err(err),
                            Err(_) => {}
                        }
                        r.value(
                            b,
                            index,
                            domain.contains_point(Point2::new(point.x, point.y), 0.0001),
                            Object::Trim(eu),
                            0.0001,
                            UNDEF_DOUBLE,
                        )?;
                    }
                }
            }
            let vu = required(b.edgeuse(eu)?.vertexuse)?;
            r.check(
                b,
                74,
                matches!(b.vertexuse(vu)?.owner, VertexuseOwner::Edgeuse(_)),
                EntityRef::Vertexuse(vu),
            )?;
            r.check(
                b,
                75,
                b.vertexuse(vu)?.owner == VertexuseOwner::Edgeuse(Some(eu)),
                EntityRef::Edgeuse(eu),
            )?;
            let radial = cache.mate(b, eu, true)?;
            let radial_back = match radial {
                Some(x) => cache.mate(b, x, true)?,
                None => None,
            };
            r.check(b, 76, radial_back == Some(eu), EntityRef::Edgeuse(eu))?;
            let mate = cache.mate(b, eu, false)?;
            let mate_back = match mate {
                Some(x) => cache.mate(b, x, false)?,
                None => None,
            };
            r.check(b, 77, mate_back == Some(eu), EntityRef::Edgeuse(eu))?;
            if !edge::is_wire(b, e)? {
                let m = required(mate)?;
                let lu = required(eu1::loopuse(b, eu)?)?;
                let lm = required(eu1::loopuse(b, m)?)?;
                r.check(
                    b,
                    78,
                    b.loopuse(lu)?.loop_ == b.loopuse(lm)?.loop_,
                    EntityRef::Loopuse(lu),
                )?;
                let fu = required(b.loopuse(lu)?.faceuse)?;
                let fm = required(b.loopuse(lm)?.faceuse)?;
                r.check(
                    b,
                    81,
                    b.faceuse(fu)?.face == b.faceuse(fm)?.face,
                    EntityRef::Faceuse(fu),
                )?;
            }
        }
    }
    Ok(())
}

fn check_vertices(b: &Brep, r: &mut Reports<'_>) -> Result<()> {
    for &v in b.vertex_list() {
        let data = b.vertex(v)?;
        r.value(
            b,
            82,
            data.tolerance < b.tolerance() * 1000.0,
            EntityRef::Vertex(v),
            b.tolerance() * 1000.0,
            data.tolerance,
        )?;
        r.value(
            b,
            83,
            data.tolerance > b.tolerance() * 0.99,
            EntityRef::Vertex(v),
            b.tolerance() * 0.99,
            data.tolerance,
        )?;
        for &vu in &data.vertexuses {
            let data = b.vertexuse(vu)?;
            r.check(b, 84, data.vertex == Some(v), EntityRef::Vertexuse(vu))?;
            match data.owner {
                VertexuseOwner::Shell(s) => {
                    let s = required(s)?;
                    r.check(
                        b,
                        86,
                        b.shell(s)?.shell_type == ShellType::Vertex,
                        EntityRef::Shell(s),
                    )?;
                    r.check(
                        b,
                        87,
                        b.shell(s)?.members == ShellMembers::Vertexuse(vu),
                        EntityRef::Shell(s),
                    )?;
                }
                VertexuseOwner::Loopuse(lu) => {
                    let lu = required(lu)?;
                    r.check(
                        b,
                        89,
                        matches!(b.loopuse(lu)?.start, LoopuseStart::Vertexuse(_)),
                        EntityRef::Loopuse(lu),
                    )?;
                    r.check(
                        b,
                        90,
                        b.loopuse(lu)?.start == LoopuseStart::Vertexuse(Some(vu)),
                        EntityRef::Loopuse(lu),
                    )?;
                }
                VertexuseOwner::Edgeuse(eu) => {
                    let eu = required(eu)?;
                    r.check(
                        b,
                        92,
                        b.edgeuse(eu)?.vertexuse == Some(vu),
                        EntityRef::Edgeuse(eu),
                    )?;
                    r.check(b, 93, b.edgeuse(eu)?.edge.is_some(), EntityRef::Edgeuse(eu))?;
                }
                VertexuseOwner::Unknown => {}
            }
        }
    }
    Ok(())
}

fn check_edgeuse_geometry(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<()> {
    if eu1::uv_trim_curve(b, eu)?.0.is_none() {
        return Ok(());
    }
    let lu = required(eu1::loopuse(b, eu)?)?;
    let fu = required(b.loopuse(lu)?.faceuse)?;
    let f = required(b.faceuse(fu)?.face)?;
    let vu = required(b.edgeuse(eu)?.vertexuse)?;
    let v = required(b.vertexuse(vu)?.vertex)?;
    let cw = required(b.edgeuse(eu)?.cw)?;
    let uv = eu3::normalized_evaluate(k, b, eu, 0.0, true, false, true)?.point;
    // These evaluations can create UV curves or fail even though the following
    // distance tests only issue warnings in the release reference.
    eu3::normalized_evaluate(k, b, eu, 0.0, false, false, true)?;
    eu3::normalized_evaluate(k, b, cw, 1.0, true, false, true)?;
    eu3::normalized_evaluate(k, b, cw, 1.0, false, false, true)?;
    let point =
        required(b.face(f)?.surface.as_ref())?.evaluate_point(k, Point2::new(uv.x, uv.y))?;
    let _distance = point.distance(b.vertex(v)?.point);
    Ok(())
}

fn fix_reversed(k: &Kernel, c: &mut crate::curve::curve::Curve) -> Result<()> {
    fix_reversed_on(k, c, None)
}

/// Not upstream (WR-406, source issue #891; the source fork's
/// `nl_EndTurnsBack`): whether the control polygon `p0, p1, p2` at one end of a
/// curve turns back on itself. Without `metric` this is `N_CrvIsNotReversed`'s test;
/// with the first fundamental form `[E, F, G]` of a surface at that end the legs are
/// a UV curve's `(x, y)` steps and their lengths and angle are the surface's. A leg of
/// length 0 there turns back from nothing.
fn end_turns_back(p0: Vec3, p1: Vec3, p2: Vec3, metric: Option<[f64; 3]>) -> bool {
    let d1 = p1 - p0;
    let d2 = p2 - p1;
    let Some([e, f, g]) = metric else {
        let (d1, _) = crate::nurbs::geom::normalize(d1).unwrap_or((Vec3::new(0.0, 0.0, 0.0), 0.0));
        let (d2, mag) =
            crate::nurbs::geom::normalize(d2).unwrap_or((Vec3::new(0.0, 0.0, 0.0), 0.0));
        return d1.dot(d2) < -0.95 && mag > crate::nurbs::math::MODEL_TOL;
    };
    let aa = e * d1.x * d1.x + 2.0 * f * d1.x * d1.y + g * d1.y * d1.y;
    let bb = e * d2.x * d2.x + 2.0 * f * d2.x * d2.y + g * d2.y * d2.y;
    let ab = e * d1.x * d2.x + f * (d1.x * d2.y + d1.y * d2.x) + g * d1.y * d2.y;
    if aa.is_nan() || aa <= 0.0 || bb.is_nan() || bb <= 0.0 {
        return false;
    }
    let mag = bb.sqrt();
    let dot = ab / (aa.sqrt() * mag);
    dot < -0.95 && mag > crate::nurbs::math::MODEL_TOL
}

/// The first fundamental form `[E, F, G]` of `surface` at `uv`, its derivatives taken
/// on the given sides (the fork's `sm_FirstFundamentalForm`); `None` where the
/// surface cannot be evaluated.
fn first_fundamental_form(
    k: &Kernel,
    surface: &crate::surface::surface::Surface,
    uv: Point2,
    u_from_left: bool,
    v_from_left: bool,
) -> Result<Option<[f64; 3]>> {
    match surface.evaluate_1st_derivatives(k, uv, u_from_left, v_from_left) {
        Ok((_, du, dv)) => Ok(Some([du.dot(du), du.dot(dv), dv.dot(dv)])),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// `N_CrvIsNotReversed(Fixit=TRUE)`; with `surface` (WR-406, opt-in) a UV curve's turn
/// at each end is judged on the surface, from its first fundamental form there (the
/// fork's `PassesReverseDirectionCheckOn`): a trim whose end the surface cannot
/// evaluate is left unfixed, and a 3D curve takes the plain test.
fn fix_reversed_on(
    k: &Kernel,
    c: &mut crate::curve::curve::Curve,
    surface: Option<&crate::surface::surface::Surface>,
) -> Result<()> {
    use crate::curve::curve::CurveKind;
    let owner = c.owner();
    let edge_zone_tol_3d = c.owner_zone_tol_3d();
    let base = match c.kind_mut() {
        CurveKind::BSpline(base) => base,
        CurveKind::Line(line) => {
            if line.base().gw_nurb().is_none() {
                line.make_nurb()?;
            }
            line.base_mut()
        }
        CurveKind::Ellipse(ellipse) => {
            if ellipse.base().gw_nurb().is_none() {
                ellipse.make_nurb_owned_with_edge_tol(k, owner, edge_zone_tol_3d)?;
            }
            ellipse.base_mut()
        }
        CurveKind::Hyperbola(h) => {
            if h.base().gw_nurb().is_none() {
                h.make_nurb(owner)?;
            }
            h.base_mut()
        }
        CurveKind::Parabola(p) => {
            if p.base().gw_nurb().is_none() {
                p.make_nurb(owner)?;
            }
            p.base_mut()
        }
        _ => return Ok(()),
    };
    let surface = surface.filter(|_| base.dim == 2);
    let points = base.get_or_create_gw_nurb()?.cpts_mut();
    let n = points.len();
    if n < 5 {
        return Ok(());
    }
    // each end's form, on the side its first leg runs into
    let mut metrics = [None, None];
    if let Some(surface) = surface {
        for (slot, (a, m)) in metrics.iter_mut().zip([(0, 1), (n - 1, n - 2)]) {
            let end = points
                .get(a)
                .copied()
                .ok_or_else(|| invalid("control point"))?;
            let next = points
                .get(m)
                .copied()
                .ok_or_else(|| invalid("control point"))?;
            let (end, next) = (end.to_euclid(), next.to_euclid());
            let uv = Point2::new(end.x, end.y);
            let Some(form) =
                first_fundamental_form(k, surface, uv, next.x >= end.x, next.y >= end.y)?
            else {
                return Ok(());
            };
            *slot = Some(form);
        }
    }
    for ((a, m, z), metric) in [(0, 1, 2), (n - 1, n - 2, n - 3)].into_iter().zip(metrics) {
        let p0 = points
            .get(a)
            .copied()
            .ok_or_else(|| invalid("control point"))?
            .to_euclid();
        let p1 = points
            .get(m)
            .copied()
            .ok_or_else(|| invalid("control point"))?
            .to_euclid();
        let p2 = points
            .get(z)
            .copied()
            .ok_or_else(|| invalid("control point"))?
            .to_euclid();
        if end_turns_back(p0, p1, p2, metric) {
            let p = points.get_mut(m).ok_or_else(|| invalid("control point"))?;
            p.x = (p0.x + p2.x) / 2.0;
            p.y = (p0.y + p2.y) / 2.0;
            if p.z != crate::nurbs::math::NOZ {
                p.z = (p0.z + p2.z) / 2.0;
            }
            if p.w != crate::nurbs::math::NOW {
                p.w = 1.0;
            }
        }
    }
    Ok(())
}

fn check_edge_geometry(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    cache: &Lists,
    keys: &dyn Fn(EntityRef) -> crate::geom::object::ObjectRef,
) -> Result<()> {
    let primary = required(b.edge(e)?.edgeuses.first().copied())?;
    let eu = if b.edgeuse(primary)?.orientation == OrientType::Same {
        primary
    } else {
        required(cache.mate(b, primary, false)?)?
    };
    let vu = required(b.edgeuse(eu)?.vertexuse)?;
    let start = required(b.vertexuse(vu)?.vertex)?;
    let end = required(edge::other_vertex(b, e, Some(start))?)?;
    fix_reversed(k, required(b.edge_mut(e)?.curve.as_mut())?)?;
    let curve = required(b.edge(e)?.curve.as_ref())?;
    // The owner naming scheme belongs to the caller; the type and edge-owner
    // identity resolved by that caller are kept by the curve itself.
    if curve.owner() != Some(keys(EntityRef::Edge(e))) {
        return Err(crate::SolidError::sm_err(super::OP));
    }
    let ivl = b.edge(e)?.interval;
    let p0 = curve.evaluate_point(k, ivl.min())?;
    let p1 = curve.evaluate_point(k, ivl.max())?;
    for (point, v) in [(p0, start), (p1, end)] {
        if point.distance(b.vertex(v)?.point) > b.vertex(v)?.tolerance + b.edge(e)?.tolerance {
            return Err(crate::SolidError::sm_err(super::OP));
        }
    }
    // Interval and UV-distance mismatches only log warnings; they do not change
    // this function's status. Retain their actual evaluations and reversal edits.
    for eu in b.edge(e)?.edgeuses.clone() {
        if let Some(trim) = b.edgeuse(eu)?.uv_trim_curve.as_ref() {
            let f = required(eu1::face(b, eu)?)?;
            let s = required(b.face(f)?.surface.as_ref())?;
            let domain = trim.natural_interval();
            let on_surface = crate::curve::crv_on_surf::CrvOnSurf::new(
                k,
                trim.clone_for_evaluation()?,
                s.clone_for_evaluation()?,
                None,
                0,
            )?;
            let on_surface = crate::curve::curve::Curve::from(on_surface);
            if let Err(err) = required(b.edge(e)?.curve.as_ref())?.curve_max_distance_between(
                k,
                &ivl,
                &on_surface,
                domain.min(),
                domain.max(),
                51,
                None,
            ) {
                if err.is_unported() {
                    return Err(err);
                }
            }
            // not upstream (WR-406, #891, opt-in): the turn judged on the face's surface
            let surface = if k.fixes.uv_trim_reversal_on_surface {
                Some(required(b.face(f)?.surface.as_ref())?.clone_for_evaluation()?)
            } else {
                None
            };
            fix_reversed_on(
                k,
                required(b.edgeuse_mut(eu)?.uv_trim_curve.as_mut())?,
                surface.as_ref(),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod issue_891_tests {
    use super::*;
    use crate::curve::curve::{Curve, CurveKind};
    use crate::nurbs::b_spline_curve::p1::{
        BSplineCurve, BSplineCurveForm, CanonicalData, KnotType,
    };
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::surface::surface::Surface;

    /// A half circle of radius 1 about (30, 2) on a plane spanning 60 x 4 over
    /// [0,1]^2, starting at 35 degrees, as the exact rational trim: its polygon is
    /// square on the plane but folds back by about 170 degrees in (u, v).
    fn trim() -> Curve {
        let (cx, cy, r) = (30.0_f64, 2.0_f64, 1.0_f64);
        let point = |deg: f64, scale: f64| {
            let t = deg.to_radians();
            Vec3::new(
                (cx + scale * r * t.cos()) / 60.0,
                (cy + scale * r * t.sin()) / 4.0,
                0.0,
            )
        };
        let s = 2.0_f64.sqrt();
        let control = [
            point(35.0, 1.0),
            point(80.0, s),
            point(125.0, 1.0),
            point(170.0, s),
            point(215.0, 1.0),
        ];
        let w = 0.5_f64.sqrt();
        let curve = BSplineCurve::create_canonical(&CanonicalData {
            dim: 2,
            degree: 2,
            control_points: &control,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &[3, 2, 3],
            knots: &[0.0, 0.5, 1.0],
            knot_type: KnotType::Unspecified,
            weights: Some(&[1.0, w, 1.0, w, 1.0]),
            trim: None,
        })
        .unwrap();
        Curve::from(curve)
    }

    fn middle_control_point(c: &mut Curve) -> Vec3 {
        let CurveKind::BSpline(base) = c.kind_mut() else {
            panic!("a B-spline trim");
        };
        base.get_or_create_gw_nurb().unwrap().cpts_mut()[1].to_euclid()
    }

    /// The plain check takes the exact trim for a reversal and moves its second
    /// control point onto the chord; judged on the surface it is left as it is.
    #[test]
    fn issue_891_exact_trim_is_not_straightened() {
        let k = Kernel::default();
        let plane = Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(60.0, 0.0, 0.0),
                Vec3::new(0.0, 4.0, 0.0),
                Vec3::new(60.0, 4.0, 0.0),
            )
            .unwrap(),
        );

        let mut plain = trim();
        let before = middle_control_point(&mut plain);
        fix_reversed(&k, &mut plain).unwrap();
        assert_ne!(middle_control_point(&mut plain), before);

        let mut on_surface = trim();
        fix_reversed_on(&k, &mut on_surface, Some(&plane)).unwrap();
        assert_eq!(middle_control_point(&mut on_surface), before);
    }

    fn key(e: EntityRef) -> crate::geom::object::ObjectRef {
        let (type_code, kind, i) = match e {
            EntityRef::Brep(x) => (16011, 0, x.index()),
            EntityRef::Region(x) => (16012, 1, x.index()),
            EntityRef::Shell(x) => (16013, 2, x.index()),
            EntityRef::Faceuse(x) => (16022, 3, x.index()),
            EntityRef::Face(x) => (16002, 4, x.index()),
            EntityRef::Loopuse(x) => (16004, 5, x.index()),
            EntityRef::Loop(x) => (16003, 6, x.index()),
            EntityRef::Edgeuse(x) => (16005, 7, x.index()),
            EntityRef::Edge(x) => (16016, 8, x.index()),
            EntityRef::Vertexuse(x) => (16006, 9, x.index()),
            EntityRef::Vertex(x) => (16021, 10, x.index()),
        };
        crate::geom::object::ObjectRef {
            type_code,
            key: crate::geom::attribute::ObjectKey(1 + (kind << 32) + u64::from(i)),
        }
    }

    /// A one-face B-rep on the 60 x 4 plane over [0,1]^2 bounded by the exact
    /// half circle trim of [`trim`] and the chord closing it, made through
    /// `MakeFaceWithCurves` from its UV curves.
    fn half_disc(k: &mut Kernel) -> Brep {
        use crate::geom::attribute::AttributeStore;
        use crate::topology::brep::p7::{make_face_with_curves, FaceCurvesInput};
        use crate::topology::edge as ed;
        use crate::topology::Region;
        let plane = Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(60.0, 0.0, 0.0),
                Vec3::new(0.0, 4.0, 0.0),
                Vec3::new(60.0, 4.0, 0.0),
            )
            .unwrap(),
        );
        let arc = trim();
        let ends = arc.ends(k).unwrap();
        let chord = Curve::from(
            BSplineCurve::create_canonical(&CanonicalData {
                dim: 2,
                degree: 1,
                control_points: &[ends.end, ends.start],
                form: BSplineCurveForm::Unspecified,
                knot_multiplicities: &[2, 2],
                knots: &[0.0, 1.0],
                knot_type: KnotType::Unspecified,
                weights: None,
                trim: None,
            })
            .unwrap(),
        );
        let mut b = Brep::new();
        let region = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = AttributeStore::default();
        let mut env = ed::NotifyEnv {
            records: &mut records,
            attributes: &mut attributes,
            context: None,
            key_of: &key,
        };
        let face_key = |f: crate::topology::FaceId| key(EntityRef::Face(f));
        b.set_infinite_region(Some(region));
        let made = make_face_with_curves(
            k,
            &mut b,
            None,
            FaceCurvesInput {
                loops: vec![2],
                uv_curves: Some(vec![arc.clone(), chord]),
                orientations: vec![OrientType::Same, OrientType::Same],
                ..Default::default()
            },
            plane,
            &crate::geom::extent2d::Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            OrientType::Same,
            None,
            &face_key,
            &mut env,
        )
        .unwrap();
        assert!(made.face.is_some());
        // MakeFaceWithCurves leaves the trims unset: give the arc's face edge uses
        // the exact trim (the edge whose middle is 1 from the circle's centre)
        for e in b.edge_list().to_vec() {
            let ivl = b.edge(e).unwrap().interval;
            let mid = b.edge(e).unwrap().curve.as_ref().unwrap();
            let mid = mid
                .evaluate_point(k, 0.5 * (ivl.min() + ivl.max()))
                .unwrap();
            if (mid.distance(Vec3::new(30.0, 2.0, 0.0)) - 1.0).abs() > 1e-9 {
                continue;
            }
            for eu in b.edge(e).unwrap().edgeuses.clone() {
                if eu1::face(&b, eu).unwrap().is_some() {
                    b.edgeuse_mut(eu).unwrap().uv_trim_curve = Some(arc.clone());
                }
            }
        }
        b
    }

    /// The middle control points of the B-rep's rational UV trims.
    fn rational_trim_middles(b: &mut Brep) -> Vec<Vec3> {
        let mut out = Vec::new();
        for e in b.edge_list().to_vec() {
            for eu in b.edge(e).unwrap().edgeuses.clone() {
                if let Some(t) = b.edgeuse_mut(eu).unwrap().uv_trim_curve.as_mut() {
                    let CurveKind::BSpline(base) = t.kind_mut() else {
                        continue;
                    };
                    let nurb = base.get_or_create_gw_nurb().unwrap();
                    if nurb.cpts().len() == 5 {
                        out.push(nurb.cpts()[1].to_euclid());
                    }
                }
            }
        }
        out
    }

    /// `ValidatePointers` on [`half_disc`] with `fixes.uv_trim_reversal_on_surface`
    /// as given: its status and the arc trims' middle control points before and
    /// after.
    fn validate_half_disc(on_surface: bool) -> (i32, Vec<Vec3>, Vec<Vec3>) {
        let mut k = Kernel::default();
        k.fixes.uv_trim_reversal_on_surface = on_surface;
        let mut b = half_disc(&mut k);
        let before = rational_trim_middles(&mut b);
        let mut reports = AssertArray::new();
        let status = validate_pointers(&k, &mut b, Some(&mut reports), 2, true, &key).unwrap();
        assert!(reports.is_empty(), "{:?}", reports.reports());
        (status, before, rational_trim_middles(&mut b))
    }

    /// WR-406 (source issue #891) through the Kernel: with
    /// `fixes.uv_trim_reversal_on_surface`, `ValidatePointers` judges the exact
    /// half circle trim's end turns on the face's 60 x 4 plane and leaves it exact.
    #[test]
    fn issue_891_kernel_fix_keeps_the_exact_trim() {
        let (status, before, after) = validate_half_disc(true);
        assert_eq!(status, SUCCESS);
        assert_eq!(before.len(), 2);
        assert_eq!(after, before);
        assert_eq!(after[0], middle_control_point(&mut trim()));
    }

    /// The default (upstream) judges the turn in raw (u, v), takes the exact trim
    /// for a reversal and moves its second control point onto the chord.
    #[test]
    fn validate_pointers_straightens_the_exact_trim_by_default() {
        assert!(!Kernel::default().fixes.uv_trim_reversal_on_surface);
        let (status, before, after) = validate_half_disc(false);
        assert_eq!(status, SUCCESS);
        assert_eq!(before.len(), 2);
        assert_eq!(after.len(), 2);
        for (a, b) in after.iter().zip(&before) {
            assert_ne!(a, b);
        }
    }
}
