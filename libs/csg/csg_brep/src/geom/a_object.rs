//! Attributed objects (`SmAObject`): change notification that propagates
//! attributes, the attribute copy of the copy constructor, per-object cache slots
//! and the cache manager veneer (`SmCacheMgr`, `SmCacheCheckOutIn`), the
//! stand-alone-with-graphics base (`SmSAGObject`), and two inline NLib point helpers.
//!
//! Upstream: `source/SMLib/src/SmAObject.cpp`, `source/SMLib/inc/SmAObject.h`,
//! `source/SMLib/src/SmCacheMgr.cpp`, `source/SMLib/inc/SmCacheMgr.h`,
//! `source/SMLib/src/SmSAGObject.cpp`, `source/SMLib/inc/SmSAGObject.h`,
//! `source/SMLib/inc/NLI_Math.h`.
//!
//! | Rust                                   | Upstream                                              |
//! |----------------------------------------|-------------------------------------------------------|
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`] | `SmAObject::GetType` / `GetClassType`, `GetTypeString`, `GetClassString` |
//! | [`is_kind_of`]                         | `SmAObject::IsKindOf`                                 |
//! | [`assert_valid`]                       | `SmAObject::AssertValid`                              |
//! | [`notify`]                             | `SmAObject::Notify`                                   |
//! | [`copy_construct`]                     | `SmAObject::SmAObject(const SmAObject&)`              |
//! | [`AObject`]                            | `SmAObject` (its `m_pCacheObj` slot)                  |
//! | [`AObject::cache_obj`], [`AObject::set_cache_obj`] | `SmAObject::GetCacheObj`, `SetCacheObj`   |
//! | [`AObject::destroy`]                   | `SmAObject::~SmAObject`                               |
//! | [`cache_make_or_validate`]             | `SmAObject::CacheMakeOrValidate`                      |
//! | [`CacheObj`], [`CacheStore`], [`CacheId`] | `SmCacheObj` and the heap holding cache objects    |
//! | [`CacheStore::check_out_cache`]        | `SmCacheMgr::CheckOutCache`                           |
//! | [`CacheStore::check_in_cache`]         | `SmCacheMgr::CheckInCache`                            |
//! | [`CacheStore::soft_delete`]            | `SmCacheMgr::SoftDelete`                              |
//! | [`get_or_create_object_cache`]         | `SmCacheMgr::GetOrCreateObjectCache`                  |
//! | [`get_object_cache`]                   | `SmCacheMgr::GetObjectCache`                          |
//! | [`delete_objects_cache`]               | both `SmCacheMgr::DeleteObjectsCache` overloads       |
//! | [`CacheCheckOutIn`]                    | `SmCacheCheckOutIn` (constructor, destructor)         |
//! | [`ObjectCacheType`]                    | `SmObjectCacheType`                                   |
//! | [`CacheMaker`]                         | the virtual dispatch of `CacheMakeOrValidate`         |
//! | [`SAG_TYPE`], [`SAG_TYPE_STRING`], [`SAG_CLASS_STRING`] | `SmSAGObject::GetType`, `GetTypeString`, `GetClassString` |
//! | [`sag_is_kind_of`]                     | `SmSAGObject::IsKindOf`                               |
//! | [`sag_notify`]                         | `SmSAGObject::Notify`                                 |
//! | [`sag_cache_make_or_validate`]         | `SmSAGObject::CacheMakeOrValidate`                    |
//! | `Drop` (implicit)                      | `SmSAGObject::~SmSAGObject`, `SmCacheMgr::~SmCacheMgr` |
//! | [`BooleanOperation`]                   | `SmBooleanOperationType`                              |
//! | [`cpt_to_pt_euclid_specialized`]       | `NI_CPtToPtEuclid_specialized`                        |
//! | [`vector_blend_cpt_specialized`]       | `NI_VectorBlendCPt_specialized`                       |
//! | [`reverse_knots`]                      | `NI_ReverseKnots`                                     |
//!
//! Deviations:
//! - **Object model.** There is no class hierarchy: an attributed object is an
//!   [`ObjectRef`] (type code plus [`ObjectKey`]) whose attribute list lives in an
//!   [`AttributeStore`] (the list methods `AddAttribute`, `RemoveAttribute`,
//!   `ClearAttributes`, `GetAttributes` and `FindAttribute` were co-ported there by
//!   unit `geom.attribute`), plus an [`AObject`] holding its cache slot. The context
//!   an object reaches through `GetContext` is passed explicitly (`None` for a null
//!   context). Virtual dispatch of `Notify` and `CacheMakeOrValidate` becomes the
//!   caller choosing [`notify`] / [`sag_notify`] and a [`CacheMaker`].
//! - **Type checks.** `SM_CAST_PTR(SmRegion, ...)`, `SM_CAST_PTR(SmBrep, ...)` and
//!   `SM_CAST_PTR(SmPolyBrep, ...)` compare the object's type code with
//!   `SmRegion_TYPE`, `SmBrep_TYPE` and `SmPolyBrep_TYPE` (no class derives from
//!   these).
//! - **Undefined behaviour.** Where upstream would dereference a null data object
//!   (a handler called with a missing child or target), read a dead attribute, cast
//!   a missing or foreign `SM_AI_OTHER_REGION_ID` attribute, or touch a deleted
//!   cache, Rust returns [`SolidError::InvalidInput`] instead.
//! - **Caches.** Cache objects live in a [`CacheStore`] (append-only slots addressed
//!   by [`CacheId`]); `delete pCache` frees the slot. The no-context check of
//!   `GetOrCreateObjectCache` reads the `context` argument.
//! - **Region lookup.** [`notify_with_regions`] resolves implicit native region
//!   pointers through borrowed live typed records, preserving raw void flags.
//!   The legacy entry point returns an explicit context error only when a region
//!   query is actually needed; generic attribute operations remain available.
//! - **Cache factories.** A new B-rep, curve or surface cache requires its source
//!   geometry. The object-level validators reuse existing caches; callers building
//!   new ones use `make_brep_cache`, `make_curve_cache` or the surface factories.
//!   Calling without that geometry returns [`SolidError::InvalidInput`].
//! - **Graphics and debug.** The display list of `SmSAGObject` (reset on edits and
//!   destruction), `SE_MSG` / `SM_ASSERT*` (compiled out of the release reference
//!   build) and `Dump` are not ported. `SmAObject::AssertValid`'s two checks (the
//!   attribute pointer's class, the attributes' context) cannot fail in this data
//!   model, so it reports success, as upstream does for every well-formed object.
//! - **Inline helpers.** The `NLI_Math.h` functions return their result instead of
//!   writing an out-parameter.

use crate::error::{Result, SolidError};
use crate::geom::attribute::{ids, Attribute, AttributeHandle, AttributeStore, ObjectKey};
use crate::geom::context::Context;
use crate::geom::object::{self, NotifyOperation, ObjectRef};
use crate::math::Vec3;
use crate::nurbs::math::{CPoint, NOW, NOZ};
use crate::topology;
use crate::topology::topology::TopologyRecords;

/// `SmAObject_TYPE` (`CONT_BASE_TYPE + 60`).
pub const TYPE: i64 = 12060;

/// `SmAObject::GetTypeString`.
pub const TYPE_STRING: &str = "SmAObject_TYPE";

/// `SmAObject::GetClassString`.
pub const CLASS_STRING: &str = "SmAObject";

/// `SmSAGObject_TYPE` (`TOPO_BASE_TYPE + 50`).
pub const SAG_TYPE: i64 = 16050;

/// `SmSAGObject::GetTypeString`.
pub const SAG_TYPE_STRING: &str = "SmSAGObject_TYPE";

/// `SmSAGObject::GetClassString`.
pub const SAG_CLASS_STRING: &str = "SmSAGObject";

/// `SmRegion_TYPE` (`TOPO_BASE_TYPE + 12`).
pub const REGION_TYPE: i64 = 16012;

/// `SmBrep_TYPE` (`TOPO_BASE_TYPE + 11`).
pub const BREP_TYPE: i64 = 16011;

/// `SmPolyBrep_TYPE` (`POLY_BASE_TYPE + 1`).
pub const POLY_BREP_TYPE: i64 = 18001;

/// Boolean operation recorded on a merge-region attribute (`SmBooleanOperationType`),
/// as read by the region propagation of [`notify`]. The discriminants are the
/// upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum BooleanOperation {
    /// `SM_BO_UNKNOWN`.
    Unknown = 0,
    /// `SM_BO_UNION`.
    Union = 1,
    /// `SM_BO_INTERSECTION`.
    Intersection = 2,
    /// `SM_BO_DIFFERENCE`.
    Difference = 3,
    /// `SM_BO_EXCLUSIVE_OR`.
    ExclusiveOr = 4,
    /// `SM_BO_MERGE`.
    Merge = 5,
    /// `SM_BO_PARTIAL_MERGE`.
    PartialMerge = 6,
    /// `SM_BO_IMPRINT`.
    Imprint = 7,
    /// `SM_BO_IMPRINT_CLASSIFY`.
    ImprintClassify = 8,
    /// `SM_BO_EXTRACT_SEPARATE`.
    ExtractSeparate = 9,
    /// `SM_BO_SLICE`.
    Slice = 10,
}

impl BooleanOperation {
    /// The operation with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Unknown),
            1 => Some(Self::Union),
            2 => Some(Self::Intersection),
            3 => Some(Self::Difference),
            4 => Some(Self::ExclusiveOr),
            5 => Some(Self::Merge),
            6 => Some(Self::PartialMerge),
            7 => Some(Self::Imprint),
            8 => Some(Self::ImprintClassify),
            9 => Some(Self::ExtractSeparate),
            10 => Some(Self::Slice),
            _ => None,
        }
    }
}

/// `SmAObject::IsKindOf`: [`TYPE`], then the `SmObject` chain.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    if t == TYPE {
        true
    } else {
        object::is_kind_of(t)
    }
}

/// `SmSAGObject::IsKindOf`: [`SAG_TYPE`], then the `SmAObject` chain (upstream skips
/// `SmOwningTopology` / `SmTopology` here).
#[must_use]
pub const fn sag_is_kind_of(t: i64) -> bool {
    if t == SAG_TYPE {
        true
    } else {
        is_kind_of(t)
    }
}

/// `SmAObject::AssertValid`: the `SmObject` checks (skipped when the tests are given
/// explicitly, `level_given`), then the attribute-list class and context checks,
/// which always hold here (see the module docs).
#[must_use]
pub const fn assert_valid(level_given: bool) -> bool {
    level_given || object::assert_valid()
}

/// The object an attribute handler needs, or the error upstream's null dereference
/// becomes.
fn need(o: Option<ObjectRef>, reason: &'static str) -> Result<ObjectKey> {
    o.map(|r| r.key).ok_or(SolidError::InvalidInput {
        operation: "geom::a_object::notify",
        reason,
    })
}

/// `pAttr->Merge(pOrig1, pOrig2, pMerged)`: the merge-region attribute's handler
/// reads all three objects, the default one only the merged object.
fn merge_handler(
    attributes: &mut AttributeStore,
    h: AttributeHandle,
    orig1: Option<ObjectRef>,
    orig2: Option<ObjectRef>,
    merged: Option<ObjectRef>,
    regions: Option<&dyn topology::shell::RegionLookup>,
) -> Result<()> {
    let merged = need(merged, "merge without a merged object")?;
    let is_mr = attribute_of(attributes, h)?.merge_region_data().is_some();
    if is_mr {
        let o1 = need(orig1, "merge-region merge without its first object")?;
        let o2 = need(orig2, "merge-region merge without its second object")?;
        if let Some(regions) = regions {
            attributes.merge_with_regions(h, o1, o2, merged, regions)
        } else {
            attributes.merge(h, o1, o2, merged)
        }
    } else {
        // The default handler does not read the original objects.
        let o1 = orig1.map_or(merged, |r| r.key);
        let o2 = orig2.map_or(merged, |r| r.key);
        attributes.merge(h, o1, o2, merged)
    }
}

/// The live attribute `h`, or the error upstream's dead-pointer read becomes.
fn attribute_of(attributes: &AttributeStore, h: AttributeHandle) -> Result<&Attribute> {
    attributes.get(h).ok_or(SolidError::InvalidInput {
        operation: "geom::a_object::notify",
        reason: "attribute was deleted",
    })
}

/// `SmAObject::Notify`: runs the attribute handler `operation` calls for on every
/// attribute of the source object(s), then passes the notification to the context
/// callbacks ([`object::notify`]).
///
/// Sources and handlers, per operation (`this` the notified object):
///
/// | operation              | source attributes | handler                              |
/// |------------------------|-------------------|--------------------------------------|
/// | `SplitInBrep`          | `data1`           | `Split(data1, data2, data3)`         |
/// | `MergeInBrep`          | `data1`, `data2`  | `Merge(data1, data2, data1)`         |
/// | `Coincident`           | `data2`           | `Coincident(data2, data1)`           |
/// | `ChangeGeometry`       | `data3` if `data1` | `Copy(data3, data1)`                |
/// | `Construction`         | `data2`           | `Copy(data2, this)`                  |
/// | `Copy`                 | `this`            | `Copy(this, data1)`                  |
/// | `PreEdit`              | `this`            | `Edit(this)`                         |
/// | `Split`                | `this`            | `Split(this, data1, data2)`          |
/// | `Merge`                | `data1`, `data2`  | `Merge(data1, data2, this)`          |
/// | `Destruction`          | `this`            | `Destruction(this)`                  |
/// | `RegPropagation`       | `this` (a region) | Boolean attribute propagation        |
///
/// For the two merges, an attribute of the second source whose id is also on the
/// first source's list is skipped. Other operations run no handler.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences a null or dead object
/// (a handler needing a missing data object; region propagation on a region without
/// a merge-region attribute); errors and [`SolidError::Unported`] from the handlers;
/// a region lookup context is required when the handlers query a live region;
/// use [`notify_with_regions`] for those operations.
pub fn notify(
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
) -> Result<()> {
    notify_with_regions(
        attributes, context, operation, this, data1, data2, data3, None,
    )
}

/// Notify with an explicit borrowed lookup for region attribute handlers.
/// Attribute and callback order is identical to [`notify`].
///
/// # Errors
/// Attribute errors, missing topology context or failed region resolution.
pub fn notify_with_regions(
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
    regions: Option<&dyn topology::shell::RegionLookup>,
) -> Result<()> {
    use NotifyOperation as N;
    // get all attributes from target source object
    let (src1, src2) = match operation {
        N::SplitInBrep => (data1, None),
        N::MergeInBrep | N::Merge => (data1, data2),
        N::Coincident | N::Construction => (data2, None),
        N::Copy | N::PreEdit | N::Split | N::Destruction | N::RegPropagation => (Some(this), None),
        N::ChangeGeometry => (if data1.is_some() { data3 } else { None }, None),
        _ => (None, None),
    };

    let mut attrs1: Vec<AttributeHandle> = Vec::new();
    let mut attrs2: Vec<AttributeHandle> = Vec::new();

    // for both possible source objects
    for ii in 0..2 {
        let use_second = match (ii, src1, src2) {
            (0, Some(s), _) => {
                attrs1 = attributes.attributes(s.key);
                false
            }
            (1, _, Some(s)) => {
                attrs2 = attributes.attributes(s.key);
                true
            }
            _ => {
                attrs1.clear();
                false
            }
        };

        if operation == N::RegPropagation {
            reg_propagation(attributes, this, &attrs1, &mut attrs2, regions)?;
        } else {
            let list = if use_second {
                attrs2.clone()
            } else {
                attrs1.clone()
            };
            // call the operation action function on every attribute
            for h in list {
                match operation {
                    N::SplitInBrep => {
                        let split = need(data1, "split without the split object")?;
                        let c1 = need(data2, "split without its first child")?;
                        let c2 = need(data3, "split without its second child")?;
                        attributes.split(h, split, c1, c2)?;
                    }
                    N::MergeInBrep => {
                        if do_merge_call(attributes, ii, &attrs1, h)? {
                            merge_handler(attributes, h, data1, data2, data1, regions)?;
                        }
                    }
                    N::Coincident => {
                        let from = need(data2, "coincident without its source object")?;
                        let to = need(data1, "coincident without its target object")?;
                        attributes.coincident(h, from, to)?;
                    }
                    N::ChangeGeometry => {
                        let from = need(data3, "change of geometry without the old geometry")?;
                        let to = need(data1, "change of geometry without the new geometry")?;
                        attributes.copy(h, from, to)?;
                    }
                    N::Construction => {
                        let from = need(data2, "construction without its source object")?;
                        attributes.copy(h, from, this.key)?;
                    }
                    N::Copy => {
                        let to = need(data1, "copy without its target object")?;
                        attributes.copy(h, this.key, to)?;
                    }
                    N::PreEdit => attributes.edit(h, this.key)?,
                    N::Split => {
                        let c1 = need(data1, "split without its first child")?;
                        let c2 = need(data2, "split without its second child")?;
                        attributes.split(h, this.key, c1, c2)?;
                    }
                    N::Merge => {
                        if do_merge_call(attributes, ii, &attrs1, h)? {
                            merge_handler(attributes, h, data1, data2, Some(this), regions)?;
                        }
                    }
                    N::Destruction => attributes.destruction(h, this.key)?,
                    // SM_NO_ADD_TO_BREP, SM_NO_RM_FROM_BREP, SM_NO_CHANGE_OWNER,
                    // SM_NO_POST_EDIT: nothing; SM_NO_UNKNOWN and the rest: SE_MSG only.
                    _ => {}
                }
            }
        }
    }

    // pass the call up to the base class
    object::notify(context, operation, this, data1, data2, data3);
    Ok(())
}

/// The duplicate-call check of the two merges: on the second source (`ii == 1`), an
/// attribute whose id is on the first source's list was already merged.
fn do_merge_call(
    attributes: &AttributeStore,
    ii: usize,
    attrs1: &[AttributeHandle],
    h: AttributeHandle,
) -> Result<bool> {
    if ii != 1 {
        return Ok(true);
    }
    let id = attribute_of(attributes, h)?.attribute_id();
    for &a in attrs1 {
        if attribute_of(attributes, a)?.attribute_id() == id {
            return Ok(false);
        }
    }
    Ok(true)
}

/// What region propagation does with one attribute (`SmAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// `SM_ACT_NONE`.
    None,
    /// `SM_ACT_KEEP_THIS`.
    KeepThis,
    /// `SM_ACT_SET_FROM_OTHER`.
    SetFromOther,
    /// `SM_ACT_MERGE`.
    Merge,
}

