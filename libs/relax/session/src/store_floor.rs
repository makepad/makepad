//! The free space the JIT's store keeps on its volume (relax-compiler cache.rs `prune`): under it,
//! stores go, least recently used first. Tools that share the volume (devhub's disk guard) read
//! the rule here, so it is said once.

/// The store keeps 1/FREE_SHARE of its volume free ...
pub const FREE_SHARE: u64 = 10;
/// ... and at most this many bytes: a tenth alone kept 190 GB free on a 2 TB disk, and with 185
/// GB free every launch there evicted every store not in use. It stays above what a disk guard
/// sharing the volume reclaims up to (devhub: its 25 GB floor and 10 GB margin): at 20 GB the
/// store grew under that floor, and the guard then deleted other work's build trees in its
/// place, while the store's least recently used entries (code of trees long gone) stayed.
pub const FREE_MAX: u64 = 40 << 30;

/// The bytes the store keeps free on a volume of `size` bytes.
pub fn free_floor(size: u64) -> u64 {
    (size / FREE_SHARE).min(FREE_MAX)
}
