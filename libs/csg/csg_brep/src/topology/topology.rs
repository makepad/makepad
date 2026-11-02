//! The topology object base (`SmTopology`): list links, traversal marks, flags, user
//! indices and the notification step that carries them from an object to its split,
//! merged or copied counterparts; plus topology-change tracking through the
//! context's system notify callback (`SmTrackTopologyChanges`).
//!
//! Upstream: `source/SMLib/inc/SmTopology.h`, `source/SMLib/src/SmTopology.cpp`,
//! `source/SMLib/inc/SmTrackTopologyChanges.h`,
//! `source/SMLib/src/SmTrackTopologyChanges.cpp`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`]  | `SmTopology::GetType`, `GetTypeString`, `GetClassString` |
//! | [`is_kind_of`]                               | `SmTopology::IsKindOf`                           |
//! | [`Topology`], [`Topology::new`]              | `SmTopology` (its data members), `SmTopology()`  |
//! | [`Topology::owner`]                          | `SmTopology::GetOwner`                           |
//! | [`Topology::next`], [`Topology::last`]       | `SmTopology::GetNext`, `GetLast`                 |
//! | [`Topology::set_list_links`]                 | the friend writes of `SmOwningTopology` (`SetListOwner`, `m_pNext`, `m_pLast`) |
//! | [`Topology::is_connected_to`]                | `SmTopology::IsConnectedTo(const SmTopology*)`   |
//! | [`Topology::all_flags`], [`Topology::set_all_flags`] | `SmTopology::GetAllFlags`, `SetAllFlags` |
//! | [`Topology::set_user_index1`], [`Topology::set_user_index2`], [`Topology::set_user_ptr1`] | `SmTopology::SetUserIndex1`, `SetUserIndex2`, `SetUserPtr1` |
//! | [`Topology::user_index1`], [`Topology::user_index2`], [`Topology::user_ptr1`] | `SmTopology::GetUserIndex1`, `GetUserIndex2`, `GetUserPtr1` |
//! | [`Topology::tolerance`]                      | `SmTopology::GetTolerance` (`SM_USE_OLDTOL`)     |
//! | [`Topology::mark`]                           | `SmTopology::Mark`                               |
//! | [`Topology::get_mark`]                       | `SmTopology::GetMark`                            |
//! | [`Topology::is_marked`]                      | `SmTopology::IsMarked`                           |
//! | [`destroy_topology`](crate::topology::proto_topology::p1::destroy_topology) | `SmTopology::~SmTopology` (unit `topology.proto_topology_p1`) |
//! | [`TopologyRecords`]                          | the `SmTopology*` casts of `Notify`'s arguments  |
//! | [`NotifyRecords`]                            | the `SmTopology` members of every object of a store |
//! | [`notify`]                                   | `SmTopology::Notify`                             |
//! | [`NotifyEvent`]                              | `SmNotifyEvent` (data only)                      |
//! | [`TopologyChangeLists`]                      | the arrays `SmTopologyChangeCallback` points at  |
//! | [`TopologyChangeLists::reset_arrays`]        | `SmTopologyChangeCallback::ReSetArrays`, `~SmTopologyChangeCallback` |
//! | [`TopologyChangeCallback`]                   | `SmTopologyChangeCallback` (constructor)         |
//! | [`NotifyCallback::execute`] for `TopologyChangeCallback` | `SmTopologyChangeCallback::Execute` |
//! | [`NotifyCallback::owner_link`] for `TopologyChangeCallback` | the `GetOwner()` / `GetEdge()` its geometry walk reads |
//! | `add_unique`, `remove_key` (private) | `SmTopologyChangeCallback::AddUnique`, `Remove` |
//! | [`TrackTopologyChanges`]                     | `SmTrackTopologyChanges`                         |
//! | [`TrackTopologyChanges::new`]                | `SmTrackTopologyChanges::SmTrackTopologyChanges` |
//! | [`TrackTopologyChanges::start_tracking`]     | `SmTrackTopologyChanges::StartTracking`          |
//! | [`TrackTopologyChanges::stop_tracking`], [`TrackTopologyChanges::stop_tracking_with_brep`] | `SmTrackTopologyChanges::StopTracking` |
//! | [`TopologyChangeLists::record_dependencies`] | `SmTopologyChangeCallback::RecordDependencies`   |
//! | [`TrackTopologyChanges::is_tracking`]        | `SmTrackTopologyChanges::IsTracking`             |
//! | [`TrackTopologyChanges::rm_list`], [`TrackTopologyChanges::add_list`] | `SmTrackTopologyChanges::GetRmList`, `GetAddList` |
//! | [`TrackTopologyChanges::reset_arrays`]       | `SmTrackTopologyChanges::ReSetArrays`            |
//! | [`TrackTopologyChanges::finish`]             | `SmTrackTopologyChanges::~SmTrackTopologyChanges` |
//!
//! Deviations:
//! - **Object model.** There is no class hierarchy: a topology object is a
//!   [`Topology`] record that the derived kinds (faces, edges, vertices, ...) embed,
//!   addressed by the [`ObjectKey`] it is notified under. Its list owner and
//!   neighbours are keys (`None` for null). `Notify` reaches the records of its data
//!   objects through a [`TopologyRecords`] store; a data object that has no record
//!   (upstream would cast a non-topology object, undefined behaviour) is
//!   [`SolidError::InvalidInput`]. The context the object reaches through
//!   `GetContext` is passed explicitly.
//! - `ULONG` marks and flags are `u64` (LP64). `SetUserPtr1` stores an opaque
//!   `void*`; here it is an opaque `u64` (`0` for null).
//! - `IsMarked` with a null context crashes upstream; [`Topology::is_marked`] takes
//!   the context by reference. `Mark` with a null context does nothing, as upstream.
//! - The reference build uses `SM_USE_OLDTOL`, so `GetTolerance` is the constant
//!   `SM_ZONE_TOL_3D` (overridden by the derived kinds in their units).
//! - `IsConnectedTo` is a "pure virtual" stub upstream (an `SE_MSG` compiled out of
//!   the release build, then `FALSE`); the derived kinds override it.
//! - The destructor unlinks the object from its owner's list
//!   (`SmOwningTopology::Remove`); it needs the owner's list data and is
//!   [`destroy_topology`](crate::topology::proto_topology::p1::destroy_topology).
//! - **Change tracking.** Upstream the tracker owns the Rm/Add/Chg/event arrays and
//!   installs a callback object holding pointers to them; `IsTracking` compares the
//!   context's system callback with the address of that object. Here the tracker
//!   owns a [`TopologyChangeLists`] and installs a boxed [`TopologyChangeCallback`]
//!   tagged with a caller-chosen tracker key; `IsTracking` recognises it by that
//!   key. "Was a callback already installed" (`m_pLastNotifyCallback` non-null) is
//!   kept as a flag. The tracker's B-rep only supplies its context, so the tracker
//!   keeps the [`BrepId`] and takes the context as an argument; the destructor is
//!   the explicit [`TrackTopologyChanges::finish`].
//! - `SmTopologyChangeCallback::Execute`, `AddUnique` and `Remove` are co-ported
//!   for `topology.brep_p5` healer integration. Geometry owner traversal requires
//!   the caller's actual owner map, and is bounded to 64 links (no recursive stack
//!   use). Missing metadata is recorded as a callback failure; it is never treated
//!   as a null owner. The tracker and its callback share only notification arrays
//!   through a private `Arc<Mutex<_>>` (no entity graph pointers or threads). This
//!   keeps captured events alive across callback replacement, matches active reset
//!   and keeps `Kernel: Send`; getters return owned snapshots. `Dump` and debug
//!   index arrays remain omitted.
//! - **`RecordDependencies`** runs on the tracker's lists, which hold notification
//!   keys: the caller translates keys and B-rep entities. `StopTracking` without the
//!   B-rep ([`TrackTopologyChanges::stop_tracking`], [`TrackTopologyChanges::finish`])
//!   records none; [`TrackTopologyChanges::stop_tracking_with_brep`] is the full one.
//! - `AssertValid`, `CheckPointerChain`, `Dump`, the obsolete `NewMark*` / `Mark2`
//!   helpers, `UnMark`, `IsUnMarked`, `IsMarkLocked`, the flag bit helpers and the
//!   placement `operator new` / `delete` are not part of this unit.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::{EntityRef, Result, SolidError};
use crate::geom::a_object;
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::context::{Context, MarkType, NotifyCallback};
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::tol;
use crate::topology::brep::p1::is_live_topology_member;
use crate::topology::{edge, vertex, Brep, BrepId, EdgeId, FaceId};

