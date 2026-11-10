//! SipHash (rustc library/core/src/hash/sip.rs): `SipHasher` (2-4, deprecated since 1.13) and the
//! hidden SipHasher13 (1-3) that real std's DefaultHasher::new hashes with (fast.rs). Same
//! algorithm and byte stream as real core, so each hashes to real core's values for the same keys.
//! Message words are single unaligned loads (real core's load_int_le!); an integer write takes the
//! short path (rustc_data_structures' sip128 short_write): the same bytes, no byte slice.

use super::Hasher;
#[cfg(not(relax_check))]
use crate::ptr;
#[cfg(relax_check)]
use std::ptr;

#[derive(Debug, Clone, Copy)]
struct State {
    v0: u64,
    v2: u64,
    v1: u64,
    v3: u64,
}

#[inline]
fn compress(s: &mut State) {
    s.v0 = s.v0.wrapping_add(s.v1);
    s.v2 = s.v2.wrapping_add(s.v3);
    s.v1 = s.v1.rotate_left(13);
    s.v1 ^= s.v0;
    s.v3 = s.v3.rotate_left(16);
    s.v3 ^= s.v2;
    s.v0 = s.v0.rotate_left(32);

    s.v2 = s.v2.wrapping_add(s.v1);
    s.v0 = s.v0.wrapping_add(s.v3);
    s.v1 = s.v1.rotate_left(17);
    s.v1 ^= s.v2;
    s.v3 = s.v3.rotate_left(21);
    s.v3 ^= s.v0;
    s.v2 = s.v2.rotate_left(32);
}

/// Little-endian loads at byte `i` of `buf`, one unaligned load each (real core's load_int_le!).
/// Safety: the loaded bytes are inside `buf`.
#[inline]
unsafe fn load_u64(buf: &[u8], i: usize) -> u64 {
    ptr::read_unaligned(buf.as_ptr().add(i) as *const u64).to_le()
}

#[inline]
unsafe fn load_u32(buf: &[u8], i: usize) -> u32 {
    ptr::read_unaligned(buf.as_ptr().add(i) as *const u32).to_le()
}

#[inline]
unsafe fn load_u16(buf: &[u8], i: usize) -> u16 {
    ptr::read_unaligned(buf.as_ptr().add(i) as *const u16).to_le()
}

/// Little-endian load of `len` (< 8) bytes of `buf` starting at `start` (real core's u8to64_le).
/// Safety: `start + len <= buf.len()`.
#[inline]
unsafe fn u8to64_le(buf: &[u8], start: usize, len: usize) -> u64 {
    let mut i = 0;
    let mut out = 0;
    if i + 3 < len {
        out = load_u32(buf, start + i) as u64;
        i += 4;
    }
    if i + 1 < len {
        out |= (load_u16(buf, start + i) as u64) << (i * 8);
        i += 2;
    }
    if i < len {
        out |= (*buf.get_unchecked(start + i) as u64) << (i * 8);
    }
    out
}

/// The state both variants share; `c` / `d`: the compression rounds per message word and at the end.
#[derive(Debug, Clone)]
struct Sip {
    length: usize, // how many bytes we've processed
    state: State,  // hash State
    tail: u64,     // unprocessed bytes le
    ntail: usize,  // how many bytes in tail are valid
}

impl Sip {
    const fn new_with_keys(key0: u64, key1: u64) -> Sip {
        Sip {
            length: 0,
            state: State {
                v0: key0 ^ 0x736f6d6570736575,
                v1: key1 ^ 0x646f72616e646f6d,
                v2: key0 ^ 0x6c7967656e657261,
                v3: key1 ^ 0x7465646279746573,
            },
            tail: 0,
            ntail: 0,
        }
    }

    #[inline]
    fn rounds(s: &mut State, n: usize) {
        let mut i = 0;
        while i < n {
            compress(s);
            i += 1;
        }
    }

    #[inline]
    fn write(&mut self, msg: &[u8], c: usize) {
        let length = msg.len();
        self.length += length;

        let mut needed = 0;

        if self.ntail != 0 {
            needed = 8 - self.ntail;
            let take = if length < needed { length } else { needed };
            // SAFETY: take <= msg.len()
            self.tail |= unsafe { u8to64_le(msg, 0, take) } << (8 * self.ntail);
            if length < needed {
                self.ntail += length;
                return;
            } else {
                self.state.v3 ^= self.tail;
                Sip::rounds(&mut self.state, c);
                self.state.v0 ^= self.tail;
                self.ntail = 0;
            }
        }

        // Buffered tail is now flushed, process new input.
        let len = length - needed;
        let left = len & 0x7; // len % 8

        let mut i = needed;
        while i < len - left {
            // SAFETY: i + 8 <= needed + len - left <= msg.len()
            let mi = unsafe { load_u64(msg, i) };

            self.state.v3 ^= mi;
            Sip::rounds(&mut self.state, c);
            self.state.v0 ^= mi;

            i += 8;
        }

        // SAFETY: i + left == msg.len()
        self.tail = unsafe { u8to64_le(msg, i, left) };
        self.ntail = left;
    }