/// The `SM_NO_REG_PROPAGATION` branch of `SmAObject::Notify` for one pass over the
/// source objects. `attrs1` is the pass's copy of the region's attributes (empty on
/// the second pass, as upstream); `attrs2` persists across passes like upstream's
/// `sAttributes2`.
fn reg_propagation(
    attributes: &mut AttributeStore,
    this: ObjectRef,
    attrs1: &[AttributeHandle],
    attrs2: &mut Vec<AttributeHandle>,
    regions: Option<&dyn topology::shell::RegionLookup>,
) -> Result<()> {
    const OP: &str = "geom::a_object::notify";
    // error case - this obj is not a Region, skip problem cases
    if this.type_code != REGION_TYPE {
        return Ok(());
    }
    let region = this.key;
    let mr = attributes
        .find_attribute(region, ids::OTHER_REGION_ID)
        .ok_or(SolidError::InvalidInput {
            operation: OP,
            reason: "region has no merge-region attribute",
        })?;
    let data =
        attribute_of(attributes, mr)?
            .merge_region_data()
            .ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "SM_AI_OTHER_REGION_ID attribute is not a merge-region attribute",
            })?;
    let operation = BooleanOperation::from_code(data.operation());
    let other_regions = data.other_regions().to_vec();

    // See if any Other or This Attribs are propagated through booleans
    let has_other_propagated = attributes.has_any_other_propagated_attribs(mr)?;
    let mut has_this_propagated = false;
    for &a in attrs1 {
        if attribute_of(attributes, a)?.is_propagated_through_booleans() {
            has_this_propagated = true;
            break;
        }
    }

    // no work - no propagated attributes
    if !has_other_propagated && !has_this_propagated {
        return Ok(());
    }

    let tgt_region = region;
    // for every OtherSrcRegion
    for other_region in other_regions {
        let other_solid = !region_is_void(other_region, regions)?;
        let attrib1_size = attrs1.len();
        let attrib2_size = attrs2.len();
        *attrs2 = attributes.attributes(other_region);

        // for every TgtRegion and OtherSrcRegion Attribute
        for kk in 0..attrib1_size + attrib2_size {
            let (tgt_attrib, other_attrib) = if kk < attrs1.len() {
                let tgt = attrs1.get(kk).copied().ok_or(SolidError::InvalidInput {
                    operation: OP,
                    reason: "attribute index out of range",
                })?;
                let id = attribute_of(attributes, tgt)?.attribute_id();
                let other = attributes.find_attribute(other_region, id);
                // no work - not a propagated attribute
                if !attribute_of(attributes, tgt)?.is_propagated_through_booleans() {
                    continue;
                }
                (Some(tgt), other)
            } else {
                // upstream reads past the refilled list when it shrank
                let other =
                    attrs2
                        .get(kk - attrib1_size)
                        .copied()
                        .ok_or(SolidError::InvalidInput {
                            operation: OP,
                            reason: "other region attribute index out of range",
                        })?;
                let id = attribute_of(attributes, other)?.attribute_id();
                // skip cases where TgtRegion has this attribute
                if attributes.find_attribute(tgt_region, id).is_some() {
                    continue;
                }
                // no work - not a propagated attribute
                if !attribute_of(attributes, other)?.is_propagated_through_booleans() {
                    continue;
                }
                (None, Some(other))
            };

            let tgt = tgt_attrib.is_some();
            let oth = other_attrib.is_some();

            let action = match operation {
                Some(BooleanOperation::Union) => {
                    if other_solid {
                        if tgt && oth {
                            Action::Merge
                        } else if tgt {
                            Action::KeepThis
                        } else {
                            Action::SetFromOther
                        }
                    } else if tgt && oth {
                        Action::None
                    } else if tgt {
                        Action::KeepThis
                    } else {
                        Action::None
                    }
                }
                Some(BooleanOperation::Merge | BooleanOperation::PartialMerge) => {
                    if tgt {
                        if oth {
                            Action::Merge
                        } else {
                            Action::KeepThis
                        }
                    } else if oth {
                        Action::SetFromOther
                    } else {
                        Action::None
                    }
                }
                Some(BooleanOperation::Intersection) => {
                    if other_solid {
                        if tgt && oth {
                            Action::Merge
                        } else if tgt {
                            Action::KeepThis
                        } else {
                            Action::SetFromOther
                        }
                    } else {
                        Action::None
                    }
                }
                Some(BooleanOperation::ExclusiveOr) => {
                    if other_solid {
                        if tgt && oth {
                            Action::SetFromOther
                        } else if tgt {
                            Action::KeepThis
                        } else {
                            Action::SetFromOther
                        }
                    } else {
                        Action::None
                    }
                }
                // SM_BO_DIFFERENCE, SM_BO_SLICE, SM_BO_EXTRACT_SEPARATE, SM_BO_UNKNOWN
                // and any other value.
                _ => Action::None,
            };

            // set attribute values
            match action {
                Action::SetFromOther => {
                    if let Some(t) = tgt_attrib {
                        attributes.remove_attribute(region, t, false)?;
                    }
                    let o = other_attrib.ok_or(SolidError::InvalidInput {
                        operation: OP,
                        reason: "no other attribute to set",
                    })?;
                    attributes.add_attribute(region, o)?;
                }
                Action::Merge => {
                    let t = tgt_attrib.ok_or(SolidError::InvalidInput {
                        operation: OP,
                        reason: "no target attribute to merge",
                    })?;
                    attributes.propagate_regions_through_booleans(
                        t,
                        region,
                        other_region,
                        region,
                    )?;
                }
                Action::KeepThis | Action::None => {}
            }
        }
    }
    Ok(())
}