/// `SmTopology_TYPE` (`TOPO_BASE_TYPE + 1`).
pub const TYPE: i64 = 16001;

/// `SmTopology::GetTypeString`.
pub const TYPE_STRING: &str = "SmTopology_TYPE";

/// `SmTopology::GetClassString`.
pub const CLASS_STRING: &str = "SmTopology";

/// `SmTopology::IsKindOf`: [`TYPE`], then the `SmAObject` chain.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    if t == TYPE {
        true
    } else {
        a_object::is_kind_of(t)
    }
}

/// Index of each mark type's value in [`Topology`]'s mark array (`m_lMark`,
/// `m_lMark2`, `m_lMark3`, `m_lMarkIO`, `m_lMarkAssert`); unknown types use the IO
/// mark, as the `default:` branches upstream.
const fn mark_slot(mark_type: MarkType) -> usize {
    match mark_type {
        MarkType::Mark => 0,
        MarkType::Mark2 => 1,
        MarkType::Mark3 => 2,
        MarkType::MarkAssert => 4,
        MarkType::MarkIo | MarkType::NoMark | MarkType::AllMarks => 3,
    }
}

/// The data every topology object carries (`SmTopology`'s members).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Topology {
    /// `m_pListOwner`.
    list_owner: Option<ObjectKey>,
    /// `m_pNext`.
    next: Option<ObjectKey>,
    /// `m_pLast`.
    last: Option<ObjectKey>,
    /// `m_lMark`, `m_lMark2`, `m_lMark3`, `m_lMarkIO`, `m_lMarkAssert`.
    marks: [u64; 5],
    /// `m_lFlags`.
    flags: u64,
    /// `m_lUserIndex1`.
    user_index1: u64,
    /// `m_lUserIndex2`.
    user_index2: u64,
    /// `m_pUserPtr1` (opaque; `0` is null).
    user_ptr1: u64,
    /// `m_bIsSmallTopology`.
    small: bool,
}

impl Topology {
    /// `SmTopology()`: unlinked, every mark, flag and user value zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            list_owner: None,
            next: None,
            last: None,
            marks: [0; 5],
            flags: 0,
            user_index1: 0,
            user_index2: 0,
            user_ptr1: 0,
            small: false,
        }
    }

    /// The owner of the list this object is on (`GetOwner`).
    #[must_use]
    pub const fn owner(&self) -> Option<ObjectKey> {
        self.list_owner
    }

    /// The next object on the owner's list (`GetNext`).
    #[must_use]
    pub const fn next(&self) -> Option<ObjectKey> {
        self.next
    }

    /// The previous object on the owner's list (`GetLast`).
    #[must_use]
    pub const fn last(&self) -> Option<ObjectKey> {
        self.last
    }

    /// Sets the list owner and neighbours (what `SmOwningTopology`, a friend class,
    /// writes when it links or unlinks the object).
    pub fn set_list_links(
        &mut self,
        owner: Option<ObjectKey>,
        next: Option<ObjectKey>,
        last: Option<ObjectKey>,
    ) {
        self.list_owner = owner;
        self.next = next;
        self.last = last;
    }

    /// `IsConnectedTo(const SmTopology*)` of the base class: `false` (the derived
    /// kinds override it).
    #[must_use]
    pub const fn is_connected_to(&self, _target: Option<ObjectKey>) -> bool {
        false
    }

    /// All flags as one bit array (`GetAllFlags`).
    #[must_use]
    pub const fn all_flags(&self) -> u64 {
        self.flags
    }

    /// Replaces all flags (`SetAllFlags`).
    pub fn set_all_flags(&mut self, flags: u64) {
        self.flags = flags;
    }

    /// The first user index (`GetUserIndex1`).
    #[must_use]
    pub const fn user_index1(&self) -> u64 {
        self.user_index1
    }

    /// The second user index (`GetUserIndex2`).
    #[must_use]
    pub const fn user_index2(&self) -> u64 {
        self.user_index2
    }

    /// The user pointer (`GetUserPtr1`; `0` is null).
    #[must_use]
    pub const fn user_ptr1(&self) -> u64 {
        self.user_ptr1
    }

    /// Sets the first user index (`SetUserIndex1`).
    pub fn set_user_index1(&mut self, index: u64) {
        self.user_index1 = index;
    }

    /// Sets the second user index (`SetUserIndex2`).
    pub fn set_user_index2(&mut self, index: u64) {
        self.user_index2 = index;
    }

    /// Sets the user pointer (`SetUserPtr1`; `0` is null).
    pub fn set_user_ptr1(&mut self, ptr: u64) {
        self.user_ptr1 = ptr;
    }

    /// Whether the object is intended to be tiny (`m_bIsSmallTopology`).
    #[must_use]
    pub const fn is_small_topology(&self) -> bool {
        self.small
    }

    /// The object's zone tolerance (`GetTolerance` of the `SM_USE_OLDTOL` build):
    /// [`tol::ZONE_TOL_3D`] for the base class.
    #[must_use]
    pub const fn tolerance(&self) -> f64 {
        tol::ZONE_TOL_3D
    }

    /// Marks the object with the context's current value of `mark_type` (`Mark`).
    /// Nothing happens for [`MarkType::NoMark`] or a null context;
    /// [`MarkType::AllMarks`] sets the IO mark.
    pub fn mark(&mut self, context: Option<&Context>, mark_type: MarkType) {
        if mark_type != MarkType::NoMark {
            let Some(context) = context else {
                // This should not happen
                return;
            };
            let mark = context.current_mark(mark_type);
            if let Some(m) = self.marks.get_mut(mark_slot(mark_type)) {
                *m = mark;
            }
        }
    }

    /// The object's value of `mark_type` (`GetMark`); [`MarkType::NoMark`] and
    /// [`MarkType::AllMarks`] give the IO mark.
    #[must_use]
    pub fn get_mark(&self, mark_type: MarkType) -> u64 {
        self.marks.get(mark_slot(mark_type)).copied().unwrap_or(0)
    }

    /// Whether the object carries the context's current value of `mark_type`
    /// (`IsMarked`).
    #[must_use]
    pub fn is_marked(&self, context: &Context, mark_type: MarkType) -> bool {
        self.get_mark(mark_type) == context.current_mark(mark_type)
    }
}

/// Where [`notify`] finds the [`Topology`] record of an object.
pub trait TopologyRecords {
    /// The record of `key`, if it is a topology object.
    fn topology(&self, key: ObjectKey) -> Option<&Topology>;
    /// Mutable access to the record of `key`.
    fn topology_mut(&mut self, key: ObjectKey) -> Option<&mut Topology>;
}

/// The topology records of the objects a caller notifies through, keyed by their
/// notification identities: upstream keeps one in every object (`SmTopology`'s
/// members). Reads go through the map ([`Deref`](std::ops::Deref)); writes go through
/// the methods here, so a store also remembers how far each B-rep's objects have been
/// checked and given a record (`topology::brep::p3`), which a removal forgets.
#[derive(Debug, Clone, Default)]
pub struct NotifyRecords {
    map: BTreeMap<ObjectKey, Topology>,
    progress: Vec<SeedProgress>,
}

/// How far a [`NotifyRecords`] has checked and seeded the objects of one B-rep value,
/// under one identity, per entity kind (`topology::brep::p3`). A B-rep value's arenas
/// only append and empty slots for good, so indices below a mark stay as they were.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeedProgress {
    /// The B-rep and the caller identity the progress holds for.
    pub(crate) id: Vec<u64>,
    /// The B-rep value it was taken on ([`ProgressTag`]).
    pub(crate) instance: Arc<()>,
    /// Per kind: no arena index from here on has passed the reserved-key check yet.
    pub(crate) checked_to: [usize; 10],
    /// Per kind: the indices below `checked_to` that have not passed it (all of them
    /// emptied slots, which a check only covers once they fall in its window).
    pub(crate) unchecked: [BTreeSet<usize>; 10],
    /// Per kind: the arena length up to which every live object has its record.
    pub(crate) seeded_to: [usize; 10],
}

