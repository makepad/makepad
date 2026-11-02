//! The base-class checks every topology class runs first.
//!
//! Upstream: `source/SMLib/src/SmTopology.cpp`, `SmOwningTopology.cpp`,
//! `SmSAGObject.cpp`, `SmAObject.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `topology` | `SmTopology::AssertValid` |
//! | `owning` | `SmOwningTopology::AssertValid` |
//! | `sag_object` | `SmSAGObject::AssertValid` |
//! | `Index` | the owners' doubly linked lists (`IsInList`) |
//!
//! Deviations: the list rings, contexts and marks are intrinsic to the arenas (tests
//! 1-8 of `SmTopology` and 0, 1, 3-5 of `SmOwningTopology` always pass). List
//! membership (`SmTopology` test 0) and the children's owner back links
//! (`SmOwningTopology` test 2) are checked against an index built once per run instead
//! of walking each owner's list for every member (upstream is quadratic).

use std::collections::BTreeSet;

use super::{class, Cx};
use crate::error::{EntityRef, Result};
use crate::geom::a_object;
use crate::topology::shell::ShellMembers;
use crate::topology::{Brep, BrepId, EdgeuseOwner};

/// The owner of a topology object's list (`m_pListOwner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    /// A topology object that owns the list.
    Entity(EntityRef),
    /// The B-rep's face list head (`m_pFaceListHead`).
    FaceList,
    /// The B-rep's edge list head (`m_pEdgeListHead`).
    EdgeList,
    /// The B-rep's vertex list head (`m_pVertexListHead`).
    VertexList,
}

/// Every (owner, member) pair of the B-rep's lists.
pub(super) struct Index {
    members: BTreeSet<(Owner, EntityRef)>,
}

impl Index {
    /// Indexes the lists of `b`.
    pub(super) fn new(b: &Brep) -> Self {
        let mut members = BTreeSet::new();
        let brep = Owner::Entity(EntityRef::Brep(BrepId::from_index(0)));
        for r in b.region_links() {
            members.insert((brep, EntityRef::Region(r)));
        }
        for (r, region) in b.regions().iter() {
            for &s in &region.shells {
                members.insert((Owner::Entity(EntityRef::Region(r)), EntityRef::Shell(s)));
            }
        }
        for (s, shell) in b.shells().iter() {
            if let ShellMembers::Faceuses(fus) = &shell.members {
                for &fu in fus {
                    members.insert((Owner::Entity(EntityRef::Shell(s)), EntityRef::Faceuse(fu)));
                }
            }
        }
        for (fu, faceuse) in b.faceuses().iter() {
            for &lu in &faceuse.loopuses {
                members.insert((
                    Owner::Entity(EntityRef::Faceuse(fu)),
                    EntityRef::Loopuse(lu),
                ));
            }
        }
        for &f in b.face_list() {
            members.insert((Owner::FaceList, EntityRef::Face(f)));
        }
        for (f, face) in b.faces().iter() {
            for &l in &face.loops {
                members.insert((Owner::Entity(EntityRef::Face(f)), EntityRef::Loop(l)));
            }
        }
        for &e in b.edge_list() {
            members.insert((Owner::EdgeList, EntityRef::Edge(e)));
        }
        for (e, edge) in b.edges().iter() {
            for &eu in &edge.edgeuses {
                members.insert((Owner::Entity(EntityRef::Edge(e)), EntityRef::Edgeuse(eu)));
            }
        }
        for &v in b.vertex_list() {
            members.insert((Owner::VertexList, EntityRef::Vertex(v)));
        }
        for (v, vertex) in b.vertices().iter() {
            for &vu in &vertex.vertexuses {
                members.insert((
                    Owner::Entity(EntityRef::Vertex(v)),
                    EntityRef::Vertexuse(vu),
                ));
            }
        }
        Self { members }
    }

