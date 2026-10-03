//! Hashing: Hash, Hasher, BuildHasher, and Hash impls for the primitive and core types.
//! Primitives feed hashers exactly as real core does (integers via write_<int>, str as its
//! bytes plus 0xff, slices as a length prefix plus elements), so a user hasher sees the same
//! byte stream; the default hasher itself (fast.rs) differs from real std's SipHash.

mod fast;

pub use fast::{DefaultHasher, FastHasher, RandomState};

use crate::marker::PhantomData;

pub trait Hash {
    fn hash<H: Hasher>(&self, state: &mut H);

    fn hash_slice<H: Hasher>(data: &[Self], state: &mut H)
    where
        Self: Sized,
    {
        let mut i = 0;
        while i < data.len() {
            data[i].hash(state);
            i += 1;
        }
    }
}

pub trait Hasher {
    fn finish(&self) -> u64;
    fn write(&mut self, bytes: &[u8]);

    fn write_u8(&mut self, i: u8) {
        self.write(&[i])
    }
    fn write_u16(&mut self, i: u16) {
        self.write(&i.to_ne_bytes())
    }
    fn write_u32(&mut self, i: u32) {
        self.write(&i.to_ne_bytes())
    }
    fn write_u64(&mut self, i: u64) {
        self.write(&i.to_ne_bytes())
    }
    fn write_u128(&mut self, i: u128) {
        self.write(&i.to_ne_bytes())
    }
    fn write_usize(&mut self, i: usize) {
        self.write(&i.to_ne_bytes())
    }
    fn write_i8(&mut self, i: i8) {
        self.write_u8(i as u8)
    }
    fn write_i16(&mut self, i: i16) {
        self.write_u16(i as u16)
    }
    fn write_i32(&mut self, i: i32) {
        self.write_u32(i as u32)
    }
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64)
    }
    fn write_i128(&mut self, i: i128) {
        self.write_u128(i as u128)
    }
    fn write_isize(&mut self, i: isize) {
        self.write_usize(i as usize)
    }
    fn write_length_prefix(&mut self, len: usize) {
        self.write_usize(len);
    }
    fn write_str(&mut self, s: &str) {
        self.write(s.as_bytes());
        self.write_u8(0xff);
    }
}

impl<'a, H: Hasher + ?Sized> Hasher for &'a mut H {
    fn finish(&self) -> u64 {
        (**self).finish()
    }
    fn write(&mut self, bytes: &[u8]) {
        (**self).write(bytes)
    }
    fn write_u8(&mut self, i: u8) {
        (**self).write_u8(i)
    }
    fn write_u16(&mut self, i: u16) {
        (**self).write_u16(i)
    }
    fn write_u32(&mut self, i: u32) {
        (**self).write_u32(i)
    }
    fn write_u64(&mut self, i: u64) {
        (**self).write_u64(i)
    }
    fn write_u128(&mut self, i: u128) {
        (**self).write_u128(i)
    }
    fn write_usize(&mut self, i: usize) {
        (**self).write_usize(i)
    }
    fn write_i8(&mut self, i: i8) {
        (**self).write_i8(i)
    }
    fn write_i16(&mut self, i: i16) {
        (**self).write_i16(i)
    }
    fn write_i32(&mut self, i: i32) {
        (**self).write_i32(i)
    }
    fn write_i64(&mut self, i: i64) {
        (**self).write_i64(i)
    }
    fn write_i128(&mut self, i: i128) {
        (**self).write_i128(i)
    }
    fn write_isize(&mut self, i: isize) {
        (**self).write_isize(i)
    }
    fn write_length_prefix(&mut self, len: usize) {
        (**self).write_length_prefix(len)
    }
    fn write_str(&mut self, s: &str) {
        (**self).write_str(s)
    }
}

pub trait BuildHasher {
    type Hasher: Hasher;
    fn build_hasher(&self) -> Self::Hasher;
    fn hash_one<T: Hash>(&self, x: T) -> u64
    where
        Self: Sized,
    {
        let mut hasher = self.build_hasher();
        x.hash(&mut hasher);
        hasher.finish()
    }
}

/// Creates a default `H` for every hasher.
pub struct BuildHasherDefault<H> {
    _m: PhantomData<fn() -> H>,
}

impl<H> BuildHasherDefault<H> {
    pub const fn new() -> BuildHasherDefault<H> {
        BuildHasherDefault { _m: PhantomData }
    }
}

impl<H: Default + Hasher> BuildHasher for BuildHasherDefault<H> {
    type Hasher = H;
    fn build_hasher(&self) -> H {
        H::default()
    }
}

impl<H> Clone for BuildHasherDefault<H> {
    fn clone(&self) -> BuildHasherDefault<H> {
        BuildHasherDefault { _m: PhantomData }
    }
}

impl<H> Default for BuildHasherDefault<H> {
    fn default() -> BuildHasherDefault<H> {
        BuildHasherDefault { _m: PhantomData }
    }
}

