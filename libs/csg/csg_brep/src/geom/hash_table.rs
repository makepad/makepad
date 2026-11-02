//! Voxel bucket hash of point-located items for fast coincident-pair searches
//! (`SmObjsInVoxels`), over the linked-bucket hash table it is built on
//! (`SmHashTable`).
//!
//! Upstream: `source/SMLib/inc/SmHashTable.h`.
//!
//! | Rust                                        | Upstream                                         |
//! |---------------------------------------------|--------------------------------------------------|
//! | [`ObjsInVoxels`]                            | `SmObjsInVoxels<TYPE>`                           |
//! | [`ObjsInVoxels::new`]                       | `SmObjsInVoxels::SmObjsInVoxels`                 |
//! | `Drop` (implicit)                           | `SmObjsInVoxels::~SmObjsInVoxels`                |
//! | [`ObjsInVoxels::set_size`]                  | `SmObjsInVoxels::SetSize`                        |
//! | `inc_max_item_count` (private)              | `SmObjsInVoxels::IncMaxItemCount`                |
//! | [`ObjsInVoxels::map_indices_to_voxel_index`] | `SmObjsInVoxels::MapIndicesToVoxelIndex`        |
//! | [`ObjsInVoxels::map_voxel_index_to_indices`] | `SmObjsInVoxels::MapVoxelIndexToIndices`        |
//! | [`ObjsInVoxels::map_point_to_voxel`]        | `SmObjsInVoxels::MapPointToVoxel`                |
//! | [`ObjsInVoxels::map_voxel_index_to_bbox`]   | `SmObjsInVoxels::MapVoxelIndexToBBox`            |
//! | [`ObjsInVoxels::find_point_items`]          | `SmObjsInVoxels::FindPointItems`                 |
//! | [`ObjsInVoxels::add`]                       | `SmObjsInVoxels::Add`                            |
//! | [`ObjsInVoxels::remove`]                    | `SmObjsInVoxels::Remove`                         |
//! | [`ObjsInVoxels::find_coincident_pairs`]     | `SmObjsInVoxels::FindCoincidentPairs`            |
//! | [`ObjsInVoxels::point_items`], [`ObjsInVoxels::hash_table`], ... | `GetPointItems`, `m_sHashTable`, ... |
//! | [`PointItem`]                               | `SmPointItem<TYPE>`                              |
//! | [`HashTable`]                               | `SmHashTable<TYPE>` (template bodies this unit needs) |
//! | [`BucketItem`]                              | `SmBucketItem<TYPE>`                             |
//! | [`gap_histogram_index`]                     | `SM_HISTOGRAM_GAPSIZE_INDEX`                     |
//! | `ulong_from_f64` (private)                  | the `(ULONG)double` conversions of `MapPointToVoxel` (x86-64 g++) |
//!
//! Deviations:
//! * Pointers become indices: a bucket item's `m_sItem` (a `SmPointItem*` into
//!   `m_sPointItems`) is the point-item index, the bucket / used-bucket links are
//!   bucket-item indices, `SM_UNDEF_ULONG` bucket indices and voxel indices are
//!   `None`. Growing the arrays therefore never invalidates anything. Capacities
//!   (`GetDataSize`) are still tracked so growth happens at the same moments, and the
//!   all-removed growth path that zeroes the bucket-item records upstream is
//!   reproduced.
//! * Upstream undefined behaviour (a bucket index past the table, a voxel model
//!   with a box but zero voxels in `Add`, an empty explicit bucket list, a zero
//!   divisor in `MapVoxelIndexToIndices`) returns [`SolidError::InvalidInput`]
//!   (or, for an empty bucket list, checks no bucket).
//! * Voxel dimension products are checked before changing the model; overflow
//!   returns `InvalidInput` instead of silently wrapping as upstream does.
//! * `FindCoincidentPairs` on a bucket item without a point item skips it (upstream
//!   dereferences `NULL`).
//! * Debug-only code (`SM_DEBUG_CODE`, asserts, `pOptBrep` drawing), `Dump`,
//!   `Draw`, `AssertValid`, `operator=` / `operator==` are not ported (`Clone`
//!   covers assignment).

use crate::error::{Result, SolidError};
use crate::geom::extent3d::Extent3d;
use crate::geom::tol::xsect_tol3d;
use crate::kernel::Kernel;
use crate::math::{Point3, UNDEF_DOUBLE};

/// Number of gap-size histogram buckets (`SM_HISTOGRAM_GAPSIZE_BUCKETCNT`).
pub const GAP_HISTOGRAM_BUCKETS: usize = 11;

/// Most point items one [`ObjsInVoxels::add`] creates (centre + 6 neighbours).
const MAX_ITEMS_PER_ADD: usize = 7;

/// Histogram bucket of a gap length (`SM_HISTOGRAM_GAPSIZE_INDEX`):
/// `10` above `10`, `floor(log10(gap)) + 9` above `1e-6`, then `2`, `1`, `0`.
#[must_use]
pub fn gap_histogram_index(gap: f64) -> usize {
    if gap > 10.0 {
        10
    } else if gap > 1e-6 {
        (gap.log10().floor() + 9.0) as usize
    } else if gap > 1e-8 {
        2
    } else {
        usize::from(gap > 1e-10)
    }
}

/// `cvttsd2si`: truncation to `i64`, with `i64::MIN` for NaN and out-of-range.
fn cvttsd2si(v: f64) -> i64 {
    if v.is_nan() || v >= 9_223_372_036_854_775_808.0 || v < -9_223_372_036_854_775_808.0 {
        i64::MIN
    } else {
        v as i64
    }
}