/// Native truth of the actual region's raw void flag, including UNSURE.
fn region_is_void(
    region: ObjectKey,
    regions: Option<&dyn topology::shell::RegionLookup>,
) -> Result<bool> {
    // UNPORTED(topology.shell): SmRegion::IsVoid need a caller-supplied
    // `RegionLookup`; see plan/findings/topology.shell--borrowed-region-context-for-attribute-backedges-pr-80-issue.md.
    // Callers that pass no lookup (`a_object::notify` from change-owner / geometry edits in
    // brep p6/p9/p10, edge, face, fillet) never send a region Merge or RegPropagation
    // notification, so no region handler runs without one; the `None` deferral is covered by the unit tests of this file.
    // REGION-LOOKUP(#86): no library `RegionLookup` exists yet, so a region handler
    // reached without a caller-supplied context defers here instead of inferring
    // region state. The owner of issue #86 replaces this with the real lookup
    // (grep REGION-LOOKUP).
    let lookup =
        regions.ok_or_else(|| SolidError::unported("SmRegion::IsVoid", "topology.shell"))?;
    Ok(lookup.resolve_region(region)?.void_state()? != 0)
}

/// The copy constructor `SmAObject(const SmAObject&)`: `Copy(from, to)` of every
/// attribute of `from`, so `to` gets a copy of (or a reference to) each.
///
/// # Errors
///
/// Errors of [`AttributeStore::copy`].
pub fn copy_construct(
    attributes: &mut AttributeStore,
    from: ObjectKey,
    to: ObjectKey,
) -> Result<()> {
    for h in attributes.attributes(from) {
        attributes.copy(h, from, to)?;
    }
    Ok(())
}

