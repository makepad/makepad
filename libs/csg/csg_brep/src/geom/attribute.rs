//! Attributes attached to kernel objects: typed payloads (long, vector, tag,
//! generic arrays, merge-region bookkeeping, ...), their copy/reference behaviour,
//! the user lists linking attributes to the objects that carry them, and the
//! default propagation handlers (split, copy, merge, coincident) that edit
//! operations call through `Notify`.
//!
//! Upstream: `source/SMLib/src/SmAttribute.cpp`, `source/SMLib/inc/SmAttribute.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`Attribute`]                                     | `SmAttribute` and its derived classes            |
//! | [`AttributeValue`]                                | the derived class (`SmLongAttribute`, ...) and its data |
//! | [`Behavior`]                                      | `SmAttributeBehaviorType`                        |
//! | [`ids`]                                           | the `SM_AI_*` attribute ids                      |
//! | [`tag_ids`]                                       | the `SM_TI_*` tag id types                       |
//! | [`type_code`]                                     | the `Sm*Attribute_TYPE` / `SmObject_TYPE` values |
//! | [`region_type`]                                   | the region type literals of `SmMergeRegionAttribute` |
//! | [`Attribute::new`]                                | `SmAttribute(ULONG, SmAttributeBehaviorType)`    |
//! | [`Attribute::generic`]                            | `SmGenericAttribute(...)`                        |
//! | [`Attribute::long`]                               | `SmLongAttribute(...)`                           |
//! | [`Attribute::pointer`]                            | `SmPointerAttribute(ULONG, void*, ...)`          |
//! | [`Attribute::pointer_list`]                       | `SmPointerListAttribute(...)`                    |
//! | [`Attribute::vector3d`]                           | `SmVector3dAttribute(...)`                       |
//! | [`Attribute::color`]                              | `SmColorAttribute(ULONG, const SmVector3d&, ...)` |
//! | [`Attribute::tag`]                                | `SmTagAttribute(ULONG, long x8, ...)`            |
//! | [`Attribute::merge_region`]                       | `SmMergeRegionAttribute(SmRegion*, ULONG, SmRegion*, ULONG)` |
//! | [`Attribute::make_copy`]                          | every `MakeCopy` override and copy constructor   |
//! | [`Attribute::attribute_id`] / [`Attribute::behavior`] | `GetAttributeID` / `GetBehavior`             |
//! | [`Attribute::type_code`] / [`Attribute::type_string`] / [`Attribute::is_kind_of`] | `GetType` / `GetTypeString` / `IsKindOf` (every class) |
//! | [`Attribute::is_propagated_through_booleans`]     | `IsPropagatedThroughBooleans` (base, `SmColorAttribute`) |
//! | [`Attribute::long_value`]                         | `SmLongAttribute::GetValue`                      |
//! | [`Attribute::vector3d_value`] / [`Attribute::set_vector3d_value`] | `SmVector3dAttribute::GetValue` / `SetValue` |
//! | [`MergeRegionData`]                               | `SmMergeRegionAttribute` data and simple accessors (`GetThisRegions`, `GetThisRegionTypes`, `GetThisRegion`, `GetOtherRegions`, `GetOperation`, `SetOperation`) |
//! | [`MergeRegionData::has_this_infinite_region`] / [`MergeRegionData::has_other_infinite_region`] | `HasThisInfiniteRegion` / `HasOtherInfiniteRegion` |
//! | [`MergeRegionData::has_any_solid`]                | `SmMergeRegionAttribute::HasAnySolid`            |
//! | [`MergeRegionData::set_regions`]                  | `SmMergeRegionAttribute::SetRegions`             |
//! | `append_new_regions` (private)                    | the two accumulation loops of `SmMergeRegionAttribute::Merge` |
//! | [`AttributeStore`]                                | the heap of attributes plus every `SmAObject::m_pAttributes` list |
//! | [`AttributeStore::destroy`]                       | `SmAttribute::~SmAttribute` (`delete pAttr`)     |
//! | [`AttributeStore::add_user`] / [`AttributeStore::remove_user`] / [`AttributeStore::users`] | `AddUser` / `RemoveUser` / `GetUsers` |
//! | [`AttributeStore::from_clear_attributes`]         | `SmAttribute::fromClearAttributes`               |
//! | [`AttributeStore::split`]                         | `SmAttribute::Split`                             |
//! | [`AttributeStore::copy`]                          | `SmAttribute::Copy`                              |
//! | [`AttributeStore::merge`]                         | `Merge` (base, `SmMergeRegionAttribute`)         |
//! | [`AttributeStore::coincident`]                    | `SmAttribute::Coincident`                        |
//! | [`AttributeStore::edit`] / [`AttributeStore::destruction`] | `SmAttribute::Edit` / `Destruction` (no-ops) |
//! | [`AttributeStore::propagate_regions_through_booleans`] | `PropagateRegionsThroughBooleans` (base, `SmColorAttribute`) |
//! | [`AttributeStore::has_any_other_propagated_attribs`] | `SmMergeRegionAttribute::HasAnyOtherPropagatedAttribs` |
//! | [`AttributeStore::read_indexed_attributes_from_db`] | `SmAttributeData::ReadIndexedAttributesFromDB` |
//! | [`AttributeStore::read_indexed_attributes_from_db_partial`] | the same, with the out-param a failed line skip leaves |
//! | [`AttributeStore::add_attribute`]                 | `SmAObject::AddAttribute` (co-ported)            |
//! | [`AttributeStore::remove_attribute`]              | `SmAObject::RemoveAttribute` (co-ported)         |
//! | [`AttributeStore::clear_attributes`]              | `SmAObject::ClearAttributes` (co-ported)         |
//! | [`AttributeStore::attributes`]                    | `SmAObject::GetAttributes` (co-ported)           |
//! | [`AttributeStore::find_attribute`]                | `SmAObject::FindAttribute` (co-ported)           |
//!
//! Deviations:
//! - **Ownership.** Upstream attributes are heap objects linked to `SmAObject`s by
//!   raw pointers in both directions. Here an [`AttributeStore`] owns every
//!   attribute (addressed by [`AttributeHandle`]) and every object's attribute list
//!   (keyed by an opaque [`ObjectKey`], which the owners of the attributed objects map
//!   their ids to). `delete this` frees the store slot; slots are never reused, so a
//!   stale handle reports "not live" instead of aliasing a new attribute.
//! - **Back-edges co-ported.** `SmAObject::AddAttribute`, `RemoveAttribute`,
//!   `ClearAttributes`, `GetAttributes` and `FindAttribute` (unit `geom.a_object`)
//!   are the attribute-list half of the attribute/user relation and are ported here
//!   as [`AttributeStore`] methods (`// CO-PORT(geom.a_object)`); `SmObject::IsKindOf`
//!   ([`crate::geom::object::is_kind_of`]) is the last entry of [`Attribute::is_kind_of`]'s chain.
//!   `SmObject::GetContext` / `SetContext` have no counterpart (no per-object
//!   contexts).
//! - **Region lookup.** [`AttributeStore::merge_with_regions`] supplies the actual
//!   region/owner context needed by the native merge-region handler. Detached
//!   regions retain NULL owner identity; raw void flags use native nonzero truth.
//!   The legacy merge entry point reports an explicit missing-context error for
//!   that handler instead of inventing topology from an opaque object key.
//! - **Signals and asserts.** `SE(SM_ERR)`, `SM_ASSERT*`, `SM_DBG_WARN` and `ERR_MSG`
//!   are compiled out of the release reference build and are dropped. Where upstream
//!   would then dereference `NULL`, read a dangling pointer, write past an array or
//!   read a mistyped attribute (an attribute found by id whose class is not the one
//!   the code casts to), Rust returns [`SolidError::InvalidInput`] instead.
//! - **Destructor on a dependent attribute with users.** `~SmAttribute` removes the
//!   attribute from each user's list through `RemoveAttribute`, whose `RemoveUser`
//!   deletes the attribute a second time when the last user goes (undefined
//!   behaviour) unless it is standalone. [`AttributeStore::destroy`] frees it once.
//! - **`ReadIndexedAttributesFromDB`.** Returns the index array instead of filling an
//!   out-param; on error the partially read index array is dropped (upstream leaves
//!   it in the out-param). Callers that ignore the status, as `SmBrepData`'s readers
//!   do, use [`AttributeStore::read_indexed_attributes_from_db_partial`], which keeps
//!   the indices read before a failed line skip. An ASCII count whose extraction
//!   stores nothing (stream already failed or at its end: upstream reads an
//!   uninitialised variable), a count or index above [`MAX_INDEXED_ATTRIBUTES`]
//!   (upstream allocates the size asked for, or wraps for `ULONG_MAX`), and a reader
//!   without a stream are [`SolidError::InvalidInput`], before anything is created.
//!   For safety, ASCII counts must fit the remaining input and indices grow only
//!   after successful extraction; truncated or malformed index lists are rejected
//!   with [`SolidError::InvalidInput`] instead of leaving invented zero entries.
//! - `Mark` (`m_lMark`), `Dump`, `AssertValid`, `GetMemoryUsed` and the persistence
//!   getters (`GetNumLongElements`, ...) belong to other units or are unused; the
//!   attribute's data is exposed through [`Attribute::value`].

use std::collections::BTreeMap;

use crate::error::{Result, SolidError};
use crate::geom::object;
use crate::math::Vec3;
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