/// Upstream `(ULONG)v` as the x86-64 g++ reference build evaluates it (the
/// conversion is undefined for negative / out-of-range values in C++): values
/// below `2^63` (and NaN) go through a signed truncation, so negative values
/// wrap to huge indices; values from `2^63` are offset by `2^63` first.
fn ulong_from_f64(v: f64) -> u64 {
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    if v >= TWO_63 {
        (cvttsd2si(v - TWO_63) as u64) ^ (1u64 << 63)
    } else {
        cvttsd2si(v) as u64
    }
}

/// `SmTArray::AddUnique` on a bucket-index list.
fn add_unique(list: &mut Vec<usize>, v: usize) {
    if !list.contains(&v) {
        list.push(v);
    }
}

/// Growth rule of the item arrays (`SmHashTable::InitSize` / `IncMaxItemCount`):
/// `max((ULONG)(1.5 * n), n + 64)`.
fn grown_capacity(n: usize) -> usize {
    ((1.5 * n as f64) as usize).max(n.wrapping_add(64))
}

/// One entry of a [`HashTable`] bucket (`SmBucketItem`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketItem {
    /// First item of the next used bucket; only set on a bucket's first item
    /// (`m_pNextUsedBucket`).
    pub next_used_bucket: Option<usize>,
    /// Next item in the same bucket (`m_pNextBucketItem`).
    pub next_bucket_item: Option<usize>,
    /// Bucket holding the item; `None` marks a removed item (`m_lBucketIndex`
    /// `== SM_UNDEF_ULONG`).
    pub bucket_index: Option<usize>,
    /// The hashed item: for [`ObjsInVoxels`] the index of its [`PointItem`]
    /// (`m_sItem`; `None` only for records zeroed by a growth, see
    /// [`HashTable::inc_max_item_count`]).
    pub item: Option<usize>,
    /// Caller id stored with the item (`m_lItemId`).
    pub item_id: usize,
}

impl BucketItem {
    /// A removed bucket item (`IsRemovedBucketItem`).
    #[must_use]
    pub fn is_removed(&self) -> bool {
        self.bucket_index.is_none()
    }
}

/// Bucket sort of items into linked buckets with a linked list of the used
/// buckets (`SmHashTable`, the members [`ObjsInVoxels`] needs).
///
/// Items are appended to one array and never moved: removed items stay as
/// unlinked records.
#[derive(Debug, Clone, Default)]
pub struct HashTable {
    /// First item of each bucket (`m_sBuckets`).
    buckets: Vec<Option<usize>>,
    /// Every added item, removed ones included (`m_sBucketItems`).
    bucket_items: Vec<BucketItem>,
    /// Allocated item slots (`m_sBucketItems.GetDataSize()`).
    capacity: usize,
    /// Removed items still in `bucket_items` (`m_lRemovedBucketItemCount`).
    removed_bucket_item_count: usize,
    /// First item of the most recently used bucket (`m_pUsedBuckets`).
    used_buckets: Option<usize>,
    /// Length of the used-bucket list (`m_lUsedBucketCount`).
    used_bucket_count: usize,
}

impl HashTable {
    /// Empties the table and sizes it (`SmHashTable(lItemEstimate, lBucketCount)`).
    ///
    /// # Errors
    ///
    /// As [`HashTable::init_size`].
    pub fn new(item_estimate: usize, bucket_count: usize) -> Result<Self> {
        let mut t = Self::default();
        t.init_size(item_estimate, bucket_count)?;
        Ok(t)
    }

    /// Empties every bucket, sets `bucket_count` buckets and reserves item slots
    /// for `item_estimate` items (`InitSize`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `bucket_count` buckets cannot be allocated.
    pub fn init_size(&mut self, item_estimate: usize, bucket_count: usize) -> Result<()> {
        self.used_buckets = None;
        self.buckets.clear();
        self.buckets
            .try_reserve_exact(bucket_count)
            .map_err(|_| SolidError::InvalidInput {
                operation: "geom::hash_table::HashTable::init_size",
                reason: "bucket count too large",
            })?;
        self.buckets.resize(bucket_count, None);
        self.bucket_items.clear();
        self.capacity = 0;
        if item_estimate > 0 {
            self.capacity = grown_capacity(item_estimate);
        }
        self.used_bucket_count = 0;
        self.removed_bucket_item_count = 0;
        Ok(())
    }

    /// Grows the item capacity to `max(1.5 * capacity, capacity + 64)`
    /// (`IncMaxItemCount`).
    ///
    /// Upstream copies the item records into the new array only while some item
    /// is still in a bucket; otherwise the records come back zero-filled
    /// (bucket `0`, no item, id `0`, no links), which is reproduced here.
    pub fn inc_max_item_count(&mut self) {
        self.capacity = grown_capacity(self.capacity);
        if self.used_bucket_item_count() == 0 {
            self.bucket_items.fill(BucketItem {
                next_used_bucket: None,
                next_bucket_item: None,
                bucket_index: Some(0),
                item: None,
                item_id: 0,
            });
        }
    }

    /// Bucket heads (`GetBuckets`).
    #[must_use]
    pub fn buckets(&self) -> &[Option<usize>] {
        &self.buckets
    }

    /// Every item record, removed ones included (`GetBucketItems`).
    #[must_use]
    pub fn bucket_items(&self) -> &[BucketItem] {
        &self.bucket_items
    }