/// Names one B-rep value for a [`NotifyRecords`]' progress: what was checked and
/// seeded of one value stays so, but a copy is another value (`Clone` gives an empty
/// tag) whose objects may differ. Compared by allocation ([`Arc::ptr_eq`]), never hashed.
#[derive(Debug, Default)]
pub(crate) struct ProgressTag(Option<Arc<()>>);

impl Clone for ProgressTag {
    /// An empty tag: the copy is another B-rep value.
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl ProgressTag {
    /// The value's tag, made on first use.
    pub(crate) fn get(&mut self) -> Arc<()> {
        Arc::clone(self.0.get_or_insert_with(|| Arc::new(())))
    }
}

impl std::ops::Deref for NotifyRecords {
    type Target = BTreeMap<ObjectKey, Topology>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl PartialEq for NotifyRecords {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl From<BTreeMap<ObjectKey, Topology>> for NotifyRecords {
    fn from(map: BTreeMap<ObjectKey, Topology>) -> Self {
        Self {
            map,
            progress: Vec::new(),
        }
    }
}

impl FromIterator<(ObjectKey, Topology)> for NotifyRecords {
    fn from_iter<I: IntoIterator<Item = (ObjectKey, Topology)>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<BTreeMap<_, _>>())
    }
}

impl NotifyRecords {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The record of `key`, made default first if it has none.
    pub fn or_default(&mut self, key: ObjectKey) -> &mut Topology {
        self.map.entry(key).or_default()
    }

    /// Sets the record of `key`, returning the previous one.
    pub fn insert(&mut self, key: ObjectKey, record: Topology) -> Option<Topology> {
        self.map.insert(key, record)
    }

    /// Mutable access to the record of `key`.
    pub fn get_mut(&mut self, key: &ObjectKey) -> Option<&mut Topology> {
        self.map.get_mut(key)
    }

    /// Mutable access to every record, in key order.
    pub fn values_mut(
        &mut self,
    ) -> std::collections::btree_map::ValuesMut<'_, ObjectKey, Topology> {
        self.map.values_mut()
    }

    /// Removes the record of `key`.
    pub fn remove(&mut self, key: &ObjectKey) -> Option<Topology> {
        self.progress.clear();
        self.map.remove(key)
    }

    /// Keeps the records `keep` accepts (it may also change them).
    pub fn retain(&mut self, keep: impl FnMut(&ObjectKey, &mut Topology) -> bool) {
        self.progress.clear();
        self.map.retain(keep);
    }

    /// Removes every record.
    pub fn clear(&mut self) {
        self.progress.clear();
        self.map.clear();
    }

    /// The records and the progress kept for `id` on the B-rep value `instance` (made
    /// empty if there is none), for `topology::brep::p3`'s per-edit check and seeding,
    /// which only adds records.
    pub(crate) fn progress_for(
        &mut self,
        id: Vec<u64>,
        instance: Arc<()>,
    ) -> Option<(&mut BTreeMap<ObjectKey, Topology>, &mut SeedProgress)> {
        // (a value only the store still names is gone: its progress is never used again)
        self.progress.retain(|p| Arc::strong_count(&p.instance) > 1);
        let at = self
            .progress
            .iter()
            .position(|p| p.id == id && Arc::ptr_eq(&p.instance, &instance));
        let Self { map, progress } = self;
        let p = if let Some(at) = at {
            progress.get_mut(at)
        } else {
            progress.push(SeedProgress {
                id,
                instance,
                ..SeedProgress::default()
            });
            progress.last_mut()
        };
        Some((map, p?))
    }
}

impl TopologyRecords for NotifyRecords {
    fn topology(&self, key: ObjectKey) -> Option<&Topology> {
        self.map.get(&key)
    }

    fn topology_mut(&mut self, key: ObjectKey) -> Option<&mut Topology> {
        self.map.get_mut(&key)
    }
}

impl TopologyRecords for BTreeMap<ObjectKey, Topology> {
    fn topology(&self, key: ObjectKey) -> Option<&Topology> {
        self.get(&key)
    }

    fn topology_mut(&mut self, key: ObjectKey) -> Option<&mut Topology> {
        self.get_mut(&key)
    }
}

/// The error of a notified object that has no [`Topology`] record.
const NOT_TOPOLOGY: SolidError = SolidError::InvalidInput {
    operation: "topology::topology::notify",
    reason: "notified object is not a topology object",
};

fn record<R: TopologyRecords + ?Sized>(records: &R, key: ObjectKey) -> Result<Topology> {
    records.topology(key).copied().ok_or(NOT_TOPOLOGY)
}

fn record_mut<R: TopologyRecords + ?Sized>(
    records: &mut R,
    key: ObjectKey,
) -> Result<&mut Topology> {
    records.topology_mut(key).ok_or(NOT_TOPOLOGY)
}

/// Carries the flags (unless `merge`), the three general marks (zeroed instead when
/// `reset_marks`) and the user values of `src` to `dst`: one target of `Notify`.
fn propagate(src: &Topology, dst: &mut Topology, merge: bool, reset_marks: bool) {
    if !merge {
        dst.set_all_flags(src.all_flags());
    }
    if reset_marks {
        // init marks for next mark usage - don't copy marks
        dst.marks[..3].fill(0);
    } else {
        dst.marks[..3].copy_from_slice(&src.marks[..3]);
    }
    dst.user_index1 = src.user_index1;
    dst.user_index2 = src.user_index2;
    dst.user_ptr1 = src.user_ptr1;
}

/// `SmTopology::Notify`: for splits, merges, copies and coincidences, carries the
/// flags, the three general marks and the user values of the source object to the
/// other objects of the operation, then passes the notification to
/// [`a_object::notify`] (attribute handlers and context callbacks).
///
/// | operation      | source (`this`)  | targets            | kind  |
/// |----------------|------------------|--------------------|-------|
/// | `SplitInBrep`  | `data1`          | `data2`, `data3`   |       |
/// | `MergeInBrep`  | `data2`          | `data1`, `data2`   | merge |
/// | `Copy`         | `this`           | `data1`            | copy  |
/// | `Split`        | `this`           | `data1`, `data2`   |       |
/// | `Merge`        | `this`           | `data1`, `data2`   | merge |
/// | `Coincident`   | `data2`          | `data1`            |       |
/// | `Construction` | `this` if `data2`| `data2`            | copy  |
///
/// A merge first ORs the two targets' flags into both and does not otherwise copy
/// flags; a copy resets the target's general marks to zero instead of copying them.
/// A target equal to the source is skipped.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when an object the operation reads or writes has no
/// record in `records` (upstream casts it to `SmTopology*` regardless); errors of
/// [`a_object::notify`].
pub fn notify<R: TopologyRecords + ?Sized>(
    records: &mut R,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
) -> Result<()> {
    notify_with_regions(
        records, attributes, context, operation, this, data1, data2, data3, None,
    )
}

/// Propagate topology marks, then notify attributes with an actual region lookup.
///
/// # Errors
/// Missing topology records, attribute errors or failed region resolution.
pub fn notify_with_regions<R: TopologyRecords + ?Sized>(
    records: &mut R,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
    regions: Option<&dyn crate::topology::shell::RegionLookup>,
) -> Result<()> {
    use NotifyOperation as N;
    let key = |o: Option<ObjectRef>| o.map(|r| r.key);
    let mut copy = false;
    let mut merge = false;

    let (p_this, other1, other2) = match operation {
        N::SplitInBrep => (key(data1), key(data2), key(data3)),
        N::MergeInBrep => {
            merge = true;
            (key(data2), key(data1), key(data2))
        }
        N::Copy => {
            copy = true;
            (Some(this.key), key(data1), None)
        }
        N::Split => (Some(this.key), key(data1), key(data2)),
        N::Merge => {
            merge = true;
            (Some(this.key), key(data1), key(data2))
        }
        N::Coincident => (key(data2), key(data1), None),
        N::Construction => {
            if data2.is_some() {
                copy = true;
                (Some(this.key), key(data2), None)
            } else {
                (None, None, None)
            }
        }
        // SM_NO_UNKNOWN: SE_MSG only (compiled out of the release build)
        _ => (None, None, None),
    };

    // for merge
    if let (Some(o1), Some(o2), true) = (other1, other2, merge) {
        let f2 = record(records, o2)?.all_flags();
        let r1 = record_mut(records, o1)?;
        r1.set_all_flags(r1.all_flags() | f2);
        let f1 = r1.all_flags();
        let r2 = record_mut(records, o2)?;
        r2.set_all_flags(f1 | r2.all_flags());
    }

    // propagate the values to pOther1 (a copy resets its marks), then to pOther2
    for (other, reset_marks) in [(other1, copy), (other2, false)] {
        if let (Some(t), Some(o)) = (p_this, other) {
            if o != t {
                let src = record(records, t)?;
                propagate(&src, record_mut(records, o)?, merge, reset_marks);
            }
        }
    }

    // pass the call along to the base class
    a_object::notify_with_regions(
        attributes, context, operation, this, data1, data2, data3, regions,
    )
}