/// The `SM_AI_*` attribute ids (`m_lAttributeID` values) defined by SMLib.
pub mod ids {
    /// `SM_AI_COLOR`: colour (read back from files as a vector attribute).
    pub const COLOR: u64 = 100;
    /// `SM_AI_MARK`.
    pub const MARK: u64 = 101;
    /// `SM_AI_OFFSETMAP`.
    pub const OFFSETMAP: u64 = 102;
    /// `SM_AI_GENERIC`.
    pub const GENERIC: u64 = 103;
    /// `SM_AI_UNKNOWN`: placeholder for attributes being read from a database.
    pub const UNKNOWN: u64 = 104;
    /// `SM_AI_SURFACEMAP`.
    pub const SURFACEMAP: u64 = 105;
    /// `SM_AI_TAG`.
    pub const TAG: u64 = 106;
    /// `SM_AI_FILLET_EDGEMAP`.
    pub const FILLET_EDGEMAP: u64 = 107;
    /// `SM_AI_FILLET_VERTMAP`.
    pub const FILLET_VERTMAP: u64 = 108;
    /// `SM_AI_FILLET_REGIONMAP`.
    pub const FILLET_REGIONMAP: u64 = 109;
    /// `SM_AI_FILLET_EDGES`.
    pub const FILLET_EDGES: u64 = 110;
    /// `SM_AI_ALL_FILLET_EDGES`.
    pub const ALL_FILLET_EDGES: u64 = 111;
    /// `SM_AI_SSSPOINT`.
    pub const SSSPOINT: u64 = 112;
    /// `SM_AI_MICRO_COLOR`.
    pub const MICRO_COLOR: u64 = 113;
    /// `SM_AI_OTHER_REGION_ID`: region propagation through Boolean operations.
    pub const OTHER_REGION_ID: u64 = 114;
    /// `SM_AI_FACE_ID` (testing).
    pub const FACE_ID: u64 = 116;
    /// `SM_AI_EDGE_ID` (testing).
    pub const EDGE_ID: u64 = 118;
    /// `SM_AI_VERTEX_ID` (testing).
    pub const VERTEX_ID: u64 = 120;
    /// `SM_AI_REUSE_UVCURVES_ID`.
    pub const REUSE_UVCURVES_ID: u64 = 122;
    /// `SM_AI_TESSELLATING`.
    pub const TESSELLATING: u64 = 124;
    /// `SM_AI_REPLACE_COMPOSITES`.
    pub const REPLACE_COMPOSITES: u64 = 126;
    /// `SM_AI_FILLET_CORNER_FACE` (the fork's, source fork `60af574`): a face of
    /// a set-back fillet corner's patch (`SmFilletExecutive::SetMarkCornerPatches`).
    pub const FILLET_CORNER_FACE: u64 = 132;
    /// `SM_AI_METRIC_BASE`: first of the ids used by metric attributes (150-199).
    pub const METRIC_BASE: u64 = 150;
    /// `SM_AI_MASS_PROPERTIES`.
    pub const MASS_PROPERTIES: u64 = 201;
    /// `SM_AI_POLY_ERRORBOUNDS`.
    pub const POLY_ERRORBOUNDS: u64 = 301;
    /// `SM_AI_VERTEX_DECIMATION_VALUES`.
    pub const VERTEX_DECIMATION_VALUES: u64 = 302;
    /// `SM_AI_QUADRIC_VERTEX_VALUES`.
    pub const QUADRIC_VERTEX_VALUES: u64 = 303;
    /// `SM_AI_QUADRIC_EDGE_VALUES`.
    pub const QUADRIC_EDGE_VALUES: u64 = 304;
    /// `SM_AI_QUADRIC_VERTEX_N_VALUES`.
    pub const QUADRIC_VERTEX_N_VALUES: u64 = 305;
    /// `SM_AI_BOOLEAN_INTERSECT`.
    pub const BOOLEAN_INTERSECT: u64 = 307;
    /// `SM_AI_BOOLEAN_SAVE`.
    pub const BOOLEAN_SAVE: u64 = 308;
    /// `SM_AI_BOOLEAN_DELETE`.
    pub const BOOLEAN_DELETE: u64 = 309;
    /// `SM_AI_POLY_TO_EUVU`.
    pub const POLY_TO_EUVU: u64 = 310;
    /// `SM_AI_MESH_ATTRIBUTE`.
    pub const MESH_ATTRIBUTE: u64 = 311;
    /// `SM_AI_TESSELATION_POLY_EDGE_VERTEX_USE`.
    pub const TESSELATION_POLY_EDGE_VERTEX_USE: u64 = 312;
    /// `SM_AI_USER_1`: first application id.
    pub const USER_1: u64 = 10000;
    /// `SM_AI_USER_2`.
    pub const USER_2: u64 = 20000;
    /// `SM_AI_USER_3`.
    pub const USER_3: u64 = 30000;
}

/// The `SM_TI_*` tag id types used in [`TagData`].
pub mod tag_ids {
    /// `SM_TI_UNDEFINED`: id is not defined.
    pub const UNDEFINED: i64 = 0;
    /// `SM_TI_GENERATOR_CURVE`: id is from a generator curve.
    pub const GENERATOR_CURVE: i64 = 1;
    /// `SM_TI_SWEEP_STEP_DELTA`: id is some sort of copy of a generator curve.
    pub const SWEEP_STEP_DELTA: i64 = 100000;
    /// `SM_TI_ORDINAL`: id is an ordinal number.
    pub const ORDINAL: i64 = 2;
    /// `SM_TI_START_OF_SWEEP`.
    pub const START_OF_SWEEP: i64 = -100000;
    /// `SM_TI_END_OF_SWEEP`.
    pub const END_OF_SWEEP: i64 = -200000;
    /// `SM_TI_COMBO_OPERATION`: id from a combination of two entities.
    pub const COMBO_OPERATION: i64 = 100000;
    /// `SM_TI_INTERSECTION_EDGES`: id of newly created intersection edges.
    pub const INTERSECTION_EDGES: i64 = 3;
}

/// Upstream `SM_TYPE` values reported by [`Attribute::type_code`] and accepted by
/// [`Attribute::is_kind_of`].
pub mod type_code {
    /// `SmObject_TYPE` (`OS_BASE_TYPE + 1`).
    pub const OBJECT: i64 = 11001;
    /// `SmAttribute_TYPE` (`CONT_BASE_TYPE + 70`).
    pub const ATTRIBUTE: i64 = 12070;
    /// `SmLongAttribute_TYPE`.
    pub const LONG: i64 = 12071;
    /// `SmVector3dAttribute_TYPE`.
    pub const VECTOR3D: i64 = 12072;
    /// `SmGenericAttribute_TYPE`.
    pub const GENERIC: i64 = 12073;
    /// `SmPointerAttribute_TYPE`.
    pub const POINTER: i64 = 12074;
    /// `SmPointerListAttribute_TYPE`.
    pub const POINTER_LIST: i64 = 12062;
    /// `SmMergeRegionAttribute_TYPE`.
    pub const MERGE_REGION: i64 = 12064;
    /// `SmTagAttribute_TYPE`.
    pub const TAG: i64 = 12076;
    /// `SmColorAttribute_TYPE`.
    pub const COLOR: i64 = 12066;
}

/// Source region types stored in [`MergeRegionData`] (upstream literals in
/// `SmMergeRegionAttribute::Merge`).
pub mod region_type {
    /// Internal solid region.
    pub const SOLID: i64 = 0;
    /// Internal void region.
    pub const VOID: i64 = 1;
    /// The infinite region.
    pub const INFINITE: i64 = 2;
}

/// `SM_UNDEF_ULONG`: the id a destroyed attribute is left with.
pub const UNDEF_ULONG: u64 = 123_456_789;

/// `SM_ERR`.
const SM_ERR: i32 = 1001;

/// Largest attribute count or index [`AttributeStore::read_indexed_attributes_from_db`]
/// accepts (a guard against corrupt input; upstream allocates whatever it reads).
pub const MAX_INDEXED_ATTRIBUTES: u64 = 1 << 20;

/// How an attribute propagates and when it is deleted (upstream
/// `SmAttributeBehaviorType`); the discriminant is the upstream value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Behavior {
    /// `SM_AB_COPY`: one user per attribute, copied on propagation, deleted when its
    /// last user goes (the default).
    Copy = 0,
    /// `SM_AB_REFERENCE`: shared by many users, deleted when its last user goes.
    Reference = 1,
    /// `SM_AB_STANDALONE_COPY`: like `Copy`, but never deleted automatically.
    StandaloneCopy = 2,
    /// `SM_AB_STANDALONE_REFERENCE`: like `Reference`, but never deleted
    /// automatically.
    StandaloneReference = 3,
    /// `SM_AB_TEMP`: not copied, not persistent.
    Temp = 4,
}

impl Behavior {
    /// The upstream `SmAttributeBehaviorType` value.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The behaviour for an upstream value.
    pub fn from_code(code: i32) -> Result<Self> {
        match code {
            0 => Ok(Self::Copy),
            1 => Ok(Self::Reference),
            2 => Ok(Self::StandaloneCopy),
            3 => Ok(Self::StandaloneReference),
            4 => Ok(Self::Temp),
            _ => Err(SolidError::InvalidInput {
                operation: "geom::attribute::Behavior::from_code",
                reason: "not an SmAttributeBehaviorType value",
            }),
        }
    }

    /// `SM_AB_COPY` or `SM_AB_STANDALONE_COPY`: propagation places a deep copy.
    #[must_use]
    pub const fn is_copy(self) -> bool {
        matches!(self, Self::Copy | Self::StandaloneCopy)
    }

    /// `SM_AB_REFERENCE` or `SM_AB_STANDALONE_REFERENCE`: propagation shares the
    /// attribute.
    #[must_use]
    pub const fn is_reference(self) -> bool {
        matches!(self, Self::Reference | Self::StandaloneReference)
    }

    /// `SM_AB_STANDALONE_COPY` or `SM_AB_STANDALONE_REFERENCE`: never deleted when
    /// the user list empties.
    #[must_use]
    pub const fn is_standalone(self) -> bool {
        matches!(self, Self::StandaloneCopy | Self::StandaloneReference)
    }
}

/// Identity of an attributed object (an upstream `SmAObject*`: topology, geometry,
/// B-rep, ...). Opaque to this module: the owners of those objects choose the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey(pub u64);

/// Handle of an attribute in an [`AttributeStore`] (an upstream `SmAttribute*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttributeHandle(u32);

impl AttributeHandle {
    /// Slot index in the store (creation order).
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// `SmTagAttribute` data: ids identifying a piece of geometry or topology across a
/// re-execution of a parametric script. Zero means "undefined"; ordinals start at 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagData {
    /// `m_lPrimaryID`: id of the object created during generation.
    pub primary_id: i64,
    /// `m_lPrimaryOrdinal`.
    pub primary_ordinal: i64,
    /// `m_lSecondaryIDType`: one of [`tag_ids`].
    pub secondary_id_type: i64,
    /// `m_lSecondaryID`.
    pub secondary_id: i64,
    /// `m_lSecondaryOrdinal`.
    pub secondary_ordinal: i64,
    /// `m_lThirdIDType`: one of [`tag_ids`].
    pub third_id_type: i64,
    /// `m_lThirdID`.
    pub third_id: i64,
    /// `m_lThirdOrdinal`.
    pub third_ordinal: i64,
}

impl TagData {
    /// The eight values in upstream member order (`GetLongElementsAddress`).
    #[must_use]
    pub const fn to_array(&self) -> [i64; 8] {
        [
            self.primary_id,
            self.primary_ordinal,
            self.secondary_id_type,
            self.secondary_id,
            self.secondary_ordinal,
            self.third_id_type,
            self.third_id,
            self.third_ordinal,
        ]
    }