/// `SmSAGObject::Notify`: drops the saved display list (no graphics here) and passes
/// the notification to `SmOwningTopology::Notify` (which is
/// [`topology::topology::notify`]; `this` and the data objects that operation
/// touches need records in `records`).
///
/// # Errors
///
/// Errors of [`topology::topology::notify`].
pub fn sag_notify<R: TopologyRecords + ?Sized>(
    records: &mut R,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
) -> Result<()> {
    sag_notify_with_regions(
        records, attributes, context, operation, this, data1, data2, data3, None,
    )
}

/// SAG notification with the caller's actual region lookup.
///
/// # Errors
/// Topology/attribute errors or failed region resolution.
pub fn sag_notify_with_regions<R: TopologyRecords + ?Sized>(
    records: &mut R,
    attributes: &mut AttributeStore,
    context: Option<&mut Context>,
    operation: NotifyOperation,
    this: ObjectRef,
    data1: Option<ObjectRef>,
    data2: Option<ObjectRef>,
    data3: Option<ObjectRef>,
    regions: Option<&dyn topology::shell::RegionLookup>,
) -> Result<()> {
    topology::topology::notify_with_regions(
        records, attributes, context, operation, this, data1, data2, data3, regions,
    )
}

// ---- caches ---------------------------------------------------------------------

