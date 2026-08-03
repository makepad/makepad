//! Generation cache.
//!
//! A forest is hundreds of instances drawn from a handful of distinct meshes.
//! Keying on the full recipe — preset, seed, LOD and every knob — means 200
//! trees sharing 6 seeds cost 6 generations, and an unchanged world costs
//! none at all.
//!
//! The key is a hash of the recipe rather than the recipe itself, so callers
//! can mix trees, blobs and tracks in one cache without a common parameter
//! type. Hashing is FNV-1a over the knobs' exact bit patterns: two f32s that
//! differ in the last bit are genuinely different meshes.

use crate::implicit::{blob, BlobParams};
use crate::mesh::GenMesh;
use crate::tree::{tree, Lod, TreeParams};
use std::collections::HashMap;
use std::rc::Rc;

/// Identifies a generated mesh. Equal keys must mean identical geometry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GenKey(pub u64);

struct Hasher(u64);

impl Hasher {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        // FNV-1a, byte at a time.
        for b in v.to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x1000_0000_01b3);
        }
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.u64(v as u64)
    }
    /// Hash a float by its bits, so 0.1 and 0.1000001 are distinct recipes.
    fn f32(&mut self, v: f32) -> &mut Self {
        // Normalise the two zeroes so -0.0 and 0.0 share a cache entry; they
        // produce identical geometry and would otherwise double the work.
        let v = if v == 0.0 { 0.0 } else { v };
        self.u32(v.to_bits())
    }
    fn str(&mut self, s: &str) -> &mut Self {
        for b in s.as_bytes() {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x1000_0000_01b3);
        }
        self.u64(s.len() as u64)
    }
    fn done(&self) -> GenKey {
        GenKey(self.0)
    }
}

fn lod_id(l: Lod) -> u32 {
    match l {
        Lod::Low => 0,
        Lod::Medium => 1,
        Lod::High => 2,
    }
}

/// Key for a tree. Covers every knob that changes the mesh.
pub fn tree_key(species: &str, p: &TreeParams) -> GenKey {
    let mut h = Hasher::new();
    h.str("tree")
        .str(species)
        .u64(p.seed)
        .f32(p.height)
        .f32(p.bushiness)
        .f32(p.lean)
        .u32(lod_id(p.lod));
    for c in p.bark.iter().chain(p.foliage.iter()) {
        h.f32(*c);
    }
    h.done()
}

/// Key for a blob preset.
pub fn blob_key(kind: &str, p: &BlobParams) -> GenKey {
    let mut h = Hasher::new();
    h.str("blob")
        .str(kind)
        .u64(p.seed)
        .f32(p.size)
        .u32(p.resolution as u32);
    for c in p.color.iter() {
        h.f32(*c);
    }
    h.done()
}

/// Bounded cache of generated meshes.
///
/// Meshes are handed out as `Rc` so a hundred instances share one allocation;
/// eviction cannot pull geometry out from under a frame that is mid-draw.
pub struct GenCache {
    entries: HashMap<GenKey, (Rc<GenMesh>, u64)>,
    /// Monotonic counter standing in for time — deterministic, and generation
    /// order is the only recency signal that matters here.
    clock: u64,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
    hits: u64,
    misses: u64,
}

impl Default for GenCache {
    fn default() -> Self {
        Self::new(256, 32 << 20)
    }
}