// ---- change tracking ---------------------------------------------------------------

/// One recorded notification (`SmNotifyEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyEvent {
    /// `m_eAction`.
    pub operation: NotifyOperation,
    /// `m_pCaller`.
    pub caller: Option<ObjectRef>,
    /// `m_pData1`.
    pub data1: Option<ObjectRef>,
    /// `m_pData2`.
    pub data2: Option<ObjectRef>,
    /// `m_pData3`.
    pub data3: Option<ObjectRef>,
}

/// The lists a change tracker accumulates (`m_sRmList`, `m_sAddList`, `m_sChgList`,
/// `m_sNotifyList` of `SmTrackTopologyChanges`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TopologyChangeLists {
    /// Objects removed from the topology graph (stale handles).
    pub rm_list: Vec<ObjectKey>,
    /// Objects added and not removed since.
    pub add_list: Vec<ObjectKey>,
    /// Objects whose geometry changed: diagnostics only, they request no healing.
    pub chg_list: Vec<ObjectKey>,
    /// The events that changed the lists.
    pub notify_events: Vec<NotifyEvent>,
}

impl TopologyChangeLists {
    /// Empties all four lists (`SmTopologyChangeCallback::ReSetArrays`, also what
    /// `~SmTopologyChangeCallback` does).
    pub fn reset_arrays(&mut self) {
        self.rm_list.clear();
        self.add_list.clear();
        self.chg_list.clear();
        self.notify_events.clear();
    }

    /// `SmTopologyChangeCallback::RecordDependencies(pBrep)`: after a completed edit,
    /// every live vertex or edge on the add list (and every edge reached from such a
    /// vertex) puts its connected edges and faces on both the remove and the add list,
    /// so their properties are replaced. The change list stays diagnostic; removed
    /// handles are never traversed. `entity_of` names a key's entity of `b` (`None`:
    /// not a live member), `key_of` the key of an entity.
    ///
    /// # Errors
    ///
    /// Those of the vertex and edge queries.
    pub fn record_dependencies(
        &mut self,
        b: &Brep,
        entity_of: &dyn Fn(ObjectKey) -> Option<EntityRef>,
        key_of: &dyn Fn(EntityRef) -> ObjectKey,
    ) -> Result<()> {
        let mut targets: Vec<ObjectKey> = Vec::new();
        for &key in &self.add_list {
            if !targets.contains(&key) {
                targets.push(key);
            }
        }
        let mut target = 0;
        while let Some(&key) = targets.get(target) {
            target += 1;
            let entity = entity_of(key);
            if !is_live_topology_member(b, entity) {
                continue;
            }

            let mut edges: Vec<EdgeId> = Vec::new();
            let mut faces: Vec<FaceId> = Vec::new();
            match entity {
                Some(EntityRef::Vertex(v)) => {
                    edges = vertex::edges(b, v, None)?;
                    faces = vertex::faces(b, v)?;
                }
                Some(EntityRef::Edge(e)) => faces = edge::faces(b, e, None)?,
                _ => {}
            }

            for e in edges {
                let key = key_of(EntityRef::Edge(e));
                push_unique(&mut self.rm_list, key);
                push_unique(&mut self.add_list, key);
                push_unique(&mut targets, key);
            }
            for f in faces {
                let key = key_of(EntityRef::Face(f));
                push_unique(&mut self.rm_list, key);
                push_unique(&mut self.add_list, key);
            }
        }
        Ok(())
    }
}

fn push_unique(list: &mut Vec<ObjectKey>, key: ObjectKey) {
    if !list.contains(&key) {
        list.push(key);
    }
}

/// The system notify callback a [`TrackTopologyChanges`] installs
/// (`SmTopologyChangeCallback`), tagged with its tracker's key.
#[derive(Debug)]
pub struct TopologyChangeCallback {
    tracker: u64,
    state: Arc<Mutex<ChangeState>>,
}

#[derive(Debug, Default)]
struct ChangeState {
    lists: TopologyChangeLists,
    // A present None is a known null owner; absent metadata is never guessed.
    owners: BTreeMap<ObjectKey, Option<ObjectRef>>,
    failure: Option<SolidError>,
}

fn change_state(state: &Mutex<ChangeState>) -> MutexGuard<'_, ChangeState> {
    // These private locks never execute user code. Recovering the ordered state
    // after poisoning also preserves partial native notification effects.
    match state.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl TopologyChangeCallback {
    /// Creates a callback with no geometry-owner metadata. Direct topology
    /// notifications require none; geometry walks require [`Self::with_owners`].
    #[must_use]
    pub fn new(tracker: u64) -> Self {
        Self::with_owners(tracker, BTreeMap::new())
    }

    /// Uses actual curve/surface owners and edgeuse-to-edge links supplied by the
    /// caller's object store. Keys must be the keys used by notifications.
    #[must_use]
    pub fn with_owners(tracker: u64, owners: BTreeMap<ObjectKey, Option<ObjectRef>>) -> Self {
        Self {
            tracker,
            state: Arc::new(Mutex::new(ChangeState {
                owners,
                ..ChangeState::default()
            })),
        }
    }

    /// Key of the tracker that installed this callback.
    #[must_use]
    pub const fn tracker(&self) -> u64 {
        self.tracker
    }

    /// Captured native change lists, including stale identities in the remove list.
    #[must_use]
    pub fn lists(&self) -> TopologyChangeLists {
        change_state(&self.state).lists.clone()
    }

    /// A missing owner mapping or excessive owner chain encountered while notifying.
    /// `SmObject::Notify` ignores callback statuses, so consumers must check this.
    #[must_use]
    pub fn failure(&self) -> Option<SolidError> {
        change_state(&self.state).failure.clone()
    }

    fn capture(
        &mut self,
        operation: NotifyOperation,
        caller: ObjectRef,
        data1: Option<ObjectRef>,
        data2: Option<ObjectRef>,
        data3: Option<ObjectRef>,
    ) -> Result<()> {
        use NotifyOperation::{
            AddToBrep, ChangeGeometry, ChangeOwner, Destruction, Merge, MergeInBrep, PostEdit,
            RegPropagation, RmFromBrep, Split, SplitInBrep, TrimNoSplitInBrep,
        };
        let mut state = change_state(&self.state);
        if operation == RegPropagation {
            return Ok(());
        }
        let mut tracked = match operation {
            ChangeGeometry | PostEdit => Some(caller),
            Split | Merge => data3,
            _ => data1,
        };
        if (operation == PostEdit && geometry_type(caller.type_code))
            || matches!(operation, Split | Merge)
        {
            // wasm has no stack guard; a finite loop also bounds corrupt cycles.
            let mut depth = 0;
            while let Some(object) = tracked {
                if !geometry_type(object.type_code) && !matches!(object.type_code, 16005 | 16031) {
                    break;
                }
                if depth >= 64 {
                    return Err(SolidError::invalid_input(
                        "topology::topology::track_changes",
                        "geometry owner chain exceeds 64",
                    ));
                }
                depth += 1;
                tracked = *state
                    .owners
                    .get(&object.key)
                    .ok_or(SolidError::Unsupported {
                        operation: "topology::topology::track_changes",
                        reason: "actual geometry owner or edgeuse-to-edge mapping is required",
                    })?;
            }
        }
        // Maintain actual metadata for later callbacks, without adding events.
        if operation == ChangeOwner {
            state.owners.insert(caller.key, data1);
        }
        if operation == Destruction {
            if let Some(object) = data1 {
                state.owners.remove(&object.key);
            }
        }
        let Some(tracked) =
            tracked.filter(|o| matches!(o.type_code, 16002 | 16016 | 16021 | 16033 | 16034))
        else {
            return Ok(());
        };
        if matches!(
            operation,
            AddToBrep
                | SplitInBrep
                | MergeInBrep
                | TrimNoSplitInBrep
                | RmFromBrep
                | ChangeGeometry
                | PostEdit
                | Split
                | Merge
        ) {
            state.lists.notify_events.push(NotifyEvent {
                operation,
                caller: Some(caller),
                data1,
                data2,
                data3,
            });
        }
        let lists = &mut state.lists;
        match operation {
            AddToBrep => add_unique(&mut lists.add_list, data1),
            SplitInBrep => {
                add_unique(&mut lists.rm_list, data1);
                remove_key(&mut lists.add_list, data1);
                remove_key(&mut lists.chg_list, data1);
                add_unique(&mut lists.add_list, data2);
                add_unique(&mut lists.add_list, data3);
            }
            MergeInBrep => {
                add_unique(&mut lists.rm_list, data1);
                add_unique(&mut lists.rm_list, data2);
                for object in [data1, data2] {
                    remove_key(&mut lists.add_list, object);
                    remove_key(&mut lists.chg_list, object);
                }
                add_unique(&mut lists.add_list, data1);
            }
            TrimNoSplitInBrep => {
                add_unique(&mut lists.rm_list, data1);
                remove_key(&mut lists.chg_list, data1);
                add_unique(&mut lists.add_list, data1);
            }
            RmFromBrep => {
                add_unique(&mut lists.rm_list, data1);
                remove_key(&mut lists.add_list, data1);
                remove_key(&mut lists.chg_list, data1);
            }
            ChangeGeometry | Split | Merge => add_unique(&mut lists.chg_list, Some(tracked)),
            PostEdit => {
                if tracked == caller {
                    add_unique(&mut lists.rm_list, Some(tracked));
                    add_unique(&mut lists.add_list, Some(tracked));
                } else {
                    add_unique(&mut lists.chg_list, Some(tracked));
                }
            }
            Destruction => {
                remove_key(&mut lists.add_list, data1);
                remove_key(&mut lists.chg_list, data1);
            }
            _ => {}
        }
        Ok(())
    }
}