/// The kind of cache asked for (`SmObjectCacheType`). The discriminants are the
/// upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum ObjectCacheType {
    /// `SM_OC_CURVE`.
    Curve = 0,
    /// `SM_OC_SURFACE`.
    Surface = 1,
    /// `SM_OC_TRIMSRF`.
    TrimSrf = 2,
    /// `SM_OC_BREP`.
    Brep = 3,
}

impl ObjectCacheType {
    /// The cache type with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Curve),
            1 => Some(Self::Surface),
            2 => Some(Self::TrimSrf),
            3 => Some(Self::Brep),
            _ => None,
        }
    }
}

/// Address of a cache object in a [`CacheStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheId(u32);

impl CacheId {
    /// Slot index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A cache object (`SmCacheObj`): its data plus the use count that delays its
/// deletion.
#[derive(Debug, Clone, PartialEq)]
pub struct CacheObj<T> {
    /// `m_lUseCount`: check-outs not yet checked in.
    use_count: u64,
    /// `m_bDeleted`: deletion was asked for while checked out.
    deleted: bool,
    /// The cached data (the derived cache class).
    data: T,
}

impl<T> CacheObj<T> {
    /// `SmCacheObj()`: not checked out, not deleted.
    pub const fn new(data: T) -> Self {
        Self {
            use_count: 0,
            deleted: false,
            data,
        }
    }

    /// `m_lUseCount`.
    #[must_use]
    pub const fn use_count(&self) -> u64 {
        self.use_count
    }

    /// `m_bDeleted`.
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// The cached data.
    #[must_use]
    pub const fn data(&self) -> &T {
        &self.data
    }

    /// The cached data, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

/// Owner of cache objects (the heap they are `new`ed on). Slots are never reused, so
/// a stale [`CacheId`] reports "not live".
#[derive(Debug, Clone, PartialEq)]
pub struct CacheStore<T> {
    slots: Vec<Option<CacheObj<T>>>,
}

impl<T> Default for CacheStore<T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

fn dead_cache(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "cache object was deleted",
    }
}

impl<T> CacheStore<T> {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes ownership of a new cache object.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the store is full.
    pub fn insert(&mut self, data: T) -> Result<CacheId> {
        let index = u32::try_from(self.slots.len()).map_err(|_| SolidError::InvalidInput {
            operation: "geom::a_object::CacheStore::insert",
            reason: "too many cache objects",
        })?;
        self.slots.push(Some(CacheObj::new(data)));
        Ok(CacheId(index))
    }

    /// The cache object, if live.
    #[must_use]
    pub fn get(&self, id: CacheId) -> Option<&CacheObj<T>> {
        self.slots.get(id.index()).and_then(Option::as_ref)
    }

    /// The cache object, mutably, if live.
    pub fn get_mut(&mut self, id: CacheId) -> Option<&mut CacheObj<T>> {
        self.slots.get_mut(id.index()).and_then(Option::as_mut)
    }

    /// Whether the cache object has not been deleted.
    #[must_use]
    pub fn is_live(&self, id: CacheId) -> bool {
        self.get(id).is_some()
    }

    /// `SmCacheMgr::CheckOutCache`: one more use of the cache (no-op for `None`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted cache.
    pub fn check_out_cache(&mut self, cache: Option<CacheId>) -> Result<()> {
        if let Some(id) = cache {
            let c = self
                .get_mut(id)
                .ok_or_else(|| dead_cache("geom::a_object::CacheStore::check_out_cache"))?;
            c.use_count = c.use_count.wrapping_add(1);
        }
        Ok(())
    }

    /// `SmCacheMgr::CheckInCache`: one use less; a cache whose deletion was asked for
    /// is soft-deleted again (deleted once no use is left). No-op for `None`. The
    /// count wraps like upstream's `ULONG`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted cache.
    pub fn check_in_cache(&mut self, cache: Option<CacheId>) -> Result<()> {
        if let Some(id) = cache {
            let c = self
                .get_mut(id)
                .ok_or_else(|| dead_cache("geom::a_object::CacheStore::check_in_cache"))?;
            c.use_count = c.use_count.wrapping_sub(1);
            if c.deleted {
                self.soft_delete(id)?;
            }
        }
        Ok(())
    }

    /// `SmCacheMgr::SoftDelete`: deletes the cache when unused, else marks it for
    /// deletion at its last check-in. Returns whether it was deleted.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted cache.
    pub fn soft_delete(&mut self, cache: CacheId) -> Result<bool> {
        let c = self
            .get_mut(cache)
            .ok_or_else(|| dead_cache("geom::a_object::CacheStore::soft_delete"))?;
        if c.use_count == 0 {
            if let Some(slot) = self.slots.get_mut(cache.index()) {
                *slot = None;
            }
            Ok(true)
        } else {
            c.deleted = true;
            Ok(false)
        }
    }
}

/// The part of an attributed object that is not its attribute list (kept in an
/// [`AttributeStore`]): its cache slot (`m_pCacheObj`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AObject {
    /// `m_pCacheObj`.
    cache: Option<CacheId>,
}

impl AObject {
    /// `SmAObject()` and the copy constructor's members: no cache.
    #[must_use]
    pub const fn new() -> Self {
        Self { cache: None }
    }

    /// `GetCacheObj`.
    #[must_use]
    pub const fn cache_obj(&self) -> Option<CacheId> {
        self.cache
    }

    /// `SetCacheObj`.
    pub fn set_cache_obj(&mut self, cache: Option<CacheId>) {
        self.cache = cache;
    }

    /// `~SmAObject`: `ClearAttributes`, then `SmCacheMgr::DeleteObjectsCache(this)`.
    ///
    /// # Errors
    ///
    /// Errors of [`AttributeStore::clear_attributes`] and [`delete_objects_cache`].
    pub fn destroy<T>(
        mut self,
        key: ObjectKey,
        attributes: &mut AttributeStore,
        caches: &mut CacheStore<T>,
    ) -> Result<()> {
        attributes.clear_attributes(key)?;
        delete_objects_cache(caches, &mut self)
    }
}