    /// Builds the data from the eight values in upstream member order.
    #[must_use]
    pub const fn from_array(a: [i64; 8]) -> Self {
        Self {
            primary_id: a[0],
            primary_ordinal: a[1],
            secondary_id_type: a[2],
            secondary_id: a[3],
            secondary_ordinal: a[4],
            third_id_type: a[5],
            third_id: a[6],
            third_ordinal: a[7],
        }
    }
}

/// `SmMergeRegionAttribute` data: the input regions of both Boolean operands that
/// share points with the output region carrying the attribute.
///
/// Region types are the [`region_type`] values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRegionData {
    /// `m_eOperation`: the `SmBooleanOperationType` value (`0` = `SM_BO_UNKNOWN`).
    pub operation: i32,
    /// `m_vThisSourceRegions` (used as ids).
    pub this_regions: Vec<ObjectKey>,
    /// `m_vThisSourceType`, parallel to `this_regions`.
    pub this_types: Vec<i64>,
    /// `m_vOtherSourceRegions` (used as ids).
    pub other_regions: Vec<ObjectKey>,
    /// `m_vOtherSourceType`, parallel to `other_regions`.
    pub other_types: Vec<i64>,
}

impl MergeRegionData {
    /// `GetThisRegions`.
    #[must_use]
    pub fn this_regions(&self) -> &[ObjectKey] {
        &self.this_regions
    }

    /// `GetThisRegionTypes`.
    #[must_use]
    pub fn this_region_types(&self) -> &[i64] {
        &self.this_types
    }

    /// `GetThisRegion(i)`; `None` out of range (upstream asserts and reads past the
    /// array).
    #[must_use]
    pub fn this_region(&self, i: usize) -> Option<ObjectKey> {
        self.this_regions.get(i).copied()
    }

    /// `GetOtherRegions`.
    #[must_use]
    pub fn other_regions(&self) -> &[ObjectKey] {
        &self.other_regions
    }

    /// `GetOperation`.
    #[must_use]
    pub const fn operation(&self) -> i32 {
        self.operation
    }

    /// `SetOperation`.
    pub fn set_operation(&mut self, operation: i32) {
        self.operation = operation;
    }

    /// `HasThisInfiniteRegion`: some "this" source region is the infinite region.
    #[must_use]
    pub fn has_this_infinite_region(&self) -> bool {
        self.this_types.contains(&region_type::INFINITE)
    }

    /// `HasOtherInfiniteRegion`: some "other" source region is the infinite region.
    #[must_use]
    pub fn has_other_infinite_region(&self) -> bool {
        self.other_types.contains(&region_type::INFINITE)
    }

    /// `HasAnySolid(flag)`: some source region in the requested set (`flag & 1`:
    /// "this" sources, `flag & 2`: "other" sources) is [`region_type::SOLID`].
    #[must_use]
    pub fn has_any_solid(&self, this_other_flag: u64) -> bool {
        if this_other_flag & 1 != 0 && self.this_types.contains(&region_type::SOLID) {
            return true;
        }
        if this_other_flag & 2 != 0 && self.other_types.contains(&region_type::SOLID) {
            return true;
        }
        false
    }

    /// `SetRegions`: overwrites entry `index` of all four lists.
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) when `index` is not below the number of
    /// "this" regions. [`SolidError::InvalidInput`] (upstream writes past the array)
    /// when it is, but one of the other lists is shorter.
    pub fn set_regions(
        &mut self,
        index: usize,
        this_region: ObjectKey,
        this_type: i64,
        other_region: ObjectKey,
        other_type: i64,
    ) -> Result<()> {
        const OP: &str = "geom::attribute::MergeRegionData::set_regions";
        if index >= self.this_regions.len() {
            return Err(SolidError::Upstream {
                operation: OP,
                code: SM_ERR,
                entity: None,
            });
        }
        let (Some(tr), Some(tt), Some(or), Some(ot)) = (
            self.this_regions.get_mut(index),
            self.this_types.get_mut(index),
            self.other_regions.get_mut(index),
            self.other_types.get_mut(index),
        ) else {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "index past the end of a parallel region list",
            });
        };
        *tr = this_region;
        *tt = this_type;
        *or = other_region;
        *ot = other_type;
        Ok(())
    }
}

/// The derived attribute class and its data.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    /// `SmAttribute`: no data (a label).
    None,
    /// `SmGenericAttribute`: arrays of longs, doubles and characters (what
    /// persistence reads unknown ids into).
    Generic {
        /// `m_vLongElements`.
        longs: Vec<i64>,
        /// `m_vDoubleElements`.
        doubles: Vec<f64>,
        /// `m_vCharacterElements`.
        chars: Vec<u8>,
    },
    /// `SmLongAttribute`.
    Long(i64),
    /// `SmPointerAttribute`: an opaque address-like value, never dereferenced.
    Pointer(u64),
    /// `SmPointerListAttribute`: opaque address-like values, never dereferenced.
    PointerList(Vec<u64>),
    /// `SmVector3dAttribute`.
    Vector3d(Vec3),
    /// `SmMergeRegionAttribute`.
    MergeRegion(MergeRegionData),
    /// `SmTagAttribute`.
    Tag(TagData),
    /// `SmColorAttribute` (a vector attribute with colour-averaging propagation).
    Color(Vec3),
    /// `SmPolyToEUVUAttr` (unit `query.tess`, [`crate::query::tess`]): the edge use
    /// and vertex use of the tessellation B-rep behind a poly edge.
    PolyToEuVu {
        /// `m_pEdgeuse`.
        edgeuse: Option<ObjectKey>,
        /// `m_pVertexuse`.
        vertexuse: Option<ObjectKey>,
    },
    /// `SmPolyEdgeToVertexUseAttr` (unit `query.tess`, [`crate::query::tess`]): the
    /// vertex use of the tessellation B-rep behind a poly edge.
    PolyEdgeToVertexUse {
        /// `m_pVertexuse`.
        vertexuse: Option<ObjectKey>,
    },
    /// `SmDecimateVertexAttr` (unit `query.brep`, [`crate::query::brep`]).
    DecimateVertex(crate::query::brep::DecimateVertexData),
    /// `SmQuadricVertexAttr` (unit `query.brep`, [`crate::query::brep`]).
    QuadricVertex(crate::query::brep::QuadricVertexData),
    /// `SmQuadricEdgeAttr` (unit `query.brep`, [`crate::query::brep`]).
    QuadricEdge(crate::query::brep::QuadricEdgeData),
}

/// An attribute: an id naming its purpose, a [`Behavior`], the list of objects
/// using it and its typed data (upstream `SmAttribute` and derived classes).
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    id: u64,
    behavior: Behavior,
    users: Vec<ObjectKey>,
    value: AttributeValue,
}

impl Attribute {
    /// `SmAttribute(lAttributeID, eBehavior)`: an attribute without data.
    #[must_use]
    pub fn new(attribute_id: u64, behavior: Behavior) -> Self {
        Self::with_value(attribute_id, behavior, AttributeValue::None)
    }

    pub(crate) fn with_value(attribute_id: u64, behavior: Behavior, value: AttributeValue) -> Self {
        Self {
            id: attribute_id,
            behavior,
            users: Vec::new(),
            value,
        }
    }

    /// `SmGenericAttribute(lAttributeID, eBehavior, rLongs, rDoubles, rChars)`.
    #[must_use]
    pub fn generic(
        attribute_id: u64,
        behavior: Behavior,
        longs: &[i64],
        doubles: &[f64],
        chars: &[u8],
    ) -> Self {
        Self::with_value(
            attribute_id,
            behavior,
            AttributeValue::Generic {
                longs: longs.to_vec(),
                doubles: doubles.to_vec(),
                chars: chars.to_vec(),
            },
        )
    }

    /// `SmLongAttribute(lAttributeID, lValue, eBehavior)`.
    #[must_use]
    pub fn long(attribute_id: u64, value: i64, behavior: Behavior) -> Self {
        Self::with_value(attribute_id, behavior, AttributeValue::Long(value))
    }

    /// `SmPointerAttribute(lAttributeID, pPointer, eBehavior)`.
    #[must_use]
    pub fn pointer(attribute_id: u64, pointer: u64, behavior: Behavior) -> Self {
        Self::with_value(attribute_id, behavior, AttributeValue::Pointer(pointer))
    }

    /// `SmPointerListAttribute(lAttributeID, pOptListToCopy, eBehavior)`.
    #[must_use]
    pub fn pointer_list(attribute_id: u64, list: Option<&[u64]>, behavior: Behavior) -> Self {
        Self::with_value(
            attribute_id,
            behavior,
            AttributeValue::PointerList(list.map(<[u64]>::to_vec).unwrap_or_default()),
        )
    }

    /// `SmVector3dAttribute(lAttributeID, rValue, eBehavior)`.
    #[must_use]
    pub fn vector3d(attribute_id: u64, value: Vec3, behavior: Behavior) -> Self {
        Self::with_value(attribute_id, behavior, AttributeValue::Vector3d(value))
    }

    /// `SmColorAttribute(lAttributeID, rValue, eBehavior)`.
    #[must_use]
    pub fn color(attribute_id: u64, value: Vec3, behavior: Behavior) -> Self {
        Self::with_value(attribute_id, behavior, AttributeValue::Color(value))
    }

    /// `SmTagAttribute(lAttributeID, lPrimaryID, ..., lThirdOrdinal, eBehavior)`
    /// (upstream defaults: id [`ids::TAG`], all values `0`, [`Behavior::Copy`]).
    #[must_use]
    pub fn tag(attribute_id: u64, tag: TagData, behavior: Behavior) -> Self {
        Self::with_value(attribute_id, behavior, AttributeValue::Tag(tag))
    }

    /// `SmMergeRegionAttribute(pThisSourceRegion, lThisSourceType,
    /// pOtherSourceRegion, lOtherSourceType)`: id [`ids::OTHER_REGION_ID`],
    /// [`Behavior::Copy`], operation `SM_BO_UNKNOWN`, one entry per list.
    #[must_use]
    pub fn merge_region(
        this_region: ObjectKey,
        this_type: i64,
        other_region: ObjectKey,
        other_type: i64,
    ) -> Self {
        Self::with_value(
            ids::OTHER_REGION_ID,
            Behavior::Copy,
            AttributeValue::MergeRegion(MergeRegionData {
                operation: 0,
                this_regions: vec![this_region],
                this_types: vec![this_type],
                other_regions: vec![other_region],
                other_types: vec![other_type],
            }),
        )
    }