    fn contains(&self, owner: Owner, member: EntityRef) -> bool {
        self.members.contains(&(owner, member))
    }
}

/// `m_pListOwner` of `e`.
fn list_owner(b: &Brep, e: EntityRef) -> Result<Option<Owner>> {
    Ok(match e {
        EntityRef::Brep(_) => None,
        EntityRef::Region(_) => Some(Owner::Entity(EntityRef::Brep(BrepId::from_index(0)))),
        EntityRef::Shell(s) => b
            .shell(s)?
            .region
            .map(|r| Owner::Entity(EntityRef::Region(r))),
        EntityRef::Faceuse(fu) => b
            .faceuse(fu)?
            .shell
            .map(|s| Owner::Entity(EntityRef::Shell(s))),
        EntityRef::Face(_) => Some(Owner::FaceList),
        EntityRef::Loop(l) => Some(Owner::Entity(EntityRef::Face(b.loop_(l)?.face))),
        EntityRef::Loopuse(lu) => b
            .loopuse(lu)?
            .faceuse
            .map(|fu| Owner::Entity(EntityRef::Faceuse(fu))),
        EntityRef::Edge(_) => Some(Owner::EdgeList),
        EntityRef::Edgeuse(eu) => b
            .edgeuse(eu)?
            .edge
            .map(|e| Owner::Entity(EntityRef::Edge(e))),
        EntityRef::Vertex(_) => Some(Owner::VertexList),
        EntityRef::Vertexuse(vu) => b
            .vertexuse(vu)?
            .vertex
            .map(|v| Owner::Entity(EntityRef::Vertex(v))),
    })
}

/// `SmTopology::AssertValid` of `e` (its `SmAObject` base first).
pub(super) fn topology(b: &Brep, cx: &mut Cx<'_>, e: EntityRef) -> Result<bool> {
    const C: i64 = class::TOPOLOGY;
    let mut rtn = true;

    // call the base class AssertValid
    rtn &= if cx.bases() {
        a_object::assert_valid(false)
    } else {
        true
    };

    // when this object is a member of an Owner's list
    if let Some(owner) = list_owner(b, e)? {
        // make sure this object is on owner's list
        let ok = !cx.rep.need(0) || cx.index.contains(owner, e);
        rtn &= cx.rep.this_bool(b, C, e, 0, ok)?;
        // the owner and this object should have the same context; this object's marks
        // are at most the context's (intrinsic)
        for test in 1..=4 {
            rtn &= cx.rep.this_bool(b, C, e, test, true)?;
        }
    } else {
        // should not be part of a peer list (intrinsic: arena lists have no peers)
        rtn &= cx.rep.this_bool(b, C, e, 5, true)?;
        rtn &= cx.rep.this_bool(b, C, e, 6, true)?;
    }

    // the doubly-linked list is a ring (intrinsic) and the owner's count is its length
    rtn &= cx.rep.this_bool(b, C, e, 7, true)?;
    rtn &= cx.rep.this_bool(b, C, e, 8, true)?;

    // all done
    Ok(rtn)
}