    /// Allocated item slots (`GetBucketItems().GetDataSize()`).
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// First item of the most recently used bucket (`GetUsedBucket(0)`).
    #[must_use]
    pub fn first_used_bucket(&self) -> Option<usize> {
        self.used_buckets
    }

    /// Number of buckets (`GetBucketCount`).
    #[must_use]
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// Number of used buckets (`GetUsedBucketCount`).
    #[must_use]
    pub fn used_bucket_count(&self) -> usize {
        self.used_bucket_count
    }

    /// Item records, removed ones included (`GetTotalBucketItemCount`).
    #[must_use]
    pub fn total_bucket_item_count(&self) -> usize {
        self.bucket_items.len()
    }

    /// Items still in buckets (`GetUsedBucketItemCount`; wraps like the
    /// upstream `ULONG`).
    #[must_use]
    pub fn used_bucket_item_count(&self) -> usize {
        self.bucket_items
            .len()
            .wrapping_sub(self.removed_bucket_item_count)
    }

    /// Removed item records (`GetRemovedBucketItemCount`).
    #[must_use]
    pub fn removed_bucket_item_count(&self) -> usize {
        self.removed_bucket_item_count
    }

    /// Adds `item` to bucket `bucket_index` with id `item_id` (`Add`): the first
    /// item of an empty bucket is pushed on the used-bucket list, later items go
    /// second in their bucket.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `bucket_index` is not a bucket (undefined
    /// behaviour upstream).
    pub fn add(&mut self, item: usize, bucket_index: usize, item_id: usize) -> Result<()> {
        let Some(&head) = self.buckets.get(bucket_index) else {
            return Err(SolidError::InvalidInput {
                operation: "geom::hash_table::HashTable::add",
                reason: "bucket index out of range",
            });
        };
        if self.bucket_items.len() == self.capacity {
            self.inc_max_item_count();
        }
        let new = self.bucket_items.len();
        let mut rec = BucketItem {
            next_used_bucket: None,
            next_bucket_item: None,
            bucket_index: Some(bucket_index),
            item: Some(item),
            item_id,
        };
        match head {
            None => {
                rec.next_used_bucket = self.used_buckets;
                self.used_buckets = Some(new);
                self.used_bucket_count = self.used_bucket_count.wrapping_add(1);
                rec.next_bucket_item = None;
                if let Some(b) = self.buckets.get_mut(bucket_index) {
                    *b = Some(new);
                }
            }
            Some(h) => {
                if let Some(hr) = self.bucket_items.get_mut(h) {
                    rec.next_bucket_item = hr.next_bucket_item;
                    hr.next_bucket_item = Some(new);
                }
                rec.next_used_bucket = None;
            }
        }
        self.bucket_items.push(rec);
        Ok(())
    }

    fn rec(&mut self, i: usize) -> Option<&mut BucketItem> {
        self.bucket_items.get_mut(i)
    }

    /// Unlinks the items at `indices` from their buckets and marks them removed
    /// (`Remove`); already removed (or out-of-range) entries are skipped.
    /// Returns the number of items removed.
    pub fn remove(&mut self, indices: &[usize]) -> usize {
        let mut removed = 0usize;
        for &idx in indices {
            let Some(bi) = self.bucket_items.get(idx).copied() else {
                continue;
            };
            let Some(b) = bi.bucket_index.filter(|&b| b < self.buckets.len()) else {
                continue;
            };
            let head = self.buckets.get(b).copied().flatten();
            if head != Some(idx) {
                // linear search of the bucket for the item pointing at idx
                let mut cur = head;
                while let Some(c) = cur {
                    let Some(cr) = self.rec(c) else { break };
                    if cr.next_bucket_item == Some(idx) {
                        cr.next_bucket_item = bi.next_bucket_item;
                        break;
                    }
                    cur = cr.next_bucket_item;
                }
            } else if self.used_buckets == Some(idx) {
                if let Some(n) = bi.next_bucket_item {
                    if let Some(nr) = self.rec(n) {
                        nr.next_used_bucket = bi.next_used_bucket;
                    }
                    self.used_buckets = Some(n);
                    if let Some(s) = self.buckets.get_mut(b) {
                        *s = Some(n);
                    }
                } else {
                    self.used_buckets = bi.next_used_bucket;
                    if let Some(s) = self.buckets.get_mut(b) {
                        *s = None;
                    }
                    self.used_bucket_count = self.used_bucket_count.wrapping_sub(1);
                }
            } else {
                // linear search of the used-bucket list for the bucket before idx
                let mut cur = self.used_buckets;
                while let Some(c) = cur {
                    let Some(cr) = self.bucket_items.get(c).copied() else {
                        break;
                    };
                    if cr.next_used_bucket == Some(idx) {
                        if let Some(n) = bi.next_bucket_item {
                            if let Some(nr) = self.rec(n) {
                                nr.next_used_bucket = bi.next_used_bucket;
                            }
                            if let Some(pr) = self.rec(c) {
                                pr.next_used_bucket = Some(n);
                            }
                            if let Some(s) = self.buckets.get_mut(b) {
                                *s = Some(n);
                            }
                        } else {
                            if let Some(pr) = self.rec(c) {
                                pr.next_used_bucket = bi.next_used_bucket;
                            }
                            if let Some(s) = self.buckets.get_mut(b) {
                                *s = None;
                            }
                            self.used_bucket_count = self.used_bucket_count.wrapping_sub(1);
                        }
                        break;
                    }
                    cur = cr.next_used_bucket;
                }
            }
            self.removed_bucket_item_count = self.removed_bucket_item_count.wrapping_add(1);
            removed += 1;
            if let Some(r) = self.rec(idx) {
                r.next_used_bucket = None;
                r.next_bucket_item = None;
                r.bucket_index = None;
            }
        }
        removed
    }
}