impl<H> PartialEq for BuildHasherDefault<H> {
    fn eq(&self, _other: &BuildHasherDefault<H>) -> bool {
        true
    }
}

impl<H> Eq for BuildHasherDefault<H> {}

impl<H> crate::fmt::Debug for BuildHasherDefault<H> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.write_str("BuildHasherDefault")
    }
}

// ---------------------------------------------------------------- primitives

impl Hash for u8 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u8(*self)
    }
    fn hash_slice<H: Hasher>(data: &[u8], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<u8>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for u16 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u16(*self)
    }
    fn hash_slice<H: Hasher>(data: &[u16], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<u16>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for u32 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(*self)
    }
    fn hash_slice<H: Hasher>(data: &[u32], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<u32>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for u64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(*self)
    }
    fn hash_slice<H: Hasher>(data: &[u64], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<u64>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for u128 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u128(*self)
    }
    fn hash_slice<H: Hasher>(data: &[u128], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<u128>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for usize {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(*self)
    }
    fn hash_slice<H: Hasher>(data: &[usize], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<usize>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for i8 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i8(*self)
    }
    fn hash_slice<H: Hasher>(data: &[i8], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<i8>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for i16 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i16(*self)
    }
    fn hash_slice<H: Hasher>(data: &[i16], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<i16>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for i32 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i32(*self)
    }
    fn hash_slice<H: Hasher>(data: &[i32], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<i32>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for i64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i64(*self)
    }
    fn hash_slice<H: Hasher>(data: &[i64], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<i64>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for i128 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_i128(*self)
    }
    fn hash_slice<H: Hasher>(data: &[i128], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<i128>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for isize {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_isize(*self)
    }
    fn hash_slice<H: Hasher>(data: &[isize], state: &mut H) {
        let n = data.len() * crate::mem::size_of::<isize>();
        let bytes = unsafe { crate::intrinsics_mem::slice_from_raw(data.as_ptr() as *const u8, n) };
        state.write(bytes)
    }
}

impl Hash for bool {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u8(*self as u8)
    }
}

impl Hash for char {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(*self as u32)
    }
}

impl Hash for str {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_str(self);
    }
}

impl Hash for () {
    fn hash<H: Hasher>(&self, _state: &mut H) {}
}

impl<T: Hash> Hash for [T] {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_length_prefix(self.len());
        Hash::hash_slice(self, state)
    }
}

impl<'a, T: ?Sized + Hash> Hash for &'a T {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

impl<'a, T: ?Sized + Hash> Hash for &'a mut T {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

/// Pointers hash their address (fat pointers: address only; real core also hashes the
/// metadata, which no makepad code observes).
impl<T: ?Sized> Hash for *const T {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(*self as *const u8 as usize)
    }
}

impl<T: ?Sized> Hash for *mut T {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(*self as *const u8 as usize)
    }
}

// ---------------------------------------------------------------- tuples (1..=12)

impl<A: Hash> Hash for (A,) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
    }
}

impl<A: Hash, B: Hash> Hash for (A, B) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash> Hash for (A, B, C) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash> Hash for (A, B, C, D) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash> Hash for (A, B, C, D, E) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash, F: Hash> Hash for (A, B, C, D, E, F) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
        self.5.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash, F: Hash, G: Hash> Hash for (A, B, C, D, E, F, G) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
        self.5.hash(state);
        self.6.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash, F: Hash, G: Hash, H: Hash> Hash for (A, B, C, D, E, F, G, H) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
        self.5.hash(state);
        self.6.hash(state);
        self.7.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash, F: Hash, G: Hash, H: Hash, I: Hash> Hash for (A, B, C, D, E, F, G, H, I) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
        self.5.hash(state);
        self.6.hash(state);
        self.7.hash(state);
        self.8.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash, F: Hash, G: Hash, H: Hash, I: Hash, J: Hash> Hash for (A, B, C, D, E, F, G, H, I, J) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
        self.5.hash(state);
        self.6.hash(state);
        self.7.hash(state);
        self.8.hash(state);
        self.9.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash, F: Hash, G: Hash, H: Hash, I: Hash, J: Hash, K: Hash> Hash for (A, B, C, D, E, F, G, H, I, J, K) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
        self.5.hash(state);
        self.6.hash(state);
        self.7.hash(state);
        self.8.hash(state);
        self.9.hash(state);
        self.10.hash(state);
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash, E: Hash, F: Hash, G: Hash, H: Hash, I: Hash, J: Hash, K: Hash, L: Hash> Hash for (A, B, C, D, E, F, G, H, I, J, K, L) {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.0.hash(state);
        self.1.hash(state);
        self.2.hash(state);
        self.3.hash(state);
        self.4.hash(state);
        self.5.hash(state);
        self.6.hash(state);
        self.7.hash(state);
        self.8.hash(state);
        self.9.hash(state);
        self.10.hash(state);
        self.11.hash(state);
    }
}