/// The members of `e`'s owned list (`m_pList`) and whether `TypicalListOwnerUse`
/// holds (every member's `m_pListOwner` is `e`).
fn owned(b: &Brep, e: EntityRef) -> Result<(Vec<EntityRef>, bool)> {
    Ok(match e {
        EntityRef::Brep(_) => (
            crate::topology::brep::p1::regions(b, None)?
                .into_iter()
                .map(EntityRef::Region)
                .collect(),
            true,
        ),
        EntityRef::Region(r) => (
            b.region(r)?
                .shells
                .iter()
                .map(|&s| EntityRef::Shell(s))
                .collect(),
            true,
        ),
        EntityRef::Shell(s) => match &b.shell(s)?.members {
            ShellMembers::Faceuses(fus) => {
                (fus.iter().map(|&fu| EntityRef::Faceuse(fu)).collect(), true)
            }
            ShellMembers::Edgeuse(eu) => (vec![EntityRef::Edgeuse(*eu)], false),
            ShellMembers::Vertexuse(vu) => (vec![EntityRef::Vertexuse(*vu)], false),
            ShellMembers::Empty => (Vec::new(), false),
        },
        EntityRef::Faceuse(fu) => (
            b.faceuse(fu)?
                .loopuses
                .iter()
                .map(|&lu| EntityRef::Loopuse(lu))
                .collect(),
            true,
        ),
        EntityRef::Edge(x) => (
            b.edge(x)?
                .edgeuses
                .iter()
                .map(|&eu| EntityRef::Edgeuse(eu))
                .collect(),
            true,
        ),
        EntityRef::Vertex(v) => (
            b.vertex(v)?
                .vertexuses
                .iter()
                .map(|&vu| EntityRef::Vertexuse(vu))
                .collect(),
            true,
        ),
        _ => (Vec::new(), true),
    })
}

/// Whether `member`'s `m_pListOwner` is `owner`.
fn owned_by(b: &Brep, member: EntityRef, owner: EntityRef) -> Result<bool> {
    Ok(match (member, owner) {
        (EntityRef::Region(r), EntityRef::Brep(_)) => b.region(r)?.in_brep,
        (EntityRef::Shell(s), EntityRef::Region(r)) => b.shell(s)?.region == Some(r),
        (EntityRef::Faceuse(fu), EntityRef::Shell(s)) => b.faceuse(fu)?.shell == Some(s),
        (EntityRef::Loopuse(lu), EntityRef::Faceuse(fu)) => b.loopuse(lu)?.faceuse == Some(fu),
        (EntityRef::Edgeuse(eu), EntityRef::Edge(x)) => b.edgeuse(eu)?.edge == Some(x),
        (EntityRef::Vertexuse(vu), EntityRef::Vertex(v)) => b.vertexuse(vu)?.vertex == Some(v),
        (EntityRef::Edgeuse(eu), EntityRef::Shell(s)) => {
            b.edgeuse(eu)?.owner == EdgeuseOwner::Shell(Some(s))
        }
        _ => false,
    })
}

/// `SmOwningTopology::AssertValid` of `e` (its `SmTopology` base first).
pub(super) fn owning(b: &Brep, cx: &mut Cx<'_>, e: EntityRef) -> Result<bool> {
    const C: i64 = class::OWNING_TOPOLOGY;
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= topology(b, cx, e)?;
    }

    // when OwningTopology has a list of children
    let (members, typical) = owned(b, e)?;
    if members.is_empty() {
        // has no list: the list size is zero (intrinsic)
        rtn &= cx.rep.this_bool(b, C, e, 5, true)?;
    } else {
        let size = members.len();
        for (count, &member) in members.iter().enumerate() {
            // check next/last pointer reciprocity (intrinsic)
            rtn &= cx.rep.this_bool(b, C, e, 1, true)?;
            // check owner pointers
            if typical {
                let ok = !cx.rep.need(0) || owned_by(b, member, e)?;
                rtn &= cx.rep.this_bool(b, C, e, 2, ok)?;
            }
            // check list length (intrinsic: the last member closes the ring)
            if count + 1 == size {
                rtn &= cx.rep.this_bool(b, C, e, 3, true)?;
            }
            // check for common contexts (intrinsic)
            rtn &= cx.rep.this_bool(b, C, e, 4, true)?;
        }
    }

    // all done
    Ok(rtn)
}

/// `SmSAGObject::AssertValid` of the B-rep (its `SmOwningTopology` base; no checks of
/// its own).
pub(super) fn sag_object(b: &Brep, cx: &mut Cx<'_>) -> Result<bool> {
    let mut rtn = true;
    if cx.bases() {
        rtn &= owning(b, cx, EntityRef::Brep(BrepId::from_index(0)))?;
    }
    Ok(rtn)
}