    /// `MakeCopy`: a new attribute with the same id and data and no users.
    ///
    /// As upstream, `SmLongAttribute`, `SmVector3dAttribute` and `SmColorAttribute`
    /// copies get [`Behavior::Copy`] whatever the original's behaviour; the other
    /// classes keep it. The decimation attributes copy everything but their sort
    /// index, which restarts at `0` ([`crate::query::brep::DecimateVertexData::make_copy`],
    /// [`crate::query::brep::QuadricEdgeData::make_copy`]).
    #[must_use]
    pub fn make_copy(&self) -> Self {
        let id = self.id;
        match &self.value {
            AttributeValue::Long(v) => Self::long(id, *v, Behavior::Copy),
            AttributeValue::Vector3d(v) => Self::vector3d(id, *v, Behavior::Copy),
            AttributeValue::Color(v) => Self::color(id, *v, Behavior::Copy),
            AttributeValue::DecimateVertex(d) => Self::with_value(
                id,
                self.behavior,
                AttributeValue::DecimateVertex(d.make_copy()),
            ),
            AttributeValue::QuadricEdge(d) => Self::with_value(
                id,
                self.behavior,
                AttributeValue::QuadricEdge(d.make_copy()),
            ),
            other => Self::with_value(id, self.behavior, other.clone()),
        }
    }

    /// `GetAttributeID`.
    #[must_use]
    pub const fn attribute_id(&self) -> u64 {
        self.id
    }

    /// `GetBehavior`.
    #[must_use]
    pub const fn behavior(&self) -> Behavior {
        self.behavior
    }

    /// The derived class and its data.
    #[must_use]
    pub const fn value(&self) -> &AttributeValue {
        &self.value
    }

    /// Mutable access to the data (upstream's `Get...TArray()` references, public
    /// tag members, ...).
    pub fn value_mut(&mut self) -> &mut AttributeValue {
        &mut self.value
    }

    /// The objects using this attribute, in the order they were added (`m_vUsers`).
    #[must_use]
    pub fn users(&self) -> &[ObjectKey] {
        &self.users
    }

    /// `GetType`. `SmMergeRegionAttribute` reports `SmPointerAttribute_TYPE` and
    /// `SmColorAttribute` `SmVector3dAttribute_TYPE`, as upstream.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        match self.value {
            AttributeValue::None
            | AttributeValue::PolyToEuVu { .. }
            | AttributeValue::PolyEdgeToVertexUse { .. }
            | AttributeValue::DecimateVertex(_)
            | AttributeValue::QuadricVertex(_)
            | AttributeValue::QuadricEdge(_) => type_code::ATTRIBUTE,
            AttributeValue::Generic { .. } => type_code::GENERIC,
            AttributeValue::Long(_) => type_code::LONG,
            AttributeValue::Pointer(_) | AttributeValue::MergeRegion(_) => type_code::POINTER,
            AttributeValue::PointerList(_) => type_code::POINTER_LIST,
            AttributeValue::Vector3d(_) | AttributeValue::Color(_) => type_code::VECTOR3D,
            AttributeValue::Tag(_) => type_code::TAG,
        }
    }

    /// `GetTypeString`: the name of the [`Attribute::type_code`] macro.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        match self.value {
            AttributeValue::None
            | AttributeValue::PolyToEuVu { .. }
            | AttributeValue::PolyEdgeToVertexUse { .. }
            | AttributeValue::DecimateVertex(_)
            | AttributeValue::QuadricVertex(_)
            | AttributeValue::QuadricEdge(_) => "SmAttribute_TYPE",
            AttributeValue::Generic { .. } => "SmGenericAttribute_TYPE",
            AttributeValue::Long(_) => "SmLongAttribute_TYPE",
            AttributeValue::Pointer(_) | AttributeValue::MergeRegion(_) => {
                "SmPointerAttribute_TYPE"
            }
            AttributeValue::PointerList(_) => "SmPointerListAttribute_TYPE",
            AttributeValue::Vector3d(_) | AttributeValue::Color(_) => "SmVector3dAttribute_TYPE",
            AttributeValue::Tag(_) => "SmTagAttribute_TYPE",
        }
    }

    /// `IsKindOf(t)`: `t` is the class's own type or one of its base classes'
    /// (`SmMergeRegionAttribute` answers for `SmMergeRegionAttribute_TYPE`, not the
    /// pointer type its `GetType` reports; `SmColorAttribute` for both its own and
    /// the vector type).
    #[must_use]
    pub fn is_kind_of(&self, t: i64) -> bool {
        let own: &[i64] = match self.value {
            AttributeValue::None
            | AttributeValue::PolyToEuVu { .. }
            | AttributeValue::PolyEdgeToVertexUse { .. }
            | AttributeValue::DecimateVertex(_)
            | AttributeValue::QuadricVertex(_)
            | AttributeValue::QuadricEdge(_) => &[],
            AttributeValue::Generic { .. } => &[type_code::GENERIC],
            AttributeValue::Long(_) => &[type_code::LONG],
            AttributeValue::Pointer(_) => &[type_code::POINTER],
            AttributeValue::PointerList(_) => &[type_code::POINTER_LIST],
            AttributeValue::Vector3d(_) => &[type_code::VECTOR3D],
            AttributeValue::MergeRegion(_) => &[type_code::MERGE_REGION],
            AttributeValue::Tag(_) => &[type_code::TAG],
            AttributeValue::Color(_) => &[type_code::COLOR, type_code::VECTOR3D],
        };
        // SmObject::IsKindOf ends the chain.
        own.contains(&t) || t == type_code::ATTRIBUTE || object::is_kind_of(t)
    }

    /// `IsPropagatedThroughBooleans`: only colour attributes are propagated from the
    /// other operand's regions into the result.
    #[must_use]
    pub const fn is_propagated_through_booleans(&self) -> bool {
        matches!(self.value, AttributeValue::Color(_))
    }

    /// `SmLongAttribute::GetValue`; `None` for other classes.
    #[must_use]
    pub const fn long_value(&self) -> Option<i64> {
        match self.value {
            AttributeValue::Long(v) => Some(v),
            _ => None,
        }
    }

    /// `SmVector3dAttribute::GetValue` (also colour attributes); `None` for other
    /// classes.
    #[must_use]
    pub const fn vector3d_value(&self) -> Option<Vec3> {
        match self.value {
            AttributeValue::Vector3d(v) | AttributeValue::Color(v) => Some(v),
            _ => None,
        }
    }

    /// `SmVector3dAttribute::SetValue` (also colour attributes).
    pub fn set_vector3d_value(&mut self, v: Vec3) -> Result<()> {
        match &mut self.value {
            AttributeValue::Vector3d(x) | AttributeValue::Color(x) => {
                *x = v;
                Ok(())
            }
            _ => Err(SolidError::InvalidInput {
                operation: "geom::attribute::Attribute::set_vector3d_value",
                reason: "not a vector attribute",
            }),
        }
    }

    /// The `SmMergeRegionAttribute` data; `None` for other classes.
    #[must_use]
    pub const fn merge_region_data(&self) -> Option<&MergeRegionData> {
        match &self.value {
            AttributeValue::MergeRegion(d) => Some(d),
            _ => None,
        }
    }

    /// Mutable `SmMergeRegionAttribute` data; `None` for other classes.
    pub fn merge_region_data_mut(&mut self) -> Option<&mut MergeRegionData> {
        match &mut self.value {
            AttributeValue::MergeRegion(d) => Some(d),
            _ => None,
        }
    }
}

fn not_live(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "attribute handle is not live",
    }
}

/// Owner of attributes and of the attribute lists of attributed objects: the
/// two-way attribute/user relation of upstream `SmAttribute::m_vUsers` and
/// `SmAObject::m_pAttributes`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttributeStore {
    /// Attribute slots in creation order; `None` once deleted (never reused).
    slots: Vec<Option<Attribute>>,
    /// Each object's attribute list, in the order attributes were added.
    lists: BTreeMap<ObjectKey, Vec<AttributeHandle>>,
}

