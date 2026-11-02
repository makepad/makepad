//! `SmOwningTopology`: the doubly linked, circular list of subordinate topology objects
//! an owner keeps (`m_pList`, `m_lListSize`) and the list operations on it.
//!
//! The list links themselves (`m_pListOwner`, `m_pNext`, `m_pLast`) live in each
//! member's [`Topology`] record; this module edits them through a [`TopologyRecords`]
//! store, exactly as the friend class edits them upstream.

use std::collections::BTreeSet;

use crate::error::{Result, SolidError};
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::topology::topology::{self as topo, Topology, TopologyRecords};

const OP_REMOVE: &str = "topology::proto_topology::p1::OwningTopology::remove";
const OP_INSERT_AFTER: &str = "topology::proto_topology::p1::OwningTopology::insert_after";
const OP_UPDATE_SIZE: &str = "topology::proto_topology::p1::OwningTopology::update_list_size";
const OP_GET_ALL: &str = "topology::proto_topology::p1::OwningTopology::get_all";
const OP_LIST: &str = "topology::proto_topology::p1::OwningTopology";

/// `SmOwningTopology_TYPE` (`TOPO_BASE_TYPE + 10`).
pub const TYPE: i64 = 16010;

/// `SmOwningTopology::GetTypeString`.
pub const TYPE_STRING: &str = "SmOwningTopology_TYPE";

/// `SmOwningTopology::GetClassString`.
pub const CLASS_STRING: &str = "SmOwningTopology";

/// `SmOwningTopology::IsKindOf`: [`TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    if t == TYPE {
        true
    } else {
        topo::is_kind_of(t)
    }
}

/// The list an owner keeps (`SmOwningTopology`'s members): the first member and the
/// member count. Both are plain data; the links of the members are in their
/// [`Topology`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OwningTopology {
    /// `m_pList`.
    list: Option<ObjectKey>,
    /// `m_lListSize`.
    list_size: u64,
}

/// A store of topology records that also holds the list data of the owners
/// (`SmOwningTopology` objects) among them.
pub trait OwningRecords: TopologyRecords {
    /// The list data of `key`, if it is an owning object.
    fn owning(&self, key: ObjectKey) -> Option<OwningTopology>;
    /// Replaces the list data of `key`; `None` if it is not an owning object.
    fn set_owning(&mut self, key: ObjectKey, owning: OwningTopology) -> Option<()>;
}

/// A store without owning objects (plain topology records).
impl OwningRecords for crate::topology::topology::NotifyRecords {
    fn owning(&self, _key: ObjectKey) -> Option<OwningTopology> {
        None
    }

    fn set_owning(&mut self, _key: ObjectKey, _owning: OwningTopology) -> Option<()> {
        None
    }
}

/// The error for a list walk that dereferences a missing object or link (a `NULL`
/// pointer upstream) or never returns to the list head (upstream loops forever).
const fn broken_list(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "list link is null, unknown or does not return to the list head",
    }
}

fn rec<R: TopologyRecords + ?Sized>(
    records: &R,
    key: ObjectKey,
    operation: &'static str,
) -> Result<Topology> {
    records.topology(key).copied().ok_or(broken_list(operation))
}

fn rec_mut<'r, R: TopologyRecords + ?Sized>(
    records: &'r mut R,
    key: ObjectKey,
    operation: &'static str,
) -> Result<&'r mut Topology> {
    records.topology_mut(key).ok_or(broken_list(operation))
}

fn set_next<R: TopologyRecords + ?Sized>(
    records: &mut R,
    key: ObjectKey,
    next: Option<ObjectKey>,
    operation: &'static str,
) -> Result<()> {
    let r = rec_mut(records, key, operation)?;
    let (owner, last) = (r.owner(), r.last());
    r.set_list_links(owner, next, last);
    Ok(())
}

fn set_last<R: TopologyRecords + ?Sized>(
    records: &mut R,
    key: ObjectKey,
    last: Option<ObjectKey>,
    operation: &'static str,
) -> Result<()> {
    let r = rec_mut(records, key, operation)?;
    let (owner, next) = (r.owner(), r.next());
    r.set_list_links(owner, next, last);
    Ok(())
}

