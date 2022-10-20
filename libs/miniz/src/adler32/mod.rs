//! Adler-32 checksum implementation.
//!
//! This implementation features:
//!
//! - Permissively licensed (0BSD) clean-room implementation.
//! - Zero dependencies.
//! - Zero `unsafe`.
//! - Decent performance (3-4 GB/s).
//! - `#![no_std]` support (with `default-features = false`).
/*
#![doc(html_root_url = "https://docs.rs/adler/1.0.2")]
// Deny a few warnings in doctests, since rustdoc `allow`s many warnings by default
#![doc(test(attr(deny(unused_imports, unused_must_use))))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_debug_implementations)]
#![forbid(unsafe_code)]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate core as std;
*/
mod algo;

//use std::hash::Hasher;
/*
#[cfg(feature = "std")]
use std::io::{self, BufRead};
*/
/// Adler-32 checksum calculator.
///
/// An instance of this type is equivalent to an Adler-32 checksum: It can be created in the default
/// state via [`new`] (or the provided `Default` impl), or from a precalculated checksum via
/// [`from_checksum`], and the currently stored checksum can be fetched via [`checksum`].
///
/// This type also implements `Hasher`, which makes it easy to calculate Adler-32 checksums of any
/// type that implements or derives `Hash`. This also allows using Adler-32 in a `HashMap`, although
/// that is not recommended (while every checksum is a hash function, they are not necessarily a
/// good one).
///
/// # Examples
///
/// Basic, piecewise checksum calculation:
///
/// ```
/// use makepad_miniz::adler32::Adler32;
///
/// let mut adler = Adler32::new();
///
/// adler.write_slice(&[0, 1, 2]);
/// adler.write_slice(&[3, 4, 5]);
///
/// assert_eq!(adler.checksum(), 0x00290010);
/// ```
///
/// Using `Hash` to process structures:
///
/// ```
/// use std::hash::Hash;
/// use makepad_miniz::adler32::Adler32;
///
/// #[derive(Hash)]
/// struct Data {
///     byte: u8,
///     word: u16,
///     big: u64,
/// }
///
/// let mut adler = Adler32::new();
///
/// let data = Data { byte: 0x1F, word: 0xABCD, big: !0 };
/// data.hash(&mut adler);
///
/// // hash value depends on architecture endianness
/// if cfg!(target_endian = "little") {
///     assert_eq!(adler.checksum(), 0x33410990);
/// }
/// if cfg!(target_endian = "big") {
///     assert_eq!(adler.checksum(), 0x331F0990);
/// }
///
/// ```
///
/// [`new`]: #method.new
/// [`from_checksum`]: #method.from_checksum
/// [`checksum`]: #method.checksum
#[derive(Debug, Copy, Clone)]
pub struct Adler32 {
    a: u16,
    b: u16,
}

impl Adler32 {
    /// Creates a new Adler-32 instance with default state.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an `Adler32` instance from a precomputed Adler-32 checksum.
    ///
    /// This allows resuming checksum calculation without having to keep the `Adler32` instance
    /// around.
    ///
    /// # Example
    ///
    /// ```
    /// # use makepad_miniz::adler32::Adler32;
    /// let parts = [
    ///     "rust",
    ///     "acean",
    /// ];
    /// let whole = makepad_miniz::adler32::adler32_slice(b"rustacean");
    ///
    /// let mut sum = Adler32::new();
    /// sum.write_slice(parts[0].as_bytes());
    /// let partial = sum.checksum();
    ///
    /// // ...later
    ///
    /// let mut sum = Adler32::from_checksum(partial);
    /// sum.write_slice(parts[1].as_bytes());
    /// assert_eq!(sum.checksum(), whole);
    /// ```
    #[inline]
    pub fn from_checksum(sum: u32) -> Self {
        Adler32 {
            a: sum as u16,
            b: (sum >> 16) as u16,
        }
    }

    /// Returns the calculated checksum at this point in time.
    #[inline]
    pub fn checksum(&self) -> u32 {
        (u32::from(self.b) << 16) | u32::from(self.a)
    }

    /// Adds `bytes` to the checksum calculation.
    ///
    /// If efficiency matters, this should be called with Byte slices that contain at least a few
    /// thousand Bytes.
    pub fn write_slice(&mut self, bytes: &[u8]) {
        self.compute(bytes);
    }
}

impl Default for Adler32 {
    #[inline]
    fn default() -> Self {
        Adler32 { a: 1, b: 0 }
    }
}

impl std::hash::Hasher for Adler32 {
    #[inline]
    fn finish(&self) -> u64 {
        u64::from(self.checksum())
    }

    fn write(&mut self, bytes: &[u8]) {
        self.write_slice(bytes);
    }
}

/// Calculates the Adler-32 checksum of a byte slice.
///
/// This is a convenience function around the [`Adler32`] type.
///
/// [`Adler32`]: struct.Adler32.html
pub fn adler32_slice(data: &[u8]) -> u32 {
    let mut h = Adler32::new();
    h.write_slice(data);
    h.checksum()
}