impl AttributeStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes ownership of a new attribute (upstream `new (crContext) Sm...Attribute`).
    /// It has no users until added to an object.
    pub fn insert(&mut self, attribute: Attribute) -> Result<AttributeHandle> {
        let index = u32::try_from(self.slots.len()).map_err(|_| SolidError::InvalidInput {
            operation: "geom::attribute::AttributeStore::insert",
            reason: "too many attributes",
        })?;
        // upstream's `new` of an attribute fails on memory exhaustion; never abort
        self.slots
            .try_reserve(1)
            .map_err(|_| SolidError::InvalidInput {
                operation: "geom::attribute::AttributeStore::insert",
                reason: "attribute store cannot grow",
            })?;
        self.slots.push(Some(attribute));
        Ok(AttributeHandle(index))
    }

    /// The attribute, if the handle is live.
    #[must_use]
    pub fn get(&self, h: AttributeHandle) -> Option<&Attribute> {
        self.slots.get(h.index()).and_then(Option::as_ref)
    }

    /// The attribute, mutably, if the handle is live.
    pub fn get_mut(&mut self, h: AttributeHandle) -> Option<&mut Attribute> {
        self.slots.get_mut(h.index()).and_then(Option::as_mut)
    }

    /// Whether the attribute has not been deleted.
    #[must_use]
    pub fn is_live(&self, h: AttributeHandle) -> bool {
        self.get(h).is_some()
    }

    fn attr(&self, h: AttributeHandle, operation: &'static str) -> Result<&Attribute> {
        self.get(h).ok_or_else(|| not_live(operation))
    }

    fn attr_mut(&mut self, h: AttributeHandle, operation: &'static str) -> Result<&mut Attribute> {
        self.get_mut(h).ok_or_else(|| not_live(operation))
    }

    /// `delete this`.
    fn free(&mut self, h: AttributeHandle) {
        if let Some(slot) = self.slots.get_mut(h.index()) {
            *slot = None;
        }
    }

    // ---- user list (SmAttribute) --------------------------------------------------

    /// `GetUsers`: copy of the user list.
    pub fn users(&self, h: AttributeHandle) -> Result<Vec<ObjectKey>> {
        Ok(self
            .attr(h, "geom::attribute::AttributeStore::users")?
            .users
            .clone())
    }

    /// `AddUser`: appends `user` to the user list (also when it is already there;
    /// upstream signals `SM_ERR` and appends anyway).
    pub fn add_user(&mut self, h: AttributeHandle, user: ObjectKey) -> Result<()> {
        let a = self.attr_mut(h, "geom::attribute::AttributeStore::add_user")?;
        a.users.push(user);
        Ok(())
    }

    /// `RemoveUser(pUser, bDoNotDelete)`: removes the first occurrence of `user`;
    /// when that empties the list, deletes the attribute unless `do_not_delete` or
    /// the behaviour is standalone. Returns whether it was deleted. Removing a user
    /// that is not on the list does nothing (upstream warns).
    pub fn remove_user(
        &mut self,
        h: AttributeHandle,
        user: ObjectKey,
        do_not_delete: bool,
    ) -> Result<bool> {
        let a = self.attr_mut(h, "geom::attribute::AttributeStore::remove_user")?;
        let Some(pos) = a.users.iter().position(|&u| u == user) else {
            return Ok(false);
        };
        a.users.remove(pos);
        if a.users.is_empty() && !do_not_delete && !a.behavior.is_standalone() {
            self.free(h);
            return Ok(true);
        }
        Ok(false)
    }

    /// `fromClearAttributes(pThis)`: the attribute side of
    /// [`AttributeStore::clear_attributes`]. With exactly one user (whoever it is)
    /// the user list is emptied and a dependent attribute deleted; otherwise this is
    /// `remove_user(h, obj, false)`. Returns whether the attribute was deleted.
    pub fn from_clear_attributes(&mut self, h: AttributeHandle, obj: ObjectKey) -> Result<bool> {
        let a = self.attr_mut(h, "geom::attribute::AttributeStore::from_clear_attributes")?;
        if a.users.len() != 1 {
            return self.remove_user(h, obj, false);
        }
        a.users.clear();
        if !a.behavior.is_standalone() {
            self.free(h);
            return Ok(true);
        }
        Ok(false)
    }

    /// `~SmAttribute` (`delete pAttribute`): removes the attribute from the list of
    /// each user (`RemoveAttribute`) and deletes it. Stops early, as upstream, when a
    /// user does not list the attribute.
    pub fn destroy(&mut self, h: AttributeHandle) -> Result<()> {
        const OP: &str = "geom::attribute::AttributeStore::destroy";
        let a = self.attr_mut(h, OP)?;
        a.id = UNDEF_ULONG;
        loop {
            let a = self.attr(h, OP)?;
            let Some(&user) = a.users.first() else {
                break;
            };
            let size_check = a.users.len();
            // RemoveAttribute(this) without its second delete (see module docs).
            self.remove_attribute(user, h, true)?;
            if self.attr(h, OP)?.users.len() >= size_check {
                break;
            }
        }
        self.free(h);
        Ok(())
    }

    // ---- attribute lists (SmAObject, co-ported) -----------------------------------

    /// `GetAttributes`: copy of the object's attribute list.
    #[must_use]
    pub fn attributes(&self, obj: ObjectKey) -> Vec<AttributeHandle> {
        // CO-PORT(geom.a_object): SmAObject::GetAttributes
        self.lists.get(&obj).cloned().unwrap_or_default()
    }

    /// `FindAttribute(lAttributeID)`: the first attribute in the object's list with
    /// that id.
    #[must_use]
    pub fn find_attribute(&self, obj: ObjectKey, attribute_id: u64) -> Option<AttributeHandle> {
        // CO-PORT(geom.a_object): SmAObject::FindAttribute
        self.lists
            .get(&obj)?
            .iter()
            .copied()
            .find(|&h| self.get(h).is_some_and(|a| a.id == attribute_id))
    }

    /// `AddAttribute(pAttribute)`: appends the attribute to the object's list and
    /// the object to its users, unless the first attribute of that id on the object
    /// is this very attribute. A *different* attribute with the same id is added.
    pub fn add_attribute(&mut self, obj: ObjectKey, h: AttributeHandle) -> Result<()> {
        // CO-PORT(geom.a_object): SmAObject::AddAttribute
        let id = self
            .attr(h, "geom::attribute::AttributeStore::add_attribute")?
            .id;
        if self.find_attribute(obj, id) == Some(h) {
            return Ok(());
        }
        self.lists.entry(obj).or_default().push(h);
        self.add_user(h, obj)
    }

    /// `RemoveAttribute(pAttribute, bDoNotDelete)`: removes the first occurrence
    /// from the object's list, then `RemoveUser(obj, bDoNotDelete)`. Returns whether
    /// the attribute was deleted; an attribute not on the list is left alone
    /// (upstream signals `SM_ERR`).
    pub fn remove_attribute(
        &mut self,
        obj: ObjectKey,
        h: AttributeHandle,
        do_not_delete: bool,
    ) -> Result<bool> {
        // CO-PORT(geom.a_object): SmAObject::RemoveAttribute
        let Some(list) = self.lists.get_mut(&obj) else {
            return Ok(false);
        };
        let Some(pos) = list.iter().position(|&x| x == h) else {
            return Ok(false);
        };
        list.remove(pos);
        self.remove_user(h, obj, do_not_delete)
    }

    /// `ClearAttributes`: `fromClearAttributes(obj)` on every entry of the object's
    /// list, in order, then empties the list.
    pub fn clear_attributes(&mut self, obj: ObjectKey) -> Result<()> {
        // CO-PORT(geom.a_object): SmAObject::ClearAttributes
        let Some(list) = self.lists.remove(&obj) else {
            return Ok(());
        };
        for h in list {
            self.from_clear_attributes(h, obj)?;
        }
        Ok(())
    }

    // ---- moving attributes between stores (not upstream) ---------------------------

    /// Every object with a non-empty attribute list, in key order.
    #[must_use]
    pub fn attributed_objects(&self) -> Vec<ObjectKey> {
        self.lists
            .iter()
            .filter(|(_, list)| !list.is_empty())
            .map(|(&key, _)| key)
            .collect()
    }

    /// (not upstream: upstream attributes live on the objects, so handing an object to
    /// an operation hands its attributes with it.) Moves every live attribute of
    /// `source` into this store under new handles, renaming each object key through
    /// `rekey`, in `source`'s slot and key order. Attributes keep their sharing (a
    /// reference attribute listed by several objects of `source` is one attribute here
    /// too) and their user order.
    ///
    /// # Errors
    /// `rekey`'s error (an object key with no name here), or a store that cannot grow.
    pub(crate) fn absorb(
        &mut self,
        source: AttributeStore,
        rekey: &dyn Fn(ObjectKey) -> Result<ObjectKey>,
    ) -> Result<()> {
        let mut handles = Vec::new();
        for slot in source.slots {
            let new = match slot {
                Some(mut attribute) => {
                    attribute.users = attribute
                        .users
                        .iter()
                        .map(|&user| rekey(user))
                        .collect::<Result<_>>()?;
                    Some(self.insert(attribute)?)
                }
                None => None,
            };
            handles.push(new);
        }
        for (key, list) in source.lists {
            let renamed: Vec<AttributeHandle> = list
                .iter()
                .filter_map(|h| handles.get(h.index()).copied().flatten())
                .collect();
            if renamed.is_empty() {
                continue;
            }
            self.lists.entry(rekey(key)?).or_default().extend(renamed);
        }
        Ok(())
    }

    /// (not upstream) Ends an operation's naming: each attributed object `keep` does
    /// not name is no longer an object of the result, so its list is cleared as
    /// upstream's object destructor does (`~SmAObject` → [`Self::clear_attributes`]:
    /// dependent attributes die with their last user, standalone ones survive without
    /// users), in key order; the remaining lists and user lists are then renamed
    /// through `keep`.
    ///
    /// # Errors
    /// None in practice (every listed handle is live).
    pub(crate) fn retain_rekeyed(
        &mut self,
        keep: &dyn Fn(ObjectKey) -> Option<ObjectKey>,
    ) -> Result<()> {
        let gone: Vec<ObjectKey> = self
            .lists
            .keys()
            .copied()
            .filter(|&key| keep(key).is_none())
            .collect();
        for key in gone {
            self.clear_attributes(key)?;
        }
        let lists = core::mem::take(&mut self.lists);
        for (key, list) in lists {
            if let Some(renamed) = keep(key) {
                self.lists.entry(renamed).or_default().extend(list);
            }
        }
        for attribute in self.slots.iter_mut().flatten() {
            attribute.users = attribute.users.iter().filter_map(|&u| keep(u)).collect();
        }
        Ok(())
    }

    // ---- propagation handlers -----------------------------------------------------

    /// Places this attribute on `target` with its behaviour when `target` has no
    /// attribute of its id: a deep copy (copy behaviours) or the attribute itself
    /// (reference behaviours); temporary attributes are not propagated. The common
    /// body of `Split`, `Copy`, `Merge` and `Coincident`.
    fn propagate_to(
        &mut self,
        h: AttributeHandle,
        target: ObjectKey,
        op: &'static str,
    ) -> Result<()> {
        let a = self.attr(h, op)?;
        if self.find_attribute(target, a.id).is_some() {
            return Ok(());
        }
        self.place_on(h, target, op)
    }

    /// The behaviour branch alone: copy or reference `h` onto `target`.
    fn place_on(&mut self, h: AttributeHandle, target: ObjectKey, op: &'static str) -> Result<()> {
        let a = self.attr(h, op)?;
        if a.behavior.is_copy() {
            let copy = a.make_copy();
            let new = self.insert(copy)?;
            self.add_attribute(target, new)
        } else if a.behavior.is_reference() {
            self.add_attribute(target, h)
        } else {
            Ok(())
        }
    }

    /// `Split(pSplitObject, pChild1, pChild2)`: each child other than the split
    /// object itself that has no attribute of this id gets a copy of (or a reference
    /// to) this attribute.
    pub fn split(
        &mut self,
        h: AttributeHandle,
        split_object: ObjectKey,
        child1: ObjectKey,
        child2: ObjectKey,
    ) -> Result<()> {
        const OP: &str = "geom::attribute::AttributeStore::split";
        if child1 != split_object {
            self.propagate_to(h, child1, OP)?;
        }
        if child2 != split_object {
            self.propagate_to(h, child2, OP)?;
        }
        Ok(())
    }

    /// `Copy(pFromObj, pToObj)`: `to` gets a copy of (or a reference to) this
    /// attribute unless it already has one of its id (`from` is not used).
    pub fn copy(&mut self, h: AttributeHandle, _from: ObjectKey, to: ObjectKey) -> Result<()> {
        self.propagate_to(h, to, "geom::attribute::AttributeStore::copy")
    }

    /// `Merge(pOrigObject1, pOrigObject2, pMergedResult)` (virtual): the merge-region
    /// attribute's own handler, otherwise the default one (`merged` gets a copy of
    /// or a reference to this attribute unless it has one of its id).
    /// Merge-region attributes require [`Self::merge_with_regions`]; this legacy
    /// entry point has no topology lookup and returns an explicit context error.
    pub fn merge(
        &mut self,
        h: AttributeHandle,
        orig1: ObjectKey,
        orig2: ObjectKey,
        merged: ObjectKey,
    ) -> Result<()> {
        self.merge_using_regions(h, orig1, orig2, merged, None)
    }

    /// Merge with the caller's actual typed region context. Ordinary attributes
    /// use the same default handler; merge-region attributes resolve live owners
    /// and flags rather than inferring them from opaque object keys.
    ///
    /// # Errors
    /// Invalid attribute handles, failed region lookup or native merge errors.
    pub fn merge_with_regions(
        &mut self,
        h: AttributeHandle,
        orig1: ObjectKey,
        orig2: ObjectKey,
        merged: ObjectKey,
        regions: &dyn crate::topology::shell::RegionLookup,
    ) -> Result<()> {
        self.merge_using_regions(h, orig1, orig2, merged, Some(regions))
    }

    fn merge_using_regions(
        &mut self,
        h: AttributeHandle,
        orig1: ObjectKey,
        orig2: ObjectKey,
        merged: ObjectKey,
        regions: Option<&dyn crate::topology::shell::RegionLookup>,
    ) -> Result<()> {
        const OP: &str = "geom::attribute::AttributeStore::merge";
        if matches!(self.attr(h, OP)?.value, AttributeValue::MergeRegion(_)) {
            return self.merge_region_merge(h, orig1, orig2, merged, regions);
        }
        self.propagate_to(h, merged, OP)
    }

    /// `Coincident(pFromObj, pToObj, pFromBrep, pToBrep)`: `to` gets a copy of (or
    /// a reference to) this attribute unless it has one of its id (the B-reps are
    /// not used and dropped from the signature).
    pub fn coincident(
        &mut self,
        h: AttributeHandle,
        _from: ObjectKey,
        to: ObjectKey,
    ) -> Result<()> {
        self.propagate_to(h, to, "geom::attribute::AttributeStore::coincident")
    }

    /// `Edit(pEditedObject)`: the default handler does nothing.
    pub fn edit(&mut self, h: AttributeHandle, _edited: ObjectKey) -> Result<()> {
        self.attr(h, "geom::attribute::AttributeStore::edit")
            .map(|_| ())
    }

    /// `Destruction(pToBeDeleted)`: the default handler does nothing.
    pub fn destruction(&mut self, h: AttributeHandle, _deleted: ObjectKey) -> Result<()> {
        self.attr(h, "geom::attribute::AttributeStore::destruction")
            .map(|_| ())
    }

    /// `PropagateRegionsThroughBooleans(pOrigObject1, pOrigObject2, pMergedResult)`
    /// (virtual): colour attributes average or carry over the colours of the two
    /// regions; every other class runs the *default* `Merge` (also the merge-region
    /// attribute, whose own `Merge` is not called).
    pub fn propagate_regions_through_booleans(
        &mut self,
        h: AttributeHandle,
        orig1: ObjectKey,
        orig2: ObjectKey,
        merged: ObjectKey,
    ) -> Result<()> {
        const OP: &str = "geom::attribute::AttributeStore::propagate_regions_through_booleans";
        if matches!(self.attr(h, OP)?.value, AttributeValue::Color(_)) {
            return self.color_propagate(h, orig1, orig2, merged);
        }
        self.propagate_to(h, merged, OP)
    }

    /// `SmColorAttribute::PropagateRegionsThroughBooleans`.
    fn color_propagate(
        &mut self,
        h: AttributeHandle,
        orig1: ObjectKey,
        orig2: ObjectKey,
        merged: ObjectKey,
    ) -> Result<()> {
        const OP: &str = "geom::attribute::AttributeStore::propagate_regions_through_booleans";
        let c1 = self.find_attribute(orig1, ids::COLOR);
        let c2 = self.find_attribute(orig2, ids::COLOR);
        let cm = self.find_attribute(merged, ids::COLOR);
        match (c1, c2) {
            (None, None) => Ok(()),
            (Some(tgt), None) | (None, Some(tgt)) => {
                if Some(tgt) != cm {
                    // A copy of *this* attribute, whatever pColorTgt is.
                    self.place_on(h, merged, OP)?;
                }
                Ok(())
            }
            (Some(c1), Some(c2)) => {
                let colour = |s: &Self, c| {
                    s.attr(c, OP)?
                        .vector3d_value()
                        .ok_or(SolidError::InvalidInput {
                            operation: OP,
                            reason: "colour attribute is not a vector attribute",
                        })
                };
                let a = colour(self, c1)?;
                let b = colour(self, c2)?;
                let cm = cm.ok_or(SolidError::InvalidInput {
                    operation: OP,
                    reason: "merged object has no colour attribute",
                })?;
                self.attr_mut(cm, OP)?.set_vector3d_value((a + b) / 2.0)
            }
        }
    }

    /// The merge-region attribute behind `id` on an object, if any; an attribute of
    /// that id of another class is an error (upstream casts it regardless).
    fn merge_region_on(&self, obj: ObjectKey, op: &'static str) -> Result<Option<AttributeHandle>> {
        let Some(h) = self.find_attribute(obj, ids::OTHER_REGION_ID) else {
            return Ok(None);
        };
        if self.attr(h, op)?.merge_region_data().is_none() {
            return Err(SolidError::InvalidInput {
                operation: op,
                reason: "SM_AI_OTHER_REGION_ID attribute is not a merge-region attribute",
            });
        }
        Ok(Some(h))
    }

    /// `SmMergeRegionAttribute::Merge`: accumulates the source regions of two merged
    /// regions of the same B-rep into the result region's merge-region attribute.
    fn merge_region_merge(
        &mut self,
        h: AttributeHandle,
        orig1: ObjectKey,
        orig2: ObjectKey,
        merged: ObjectKey,
        regions: Option<&dyn crate::topology::shell::RegionLookup>,
    ) -> Result<()> {
        const OP: &str = "geom::attribute::AttributeStore::merge";
        let brep1 = region_brep(orig1, regions)?;
        if brep1 != region_brep(orig2, regions)? {
            return Ok(());
        }
        let out = self.merge_region_on(merged, OP)?;
        let in1 = self.merge_region_on(orig1, OP)?;
        let in2 = self.merge_region_on(orig2, OP)?;
        if let (Some(in1), Some(in2)) = (in1, in2) {
            let add = if orig1 == merged { in2 } else { in1 };
            let src = self
                .attr(add, OP)?
                .merge_region_data()
                .cloned()
                .ok_or_else(|| not_live(OP))?;
            let out = out.ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "merged region has no merge-region attribute",
            })?;
            let dst = self
                .attr_mut(out, OP)?
                .merge_region_data_mut()
                .ok_or_else(|| not_live(OP))?;
            append_new_regions(
                &mut dst.this_regions,
                &mut dst.this_types,
                &src.this_regions,
                &src.this_types,
            )?;
            append_new_regions(
                &mut dst.other_regions,
                &mut dst.other_types,
                &src.other_regions,
                &src.other_types,
            )?;
            return Ok(());
        }
        let out = if let Some(out) = out {
            out
        } else {
            let copy = self.attr(h, OP)?.make_copy();
            let new = self.insert(copy)?;
            self.add_attribute(merged, new)?;
            new
        };
        let add_region = if in1.is_none() { orig1 } else { orig2 };
        let add_type = if region_is_infinite(add_region, regions)? {
            region_type::INFINITE
        } else if region_is_void(add_region, regions)? {
            region_type::VOID
        } else {
            region_type::SOLID
        };
        let dst = self
            .attr_mut(out, OP)?
            .merge_region_data_mut()
            .ok_or_else(|| not_live(OP))?;
        dst.this_regions.push(add_region);
        dst.this_types.push(add_type);
        Ok(())
    }

    /// `SmMergeRegionAttribute::HasAnyOtherPropagatedAttribs`: some "other" source
    /// region carries an attribute that is propagated through Booleans.
    pub fn has_any_other_propagated_attribs(&self, h: AttributeHandle) -> Result<bool> {
        const OP: &str = "geom::attribute::AttributeStore::has_any_other_propagated_attribs";
        let d = self
            .attr(h, OP)?
            .merge_region_data()
            .ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "not a merge-region attribute",
            })?;
        for &r in &d.other_regions {
            for a in self.attributes(r) {
                if self.attr(a, OP)?.is_propagated_through_booleans() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // ---- persistence --------------------------------------------------------------

    /// `SmAttributeData::ReadIndexedAttributesFromDB`: reads one entity's attribute
    /// index list (a count, then the indices; ASCII: the rest of the line is
    /// skipped) and makes sure `all_attributes` has a slot for every index, filling
    /// empty slots with a placeholder ([`ids::UNKNOWN`],
    /// [`Behavior::StandaloneReference`]). Returns the indices.
    ///
    /// ASCII lines are skipped as upstream's `GOTO_NEXT_ATTLINE`: more than 1000
    /// bytes before the newline, or the end of the data (also a `0xFF` byte, which
    /// compares equal to `EOF` as a `signed char`), is `SM_ERR`
    /// ([`SolidError::Upstream`]).
    pub fn read_indexed_attributes_from_db(
        &mut self,
        all_attributes: &mut Vec<Option<AttributeHandle>>,
        db: &mut DatabaseReader,
    ) -> Result<Vec<u64>> {
        let read = self.read_indexed_attributes_from_db_partial(all_attributes, db)?;
        match read.status {
            Some(e) => Err(e),
            None => Ok(read.indices),
        }
    }

    /// [`Self::read_indexed_attributes_from_db`] for callers that ignore its status
    /// (upstream `SmBrepData`'s readers): when the ASCII line skip after the indices
    /// fails (`SM_ERR`), the indices read so far are returned with that status in
    /// [`IndexedAttributes::status`] and, as upstream, no placeholder is created.
    ///
    /// # Errors
    ///
    /// Those of [`Self::read_indexed_attributes_from_db`] other than the line skip's.
    pub fn read_indexed_attributes_from_db_partial(
        &mut self,
        all_attributes: &mut Vec<Option<AttributeHandle>>,
        db: &mut DatabaseReader,
    ) -> Result<IndexedAttributes> {
        const OP: &str = "geom::attribute::AttributeStore::read_indexed_attributes_from_db";
        let ascii = db.file_type() == FileType::Ascii;
        let no_stream = || SolidError::InvalidInput {
            operation: OP,
            reason: "reader has no stream",
        };
        let count = if ascii {
            let s = db.in_stream_mut().ok_or_else(no_stream)?;
            s.extract_ulong().ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "attribute count could not be extracted",
            })?
        } else {
            db.read_ulong()?
        };
        if count == 0 {
            let mut status = None;
            if ascii {
                status = goto_next_attline(db.in_stream_mut().ok_or_else(no_stream)?).err();
            }
            return Ok(IndexedAttributes {
                indices: Vec::new(),
                status,
            });
        }
        let too_big = || SolidError::InvalidInput {
            operation: OP,
            reason: "attribute count or index too large",
        };
        if count > MAX_INDEXED_ATTRIBUTES {
            return Err(too_big());
        }
        let n = count as usize;
        let indices = if ascii {
            db.ensure_available(n, 1, OP)?;
            let s = db.in_stream_mut().ok_or_else(no_stream)?;
            let mut v = Vec::new();
            for _ in 0..n {
                let value =
                    s.extract_ulong()
                        .filter(|_| !s.fail())
                        .ok_or(SolidError::InvalidInput {
                            operation: OP,
                            reason: "attribute index could not be extracted",
                        })?;
                v.push(value);
            }
            if let Err(e) = goto_next_attline(s) {
                return Ok(IndexedAttributes {
                    indices: v,
                    status: Some(e),
                });
            }
            v
        } else {
            db.read_longs(n)?
        };
        if indices.iter().any(|&i| i > MAX_INDEXED_ATTRIBUTES) {
            return Err(too_big());
        }
        for &i in &indices {
            let i = i as usize;
            if all_attributes.len() < i + 1 {
                all_attributes.resize(i + 1, None);
            }
            if let Some(slot) = all_attributes.get_mut(i) {
                if slot.is_none() {
                    let h =
                        self.insert(Attribute::new(ids::UNKNOWN, Behavior::StandaloneReference))?;
                    *slot = Some(h);
                }
            }
        }
        Ok(IndexedAttributes {
            indices,
            status: None,
        })
    }
}