    /// An integer write of `size` (1..=8) bytes, `x` its value (its little-endian bytes, the rest
    /// zero): the stream of `write(&x.to_le_bytes()[..size])`, straight into the tail.
    #[inline]
    fn short_write(&mut self, x: u64, size: usize, c: usize) {
        self.length += size;
        let ntail = self.ntail;
        // ntail < 8: the bytes of x that fit the tail; the rest are past the word
        self.tail |= x << (8 * ntail);
        let fill = 8 - ntail;
        if size < fill {
            self.ntail = ntail + size;
            return;
        }
        let m = self.tail;
        self.state.v3 ^= m;
        Sip::rounds(&mut self.state, c);
        self.state.v0 ^= m;
        self.ntail = size - fill;
        // x's bytes past the word (none when the tail was empty: fill is 8)
        self.tail = if fill < 8 { x >> (8 * fill) } else { 0 };
    }

    #[inline]
    fn finish(&self, c: usize, d: usize) -> u64 {
        let mut state = self.state;

        let b: u64 = ((self.length as u64 & 0xff) << 56) | self.tail;

        state.v3 ^= b;
        Sip::rounds(&mut state, c);
        state.v0 ^= b;

        state.v2 ^= 0xff;
        Sip::rounds(&mut state, d);

        state.v0 ^ state.v1 ^ state.v2 ^ state.v3
    }
}

/// SipHash-2-4.
#[derive(Debug, Clone)]
pub struct SipHasher(Sip);

impl SipHasher {
    #[deprecated(since = "1.13.0", note = "use `std::hash::DefaultHasher` instead")]
    #[must_use]
    pub fn new() -> SipHasher {
        SipHasher(Sip::new_with_keys(0, 0))
    }

    #[deprecated(since = "1.13.0", note = "use `std::hash::DefaultHasher` instead")]
    #[must_use]
    pub fn new_with_keys(key0: u64, key1: u64) -> SipHasher {
        SipHasher(Sip::new_with_keys(key0, key1))
    }
}

impl Default for SipHasher {
    fn default() -> SipHasher {
        SipHasher(Sip::new_with_keys(0, 0))
    }
}

impl Hasher for SipHasher {
    #[inline]
    fn write(&mut self, msg: &[u8]) {
        self.0.write(msg, 2)
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.0.short_write(i as u64, 1, 2)
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.0.short_write(i as u64, 2, 2)
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.0.short_write(i as u64, 4, 2)
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0.short_write(i, 8, 2)
    }
    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.0.short_write(i as u64, 8, 2);
        self.0.short_write((i >> 64) as u64, 8, 2)
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.0.short_write(i as u64, (usize::BITS / 8) as usize, 2)
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0.finish(2, 4)
    }
}

/// SipHash-1-3, real core's hidden SipHasher13: the hash of real std's `DefaultHasher::new()`. Its
/// byte stream is real core's: integers as their little-endian bytes (here the short path), a str as
/// its bytes and 0xff (the Hasher default).
#[derive(Debug, Clone)]
pub(crate) struct SipHasher13(Sip);

impl SipHasher13 {
    #[inline]
    pub(crate) const fn new_with_keys(key0: u64, key1: u64) -> SipHasher13 {
        SipHasher13(Sip::new_with_keys(key0, key1))
    }
}

impl Hasher for SipHasher13 {
    #[inline]
    fn write(&mut self, msg: &[u8]) {
        self.0.write(msg, 1)
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.0.short_write(i as u64, 1, 1)
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.0.short_write(i as u64, 2, 1)
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.0.short_write(i as u64, 4, 1)
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0.short_write(i, 8, 1)
    }
    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.0.short_write(i as u64, 8, 1);
        self.0.short_write((i >> 64) as u64, 8, 1)
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.0.short_write(i as u64, (usize::BITS / 8) as usize, 1)
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0.finish(1, 3)
    }
}