fn geometry_type(t: i64) -> bool {
    use crate::curve::curve::p2 as c;
    use crate::surface::surface::p2 as s;
    matches!(
        t,
        c::TYPE
            | c::LINE_TYPE
            | c::CONIC_TYPE
            | c::CIRCLE_TYPE
            | c::ELLIPSE_TYPE
            | c::PARABOLA_TYPE
            | c::HYPERBOLA_TYPE
            | c::COMPOSITE_CURVE_TYPE
            | c::BSPLINE_CURVE_TYPE
            | c::HERMITE_CURVE_TYPE
            | c::OFFSET_CURVE_TYPE
            | c::PROJECTED_CURVE_TYPE
            | c::ISO_CURVE_TYPE
            | c::CRV_ON_SURF_TYPE
            | c::TANGENT_FIELD_TYPE
            | c::CRV_IN_VOLUME_TYPE
            | s::TYPE
            | s::B_SPLINE_SURFACE_TYPE
            | s::PLANE_TYPE
            | s::CONE_TYPE
            | s::CYLINDER_TYPE
            | s::SPHERE_TYPE
            | s::TORUS_TYPE
            | s::SURF_OF_REVOLUTION_TYPE
            | s::SURF_OF_EXTRUSION_TYPE
            | s::CURVE_BOUNDED_SURFACE_TYPE
            | s::SRF_IN_VOLUME_TYPE
            | s::OFFSET_SURFACE_TYPE
            | s::STEP_SURFACE_TYPE
    )
}

fn add_unique(list: &mut Vec<ObjectKey>, object: Option<ObjectRef>) {
    if let Some(object) = object {
        if !list.contains(&object.key) {
            list.push(object.key);
        }
    }
}
fn remove_key(list: &mut Vec<ObjectKey>, object: Option<ObjectRef>) {
    if let Some(i) = object.and_then(|o| list.iter().position(|&k| k == o.key)) {
        list.remove(i);
    }
}

impl NotifyCallback for TopologyChangeCallback {
    fn execute(
        &mut self,
        operation: NotifyOperation,
        caller: ObjectRef,
        data1: Option<ObjectRef>,
        data2: Option<ObjectRef>,
        data3: Option<ObjectRef>,
    ) -> Result<()> {
        let result = self.capture(operation, caller, data1, data2, data3);
        let mut state = change_state(&self.state);
        if state.failure.is_none() {
            state.failure = result.as_ref().err().cloned();
        }
        result
    }
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
    /// The owner link the geometry walk of a later `SM_NO_POST_EDIT` follows
    /// (upstream: `GetOwner()` / `GetEdge()` of the object).
    fn owner_link(&mut self, object: ObjectKey, owner: Option<ObjectRef>) {
        change_state(&self.state).owners.insert(object, owner);
    }
}

/// Tracks changes to a B-rep's topology graph through its context's system notify
/// callback (`SmTrackTopologyChanges`).
#[derive(Debug)]
pub struct TrackTopologyChanges {
    /// `m_pBrep`: the tracked B-rep (`None` for null).
    brep: Option<BrepId>,
    /// Identifies this tracker's callback in a context (upstream: its address).
    key: u64,
    /// The accumulated lists.
    state: Arc<Mutex<ChangeState>>,
    /// `m_pLastNotifyCallback != NULL`: a system callback was already installed when
    /// tracking last started.
    last_notify_callback: bool,
}

impl TrackTopologyChanges {
    /// `SmTrackTopologyChanges(pBrep, bStartNow)`: a tracker of `brep` whose
    /// callback is recognised by `key` (unique among live trackers of a context).
    /// `context` is the B-rep's context. Starts tracking when `start_now`, else just
    /// empties the lists.
    pub fn new(
        brep: Option<BrepId>,
        key: u64,
        start_now: bool,
        context: Option<&mut Context>,
    ) -> Self {
        let mut t = Self {
            brep,
            key,
            state: Arc::new(Mutex::new(ChangeState::default())),
            last_notify_callback: false,
        };
        // when asked start tracking now - else just init the Rm and Add arrays.
        if start_now {
            t.start_tracking(context);
        } else {
            t.reset_arrays();
        }
        t
    }

    /// The tracked B-rep.
    #[must_use]
    pub const fn brep(&self) -> Option<BrepId> {
        self.brep
    }

    /// Whether a system callback was already installed when tracking last started
    /// (`m_pLastNotifyCallback != NULL`); [`TrackTopologyChanges::stop_tracking`]
    /// then leaves the context alone.
    #[must_use]
    pub const fn last_notify_callback(&self) -> bool {
        self.last_notify_callback
    }

    /// The key identifying this tracker's callback.
    #[must_use]
    pub const fn key(&self) -> u64 {
        self.key
    }

    /// Empties the lists and, when the B-rep and its context exist and the context
    /// has no system notify callback, installs this tracker's callback
    /// (`StartTracking`). An already installed callback is left in place (upstream
    /// retains it and starts no nested tracking).
    pub fn start_tracking(&mut self, context: Option<&mut Context>) {
        // init internal arrays
        self.reset_arrays();

        // when m_pBrep and its Context exist
        if let (Some(_), Some(context)) = (self.brep, context) {
            // start TopoGraph Change Tracking
            self.last_notify_callback = context.sys_notify_callback().is_some();
            if !self.last_notify_callback {
                context.set_sys_notify_callback(Some(Box::new(TopologyChangeCallback {
                    tracker: self.key,
                    state: Arc::clone(&self.state),
                })));
            }
        }
    }

    /// Whether the context's system notify callback is this tracker's
    /// (`IsTracking`).
    #[must_use]
    pub fn is_tracking(&self, context: Option<&Context>) -> bool {
        let Some(context) = context.filter(|_| self.brep.is_some()) else {
            return false;
        };
        context
            .sys_notify_callback()
            .and_then(NotifyCallback::as_any)
            .and_then(|a| a.downcast_ref::<TopologyChangeCallback>())
            .is_some_and(|c| c.tracker == self.key)
    }

    /// When the B-rep and its context exist and this tracker found no callback
    /// installed when it started, clears the context's system notify callback
    /// (`StopTracking`); otherwise does nothing (upstream asserts). Without the B-rep
    /// no dependencies are recorded: [`TrackTopologyChanges::stop_tracking_with_brep`]
    /// is the full `StopTracking`.
    pub fn stop_tracking(&mut self, context: Option<&mut Context>) {
        // when m_pBrep and its Context exist
        if let (Some(_), Some(context)) = (self.brep, context) {
            // when LastNotifyCallback was NULL (it was turned on by this object)
            if !self.last_notify_callback {
                // turn off SmObject::Notify() tracking
                context.set_sys_notify_callback(None);
            }
        }
    }