/// Result of [`AttributeStore::read_indexed_attributes_from_db_partial`].
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedAttributes {
    /// The attribute indices read (upstream's `rAttributes` out-param).
    pub indices: Vec<u64>,
    /// `Some` when upstream returned `SM_ERR` from the ASCII line skip
    /// (`GOTO_NEXT_ATTLINE`); the placeholders were then not created.
    pub status: Option<SolidError>,
}

/// One half of `SmMergeRegionAttribute::Merge`'s accumulation: appends each source
/// region not yet in `dst_regions`, with its type, in source order.
fn append_new_regions(
    dst_regions: &mut Vec<ObjectKey>,
    dst_types: &mut Vec<i64>,
    src_regions: &[ObjectKey],
    src_types: &[i64],
) -> Result<()> {
    for (i, &r) in src_regions.iter().enumerate() {
        if !dst_regions.contains(&r) {
            let t = src_types.get(i).copied().ok_or(SolidError::InvalidInput {
                operation: "geom::attribute::AttributeStore::merge",
                reason: "region type list shorter than region list",
            })?;
            dst_regions.push(r);
            dst_types.push(t);
        }
    }
    Ok(())
}

/// `GOTO_NEXT_ATTLINE`: consume through the next `'\n'`.
fn goto_next_attline(s: &mut InputStream) -> Result<()> {
    let err = || SolidError::Upstream {
        operation: "geom::attribute::AttributeStore::read_indexed_attributes_from_db",
        code: SM_ERR,
        entity: None,
    };
    let mut count = 0u32;
    loop {
        // `(signed char)rFileIn.get()`: EOF and a 0xFF byte both become -1.
        let ch: i8 = s.get().map_or(-1, |b| b as i8);
        if ch == b'\n' as i8 {
            return Ok(());
        }
        count += 1;
        if count > 1000 {
            return Err(err());
        }
        if ch == -1 {
            return Err(err());
        }
    }
}

