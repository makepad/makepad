//! A hash-map that behaves deterministically when the
//! `enhanced-determinism` feature is enabled.

#[cfg(all(feature = "enhanced-determinism", feature = "serde-serialize"))]
use indexmap::IndexSet as StdHashSet;
#[cfg(all(not(feature = "enhanced-determinism"), feature = "serde-serialize"))]
use std::collections::HashSet as StdHashSet;

/// Serializes only the capacity of a hash-set instead of its actual content.
#[cfg(feature = "serde-serialize")]
pub fn serialize_hashset_capacity<S: serde::Serializer, K, H: core::hash::BuildHasher>(
    set: &StdHashSet<K, H>,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_u64(set.capacity() as u64)
}

/// Creates a new hash-set with its capacity deserialized from `d`.
#[cfg(feature = "serde-serialize")]
pub fn deserialize_hashset_capacity<
    'de,
    D: serde::Deserializer<'de>,
    K,
    V,
    H: core::hash::BuildHasher + Default,
>(
    d: D,
) -> Result<StdHashSet<K, H>, D::Error> {
    struct CapacityVisitor;
    impl serde::de::Visitor<'_> for CapacityVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
            write!(formatter, "an integer between 0 and 2^64")
        }

        fn visit_u64<E: serde::de::Error>(self, val: u64) -> Result<Self::Value, E> {
            Ok(val)
        }
    }

    let capacity = d.deserialize_u64(CapacityVisitor)? as usize;
    Ok(StdHashSet::with_capacity_and_hasher(
        capacity,
        Default::default(),
    ))
}

/// Deterministic hashset using [`indexmap::IndexSet`]
#[cfg(feature = "enhanced-determinism")]
pub type FxHashSet32<K> =
    indexmap::IndexSet<K, core::hash::BuildHasherDefault<super::fx_hasher::FxHasher32>>;
#[cfg(feature = "enhanced-determinism")]
pub use self::FxHashSet32 as HashSet;

#[cfg(not(feature = "enhanced-determinism"))]
pub use hashbrown::hash_set::Entry;
/// Hashset using [`hashbrown::HashSet`]
#[cfg(not(feature = "enhanced-determinism"))]
pub type HashSet<K> = hashbrown::hash_set::HashSet<K, foldhash::fast::FixedState>;