/// An item placed into a voxel with its position and tolerance (`SmPointItem`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointItem<T> {
    /// The hashed object (`m_sTgtItem`).
    pub item: T,
    /// Its position (`m_sTgtPoint3d`; `Vec3::UNDEF` once removed).
    pub point: Point3,
    /// Its zone tolerance (`m_sTgtZoneTol3d`; `SM_UNDEF_DOUBLE` once removed).
    pub zone_tol: f64,
}

/// The voxel a point maps to ([`ObjsInVoxels::map_point_to_voxel`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoxelHit {
    /// Voxel (bucket) index `iz * (nx * ny) + iy * nx + ix`.
    pub index: usize,
    /// The voxel's box (`pOptFoundVoxelExtent3d`).
    pub extent: Extent3d,
    /// Voxel x index.
    pub ix: usize,
    /// Voxel y index.
    pub iy: usize,
    /// Voxel z index.
    pub iz: usize,
}

/// Options of [`ObjsInVoxels::find_coincident_pairs`].
#[derive(Debug, Default)]
pub struct FindPairsOptions<'a> {
    /// Skip pairs whose two items both have an id (point-item index) below this
    /// (`pOptLimitIndx`; checked on a previous call); `None` checks all pairs.
    pub limit_index: Option<usize>,
    /// Buckets to check (`pOptBucketIndices`); `None` walks every used bucket.
    pub buckets: Option<&'a [usize]>,
    /// Gap-size histogram accumulator (`pOptHistogram`).
    pub histogram: Option<&'a mut [u64; GAP_HISTOGRAM_BUCKETS]>,
}

/// Coincident pairs found by [`ObjsInVoxels::find_coincident_pairs`], as
/// point-item indices (`rCoin1`, `rCoin2`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoincidentPairs {
    /// First member of each pair.
    pub first: Vec<usize>,
    /// Second member of each pair.
    pub second: Vec<usize>,
}

/// Items hashed into a regular grid of voxels by position; an item within its
/// tolerance of a voxel side is also entered in the neighbouring voxel, so
/// coincident items always share a voxel (`SmObjsInVoxels`).
#[derive(Debug, Clone)]
pub struct ObjsInVoxels<T> {
    /// One bucket per voxel (`m_sHashTable`).
    hash: HashTable,
    /// Voxels along x (`m_lXCnt`).
    x_cnt: usize,
    /// Voxels along y (`m_lYCnt`).
    y_cnt: usize,
    /// Voxels along z (`m_lZCnt`).
    z_cnt: usize,
    /// Box of all voxels (`m_sBBox`).
    bbox: Extent3d,
    /// Removed point items (`m_lRemovedPointItemCount`; never reset, as upstream).
    removed_point_item_count: usize,
    /// One entry per (item, voxel) placement (`m_sPointItems`).
    point_items: Vec<PointItem<T>>,
    /// Allocated point-item slots (`m_sPointItems.GetDataSize()`).
    point_capacity: usize,
}

impl<T> Default for ObjsInVoxels<T> {
    /// An unsized, empty model (`SetSize(0, 0, 0, NULL, 0)`).
    fn default() -> Self {
        Self {
            hash: HashTable::default(),
            x_cnt: 0,
            y_cnt: 0,
            z_cnt: 0,
            bbox: Extent3d::new(),
            removed_point_item_count: 0,
            point_items: Vec::new(),
            point_capacity: 0,
        }
    }
}

impl<T: Copy + PartialEq> ObjsInVoxels<T> {
    /// A model of `x_cnt * y_cnt * z_cnt` voxels over `bbox` (`None`: not sized)
    /// with room for `item_estimate` items (`SmObjsInVoxels(...)`).
    ///
    /// # Errors
    ///
    /// As [`ObjsInVoxels::set_size`] (one bucket per voxel).
    pub fn new(
        x_cnt: usize,
        y_cnt: usize,
        z_cnt: usize,
        bbox: Option<&Extent3d>,
        item_estimate: usize,
    ) -> Result<Self> {
        let mut v = Self {
            hash: HashTable::default(),
            x_cnt: 0,
            y_cnt: 0,
            z_cnt: 0,
            bbox: Extent3d::new(),
            removed_point_item_count: 0,
            point_items: Vec::new(),
            point_capacity: 0,
        };
        v.set_size(x_cnt, y_cnt, z_cnt, bbox, item_estimate)?;
        Ok(v)
    }

    fn voxel_count(&self) -> usize {
        self.x_cnt.wrapping_mul(self.y_cnt).wrapping_mul(self.z_cnt)
    }