/// Resolve the native pointer through a live caller-supplied topology context.
/// Without one the region query is a deferral (`Unported`), not an invalid input:
/// the native pointer is always reachable upstream.
fn resolve_region<'a>(
    region: ObjectKey,
    regions: Option<&'a dyn crate::topology::shell::RegionLookup>,
    symbol: &'static str,
) -> Result<crate::topology::shell::RegionRef<'a>> {
    // UNPORTED(topology.shell): SmRegion::GetBrep / IsInfiniteRegion / IsVoid need a caller-supplied
    // `RegionLookup`; see plan/findings/topology.shell--borrowed-region-context-for-attribute-backedges-pr-80-issue.md.
    // Callers that pass no lookup (`a_object::notify` from change-owner / geometry edits in
    // brep p6/p9/p10, edge, face, fillet) never send a region Merge or RegPropagation
    // notification, so no region handler runs without one; the `None` deferral is covered by the unit tests of this file.
    // REGION-LOOKUP(#86): no library `RegionLookup` exists yet, so a region handler
    // reached without a caller-supplied context defers here instead of inferring
    // region state. The owner of issue #86 replaces this with the real lookup
    // (grep REGION-LOOKUP).
    regions
        .ok_or_else(|| SolidError::unported(symbol, "topology.shell"))?
        .resolve_region(region)
}

/// `SmRegion::GetBrep`, preserving NULL owner identity.
fn region_brep(
    region: ObjectKey,
    regions: Option<&dyn crate::topology::shell::RegionLookup>,
) -> Result<Option<crate::topology::BrepId>> {
    resolve_region(region, regions, "SmRegion::GetBrep")?.owning_brep()
}

/// `SmRegion::IsInfiniteRegion` on the actual arena record.
fn region_is_infinite(
    region: ObjectKey,
    regions: Option<&dyn crate::topology::shell::RegionLookup>,
) -> Result<bool> {
    resolve_region(region, regions, "SmRegion::IsInfiniteRegion")?.is_infinite()
}