fn set_owner<R: TopologyRecords + ?Sized>(
    records: &mut R,
    key: ObjectKey,
    owner: Option<ObjectKey>,
    operation: &'static str,
) -> Result<()> {
    let r = rec_mut(records, key, operation)?;
    let (next, last) = (r.next(), r.last());
    r.set_list_links(owner, next, last);
    Ok(())
}

/// Guards a list walk: an error when `key` was visited before (the walk would never
/// return to the head upstream).
fn visit(seen: &mut BTreeSet<ObjectKey>, key: ObjectKey, operation: &'static str) -> Result<()> {
    if seen.insert(key) {
        Ok(())
    } else {
        Err(broken_list(operation))
    }
}

impl OwningTopology {
    /// An owner with an empty list (`SmOwningTopology()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            list: None,
            list_size: 0,
        }
    }

    /// The member count (`GetSize`).
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.list_size
    }

    /// The first member (`GetList`).
    #[must_use]
    pub const fn list(&self) -> Option<ObjectKey> {
        self.list
    }

    /// Sets the first member without touching any link (`SetList`).
    pub fn set_list(&mut self, list_head: Option<ObjectKey>) {
        self.list = list_head;
    }

    /// The destructor `~SmOwningTopology`: forgets the list (the members keep their
    /// links).
    pub fn destroy(&mut self) {
        self.list = None;
        self.list_size = 0;
    }

    /// Recounts the members by walking the list (`UpdateListSize`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a member's successor does not point back to it (the count then
    /// holds the members walked so far); [`SolidError::InvalidInput`] for a list that
    /// upstream cannot walk (a null link past the head, an unknown member, or a walk
    /// that never returns to the head).
    pub fn update_list_size<R: TopologyRecords + ?Sized>(&mut self, records: &R) -> Result<()> {
        self.list_size = 0;
        if let Some(head) = self.list {
            let mut elem = head;
            let mut seen = BTreeSet::new();
            if rec(records, elem, OP_UPDATE_SIZE)?.next().is_some() {
                loop {
                    visit(&mut seen, elem, OP_UPDATE_SIZE)?;
                    let next = rec(records, elem, OP_UPDATE_SIZE)?
                        .next()
                        .ok_or(broken_list(OP_UPDATE_SIZE))?;
                    // check back pointers
                    if rec(records, next, OP_UPDATE_SIZE)?.last() != Some(elem) {
                        return Err(SolidError::sm_err(OP_UPDATE_SIZE));
                    }
                    elem = next;
                    self.list_size += 1;
                    if elem == head {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Inserts `to_insert` after `on_list` (`InsertAfter`). The new member takes
    /// `on_list`'s owner, while this owner's count grows.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either object is missing or `on_list` has no successor;
    /// [`SolidError::InvalidInput`] for an object without a record.
    pub fn insert_after<R: TopologyRecords + ?Sized>(
        &mut self,
        records: &mut R,
        to_insert: Option<ObjectKey>,
        on_list: Option<ObjectKey>,
    ) -> Result<()> {
        let (Some(ins), Some(cur)) = (to_insert, on_list) else {
            return Err(SolidError::sm_err(OP_INSERT_AFTER));
        };
        let Some(cur_next) = rec(records, cur, OP_INSERT_AFTER)?.next() else {
            return Err(SolidError::sm_err(OP_INSERT_AFTER));
        };
        rec(records, ins, OP_INSERT_AFTER)?;
        set_next(records, ins, Some(cur_next), OP_INSERT_AFTER)?;
        set_last(records, cur_next, Some(ins), OP_INSERT_AFTER)?;
        set_last(records, ins, Some(cur), OP_INSERT_AFTER)?;
        set_next(records, cur, Some(ins), OP_INSERT_AFTER)?;
        let owner = rec(records, cur, OP_INSERT_AFTER)?.owner();
        set_owner(records, ins, owner, OP_INSERT_AFTER)?;
        self.list_size += 1;
        Ok(())
    }

    /// Inserts `obj` at the front of the list, owned by `this` (`PreInsert`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an object without a record or a list head
    /// without a predecessor (a `NULL` dereference upstream).
    pub fn pre_insert<R: TopologyRecords + ?Sized>(
        &mut self,
        this: ObjectKey,
        records: &mut R,
        obj: ObjectKey,
    ) -> Result<()> {
        rec(records, obj, OP_LIST)?;
        match self.list {
            None => {
                set_next(records, obj, Some(obj), OP_LIST)?;
                set_last(records, obj, Some(obj), OP_LIST)?;
            }
            Some(head) => {
                set_next(records, obj, Some(head), OP_LIST)?;
                let p_last = rec(records, head, OP_LIST)?
                    .last()
                    .ok_or(broken_list(OP_LIST))?;
                set_last(records, obj, Some(p_last), OP_LIST)?;
                set_next(records, p_last, Some(obj), OP_LIST)?;
                set_last(records, head, Some(obj), OP_LIST)?;
            }
        }

        // tell pObjectToInsert that it belongs to this
        set_owner(records, obj, Some(this), OP_LIST)?;

        // this->m_pList can point to any list member - let it point to the most
        // recently added Object
        self.list = Some(obj);

        // increment the list length count
        self.list_size += 1;
        Ok(())
    }

    /// Inserts `obj` at the end of the list, owned by `this` (`PostInsert`).
    ///
    /// # Errors
    ///
    /// Those of [`OwningTopology::pre_insert`].
    pub fn post_insert<R: TopologyRecords + ?Sized>(
        &mut self,
        this: ObjectKey,
        records: &mut R,
        obj: ObjectKey,
    ) -> Result<()> {
        self.pre_insert(this, records, obj)?;
        self.list = rec(records, obj, OP_LIST)?.next();
        Ok(())
    }

    /// Unlinks `obj` from this owner's list, clearing its links (`Remove`); nothing
    /// is deleted.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `obj` is not owned by `this` or has a null link;
    /// [`SolidError::InvalidInput`] for an object without a record.
    pub fn remove<R: TopologyRecords + ?Sized>(
        &mut self,
        this: ObjectKey,
        records: &mut R,
        obj: ObjectKey,
    ) -> Result<()> {
        let r = rec(records, obj, OP_REMOVE)?;

        // check state - Object must be in our list.
        if r.owner() != Some(this) {
            return Err(SolidError::sm_err(OP_REMOVE));
        }

        // check state - look for problem circular list and ListOwner Ptrs
        let (Some(next), Some(last)) = (r.next(), r.last()) else {
            return Err(SolidError::sm_err(OP_REMOVE));
        };

        if next == obj {
            // List has just one item
            self.list_size = self.list_size.wrapping_sub(1);
            self.list = None;
            rec_mut(records, obj, OP_REMOVE)?.set_list_links(None, None, None);
        } else {
            // Handle case where this is the list head
            if self.list == Some(obj) {
                self.list = Some(next);
            }

            // Now remove this from double linked list
            set_last(records, next, Some(last), OP_REMOVE)?;
            let next_now = rec(records, obj, OP_REMOVE)?.next();
            set_next(records, last, next_now, OP_REMOVE)?;

            rec_mut(records, obj, OP_REMOVE)?.set_list_links(None, None, None);
            self.list_size = self.list_size.wrapping_sub(1);
        }
        Ok(())
    }

    /// Whether `obj` is on the list (`IsInList`), and the index it was found at, or
    /// the number of members walked when it was not found.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown member or a walk that never
    /// returns to the head (upstream loops forever).
    pub fn is_in_list<R: TopologyRecords + ?Sized>(
        &self,
        records: &R,
        obj: Option<ObjectKey>,
    ) -> Result<(bool, u64)> {
        let mut index = 0u64;
        let mut seen = BTreeSet::new();
        let mut tgt = self.list;
        while let Some(t) = tgt {
            // look for this object
            if obj == Some(t) {
                return Ok((true, index));
            }
            visit(&mut seen, t, OP_LIST)?;

            // increment iterator - check for exit
            tgt = rec(records, t, OP_LIST)?.next();
            index += 1;
            if tgt == self.list {
                break;
            }
        }
        Ok((false, index))
    }

    /// The members in list order (`GetAll`), optionally only those carrying an
    /// attribute with id `attribute_id` and/or equal to `target`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the unfiltered walk finds more members than the count (see
    /// [`OwningTopology::get_all_ignoring_status`] for the members collected up to
    /// then); [`SolidError::InvalidInput`] as [`OwningTopology::is_in_list`].
    pub fn get_all<R: TopologyRecords + ?Sized>(
        &self,
        records: &R,
        attributes: &AttributeStore,
        attribute_id: Option<u64>,
        target: Option<ObjectKey>,
    ) -> Result<Vec<ObjectKey>> {
        let (items, status) =
            self.get_all_with_status(records, attributes, attribute_id, target)?;
        status.map(|()| items)
    }

    /// [`OwningTopology::get_all`] for a caller that ignores its status: the members
    /// collected, also when the walk overruns the count.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] as [`OwningTopology::is_in_list`].
    pub fn get_all_ignoring_status<R: TopologyRecords + ?Sized>(
        &self,
        records: &R,
        attributes: &AttributeStore,
        attribute_id: Option<u64>,
        target: Option<ObjectKey>,
    ) -> Result<Vec<ObjectKey>> {
        self.get_all_with_status(records, attributes, attribute_id, target)
            .map(|(items, _)| items)
    }

    fn get_all_with_status<R: TopologyRecords + ?Sized>(
        &self,
        records: &R,
        attributes: &AttributeStore,
        attribute_id: Option<u64>,
        target: Option<ObjectKey>,
    ) -> Result<(Vec<ObjectKey>, Result<()>)> {
        let mut items = Vec::new();
        let mut count = 0u64;
        let mut seen = BTreeSet::new();

        // for nonNULL m_pList pointers
        let Some(head) = self.list else {
            return Ok((items, Ok(())));
        };
        let mut elem = head;
        loop {
            match (attribute_id, target) {
                (None, None) => {
                    // add elem and step to Next object
                    items.push(elem);
                    let next = rec(records, elem, OP_GET_ALL)?.next();

                    // check m_lListSize
                    count += 1;
                    if count > self.list_size {
                        return Ok((items, Err(SolidError::sm_err(OP_GET_ALL))));
                    }
                    match next {
                        Some(n) if n != head => elem = n,
                        _ => break,
                    }
                }
                (Some(id), _) => {
                    visit(&mut seen, elem, OP_GET_ALL)?;
                    if attributes.find_attribute(elem, id).is_some()
                        && (target.is_none() || target == Some(elem))
                    {
                        items.push(elem);
                    }
                    match rec(records, elem, OP_GET_ALL)?.next() {
                        Some(n) if n != head => elem = n,
                        _ => break,
                    }
                }
                (None, Some(t)) => {
                    visit(&mut seen, elem, OP_GET_ALL)?;
                    if elem == t {
                        items.push(elem);
                    }
                    match rec(records, elem, OP_GET_ALL)?.next() {
                        Some(n) if n != head => elem = n,
                        _ => break,
                    }
                }
            }
        }
        Ok((items, Ok(())))
    }

    /// Reverses the list in place by swapping every member's successor and
    /// predecessor (`ReverseList`); the head and the count are unchanged.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an empty list or a broken one (upstream
    /// dereferences `NULL`, or loops forever).
    pub fn reverse_list<R: TopologyRecords + ?Sized>(&self, records: &mut R) -> Result<()> {
        let start = self.list.ok_or(broken_list(OP_LIST))?;
        let mut curr = start;
        let mut seen = BTreeSet::new();
        loop {
            visit(&mut seen, curr, OP_LIST)?;
            let r = rec_mut(records, curr, OP_LIST)?;
            let (owner, next, last) = (r.owner(), r.next(), r.last());
            r.set_list_links(owner, last, next);

            // Proceed to next in list.
            curr = next.ok_or(broken_list(OP_LIST))?;
            if curr == start {
                break;
            }
        }
        Ok(())
    }
}

/// The destructor `~SmTopology` of the object `this`: when it is on an owner's list,
/// the owner removes it (`SmOwningTopology::Remove`, status ignored) and its owner
/// link is cleared.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when `this` or its owner is not in `records`, or the
/// owner has no list data (upstream's owner is always an `SmOwningTopology`).
pub fn destroy_topology<R: OwningRecords + ?Sized>(records: &mut R, this: ObjectKey) -> Result<()> {
    const OP: &str = "topology::proto_topology::p1::destroy_topology";
    let Some(owner) = rec(records, this, OP)?.owner() else {
        return Ok(());
    };
    let mut list = records.owning(owner).ok_or(broken_list(OP))?;
    if let Err(e @ (SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })) =
        list.remove(owner, records, this)
    {
        return Err(e);
    }
    records.set_owning(owner, list).ok_or(broken_list(OP))?;
    set_owner(records, this, None, OP)
}