impl GenCache {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1 << 16),
            bytes: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// Fetch or generate. `build` runs only on a miss.
    pub fn get_or_build(&mut self, key: GenKey, build: impl FnOnce() -> GenMesh) -> Rc<GenMesh> {
        self.clock += 1;
        if let Some((mesh, last)) = self.entries.get_mut(&key) {
            *last = self.clock;
            self.hits += 1;
            return mesh.clone();
        }
        self.misses += 1;
        let mesh = Rc::new(build());
        self.bytes += mesh.gpu_bytes();
        self.entries.insert(key, (mesh.clone(), self.clock));
        self.evict();
        mesh
    }

    pub fn tree(&mut self, species: &str, p: TreeParams) -> Rc<GenMesh> {
        let key = tree_key(species, &p);
        let species = species.to_string();
        self.get_or_build(key, move || tree(&species, p))
    }

    pub fn blob(&mut self, kind: &str, p: BlobParams) -> Rc<GenMesh> {
        let key = blob_key(kind, &p);
        let kind = kind.to_string();
        self.get_or_build(key, move || blob(&kind, p))
    }

    /// Evict least-recently-used entries until both limits are satisfied.
    /// An entry still referenced by a caller is dropped from the map but its
    /// memory survives until that caller lets go — which is the point of `Rc`.
    fn evict(&mut self) {
        while self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, (_, last))| *last)
                .map(|(k, _)| *k);
            match victim {
                Some(k) => {
                    if let Some((mesh, _)) = self.entries.remove(&k) {
                        self.bytes = self.bytes.saturating_sub(mesh.gpu_bytes());
                    }
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_recipes_hit_the_cache() {
        let mut c = GenCache::default();
        let p = TreeParams {
            seed: 1,
            ..Default::default()
        };
        let a = c.tree("oak", p);
        let b = c.tree("oak", p);
        assert_eq!(c.misses(), 1, "second identical request rebuilt");
        assert_eq!(c.hits(), 1);
        assert!(Rc::ptr_eq(&a, &b), "cache handed out two allocations");
    }

    #[test]
    fn a_forest_of_many_from_few_seeds_builds_only_the_few() {
        let mut c = GenCache::default();
        for i in 0..200 {
            c.tree(
                "oak",
                TreeParams {
                    seed: (i % 6) as u64,
                    ..Default::default()
                },
            );
        }
        assert_eq!(c.misses(), 6, "expected 6 generations, got {}", c.misses());
        assert_eq!(c.hits(), 194);
    }

    #[test]
    fn every_knob_participates_in_the_key() {
        let base = TreeParams::default();
        let k = tree_key("oak", &base);
        let variants = [
            TreeParams { seed: 1, ..base },
            TreeParams { height: 5.0, ..base },
            TreeParams { bushiness: 0.5, ..base },
            TreeParams { lean: 0.2, ..base },
            TreeParams { lod: Lod::High, ..base },
            TreeParams { bark: [1.0, 0.0, 0.0], ..base },
            TreeParams { foliage: [0.0, 0.0, 1.0], ..base },
        ];
        for (i, v) in variants.iter().enumerate() {
            assert_ne!(tree_key("oak", v), k, "knob {i} missing from the key");
        }
        assert_ne!(tree_key("pine", &base), k, "species missing from the key");
    }

    #[test]
    fn float_keys_distinguish_close_values_and_unify_signed_zero() {
        let a = TreeParams {
            height: 4.0,
            ..Default::default()
        };
        let b = TreeParams {
            height: 4.000_001,
            ..Default::default()
        };
        assert_ne!(tree_key("oak", &a), tree_key("oak", &b));
        let pos = TreeParams {
            lean: 0.0,
            ..Default::default()
        };
        let neg = TreeParams {
            lean: -0.0,
            ..Default::default()
        };
        assert_eq!(
            tree_key("oak", &pos),
            tree_key("oak", &neg),
            "-0.0 and 0.0 are the same tree"
        );
    }

    #[test]
    fn trees_and_blobs_do_not_collide() {
        let t = tree_key("rock", &TreeParams::default());
        let b = blob_key("rock", &BlobParams::default());
        assert_ne!(t, b, "same name across generators collided");
    }

    #[test]
    fn entry_limit_evicts_least_recently_used() {
        let mut c = GenCache::new(3, 1 << 30);
        for seed in 0..3 {
            c.tree("oak", TreeParams { seed, ..Default::default() });
        }
        assert_eq!(c.len(), 3);
        // Touch seed 0 so seed 1 becomes the oldest.
        c.tree("oak", TreeParams { seed: 0, ..Default::default() });
        c.tree("oak", TreeParams { seed: 99, ..Default::default() });
        assert_eq!(c.len(), 3);
        let before = c.misses();
        c.tree("oak", TreeParams { seed: 0, ..Default::default() });
        assert_eq!(c.misses(), before, "recently used entry was evicted");
    }

    #[test]
    fn byte_limit_is_enforced() {
        // A cap far below one tree forces eviction down to a single entry.
        let mut c = GenCache::new(100, 1 << 16);
        for seed in 0..8 {
            c.tree(
                "oak",
                TreeParams {
                    seed,
                    lod: Lod::High,
                    ..Default::default()
                },
            );
        }
        assert!(c.len() < 8, "byte cap ignored: {} entries", c.len());
        assert!(c.bytes() <= (1 << 16).max(0) + 1_000_000);
    }

    #[test]
    fn cached_geometry_matches_a_fresh_build() {
        let mut c = GenCache::default();
        let p = BlobParams {
            seed: 5,
            ..Default::default()
        };
        let cached = c.blob("rock", p);
        let fresh = blob("rock", p);
        assert_eq!(cached.vertices, fresh.vertices);
        assert_eq!(cached.indices, fresh.indices);
    }
}