/// `SmCacheMgr::GetObjectCache`: the object's cache, `None` without an object (the
/// cache type is not used).
#[must_use]
pub fn get_object_cache(_cache_type: ObjectCacheType, object: Option<&AObject>) -> Option<CacheId> {
    object.and_then(AObject::cache_obj)
}

/// `SmCacheMgr::DeleteObjectsCache` (both overloads; the cache type is not used):
/// soft-deletes the object's cache, if any, and clears its slot.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when the slot holds a deleted cache.
pub fn delete_objects_cache<T>(caches: &mut CacheStore<T>, object: &mut AObject) -> Result<()> {
    if let Some(c) = object.cache {
        caches.soft_delete(c)?;
    }
    object.cache = None;
    Ok(())
}

/// Which `CacheMakeOrValidate` override an object has (its class).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMaker {
    /// `SmAObject::CacheMakeOrValidate` (regions, topology, volumes, ...).
    AObject,
    /// `SmSAGObject::CacheMakeOrValidate` (B-reps, poly B-reps, assemblies).
    SagObject,
    /// `SmCurve::CacheMakeOrValidate`.
    Curve,
    /// `SmSurface::CacheMakeOrValidate`.
    Surface,
}

/// `SmAObject::CacheMakeOrValidate`: an object without a cache kind has no cache
/// (success, `None`).
#[must_use]
pub const fn cache_make_or_validate(
    _cache_type: ObjectCacheType,
    _old: Option<CacheId>,
) -> Option<CacheId> {
    None
}

/// `SmSAGObject::CacheMakeOrValidate` for a target object of type `target_type`: a
/// B-rep or poly B-rep reuses its existing cache `old`; without one a new
/// `SmBrepCache` is built (and stored in `this`).
///
/// # Errors
///
/// `SM_ERR` ([`SolidError::Upstream`]) when the target is neither a B-rep nor a poly
/// B-rep; [`SolidError::InvalidInput`] when a new cache needs source geometry.
pub fn sag_cache_make_or_validate(
    _cache_type: ObjectCacheType,
    target_type: i64,
    old: Option<CacheId>,
) -> Result<Option<CacheId>> {
    // check that object is an SmBrep or SmPolyBrep object
    if target_type != BREP_TYPE && target_type != POLY_BREP_TYPE {
        return Err(SolidError::sm_err(
            "geom::a_object::sag_cache_make_or_validate",
        ));
    }
    // low work - use OldCache when its given
    if let Some(old) = old {
        return Ok(Some(old));
    }
    Err(SolidError::invalid_input(
        "geom::a_object::sag_cache_make_or_validate",
        "new B-rep cache requires its geometry (make_brep_cache)",
    ))
}

/// `SmCacheMgr::GetOrCreateObjectCache`: the object's cache, validated or built by its
/// class's `CacheMakeOrValidate` (`maker`, `object_type` its type code). `None` when
/// the object has no context (upstream also signals `SM_ERR`), its class keeps no
/// cache, or the maker fails.
///
/// # Errors
///
/// [`SolidError::Unported`] when the maker needs an unported unit.
pub fn get_or_create_object_cache(
    cache_type: ObjectCacheType,
    context: Option<&Context>,
    maker: CacheMaker,
    object_type: i64,
    object: &AObject,
) -> Result<Option<CacheId>> {
    // check state - the object must have a memory context.
    if context.is_none() {
        return Ok(None);
    }
    let old = object.cache_obj();
    let made = match maker {
        CacheMaker::AObject => Ok(cache_make_or_validate(cache_type, old)),
        CacheMaker::SagObject => match sag_cache_make_or_validate(cache_type, object_type, old) {
            // Like curve/surface factories, construction needs source geometry.
            Err(e @ (SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })) => {
                return Err(e)
            }
            made => made,
        },
        CacheMaker::Curve => match crate::curve::curve::p5::validate_curve_cache(cache_type, old) {
            Ok(Some(old)) => Ok(Some(old)),
            // a new curve cache needs the curve: built with `make_curve_cache`
            Ok(None) => {
                return Err(SolidError::invalid_input(
                    "geom::a_object::get_or_create_object_cache",
                    "a new curve cache is built from the curve (make_curve_cache)",
                ))
            }
            Err(e) => Err(e),
        },
        CacheMaker::Surface => {
            // The face state of an old trimmed cache (m_bFaceWasModified, the B-rep's
            // m_bEditingEnabled) is not known here: an old cache is always reused. A
            // caller holding the cache and its face must pass that state to
            // validate_surface_cache itself (no caller yet).
            match crate::surface::surface::p1::validate_surface_cache(cache_type, old, false, false)
            {
                Ok(Some(old)) => Ok(Some(old)),
                // a new surface cache needs the surface: built with make_trim_srf_cache /
                // make_surface_cache
                Ok(None) => {
                    return Err(SolidError::invalid_input(
                        "geom::a_object::get_or_create_object_cache",
                        "a new surface cache is built from the surface (make_surface_cache)",
                    ))
                }
                Err(e) => Err(e),
            }
        }
    };
    match made {
        Ok(cache) => Ok(cache),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        // a failing CacheMakeOrValidate: no cache
        Err(_) => Ok(None),
    }
}

/// A checked-out cache (`SmCacheCheckOutIn`): a check-out on creation, the matching
/// check-in on [`CacheCheckOutIn::release`] (the destructor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "release the guard to check the cache back in"]
pub struct CacheCheckOutIn {
    /// `m_pCache`.
    cache: Option<CacheId>,
}

impl CacheCheckOutIn {
    /// `SmCacheCheckOutIn(pCache)`: checks the cache out.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted cache.
    pub fn new<T>(caches: &mut CacheStore<T>, cache: Option<CacheId>) -> Result<Self> {
        caches.check_out_cache(cache)?;
        Ok(Self { cache })
    }

    /// The guarded cache.
    pub const fn cache(&self) -> Option<CacheId> {
        self.cache
    }

    /// `~SmCacheCheckOutIn`: checks the cache back in (deleting it if its deletion
    /// was asked for and this was its last use).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted cache.
    pub fn release<T>(self, caches: &mut CacheStore<T>) -> Result<()> {
        caches.check_in_cache(self.cache)
    }
}

// ---- NLI_Math.h -------------------------------------------------------------------