    /// `StopTracking` with the tracked B-rep `b`: [`TrackTopologyChanges::stop_tracking`],
    /// then, when this tracker had installed the callback, its lists record the
    /// dependencies of the completed edit ([`TopologyChangeLists::record_dependencies`];
    /// notifications can occur midway through rewiring edge uses, so they are traversed
    /// only now). `entity_of` / `key_of` translate the notification keys.
    ///
    /// # Errors
    ///
    /// Those of [`TopologyChangeLists::record_dependencies`].
    pub fn stop_tracking_with_brep(
        &mut self,
        context: Option<&mut Context>,
        b: &Brep,
        entity_of: &dyn Fn(ObjectKey) -> Option<EntityRef>,
        key_of: &dyn Fn(EntityRef) -> ObjectKey,
    ) -> Result<()> {
        // when m_pBrep and its Context exist
        if let (Some(_), Some(context)) = (self.brep, context) {
            // when LastNotifyCallback was NULL (it was turned on by this object)
            if !self.last_notify_callback {
                // turn off SmObject::Notify() tracking
                context.set_sys_notify_callback(None);

                // Notifications can occur midway through rewiring edgeuses. Traverse
                // dependencies only now, after the caller has completed its edit.
                change_state(&self.state)
                    .lists
                    .record_dependencies(b, entity_of, key_of)?;
            }
        }
        Ok(())
    }

    /// Removed identities (which can be stale), captured even if a foreign
    /// callback has replaced this tracker's callback since the event.
    #[must_use]
    pub fn rm_list(&self) -> Vec<ObjectKey> {
        change_state(&self.state).lists.rm_list.clone()
    }

    /// Added identities in native first-insertion order.
    #[must_use]
    pub fn add_list(&self) -> Vec<ObjectKey> {
        change_state(&self.state).lists.add_list.clone()
    }

    /// A snapshot of all captured lists, active or stopped.
    #[must_use]
    pub fn lists(&self) -> TopologyChangeLists {
        change_state(&self.state).lists.clone()
    }

    /// A capture failure, including one whose callback status native Notify ignores.
    #[must_use]
    pub fn failure(&self) -> Option<SolidError> {
        change_state(&self.state).failure.clone()
    }

    /// Supplies actual geometry owners and edgeuse-to-edge links for later events.
    pub fn set_owner_metadata(&mut self, owners: BTreeMap<ObjectKey, Option<ObjectRef>>) {
        change_state(&self.state).owners = owners;
    }

    /// Empties all captured lists (`ReSetArrays`), including the active callback's
    /// arrays. The shared allocation survives callback replacement and stop.
    pub fn reset_arrays(&mut self) {
        let mut state = change_state(&self.state);
        state.lists.reset_arrays();
        state.failure = None;
    }