/// Native truth semantics: every nonzero `SmBoolean`, including `UNSURE`, is true.
fn region_is_void(
    region: ObjectKey,
    regions: Option<&dyn crate::topology::shell::RegionLookup>,
) -> Result<bool> {
    Ok(resolve_region(region, regions, "SmRegion::IsVoid")?.void_state()? != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const O1: ObjectKey = ObjectKey(1);
    const O2: ObjectKey = ObjectKey(2);
    const O3: ObjectKey = ObjectKey(3);

    #[test]
    fn behavior_codes_round_trip() {
        for c in 0..5 {
            assert_eq!(Behavior::from_code(c).unwrap().code(), c);
        }
        assert!(Behavior::from_code(5).is_err());
        assert!(Behavior::StandaloneCopy.is_copy() && Behavior::StandaloneCopy.is_standalone());
        assert!(!Behavior::Temp.is_copy() && !Behavior::Temp.is_reference());
    }

    #[test]
    fn make_copy_drops_behavior_for_long_vector_color() {
        let l = Attribute::long(7, 5, Behavior::StandaloneCopy).make_copy();
        assert_eq!(l.behavior(), Behavior::Copy);
        let t = Attribute::tag(ids::TAG, TagData::default(), Behavior::StandaloneCopy).make_copy();
        assert_eq!(t.behavior(), Behavior::StandaloneCopy);
        let c = Attribute::color(ids::COLOR, Vec3::new(1.0, 0.0, 0.0), Behavior::Reference);
        assert_eq!(c.make_copy().behavior(), Behavior::Copy);
        assert_eq!(
            c.make_copy().vector3d_value(),
            Some(Vec3::new(1.0, 0.0, 0.0))
        );
    }

    #[test]
    fn types_and_kinds() {
        let m = Attribute::merge_region(O1, 0, O2, 2);
        assert_eq!(m.type_code(), type_code::POINTER);
        assert!(m.is_kind_of(type_code::MERGE_REGION));
        assert!(!m.is_kind_of(type_code::POINTER));
        let c = Attribute::color(ids::COLOR, Vec3::new(0.0, 0.0, 0.0), Behavior::Copy);
        assert_eq!(c.type_string(), "SmVector3dAttribute_TYPE");
        assert!(c.is_kind_of(type_code::COLOR) && c.is_kind_of(type_code::VECTOR3D));
        assert!(c.is_kind_of(type_code::OBJECT) && c.is_propagated_through_booleans());
        assert!(!Attribute::new(1, Behavior::Copy).is_kind_of(type_code::LONG));
    }

    #[test]
    fn add_remove_and_delete() {
        let mut s = AttributeStore::new();
        let a = s.insert(Attribute::long(500, 1, Behavior::Copy)).unwrap();
        s.add_attribute(O1, a).unwrap();
        s.add_attribute(O1, a).unwrap(); // same attribute: ignored
        assert_eq!(s.attributes(O1), vec![a]);
        assert_eq!(s.users(a).unwrap(), vec![O1]);
        assert!(s.remove_attribute(O1, a, false).unwrap());
        assert!(!s.is_live(a));
        assert!(s.users(a).is_err());
    }

    #[test]
    fn standalone_survives_and_duplicates_are_added() {
        let mut s = AttributeStore::new();
        let a1 = s.insert(Attribute::new(9, Behavior::Copy)).unwrap();
        let a2 = s
            .insert(Attribute::new(9, Behavior::StandaloneReference))
            .unwrap();
        s.add_attribute(O1, a1).unwrap();
        s.add_attribute(O1, a2).unwrap();
        s.add_attribute(O1, a2).unwrap(); // find(9) is a1, so a2 goes in twice
        assert_eq!(s.attributes(O1), vec![a1, a2, a2]);
        assert_eq!(s.users(a2).unwrap(), vec![O1, O1]);
        s.clear_attributes(O1).unwrap();
        assert!(!s.is_live(a1));
        assert!(s.is_live(a2));
        assert!(s.users(a2).unwrap().is_empty());
    }

    #[test]
    fn split_copies_or_references() {
        let mut s = AttributeStore::new();
        let a = s.insert(Attribute::long(5, 3, Behavior::Copy)).unwrap();
        let r = s.insert(Attribute::new(6, Behavior::Reference)).unwrap();
        s.add_attribute(O1, a).unwrap();
        s.add_attribute(O1, r).unwrap();
        s.split(a, O1, O1, O2).unwrap();
        s.split(r, O1, O2, O3).unwrap();
        let on2 = s.attributes(O2);
        assert_eq!(on2.len(), 2);
        assert_ne!(on2[0], a);
        assert_eq!(s.get(on2[0]).unwrap().long_value(), Some(3));
        assert_eq!(on2[1], r);
        assert_eq!(s.users(r).unwrap(), vec![O1, O2, O3]);
        // temp attributes do not propagate
        let t = s.insert(Attribute::new(7, Behavior::Temp)).unwrap();
        s.copy(t, O1, O3).unwrap();
        assert_eq!(s.find_attribute(O3, 7), None);
    }

    #[test]
    fn color_average() {
        let mut s = AttributeStore::new();
        let c1 = s
            .insert(Attribute::color(
                ids::COLOR,
                Vec3::new(1.0, 0.0, 0.5),
                Behavior::Copy,
            ))
            .unwrap();
        let c2 = s
            .insert(Attribute::vector3d(
                ids::COLOR,
                Vec3::new(0.0, 1.0, 0.5),
                Behavior::Copy,
            ))
            .unwrap();
        s.add_attribute(O1, c1).unwrap();
        s.add_attribute(O2, c2).unwrap();
        s.propagate_regions_through_booleans(c1, O1, O2, O1)
            .unwrap();
        assert_eq!(
            s.get(c1).unwrap().vector3d_value(),
            Some(Vec3::new(0.5, 0.5, 0.5))
        );
        // only one colour: copied onto the result
        s.propagate_regions_through_booleans(c1, O1, O3, O3)
            .unwrap();
        assert!(s.find_attribute(O3, ids::COLOR).is_some());
    }

    #[test]
    fn region_lookup_preserves_brep_identity_null_owners_and_native_void_truth() {
        use crate::topology::shell::{RegionLookup, RegionRef};
        use crate::topology::{Brep, BrepId, Region, RegionId};
        struct Lookup {
            left: Brep,
            right: Brep,
            detached: Brep,
            void: RegionId,
        }
        impl RegionLookup for Lookup {
            fn resolve_region(&self, key: ObjectKey) -> Result<RegionRef<'_>> {
                let (brep, brep_id, region_id) = match key.0 {
                    1 => (&self.left, BrepId::from_index(0), RegionId::from_index(0)),
                    2 => (&self.right, BrepId::from_index(1), RegionId::from_index(0)),
                    3 => (&self.left, BrepId::from_index(0), self.void),
                    4 => (
                        &self.detached,
                        BrepId::from_index(2),
                        RegionId::from_index(0),
                    ),
                    _ => return Err(SolidError::invalid_input("test::regions", "unknown object")),
                };
                Ok(RegionRef {
                    brep,
                    brep_id,
                    region_id,
                })
            }
        }
        let mut left = Brep::new();
        let inf = left
            .add_region_record(Region {
                in_brep: true,
                ..Region::new()
            })
            .unwrap();
        left.set_infinite_region_record(Some(inf));
        let void = left
            .add_region_record(Region {
                in_brep: true,
                is_void: 2,
                ..Region::new()
            })
            .unwrap();
        let mut right = Brep::new();
        let right_inf = right
            .add_region_record(Region {
                in_brep: true,
                ..Region::new()
            })
            .unwrap();
        right.set_infinite_region_record(Some(right_inf));
        assert_eq!(
            inf, right_inf,
            "identical scoped indices belong to distinct B-reps"
        );
        let mut detached = Brep::new();
        detached.add_region_record(Region::new()).unwrap();
        let lookup = Lookup {
            left,
            right,
            detached,
            void,
        };
        assert_eq!(
            lookup.resolve_region(O1).unwrap().owning_brep().unwrap(),
            Some(BrepId::from_index(0))
        );
        assert_eq!(
            lookup
                .resolve_region(ObjectKey(4))
                .unwrap()
                .owning_brep()
                .unwrap(),
            None
        );
        assert!(lookup.resolve_region(O1).unwrap().is_infinite().unwrap());
        assert!(!lookup.resolve_region(O3).unwrap().is_infinite().unwrap());
        assert_eq!(lookup.resolve_region(O3).unwrap().void_state().unwrap(), 2);
        let mut store = AttributeStore::new();
        let h = store.insert(Attribute::merge_region(O1, 0, O2, 1)).unwrap();
        store.add_attribute(O1, h).unwrap();
        let before = store.clone();
        store.merge_with_regions(h, O1, O2, O3, &lookup).unwrap();
        assert_eq!(store, before, "different owner identities do not merge");
        store
            .merge_with_regions(h, O1, ObjectKey(4), O3, &lookup)
            .unwrap();
        assert_eq!(
            store, before,
            "NULL must differ from valid B-rep identity zero"
        );
        store.merge_with_regions(h, O1, O3, O1, &lookup).unwrap();
        let data = store.get(h).unwrap().merge_region_data().unwrap();
        assert_eq!(data.this_regions().last().copied(), Some(O3));
        assert_eq!(
            data.this_region_types().last().copied(),
            Some(region_type::VOID)
        );
    }

    #[test]
    fn merge_region_merge_requires_region_context() {
        let mut s = AttributeStore::new();
        let m = s.insert(Attribute::merge_region(O1, 0, O2, 1)).unwrap();
        s.add_attribute(O1, m).unwrap();
        assert!(matches!(
            s.merge(m, O1, O3, O1),
            Err(SolidError::Unported {
                unit: "topology.shell",
                ..
            })
        ));
        assert!(!s.has_any_other_propagated_attribs(m).unwrap());
        let c = s
            .insert(Attribute::color(
                ids::COLOR,
                Vec3::new(0.0, 0.0, 0.0),
                Behavior::Copy,
            ))
            .unwrap();
        s.add_attribute(O2, c).unwrap();
        assert!(s.has_any_other_propagated_attribs(m).unwrap());
    }

    #[test]
    fn merge_region_queries() {
        let mut d = Attribute::merge_region(O1, 0, O2, 2)
            .merge_region_data()
            .cloned()
            .unwrap();
        assert!(d.has_other_infinite_region() && !d.has_this_infinite_region());
        assert!(d.has_any_solid(1) && !d.has_any_solid(2));
        assert!(matches!(
            d.set_regions(1, O1, 1, O2, 1),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        d.set_regions(0, O3, 1, O1, 0).unwrap();
        assert_eq!(d.this_region(0), Some(O3));
        assert!(d.has_any_solid(2));
    }

    #[test]
    fn destroy_standalone_with_users() {
        let mut s = AttributeStore::new();
        let a = s
            .insert(Attribute::new(1, Behavior::StandaloneCopy))
            .unwrap();
        s.add_attribute(O1, a).unwrap();
        s.add_attribute(O2, a).unwrap();
        s.destroy(a).unwrap();
        assert!(!s.is_live(a));
        assert!(s.attributes(O1).is_empty() && s.attributes(O2).is_empty());
    }

    #[test]
    fn read_indexed_ascii() {
        let mut s = AttributeStore::new();
        let mut all = Vec::new();
        let mut db = DatabaseReader::with_stream(
            FileType::Ascii,
            InputStream::new(b"  2 3 1 tail\n 0\nx".to_vec()),
        );
        assert_eq!(
            s.read_indexed_attributes_from_db(&mut all, &mut db)
                .unwrap(),
            vec![3, 1]
        );
        assert_eq!(all.len(), 4);
        assert!(all[0].is_none() && all[1].is_some() && all[3].is_some());
        assert_eq!(s.get(all[1].unwrap()).unwrap().attribute_id(), ids::UNKNOWN);
        let before_store = s.clone();
        let before_all = all.clone();
        assert!(s
            .read_indexed_attributes_from_db(&mut all, &mut db)
            .unwrap()
            .is_empty());
        assert_eq!(s, before_store);
        assert_eq!(all, before_all);
        assert!(matches!(
            s.read_indexed_attributes_from_db(&mut all, &mut db),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
    }

    #[test]
    fn read_indexed_ascii_max_count_without_payload() {
        let mut s = AttributeStore::new();
        let h = s.insert(Attribute::long(7, 42, Behavior::Copy)).unwrap();
        let mut all = vec![Some(h), None];
        let before_store = s.clone();
        let before_all = all.clone();
        let mut db = DatabaseReader::with_stream(
            FileType::Ascii,
            InputStream::new(format!("{MAX_INDEXED_ATTRIBUTES}\n").into_bytes()),
        );
        assert!(matches!(
            s.read_indexed_attributes_from_db_partial(&mut all, &mut db),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(s, before_store);
        assert_eq!(all, before_all);
    }

    #[test]
    fn read_indexed_ascii_rejects_incomplete_extraction() {
        for data in ["3 1  \n", "3 1 2", "2 1 tail\n"] {
            let mut s = AttributeStore::new();
            let mut all = vec![None];
            let before_store = s.clone();
            let before_all = all.clone();
            let mut db = DatabaseReader::with_stream(
                FileType::Ascii,
                InputStream::new(data.as_bytes().to_vec()),
            );
            assert!(matches!(
                s.read_indexed_attributes_from_db_partial(&mut all, &mut db),
                Err(SolidError::InvalidInput { .. })
            ));
            assert_eq!(s, before_store);
            assert_eq!(all, before_all);
        }
    }

    #[test]
    fn read_indexed_ascii_preserves_indices_on_failed_line_skip() {
        let mut s = AttributeStore::new();
        let mut all = vec![None];
        let before_store = s.clone();
        let before_all = all.clone();
        let mut db =
            DatabaseReader::with_stream(FileType::Ascii, InputStream::new(b"2 3 1".to_vec()));
        let read = s
            .read_indexed_attributes_from_db_partial(&mut all, &mut db)
            .unwrap();
        assert_eq!(read.indices, vec![3, 1]);
        assert!(matches!(
            read.status,
            Some(SolidError::Upstream { code: 1001, .. })
        ));
        assert_eq!(s, before_store);
        assert_eq!(all, before_all);
    }

    #[test]
    fn read_indexed_binary() {
        let mut s = AttributeStore::new();
        let mut all = vec![None; 1];
        let mut data = Vec::new();
        for v in [2u64, 0, 2] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut db = DatabaseReader::with_stream(FileType::Binary, InputStream::new(data));
        assert_eq!(
            s.read_indexed_attributes_from_db(&mut all, &mut db)
                .unwrap(),
            vec![0, 2]
        );
        assert_eq!(all.len(), 3);
        assert!(all[1].is_none());
    }
}