/// `NI_CPtToPtEuclid_specialized`: the Euclidean point of `pw`, with the rational and
/// planar cases chosen by the flags instead of by `pw`'s `w` / `z` (`now`: not
/// rational, use `x, y, z` as they are; `noz`: planar, `z = 0`).
#[must_use]
pub fn cpt_to_pt_euclid_specialized(noz: bool, now: bool, pw: CPoint) -> Vec3 {
    if now {
        let z = if noz { 0.0 } else { pw.z };
        Vec3::new(pw.x, pw.y, z)
    } else {
        let reciprocal_w = 1.0 / pw.w;
        let x = pw.x * reciprocal_w;
        let y = pw.y * reciprocal_w;
        let z = if noz { 0.0 } else { pw.z * reciprocal_w };
        Vec3::new(x, y, z)
    }
}

/// `NI_VectorBlendCPt_specialized`: `bw + alpha * aw`, with `z` / `w` set to
/// [`NOZ`] / [`NOW`] when the flags say the point has none.
#[must_use]
pub fn vector_blend_cpt_specialized(
    noz: bool,
    now: bool,
    alpha: f64,
    aw: CPoint,
    bw: CPoint,
) -> CPoint {
    let x = bw.x + alpha * aw.x;
    let y = bw.y + alpha * aw.y;
    let z = if noz { NOZ } else { bw.z + alpha * aw.z };
    let w = if now { NOW } else { bw.w + alpha * aw.w };
    CPoint::new(x, y, z, w)
}