    /// Empties the model and redefines its voxels (`SetSize`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the voxel dimensions overflow, leaving
    /// the model unchanged. Otherwise, as [`HashTable::init_size`] (one bucket
    /// per voxel).
    pub fn set_size(
        &mut self,
        x_cnt: usize,
        y_cnt: usize,
        z_cnt: usize,
        bbox: Option<&Extent3d>,
        item_estimate: usize,
    ) -> Result<()> {
        // A zero factor makes the mathematical voxel product zero; preserve the
        // pre-existing behaviour (upstream's wrapped product is also zero) instead of
        // letting an intermediate overflow reject it.
        let voxel_count = if x_cnt == 0 || y_cnt == 0 || z_cnt == 0 {
            0
        } else {
            x_cnt
                .checked_mul(y_cnt)
                .and_then(|xy| xy.checked_mul(z_cnt))
                .ok_or(SolidError::InvalidInput {
                    operation: "geom::hash_table::ObjsInVoxels::set_size",
                    reason: "voxel dimensions overflow",
                })?
        };
        self.x_cnt = x_cnt;
        self.y_cnt = y_cnt;
        self.z_cnt = z_cnt;
        self.bbox = bbox.copied().unwrap_or_default();
        self.hash.init_size(item_estimate, voxel_count)?;
        self.point_items.clear();
        self.point_capacity = 0;
        if item_estimate > 0 {
            self.point_capacity = self.hash.capacity();
        }
        Ok(())
    }

    /// Grows the point-item and bucket-item capacities together
    /// (`IncMaxItemCount`).
    fn inc_max_item_count(&mut self) {
        self.hash.inc_max_item_count();
        self.point_capacity = self.hash.capacity();
    }

    /// The placed items (`GetPointItems`).
    #[must_use]
    pub fn point_items(&self) -> &[PointItem<T>] {
        &self.point_items
    }

