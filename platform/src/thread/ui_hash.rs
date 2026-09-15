//! Opt-in accounting for application hash tables used while recording a widget.
//! Counts bytes passed to a hasher, including table growth/reinsertion. Workers
//! have independent TLS and never contribute to the UI's frame.
use std::{cell::Cell, hash::{BuildHasherDefault, Hasher}, sync::Arc};

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_ATLAS_DIAGNOSTICS").is_some())
}

pub fn current_bytes() -> u64 { BYTES.with(Cell::get) }

pub struct HashFrame(bool);
impl HashFrame {
    pub fn begin(enabled: bool) -> Self {
        let previous = ACTIVE.with(|active| active.replace(enabled));
        if enabled && !previous { BYTES.with(|bytes| bytes.set(0)); }
        Self(previous)
    }
    pub fn bytes(&self) -> u64 { BYTES.with(Cell::get) }
    pub fn assert_budget(&self, limit: u64) {
        debug_assert!(self.bytes() <= limit, "widget-draw hashed {} bytes; limit {limit}", self.bytes());
    }
}
impl Drop for HashFrame {
    fn drop(&mut self) { ACTIVE.with(|active| active.set(self.0)); }
}

#[derive(Default)]
pub struct CountedHasher(std::collections::hash_map::DefaultHasher);
impl Hasher for CountedHasher {
    fn finish(&self) -> u64 { self.0.finish() }
    fn write(&mut self, bytes: &[u8]) {
        ACTIVE.with(|active| {
            if active.get() { BYTES.with(|count| count.set(count.get() + bytes.len() as u64)); }
        });
        self.0.write(bytes);
    }
}
pub type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<CountedHasher>>;
pub type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<CountedHasher>>;

/// Own the Arc as well as its address so a freed allocation cannot alias a
/// live cache entry. Equality and hashing never inspect the immutable payload.
#[derive(Debug)]
pub struct ArcKey<T: ?Sized>(pub Arc<T>);
impl<T: ?Sized> Clone for ArcKey<T> {
    fn clone(&self) -> Self { Self(self.0.clone()) }
}
impl<T: ?Sized> PartialEq for ArcKey<T> {
    fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}
impl<T: ?Sized> Eq for ArcKey<T> {}
impl<T: ?Sized> std::hash::Hash for ArcKey<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(Arc::as_ptr(&self.0) as *const () as usize);
    }
}
