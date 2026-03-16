//! A hash-map that behaves deterministically when the
//! `enhanced-determinism` feature is enabled.

#[cfg(all(not(feature = "enhanced-determinism"), feature = "serde-serialize"))]
use hashbrown::hash_map::HashMap as StdHashMap;
#[cfg(all(feature = "enhanced-determinism", feature = "serde-serialize"))]
use indexmap::IndexMap as StdHashMap;

/// Serializes only the capacity of a hash-map instead of its actual content.
#[cfg(feature = "serde-serialize")]
pub fn serialize_hashmap_capacity<S: serde::Serializer, K, V, H: core::hash::BuildHasher>(
    map: &StdHashMap<K, V, H>,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_u64(map.capacity() as u64)
}

/// Creates a new hash-map with its capacity deserialized from `d`.
#[cfg(feature = "serde-serialize")]
pub fn deserialize_hashmap_capacity<
    'de,
    D: serde::Deserializer<'de>,
    K,
    V,
    H: core::hash::BuildHasher + Default,
>(
    d: D,
) -> Result<StdHashMap<K, V, H>, D::Error> {
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
    Ok(StdHashMap::with_capacity_and_hasher(
        capacity,
        Default::default(),
    ))
}

/*
 * FxHasher taken from rustc_hash, except that it does not depend on the pointer size.
 */
/// Deterministic hashmap using [`indexmap::IndexMap`]
#[cfg(feature = "enhanced-determinism")]
pub type FxHashMap32<K, V> =
    indexmap::IndexMap<K, V, core::hash::BuildHasherDefault<super::fx_hasher::FxHasher32>>;
#[cfg(feature = "enhanced-determinism")]
pub use {self::FxHashMap32 as HashMap, indexmap::map::Entry};

#[cfg(not(feature = "enhanced-determinism"))]
pub use hashbrown::hash_map::Entry;
/// Hashmap using [`hashbrown::HashMap`]
#[cfg(not(feature = "enhanced-determinism"))]
pub type HashMap<K, V> = hashbrown::hash_map::HashMap<K, V, foldhash::fast::FixedState>;