    /// Number of placed items, removed ones included (`GetSize`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.point_items.len()
    }

    /// True when no item was placed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.point_items.is_empty()
    }

    /// The voxel bucket table (`m_sHashTable`).
    #[must_use]
    pub fn hash_table(&self) -> &HashTable {
        &self.hash
    }

    /// Voxel counts along x, y and z.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.x_cnt, self.y_cnt, self.z_cnt)
    }

    /// Box of all voxels.
    #[must_use]
    pub fn bbox(&self) -> &Extent3d {
        &self.bbox
    }

    /// Removed point items (`m_lRemovedPointItemCount`).
    #[must_use]
    pub fn removed_point_item_count(&self) -> usize {
        self.removed_point_item_count
    }

    /// Allocated point-item slots (`GetPointItems().GetDataSize()`).
    #[must_use]
    pub fn point_capacity(&self) -> usize {
        self.point_capacity
    }

    /// Voxel index of voxel `(ix, iy, iz)` (`MapIndicesToVoxelIndex`).
    #[must_use]
    pub fn map_indices_to_voxel_index(&self, ix: usize, iy: usize, iz: usize) -> usize {
        iz.wrapping_mul(self.x_cnt.wrapping_mul(self.y_cnt))
            .wrapping_add(iy.wrapping_mul(self.x_cnt))
            .wrapping_add(ix)
    }

    /// `(ix, iy, iz)` of voxel `index` (`MapVoxelIndexToIndices`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `x_cnt * y_cnt` is zero (a division by zero
    /// upstream).
    pub fn map_voxel_index_to_indices(&self, index: usize) -> Result<(usize, usize, usize)> {
        let xy = self.x_cnt.wrapping_mul(self.y_cnt);
        if xy == 0 {
            return Err(SolidError::InvalidInput {
                operation: "geom::hash_table::ObjsInVoxels::map_voxel_index_to_indices",
                reason: "no voxels in x or y",
            });
        }
        let iz = index / xy;
        let iy = index.wrapping_sub(iz.wrapping_mul(xy)) / self.x_cnt;
        let ix = index
            .wrapping_sub(iz.wrapping_mul(xy))
            .wrapping_sub(iy.wrapping_mul(self.x_cnt));
        Ok((ix, iy, iz))
    }

    /// The voxel containing `p` (`MapPointToVoxel`); points outside the box map
    /// to a boundary voxel. `None` when the model has no box or no voxels.
    #[must_use]
    pub fn map_point_to_voxel(&self, p: Point3) -> Option<VoxelHit> {
        if self.bbox.is_init() || self.voxel_count() == 0 {
            return None;
        }
        let dx = self.bbox.x_length() / self.x_cnt as f64;
        let dy = self.bbox.y_length() / self.y_cnt as f64;
        let dz = self.bbox.z_length() / self.z_cnt as f64;
        let mut ix = ulong_from_f64((p.x - self.bbox.min.x) / dx);
        let mut iy = ulong_from_f64((p.y - self.bbox.min.y) / dy);
        let mut iz = ulong_from_f64((p.z - self.bbox.min.z) / dz);
        if ix >= self.x_cnt as u64 {
            ix = self.x_cnt as u64 - 1;
        }
        if iy >= self.y_cnt as u64 {
            iy = self.y_cnt as u64 - 1;
        }
        if iz >= self.z_cnt as u64 {
            iz = self.z_cnt as u64 - 1;
        }
        let (ix, iy, iz) = (ix as usize, iy as usize, iz as usize);
        let index = self.map_indices_to_voxel_index(ix, iy, iz);
        Some(VoxelHit {
            index,
            extent: self.map_voxel_index_to_bbox(index),
            ix,
            iy,
            iz,
        })
    }

    /// Box of voxel `index` (`MapVoxelIndexToBBox`); the unsized
    /// [`Extent3d::new`] when the model has no box or no voxels, or when the
    /// computed corners are out of order (upstream ignores the `SetMinMax`
    /// failure).
    #[must_use]
    pub fn map_voxel_index_to_bbox(&self, index: usize) -> Extent3d {
        let mut e = Extent3d::new();
        if self.bbox.is_init() || self.voxel_count() == 0 {
            return e;
        }
        let dx = self.bbox.x_length() / self.x_cnt as f64;
        let dy = self.bbox.y_length() / self.y_cnt as f64;
        let dz = self.bbox.z_length() / self.z_cnt as f64;
        let Ok((ix, iy, iz)) = self.map_voxel_index_to_indices(index) else {
            return e;
        };
        let (ix, iy, iz) = (ix as f64, iy as f64, iz as f64);
        let m = self.bbox.min;
        let _ = e.set_min_max_coords(
            m.x + ix * dx,
            m.y + iy * dy,
            m.z + iz * dz,
            m.x + (ix + 1.0) * dx,
            m.y + (iy + 1.0) * dy,
            m.z + (iz + 1.0) * dz,
        );
        e
    }

    /// Indices of every point item (removed ones included) holding `item`
    /// (`FindPointItems`).
    #[must_use]
    pub fn find_point_items(&self, item: T) -> Vec<usize> {
        self.point_items
            .iter()
            .enumerate()
            .filter(|(_, pi)| pi.item == item)
            .map(|(i, _)| i)
            .collect()
    }

    /// Places `item` at `point` with zone tolerance `zone_tol` into its voxel,
    /// and into each neighbouring voxel whose shared side is within `zone_tol`
    /// (`Add`). Returns the voxel index, or `None` when the model has no box.
    /// The voxels used are added (once) to `updated`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the model has a box but no voxels
    /// (undefined behaviour upstream).
    pub fn add(
        &mut self,
        item: T,
        point: Point3,
        zone_tol: f64,
        mut updated: Option<&mut Vec<usize>>,
    ) -> Result<Option<usize>> {
        if self.bbox.is_init() {
            return Ok(None);
        }
        let Some(hit) = self.map_point_to_voxel(point) else {
            return Err(SolidError::InvalidInput {
                operation: "geom::hash_table::ObjsInVoxels::add",
                reason: "voxel model has no voxels",
            });
        };
        while self.point_items.len() + MAX_ITEMS_PER_ADD > self.point_capacity {
            self.inc_max_item_count();
        }
        let v = hit.index;
        let pi = PointItem {
            item,
            point,
            zone_tol,
        };
        self.place(pi, v, &mut updated)?;
        let (on_boundary, inward) = hit.extent.is_point_on_boundary(point, zone_tol);
        if on_boundary {
            let xy = self.x_cnt.wrapping_mul(self.y_cnt);
            if inward.x == -1.0 && hit.ix < self.x_cnt.wrapping_sub(1) {
                self.place(pi, v.wrapping_add(1), &mut updated)?;
            }
            if inward.x == 1.0 && hit.ix > 0 {
                self.place(pi, v.wrapping_sub(1), &mut updated)?;
            }
            if inward.y == -1.0 && hit.iy < self.y_cnt.wrapping_sub(1) {
                self.place(pi, v.wrapping_add(self.x_cnt), &mut updated)?;
            }
            if inward.y == 1.0 && hit.iy > 0 {
                self.place(pi, v.wrapping_sub(self.x_cnt), &mut updated)?;
            }
            if inward.z == -1.0 && hit.iz < self.z_cnt.wrapping_sub(1) {
                self.place(pi, v.wrapping_add(xy), &mut updated)?;
            }
            if inward.z == 1.0 && hit.iz > 0 {
                self.place(pi, v.wrapping_sub(xy), &mut updated)?;
            }
        }
        Ok(Some(v))
    }

    /// Appends one point item and enters it in `voxel`.
    fn place(
        &mut self,
        pi: PointItem<T>,
        voxel: usize,
        updated: &mut Option<&mut Vec<usize>>,
    ) -> Result<()> {
        self.point_items.push(pi);
        let idx = self.point_items.len() - 1;
        self.hash.add(idx, voxel, idx)?;
        if let Some(u) = updated.as_deref_mut() {
            add_unique(u, voxel);
        }
        Ok(())
    }

    /// Takes every placement of `item` out of its voxel and marks its point
    /// items removed (point `Vec3::UNDEF`, tolerance `SM_UNDEF_DOUBLE`); the
    /// records stay in place (`Remove`). Returns the number of bucket items
    /// removed.
    pub fn remove(&mut self, item: T) -> usize {
        let found = self.find_point_items(item);
        let n = self.hash.remove(&found);
        self.removed_point_item_count = self.removed_point_item_count.wrapping_add(n);
        for i in found {
            if let Some(pi) = self.point_items.get_mut(i) {
                pi.point.set_uninitialized();
                pi.zone_tol = UNDEF_DOUBLE;
            }
        }
        n
    }

    /// Pairs of point items sharing a voxel whose distance is below the
    /// intersection tolerance `SmXSectTol3d(tolA + tolB)` (compared squared),
    /// each pair of items listed once (`FindCoincidentPairs`).
    ///
    /// Every pair of items within a bucket is compared, except, with
    /// `opts.limit_index`, a pair whose two items are both below it. The histogram,
    /// when given, accumulates every gap compared.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `opts.buckets` names a bucket index past the
    /// table (undefined behaviour upstream).
    pub fn find_coincident_pairs(
        &mut self,
        k: &Kernel,
        opts: FindPairsOptions<'_>,
    ) -> Result<CoincidentPairs> {
        let FindPairsOptions {
            limit_index,
            buckets,
            mut histogram,
        } = opts;
        let mut out = CoincidentPairs::default();
        let mut heads: Vec<Option<usize>> = Vec::new();
        if let Some(list) = buckets {
            for &b in list {
                let Some(&h) = self.hash.buckets.get(b) else {
                    return Err(SolidError::InvalidInput {
                        operation: "geom::hash_table::ObjsInVoxels::find_coincident_pairs",
                        reason: "bucket index out of range",
                    });
                };
                heads.push(h);
            }
        } else {
            let mut cur = self.hash.used_buckets;
            while let Some(c) = cur {
                heads.push(Some(c));
                cur = self
                    .hash
                    .bucket_items
                    .get(c)
                    .and_then(|r| r.next_used_bucket);
            }
        }
        for head in heads {
            let mut beg = head.and_then(|h| self.hash.bucket_items.get(h).copied());
            while let Some(b) = beg {
                beg = b
                    .next_bucket_item
                    .and_then(|n| self.hash.bucket_items.get(n).copied());
                // this beg entry's point item
                let Some(beg_pi) = b.item else {
                    continue;
                };
                let mut end = b
                    .next_bucket_item
                    .and_then(|n| self.hash.bucket_items.get(n).copied());
                while let Some(e) = end {
                    end = e
                        .next_bucket_item
                        .and_then(|n| self.hash.bucket_items.get(n).copied());
                    // when asked - skip pairs with both point items below the limit index
                    // (presumably checked for coincidence on a previous call)
                    if limit_index.is_some_and(|lim| b.item_id < lim && e.item_id < lim) {
                        continue;
                    }
                    let Some(end_pi) = e.item else {
                        continue;
                    };
                    let (Some(end_item), Some(beg_item)) = (
                        self.point_items.get(end_pi).copied(),
                        self.point_items.get(beg_pi).copied(),
                    ) else {
                        continue;
                    };
                    let xs = xsect_tol3d(k, beg_item.zone_tol + end_item.zone_tol);
                    let xs_sq = xsect_tol3d(k, xs * xs);
                    let gap_sq = (beg_item.point - end_item.point).length_squared();
                    if let Some(h) = histogram.as_deref_mut() {
                        if let Some(c) = h.get_mut(gap_histogram_index(gap_sq.sqrt())) {
                            *c = c.wrapping_add(1);
                        }
                    }
                    if gap_sq < xs_sq {
                        let mut known = false;
                        for (&c1, &c2) in out.first.iter().zip(&out.second) {
                            let (Some(i1), Some(i2)) = (
                                self.point_items.get(c1).map(|p| p.item),
                                self.point_items.get(c2).map(|p| p.item),
                            ) else {
                                continue;
                            };
                            if i1 != beg_item.item && i1 != end_item.item {
                                continue;
                            }
                            if (i1 == beg_item.item && i2 == end_item.item)
                                || (i2 == beg_item.item && i1 == end_item.item)
                            {
                                known = true;
                                break;
                            }
                        }
                        if !known {
                            out.first.push(beg_pi);
                            out.second.push(end_pi);
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::KernelOptions;
    use crate::math::Vec3;

    fn unit_box() -> Extent3d {
        Extent3d::from_bounds(0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
    }

    #[test]
    fn new_rejects_voxel_dimension_overflow() {
        assert!(matches!(
            ObjsInVoxels::<usize>::new(usize::MAX, usize::MAX, 1, None, 0),
            Err(SolidError::InvalidInput {
                operation: "geom::hash_table::ObjsInVoxels::set_size",
                reason: "voxel dimensions overflow",
            })
        ));
    }

    #[test]
    fn new_keeps_zero_dimension_success_despite_intermediate_overflow() {
        // pre-existing behaviour: a zero factor makes the product zero, so even
        // MAX x MAX x 0 succeeds with zero buckets instead of rejecting
        let v = ObjsInVoxels::<usize>::new(usize::MAX, usize::MAX, 0, None, 0).unwrap();
        assert_eq!(v.counts(), (usize::MAX, usize::MAX, 0));
        assert_eq!(v.hash_table().bucket_count(), 0);
    }

    #[test]
    fn new_rejects_third_voxel_factor_overflow() {
        assert!(matches!(
            ObjsInVoxels::<usize>::new(1, usize::MAX, 2, None, 0),
            Err(SolidError::InvalidInput {
                operation: "geom::hash_table::ObjsInVoxels::set_size",
                reason: "voxel dimensions overflow",
            })
        ));
    }

    #[test]
    fn overflowing_set_size_preserves_model() {
        let mut v = ObjsInVoxels::<usize>::new(2, 3, 4, Some(&unit_box()), 0).unwrap();
        v.add(7, Vec3::new(0.2, 0.2, 0.2), 0.0, None).unwrap();
        let before = v.clone();

        assert!(matches!(
            v.set_size(usize::MAX, usize::MAX, 1, None, 10),
            Err(SolidError::InvalidInput {
                operation: "geom::hash_table::ObjsInVoxels::set_size",
                reason: "voxel dimensions overflow",
            })
        ));
        assert_eq!(v.counts(), before.counts());
        assert_eq!(v.bbox(), before.bbox());
        assert_eq!(v.hash.bucket_count(), before.hash.bucket_count());
        assert_eq!(v.hash.buckets, before.hash.buckets);
        assert_eq!(v.hash.bucket_items, before.hash.bucket_items);
        assert_eq!(v.hash.capacity, before.hash.capacity);
        assert_eq!(v.hash.used_buckets, before.hash.used_buckets);
        assert_eq!(v.hash.used_bucket_count, before.hash.used_bucket_count);
        assert_eq!(
            v.hash.removed_bucket_item_count,
            before.hash.removed_bucket_item_count
        );
        assert_eq!(v.point_items(), before.point_items());
        assert_eq!(v.point_capacity(), before.point_capacity());
        assert_eq!(
            v.removed_point_item_count(),
            before.removed_point_item_count()
        );
    }

    #[test]
    fn voxel_dimensions_match_bucket_count() {
        let v = ObjsInVoxels::<usize>::new(2, 3, 4, None, 0).unwrap();
        assert_eq!(v.counts(), (2, 3, 4));
        assert_eq!(v.hash_table().bucket_count(), 24);
    }

    #[test]
    fn ulong_conversion_matches_x86_gcc() {
        assert_eq!(ulong_from_f64(-0.5), 0);
        assert_eq!(ulong_from_f64(-3.2), u64::MAX - 2);
        assert_eq!(ulong_from_f64(f64::NAN), 1 << 63);
        assert_eq!(ulong_from_f64(f64::INFINITY), 0);
        assert_eq!(ulong_from_f64(f64::NEG_INFINITY), 1 << 63);
        assert_eq!(ulong_from_f64(1e19), 10_000_000_000_000_000_000);
        assert_eq!(ulong_from_f64(2e19), 0);
        assert_eq!(ulong_from_f64(3.9), 3);
    }

    #[test]
    fn histogram_index() {
        assert_eq!(gap_histogram_index(0.0), 0);
        assert_eq!(gap_histogram_index(1e-9), 1);
        assert_eq!(gap_histogram_index(1e-7), 2);
        assert_eq!(gap_histogram_index(0.5), 8);
        assert_eq!(gap_histogram_index(10.0), 10);
        assert_eq!(gap_histogram_index(11.0), 10);
    }

    #[test]
    fn map_point_and_bbox() {
        let v: ObjsInVoxels<u32> = ObjsInVoxels::new(4, 2, 1, Some(&unit_box()), 0).unwrap();
        let h = v.map_point_to_voxel(Vec3::new(0.6, 0.7, 0.5)).unwrap();
        assert_eq!((h.ix, h.iy, h.iz, h.index), (2, 1, 0, 6));
        assert_eq!(
            h.extent,
            Extent3d::from_bounds(0.5, 0.5, 0.0, 0.75, 1.0, 1.0)
        );
        // below the box: the unsigned conversion wraps, then clamps high
        let h = v.map_point_to_voxel(Vec3::new(-2.0, 0.1, 0.1)).unwrap();
        assert_eq!(h.ix, 3);
        assert_eq!(v.map_voxel_index_to_indices(6).unwrap(), (2, 1, 0));
        let u: ObjsInVoxels<u32> = ObjsInVoxels::new(2, 2, 2, None, 0).unwrap();
        assert!(u.map_point_to_voxel(Vec3::ZERO).is_none());
        assert!(u.map_voxel_index_to_bbox(0).is_init());
    }

    #[test]
    fn add_neighbours_and_find_pairs() {
        let k = Kernel::new(&KernelOptions::default());
        let mut v: ObjsInVoxels<u32> = ObjsInVoxels::new(2, 1, 1, Some(&unit_box()), 0).unwrap();
        let mut upd = Vec::new();
        assert_eq!(
            v.add(1, Vec3::new(0.5, 0.5, 0.5), 1e-3, Some(&mut upd))
                .unwrap(),
            Some(1)
        );
        assert_eq!(upd, vec![1, 0]);
        assert_eq!(v.len(), 2);
        assert_eq!(v.point_capacity(), 64);
        v.add(2, Vec3::new(0.2, 0.5, 0.5), 1e-3, None).unwrap();
        v.add(3, Vec3::new(0.2005, 0.5, 0.5), 1e-3, None).unwrap();
        let p = v
            .find_coincident_pairs(&k, FindPairsOptions::default())
            .unwrap();
        // bucket 0 holds [item1, item3, item2]: every pair is compared, item3 and
        // item2 coincide
        assert_eq!(p.first.len(), 1);
        assert_eq!(v.hash_table().used_bucket_count(), 2);
        assert_eq!(v.remove(1), 2);
        assert_eq!(v.removed_point_item_count(), 2);
        assert_eq!(v.hash_table().removed_bucket_item_count(), 2);
        assert_eq!(v.hash_table().used_bucket_count(), 1);
        let mut hist = [0u64; GAP_HISTOGRAM_BUCKETS];
        let p = v
            .find_coincident_pairs(
                &k,
                FindPairsOptions {
                    histogram: Some(&mut hist),
                    ..FindPairsOptions::default()
                },
            )
            .unwrap();
        assert_eq!(p.first.len(), 1);
        assert_eq!(hist.iter().sum::<u64>(), 1);
        assert!(v.add(9, Vec3::ZERO, 0.0, None).is_ok());
    }

    #[test]
    fn unsized_model_ignores_adds() {
        let mut v: ObjsInVoxels<u32> = ObjsInVoxels::new(0, 0, 0, None, 5).unwrap();
        assert_eq!(v.add(1, Vec3::ZERO, 0.1, None).unwrap(), None);
        assert_eq!(v.point_capacity(), 69);
        let mut w: ObjsInVoxels<u32> = ObjsInVoxels::new(0, 1, 1, Some(&unit_box()), 0).unwrap();
        assert!(w.add(1, Vec3::ZERO, 0.1, None).is_err());
    }
}