    /// The destructor `~SmTrackTopologyChanges`: stops tracking if this tracker's
    /// callback is installed.
    pub fn finish(mut self, mut context: Option<&mut Context>) {
        if self.is_tracking(context.as_deref()) {
            self.stop_tracking(context.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test key per entity: `kind * 1_000_000 + index` (vertex 1, edge 2, face 3).
    fn dep_key(e: EntityRef) -> ObjectKey {
        match e {
            EntityRef::Vertex(v) => ObjectKey(1_000_000 + u64::from(v.index())),
            EntityRef::Edge(x) => ObjectKey(2_000_000 + u64::from(x.index())),
            EntityRef::Face(f) => ObjectKey(3_000_000 + u64::from(f.index())),
            _ => ObjectKey(0),
        }
    }

    fn dep_entity(key: ObjectKey) -> Option<EntityRef> {
        let i = u32::try_from(key.0 % 1_000_000).ok()?;
        match key.0 / 1_000_000 {
            1 => Some(EntityRef::Vertex(crate::topology::VertexId::from_index(i))),
            2 => Some(EntityRef::Edge(EdgeId::from_index(i))),
            3 => Some(EntityRef::Face(FaceId::from_index(i))),
            _ => None,
        }
    }

    /// `StopTracking` after a tracked `MakeVertexSplitEdge` of a box edge: the split
    /// vertex's two edges and the faces around it are recorded for replacement.
    #[test]
    fn stop_tracking_records_the_dependencies_of_an_edge_split() {
        use crate::topology::brep::p3 as brep_p3;
        use crate::topology::brep::p9;
        let mut k = crate::kernel::Kernel::default();
        let mut b = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Point3::new(0.0, 0.0, 0.0),
            1.0,
            2.0,
            3.0,
        )
        .unwrap();
        b.set_editing_enabled(true);
        let e = b.edge_list()[0];
        let ivl = b.edge(e).unwrap().interval;
        let faces = edge::faces(&b, e, None).unwrap();
        assert_eq!(faces.len(), 2);

        let (split, lists, failure) = brep_p3::with_lent_context(&mut k, |k, ctx| {
            let mut tracker =
                TrackTopologyChanges::new(Some(BrepId::from_index(0)), 7, true, Some(&mut *ctx));
            let split = brep_p3::kernel_named_edit(k, ctx, &mut b, |k, b, env| {
                p9::make_vertex_split_edge_with_curve_identity(
                    k,
                    b,
                    e,
                    ivl.mid(),
                    env,
                    p9::CurveIdentity::Actual(&p9::issue_curve_identity),
                )
            })
            .unwrap();
            let serial = b.notify_serial().unwrap();
            tracker
                .stop_tracking_with_brep(
                    Some(&mut *ctx),
                    &b,
                    &|key| brep_p3::topology_entity(serial, key),
                    &|x| brep_p3::topology_ref(serial, x).key,
                )
                .unwrap();
            let lists = tracker.lists();
            let failure = tracker.failure();
            tracker.finish(Some(ctx));
            (split, lists, failure)
        })
        .unwrap();
        assert!(failure.is_none());
        assert!(split.split);
        let serial = b.notify_serial().unwrap();
        let key = |x| brep_p3::topology_ref(serial, x).key;
        let [e1, e2] = split.edges.unwrap();
        for x in [EntityRef::Edge(e1), EntityRef::Edge(e2)]
            .into_iter()
            .chain(faces.iter().map(|&f| EntityRef::Face(f)))
        {
            assert!(lists.rm_list.contains(&key(x)), "{x:?} not removed");
            assert!(lists.add_list.contains(&key(x)), "{x:?} not added");
        }
    }

    #[test]
    fn record_dependencies_adds_a_box_vertex_s_edges_and_faces() {
        let mut k = crate::kernel::Kernel::default();
        let b = crate::primitive::api_primitives::create_box(
            &mut k,
            crate::math::Point3::new(0.0, 0.0, 0.0),
            1.0,
            2.0,
            3.0,
        )
        .unwrap();
        let v = b.vertex_list()[0];
        let edges = vertex::edges(&b, v, None).unwrap();
        let faces = vertex::faces(&b, v).unwrap();
        assert_eq!((edges.len(), faces.len()), (3, 3));

        // an added vertex and a stale key (no live entity)
        let stale = ObjectKey(9_000_000);
        let mut lists = TopologyChangeLists {
            add_list: vec![dep_key(EntityRef::Vertex(v)), stale],
            ..TopologyChangeLists::default()
        };
        lists
            .record_dependencies(&b, &dep_entity, &dep_key)
            .unwrap();

        // edges first (each also a target, whose faces are already listed), then faces
        let mut expected: Vec<ObjectKey> =
            edges.iter().map(|&e| dep_key(EntityRef::Edge(e))).collect();
        expected.extend(faces.iter().map(|&f| dep_key(EntityRef::Face(f))));
        assert_eq!(lists.rm_list, expected);
        let mut added = vec![dep_key(EntityRef::Vertex(v)), stale];
        added.extend(expected.iter().copied());
        assert_eq!(lists.add_list, added);
        assert!(lists.chg_list.is_empty() && lists.notify_events.is_empty());

        // an added edge records only its two faces
        let e = edges[0];
        let mut lists = TopologyChangeLists {
            add_list: vec![dep_key(EntityRef::Edge(e))],
            ..TopologyChangeLists::default()
        };
        lists
            .record_dependencies(&b, &dep_entity, &dep_key)
            .unwrap();
        let edge_faces: Vec<ObjectKey> = edge::faces(&b, e, None)
            .unwrap()
            .into_iter()
            .map(|f| dep_key(EntityRef::Face(f)))
            .collect();
        assert_eq!(edge_faces.len(), 2);
        assert_eq!(lists.rm_list, edge_faces);
    }

    fn r(k: u64) -> ObjectRef {
        ObjectRef {
            type_code: TYPE,
            key: ObjectKey(k),
        }
    }

    fn pool(n: u64) -> NotifyRecords {
        (0..n).map(|k| (ObjectKey(k), Topology::new())).collect()
    }

    #[test]
    fn types_and_defaults() {
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(a_object::TYPE));
        assert!(!is_kind_of(a_object::SAG_TYPE));
        let t = Topology::new();
        assert_eq!(t, Topology::default());
        assert_eq!(t.tolerance(), 1.0e-5);
        assert!(!t.is_connected_to(None));
        assert_eq!((t.owner(), t.next(), t.last()), (None, None, None));
    }

    #[test]
    fn marks_follow_context() {
        let ctx = Context::default();
        let mut t = Topology::new();
        assert!(!t.is_marked(&ctx, MarkType::Mark));
        t.mark(Some(&ctx), MarkType::Mark2);
        assert!(t.is_marked(&ctx, MarkType::Mark2));
        assert_eq!(
            t.get_mark(MarkType::Mark2),
            ctx.current_mark(MarkType::Mark2)
        );
        t.mark(Some(&ctx), MarkType::AllMarks);
        assert_eq!(
            t.get_mark(MarkType::MarkIo),
            ctx.current_mark(MarkType::AllMarks)
        );
        let before = t;
        t.mark(None, MarkType::Mark);
        t.mark(Some(&ctx), MarkType::NoMark);
        assert_eq!(t, before);
    }

    #[test]
    fn notify_split_merge_copy() {
        let mut recs = pool(3);
        let mut attrs = AttributeStore::new();
        let src = recs.get_mut(&ObjectKey(0)).unwrap();
        src.set_all_flags(5);
        src.set_user_index1(7);
        src.marks = [1, 2, 3, 4, 5];
        notify(
            &mut recs,
            &mut attrs,
            None,
            NotifyOperation::Split,
            r(0),
            Some(r(1)),
            Some(r(2)),
            None,
        )
        .unwrap();
        for k in [1, 2] {
            let t = recs[&ObjectKey(k)];
            assert_eq!(t.all_flags(), 5);
            assert_eq!(t.user_index1(), 7);
            assert_eq!(t.marks, [1, 2, 3, 0, 0]);
        }
        recs.get_mut(&ObjectKey(2)).unwrap().set_all_flags(8);
        notify(
            &mut recs,
            &mut attrs,
            None,
            NotifyOperation::Merge,
            r(0),
            Some(r(1)),
            Some(r(2)),
            None,
        )
        .unwrap();
        assert_eq!(recs[&ObjectKey(1)].all_flags(), 13);
        assert_eq!(recs[&ObjectKey(2)].all_flags(), 13);
        notify(
            &mut recs,
            &mut attrs,
            None,
            NotifyOperation::Copy,
            r(0),
            Some(r(1)),
            None,
            None,
        )
        .unwrap();
        assert_eq!(recs[&ObjectKey(1)].all_flags(), 5);
        assert_eq!(recs[&ObjectKey(1)].marks, [0, 0, 0, 0, 0]);
        let missing = notify(
            &mut recs,
            &mut attrs,
            None,
            NotifyOperation::Copy,
            r(0),
            Some(r(9)),
            None,
            None,
        );
        assert!(matches!(missing, Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn destroy_unlisted_and_linked() {
        use crate::topology::proto_topology::p1::destroy_topology;
        let mut recs = NotifyRecords::new();
        recs.insert(ObjectKey(0), Topology::new());
        assert!(destroy_topology(&mut recs, ObjectKey(0)).is_ok());
        // an owner without list data (never an SmOwningTopology here)
        recs.insert(ObjectKey(1), Topology::new());
        if let Some(r) = recs.get_mut(&ObjectKey(0)) {
            r.set_list_links(Some(ObjectKey(1)), None, None);
        }
        assert!(matches!(
            destroy_topology(&mut recs, ObjectKey(0)),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn tracking_installs_and_removes_callback() {
        let mut ctx = Context::default();
        let t = TrackTopologyChanges::new(Some(BrepId::from_index(0)), 7, true, Some(&mut ctx));
        assert!(t.is_tracking(Some(&ctx)));
        let mut other = TrackTopologyChanges::new(Some(BrepId::from_index(0)), 8, false, None);
        other.start_tracking(Some(&mut ctx));
        assert!(!other.is_tracking(Some(&ctx)));
        other.stop_tracking(Some(&mut ctx));
        assert!(t.is_tracking(Some(&ctx)));
        t.finish(Some(&mut ctx));
        assert!(ctx.sys_notify_callback().is_none());
        let none = TrackTopologyChanges::new(None, 1, true, Some(&mut ctx));
        assert!(!none.is_tracking(Some(&ctx)));
        assert!(none.rm_list().is_empty() && none.add_list().is_empty());
    }
    #[test]
    fn tracker_snapshots_real_callback_changes_on_stop() {
        let mut context = Context::default();
        let mut tracker =
            TrackTopologyChanges::new(Some(BrepId::from_index(0)), 91, true, Some(&mut context));
        let vertex = ObjectRef {
            type_code: 16021,
            key: ObjectKey(77),
        };
        context
            .sys_notify_callback_mut()
            .unwrap()
            .execute(NotifyOperation::PostEdit, vertex, Some(vertex), None, None)
            .unwrap();
        tracker.stop_tracking(Some(&mut context));
        assert_eq!(tracker.rm_list(), &[vertex.key]);
        assert_eq!(tracker.add_list(), &[vertex.key]);
        assert_eq!(tracker.lists().notify_events.len(), 1);
        assert!(context.sys_notify_callback().is_none());
    }

    #[test]
    fn geometry_tracking_requires_real_owner_metadata_and_bounds_cycles() {
        let curve = ObjectRef {
            type_code: crate::curve::curve::p2::LINE_TYPE,
            key: ObjectKey(77),
        };
        let mut callback = TopologyChangeCallback::new(1);
        assert!(callback
            .execute(NotifyOperation::PostEdit, curve, Some(curve), None, None)
            .is_err());
        assert!(callback.failure().is_some());
        assert!(callback.lists().chg_list.is_empty());
        let mut owners = BTreeMap::new();
        owners.insert(curve.key, Some(curve));
        let mut callback = TopologyChangeCallback::with_owners(1, owners);
        assert!(callback
            .execute(NotifyOperation::PostEdit, curve, Some(curve), None, None)
            .is_err());
        assert!(callback.lists().notify_events.is_empty());
    }
    #[test]
    fn captured_lists_survive_callback_replacement_and_active_reset() {
        let mut context = Context::default();
        let mut tracker =
            TrackTopologyChanges::new(Some(BrepId::from_index(0)), 91, true, Some(&mut context));
        let vertex = ObjectRef {
            type_code: 16021,
            key: ObjectKey(77),
        };
        context
            .sys_notify_callback_mut()
            .unwrap()
            .execute(NotifyOperation::PostEdit, vertex, Some(vertex), None, None)
            .unwrap();
        assert_eq!(tracker.add_list(), vec![vertex.key]);
        tracker.reset_arrays();
        assert!(tracker.add_list().is_empty());
        context
            .sys_notify_callback_mut()
            .unwrap()
            .execute(NotifyOperation::PostEdit, vertex, Some(vertex), None, None)
            .unwrap();
        context.set_sys_notify_callback(Some(Box::new(TopologyChangeCallback::new(92))));
        tracker.stop_tracking(Some(&mut context));
        assert_eq!(tracker.rm_list(), vec![vertex.key]);
        assert_eq!(tracker.add_list(), vec![vertex.key]);
        assert!(context.sys_notify_callback().is_none());
    }

    fn topo(kind: i64, k: u64) -> ObjectRef {
        ObjectRef {
            type_code: kind,
            key: ObjectKey(k),
        }
    }

    /// The base constructor has no upstream `bSmall` argument — nothing in the port
    /// sets `m_bIsSmallTopology` — so the predicate is false for every object the API
    /// can build. A `-> false` mutation is equivalent for that reason; `-> true` is
    /// not.
    #[test]
    fn small_topology_predicate_is_false() {
        assert!(!Topology::new().is_small_topology());
    }

    #[test]
    fn tracker_key_and_brep_accessors() {
        let t = TrackTopologyChanges::new(Some(BrepId::from_index(3)), 7, false, None);
        assert_eq!(t.key(), 7);
        assert_eq!(t.brep(), Some(BrepId::from_index(3)));
        assert!(!t.last_notify_callback());
    }

    /// `with_owners` supplies the owner links a geometry walk follows: the event's
    /// caller is the curve, the recorded change is its owner.
    #[test]
    fn change_tracker_uses_owner_metadata() {
        let curve = topo(crate::curve::curve::p2::LINE_TYPE, 77);
        let owner = topo(16021, 5);
        let mut owners = BTreeMap::new();
        owners.insert(curve.key, Some(owner));
        let mut cb = TopologyChangeCallback::with_owners(1, owners);
        cb.execute(NotifyOperation::PostEdit, curve, Some(curve), None, None)
            .unwrap();
        assert!(cb.failure().is_none());
        let lists = cb.lists();
        assert_eq!(lists.chg_list, vec![owner.key]);
        assert_eq!(lists.notify_events.len(), 1);
        let ev = lists.notify_events[0];
        assert_eq!(ev.operation, NotifyOperation::PostEdit);
        assert_eq!((ev.caller, ev.data1), (Some(curve), Some(curve)));
    }

    /// Reported owner links (a UV trim curve's edge use, the edge use's edge) carry a
    /// curve's `SM_NO_POST_EDIT` to the edge, as upstream's `GetOwner()` / `GetEdge()`
    /// walk; a curve without a reported link stays a failure.
    #[test]
    fn owner_links_reach_the_edge_through_an_edgeuse() {
        let curve = topo(crate::curve::curve::p2::BSPLINE_CURVE_TYPE, 77);
        let edgeuse = topo(16005, 6);
        let edge = topo(16016, 7);
        let mut cb = TopologyChangeCallback::new(1);
        cb.owner_link(curve.key, Some(edgeuse));
        cb.owner_link(edgeuse.key, Some(edge));
        cb.execute(NotifyOperation::PostEdit, curve, Some(curve), None, None)
            .unwrap();
        assert!(cb.failure().is_none());
        let lists = cb.lists();
        assert_eq!(lists.chg_list, vec![edge.key]);
        assert_eq!(lists.notify_events.len(), 1);
        let unlinked = topo(crate::curve::curve::p2::BSPLINE_CURVE_TYPE, 78);
        assert!(cb
            .execute(
                NotifyOperation::PostEdit,
                unlinked,
                Some(unlinked),
                None,
                None
            )
            .is_err());
    }

    /// `ChangeOwner` maintains the owner map; other events must not invent entries.
    #[test]
    fn change_owner_maintains_owner_metadata() {
        let mut cb = TopologyChangeCallback::new(1);
        let curve = topo(crate::curve::curve::p2::LINE_TYPE, 77);
        let owner = topo(16021, 5);
        cb.execute(NotifyOperation::ChangeOwner, curve, Some(owner), None, None)
            .unwrap();
        cb.execute(NotifyOperation::PostEdit, curve, Some(curve), None, None)
            .unwrap();
        assert_eq!(cb.lists().chg_list, vec![owner.key]);
        // the walk must not consume the owner entry
        cb.execute(NotifyOperation::PostEdit, curve, Some(curve), None, None)
            .unwrap();
        assert_eq!(cb.lists().notify_events.len(), 2);
        // an unrelated event does not invent metadata for its caller
        let other_curve = topo(crate::curve::curve::p2::LINE_TYPE, 78);
        let other_owner = topo(16021, 6);
        cb.execute(
            NotifyOperation::RmFromBrep,
            other_curve,
            Some(other_owner),
            None,
            None,
        )
        .unwrap();
        assert!(cb
            .execute(
                NotifyOperation::PostEdit,
                other_curve,
                Some(other_curve),
                None,
                None,
            )
            .is_err());
    }

    /// `Destruction` drops the destroyed object's owner entry; a later walk needs it.
    #[test]
    fn destruction_removes_owner_metadata() {
        let curve = topo(crate::curve::curve::p2::LINE_TYPE, 77);
        let owner = topo(16021, 5);
        let mut owners = BTreeMap::new();
        owners.insert(curve.key, Some(owner));
        let mut cb = TopologyChangeCallback::with_owners(1, owners);
        cb.execute(NotifyOperation::Destruction, curve, Some(curve), None, None)
            .unwrap();
        assert!(cb
            .execute(NotifyOperation::PostEdit, curve, Some(curve), None, None)
            .is_err());
    }

    /// Every tracked operation maintains the lists and the event history.
    #[test]
    fn change_tracker_maintains_lists_and_events() {
        let mut cb = TopologyChangeCallback::new(1);
        let face = topo(16002, 1);
        let e1 = topo(16016, 2);
        let e2 = topo(16016, 3);
        let e3 = topo(16016, 4);

        // PostEdit/ChangeGeometry track the caller, not data1.
        cb.execute(NotifyOperation::PostEdit, face, None, None, None)
            .unwrap();
        assert_eq!(cb.lists().rm_list, vec![face.key]);
        assert_eq!(cb.lists().add_list, vec![face.key]);
        cb.execute(NotifyOperation::ChangeGeometry, e1, None, None, None)
            .unwrap();
        assert_eq!(cb.lists().chg_list, vec![e1.key]);
        assert_eq!(cb.lists().notify_events.len(), 2);

        // SplitInBrep: remove data1 from add/chg, add data2/data3.
        cb.execute(NotifyOperation::AddToBrep, e1, Some(e1), None, None)
            .unwrap();
        assert!(cb.lists().add_list.contains(&e1.key));
        cb.execute(
            NotifyOperation::SplitInBrep,
            e2,
            Some(e1),
            Some(e2),
            Some(e3),
        )
        .unwrap();
        let lists = cb.lists();
        assert!(lists.rm_list.contains(&e1.key));
        assert!(!lists.add_list.contains(&e1.key));
        assert!(!lists.chg_list.contains(&e1.key));
        assert!(lists.add_list.contains(&e2.key) && lists.add_list.contains(&e3.key));
        assert!(lists
            .notify_events
            .iter()
            .any(|ev| ev.operation == NotifyOperation::SplitInBrep && ev.data1 == Some(e1)));

        // TrimNoSplitInBrep: remove from chg, add it back.
        cb.execute(NotifyOperation::ChangeGeometry, e2, None, None, None)
            .unwrap();
        assert!(cb.lists().chg_list.contains(&e2.key));
        cb.execute(NotifyOperation::TrimNoSplitInBrep, e2, Some(e2), None, None)
            .unwrap();
        let lists = cb.lists();
        assert!(!lists.chg_list.contains(&e2.key));
        assert!(lists.add_list.contains(&e2.key));

        // Destruction: drop the object from the add and change lists.
        cb.execute(NotifyOperation::ChangeGeometry, e3, None, None, None)
            .unwrap();
        assert!(cb.lists().add_list.contains(&e3.key) && cb.lists().chg_list.contains(&e3.key));
        cb.execute(NotifyOperation::Destruction, e3, Some(e3), None, None)
            .unwrap();
        let lists = cb.lists();
        assert!(!lists.add_list.contains(&e3.key));
        assert!(!lists.chg_list.contains(&e3.key));
    }

    /// The geometry owner walk stops at 64 links; the short chain covered above also
    /// catches a bound that rejects early.
    #[test]
    fn change_tracker_bounds_long_owner_chains() {
        let start = topo(crate::curve::curve::p2::LINE_TYPE, 1000);
        let mut owners = BTreeMap::new();
        for i in 0..70u64 {
            owners.insert(
                ObjectKey(1000 + i),
                Some(topo(crate::curve::curve::p2::LINE_TYPE, 1001 + i)),
            );
        }
        owners.insert(ObjectKey(1070), Some(topo(16021, 2000)));
        let mut cb = TopologyChangeCallback::with_owners(1, owners);
        let err = cb
            .execute(NotifyOperation::PostEdit, start, Some(start), None, None)
            .unwrap_err();
        assert!(matches!(err, SolidError::InvalidInput { .. }));
        assert!(cb.failure().is_some());
    }
}
