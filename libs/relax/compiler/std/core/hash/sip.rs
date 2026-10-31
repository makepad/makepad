//! SipHash-2-4 (rustc library/core/src/hash/sip.rs: `SipHasher`, deprecated since 1.13). Same
//! algorithm and byte stream as real core, so `SipHasher::new_with_keys(k0, k1)` hashes to the
//! same values; only the 2-4 variant is provided (SipHasher13 is a hidden unstable type).

use super::Hasher;

#[derive(Debug, Clone, Copy)]
struct State {
    v0: u64,
    v2: u64,
    v1: u64,
    v3: u64,
}

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

/// Little-endian load of `len` (< 8) bytes of `buf` starting at `start`.
fn u8to64_le(buf: &[u8], start: usize, len: usize) -> u64 {
    let mut out = 0u64;
    let mut i = 0;
    while i < len {
        out |= (buf[start + i] as u64) << (i * 8);
        i += 1;
    }
    out
}

#[derive(Debug, Clone)]
pub struct SipHasher {
    k0: u64,
    k1: u64,
    length: usize, // how many bytes we've processed
    state: State,  // hash State
    tail: u64,     // unprocessed bytes le
    ntail: usize,  // how many bytes in tail are valid
}

impl SipHasher {
    #[deprecated(since = "1.13.0", note = "use `std::hash::DefaultHasher` instead")]
    #[must_use]
    pub fn new() -> SipHasher {
        SipHasher::new_with_keys(0, 0)
    }

    #[deprecated(since = "1.13.0", note = "use `std::hash::DefaultHasher` instead")]
    #[must_use]
    pub fn new_with_keys(key0: u64, key1: u64) -> SipHasher {
        SipHasher {
            k0: key0,
            k1: key1,
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
}

impl Default for SipHasher {
    fn default() -> SipHasher {
        #[allow(deprecated)]
        SipHasher::new_with_keys(0, 0)
    }
}

impl Hasher for SipHasher {
    fn write(&mut self, msg: &[u8]) {
        let length = msg.len();
        self.length += length;

        let mut needed = 0;

        if self.ntail != 0 {
            needed = 8 - self.ntail;
            let take = if length < needed { length } else { needed };
            self.tail |= u8to64_le(msg, 0, take) << (8 * self.ntail);
            if length < needed {
                self.ntail += length;
                return;
            } else {
                self.state.v3 ^= self.tail;
                compress(&mut self.state);
                compress(&mut self.state);
                self.state.v0 ^= self.tail;
                self.ntail = 0;
            }
        }

        // Buffered tail is now flushed, process new input.
        let len = length - needed;
        let left = len & 0x7; // len % 8

        let mut i = needed;
        while i < len - left {
            let mi = u8to64_le(msg, i, 7) | ((msg[i + 7] as u64) << 56);

            self.state.v3 ^= mi;
            compress(&mut self.state);
            compress(&mut self.state);
            self.state.v0 ^= mi;

            i += 8;
        }

        self.tail = u8to64_le(msg, i, left);
        self.ntail = left;
    }

    fn finish(&self) -> u64 {
        let mut state = self.state;

        let b: u64 = ((self.length as u64 & 0xff) << 56) | self.tail;

        state.v3 ^= b;
        compress(&mut state);
        compress(&mut state);
        state.v0 ^= b;

        state.v2 ^= 0xff;
        compress(&mut state);
        compress(&mut state);
        compress(&mut state);
        compress(&mut state);

        state.v0 ^ state.v1 ^ state.v2 ^ state.v3
    }
}