/// `NI_ReverseKnots`: the knots `u[0..=m]` reversed, `v[i] = u[0] + u[m] - u[m - i]`,
/// with the end values swapped exactly (a knot equal to `u[m]` becomes `u[0]` and one
/// equal to `u[0]` becomes `u[m]`), so a clamped end block is copied bit for bit and only
/// the other knots go through the subtraction. `v` may hold the same values as `u` (the
/// in-place form): each pair is read before it is written.
///
/// # Errors
///
/// `InvalidInput` when `u` or `v` has fewer than `m + 1` knots.
pub fn reverse_knots(u: &[f64], m: usize, v: &mut [f64]) -> Result<()> {
    const OP: &str = "geom::a_object::reverse_knots";
    let short = || SolidError::invalid_input(OP, "fewer than m + 1 knots");
    if u.len() <= m || v.len() <= m {
        return Err(short());
    }
    let u0 = *u.first().ok_or_else(short)?;
    let um = *u.get(m).ok_or_else(short)?;
    let c = u0 + um;
    let rev = |x: f64| {
        if x == um {
            u0
        } else if x == u0 {
            um
        } else {
            c - x
        }
    };
    for i in 0..=m / 2 {
        let a = *u.get(i).ok_or_else(short)?;
        let b = *u.get(m - i).ok_or_else(short)?;
        *v.get_mut(i).ok_or_else(short)? = rev(b);
        *v.get_mut(m - i).ok_or_else(short)? = rev(a);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SM_ERR;
    use crate::geom::attribute::Behavior;

    fn region(k: u64) -> ObjectRef {
        ObjectRef {
            type_code: REGION_TYPE,
            key: ObjectKey(k),
        }
    }

    fn plain(k: u64) -> ObjectRef {
        ObjectRef {
            type_code: TYPE,
            key: ObjectKey(k),
        }
    }

    fn long_on(
        store: &mut AttributeStore,
        obj: u64,
        id: u64,
        v: i64,
        b: Behavior,
    ) -> AttributeHandle {
        let h = store.insert(Attribute::long(id, v, b)).unwrap();
        store.add_attribute(ObjectKey(obj), h).unwrap();
        h
    }

    fn ids_of(store: &AttributeStore, obj: u64) -> Vec<u64> {
        store
            .attributes(ObjectKey(obj))
            .into_iter()
            .map(|h| store.get(h).unwrap().attribute_id())
            .collect()
    }

    #[test]
    fn type_chain() {
        assert!(is_kind_of(TYPE) && is_kind_of(object::OBJECT_TYPE) && !is_kind_of(SAG_TYPE));
        assert!(
            sag_is_kind_of(SAG_TYPE) && sag_is_kind_of(TYPE) && sag_is_kind_of(object::OBJECT_TYPE)
        );
        assert!(!sag_is_kind_of(16000));
        assert!(assert_valid(false) && assert_valid(true));
    }

    #[test]
    fn split_copies_to_children() {
        let mut s = AttributeStore::new();
        long_on(&mut s, 0, 5, 7, Behavior::Copy);
        notify(
            &mut s,
            None,
            NotifyOperation::Split,
            plain(0),
            Some(plain(1)),
            Some(plain(2)),
            None,
        )
        .unwrap();
        assert_eq!(ids_of(&s, 1), vec![5]);
        assert_eq!(ids_of(&s, 2), vec![5]);
        // a missing child is upstream's null dereference
        let e = notify(
            &mut s,
            None,
            NotifyOperation::Split,
            plain(0),
            Some(plain(1)),
            None,
            None,
        );
        assert!(matches!(e, Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn merge_skips_ids_of_first_source() {
        let mut s = AttributeStore::new();
        long_on(&mut s, 0, 5, 1, Behavior::Copy);
        long_on(&mut s, 1, 5, 2, Behavior::Copy);
        long_on(&mut s, 1, 6, 3, Behavior::Copy);
        notify(
            &mut s,
            None,
            NotifyOperation::Merge,
            plain(2),
            Some(plain(0)),
            Some(plain(1)),
            None,
        )
        .unwrap();
        assert_eq!(ids_of(&s, 2), vec![5, 6]);
        let first = s.attributes(ObjectKey(2))[0];
        assert_eq!(s.get(first).unwrap().long_value(), Some(1));
    }

    #[test]
    fn construction_and_change_geometry_sources() {
        let mut s = AttributeStore::new();
        long_on(&mut s, 1, 5, 1, Behavior::Copy);
        // no copy-from object: nothing to copy
        notify(
            &mut s,
            None,
            NotifyOperation::Construction,
            plain(0),
            Some(plain(0)),
            None,
            None,
        )
        .unwrap();
        assert!(ids_of(&s, 0).is_empty());
        notify(
            &mut s,
            None,
            NotifyOperation::Construction,
            plain(0),
            Some(plain(0)),
            Some(plain(1)),
            None,
        )
        .unwrap();
        assert_eq!(ids_of(&s, 0), vec![5]);
        // without new geometry the old geometry's attributes are not read
        notify(
            &mut s,
            None,
            NotifyOperation::ChangeGeometry,
            plain(3),
            None,
            None,
            Some(plain(1)),
        )
        .unwrap();
        notify(
            &mut s,
            None,
            NotifyOperation::ChangeGeometry,
            plain(3),
            Some(plain(4)),
            None,
            Some(plain(1)),
        )
        .unwrap();
        assert_eq!(ids_of(&s, 4), vec![5]);
    }

    #[test]
    fn region_propagation() {
        let mut s = AttributeStore::new();
        // not a region: nothing happens
        notify(
            &mut s,
            None,
            NotifyOperation::RegPropagation,
            plain(0),
            None,
            None,
            None,
        )
        .unwrap();
        // a region without merge-region attribute
        let e = notify(
            &mut s,
            None,
            NotifyOperation::RegPropagation,
            region(0),
            None,
            None,
            None,
        );
        assert!(matches!(e, Err(SolidError::InvalidInput { .. })));
        let mr = s
            .insert(Attribute::merge_region(ObjectKey(0), 1, ObjectKey(1), 1))
            .unwrap();
        s.add_attribute(ObjectKey(0), mr).unwrap();
        // nothing propagated through Booleans: no region query
        long_on(&mut s, 1, 5, 1, Behavior::Copy);
        notify(
            &mut s,
            None,
            NotifyOperation::RegPropagation,
            region(0),
            None,
            None,
            None,
        )
        .unwrap();
        // a colour on the other region needs SmRegion::IsVoid
        let c = s
            .insert(Attribute::color(
                ids::COLOR,
                Vec3::new(1.0, 0.0, 0.0),
                Behavior::Copy,
            ))
            .unwrap();
        s.add_attribute(ObjectKey(1), c).unwrap();
        let e = notify(
            &mut s,
            None,
            NotifyOperation::RegPropagation,
            region(0),
            None,
            None,
            None,
        );
        assert!(matches!(
            e,
            Err(SolidError::Unported {
                unit: "topology.shell",
                ..
            })
        ));
    }

    #[test]
    fn copy_constructor_copies_attributes() {
        let mut s = AttributeStore::new();
        long_on(&mut s, 0, 5, 1, Behavior::Copy);
        let r = s.insert(Attribute::new(6, Behavior::Reference)).unwrap();
        s.add_attribute(ObjectKey(0), r).unwrap();
        let t = s.insert(Attribute::new(7, Behavior::Temp)).unwrap();
        s.add_attribute(ObjectKey(0), t).unwrap();
        copy_construct(&mut s, ObjectKey(0), ObjectKey(1)).unwrap();
        assert_eq!(ids_of(&s, 1), vec![5, 6]);
        assert_eq!(s.attributes(ObjectKey(1))[1], r);
    }

    #[test]
    fn cache_use_counts_delay_deletion() {
        let mut c: CacheStore<u8> = CacheStore::new();
        let id = c.insert(1).unwrap();
        let g = CacheCheckOutIn::new(&mut c, Some(id)).unwrap();
        let mut o = AObject::new();
        o.set_cache_obj(Some(id));
        delete_objects_cache(&mut c, &mut o).unwrap();
        assert_eq!(o.cache_obj(), None);
        assert!(c.get(id).unwrap().is_deleted());
        g.release(&mut c).unwrap();
        assert!(!c.is_live(id));
        assert!(c.check_out_cache(Some(id)).is_err());
        c.check_out_cache(None).unwrap();
        // check-in without check-out wraps the count
        let id2 = c.insert(2).unwrap();
        c.check_in_cache(Some(id2)).unwrap();
        assert_eq!(c.get(id2).unwrap().use_count(), u64::MAX);
        assert!(!c.soft_delete(id2).unwrap());
    }

    #[test]
    fn object_caches() {
        let mut c: CacheStore<()> = CacheStore::new();
        let id = c.insert(()).unwrap();
        let mut o = AObject::new();
        let ctx = Context::default();
        assert_eq!(get_object_cache(ObjectCacheType::Brep, None), None);
        o.set_cache_obj(Some(id));
        assert_eq!(get_object_cache(ObjectCacheType::Curve, Some(&o)), Some(id));
        let get = |m, t, o: &AObject, ctx: Option<&Context>| {
            get_or_create_object_cache(ObjectCacheType::Brep, ctx, m, t, o)
        };
        assert_eq!(
            get(CacheMaker::AObject, REGION_TYPE, &o, Some(&ctx)).unwrap(),
            None
        );
        assert_eq!(
            get(CacheMaker::SagObject, BREP_TYPE, &o, Some(&ctx)).unwrap(),
            Some(id)
        );
        assert_eq!(
            get(CacheMaker::SagObject, BREP_TYPE, &o, None).unwrap(),
            None
        );
        assert_eq!(
            get(CacheMaker::SagObject, SAG_TYPE, &o, Some(&ctx)).unwrap(),
            None
        );
        assert!(matches!(
            sag_cache_make_or_validate(ObjectCacheType::Brep, REGION_TYPE, Some(id)),
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
        assert!(matches!(
            get(
                CacheMaker::SagObject,
                POLY_BREP_TYPE,
                &AObject::new(),
                Some(&ctx)
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        // a curve cache of the wrong type: SM_ERR, so no cache
        assert_eq!(get(CacheMaker::Curve, 0, &o, Some(&ctx)).unwrap(), None);
        // a curve cache reuses the old one; a new one needs the curve
        assert_eq!(
            get_or_create_object_cache(
                ObjectCacheType::Curve,
                Some(&ctx),
                CacheMaker::Curve,
                0,
                &o
            )
            .unwrap(),
            Some(id)
        );
        assert!(matches!(
            get_or_create_object_cache(
                ObjectCacheType::Curve,
                Some(&ctx),
                CacheMaker::Curve,
                0,
                &AObject::new()
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut s = AttributeStore::new();
        long_on(&mut s, 3, 5, 1, Behavior::Copy);
        o.destroy(ObjectKey(3), &mut s, &mut c).unwrap();
        assert!(!c.is_live(id));
        assert!(s.attributes(ObjectKey(3)).is_empty());
    }

    #[test]
    fn nli_helpers() {
        let p = cpt_to_pt_euclid_specialized(false, false, CPoint::new(2.0, 4.0, 6.0, 2.0));
        assert_eq!(p, Vec3::new(1.0, 2.0, 3.0));
        let p = cpt_to_pt_euclid_specialized(true, true, CPoint::new(2.0, 4.0, 6.0, 2.0));
        assert_eq!(p, Vec3::new(2.0, 4.0, 0.0));
        let b = vector_blend_cpt_specialized(
            false,
            true,
            2.0,
            CPoint::new(1.0, 1.0, 1.0, 1.0),
            CPoint::new(0.0, 1.0, 2.0, 3.0),
        );
        assert_eq!(b, CPoint::new(2.0, 3.0, 4.0, NOW));
    }
}
