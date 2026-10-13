//! Stable hashing with no compiler types: `Sip128` (SipHash-1-3, 128-bit output, the
//! construction rustc's StableHasher uses; streaming, fixed key) and the late-input record
//! (include!/env! inputs read while bodies expand). The type and crate hashes built on it are in
//! stable.rs.

/// SipHash-1-3, 128-bit output. `new_with_rounds` exists for the reference vectors (2-4).
#[derive(Clone)]
pub struct Sip128 {
    v: [u64; 4],
    tail: u64,
    ntail: usize,
    len: u64,
    c: u8,
    d: u8,
}

#[inline(always)]
fn round(v: &mut [u64; 4]) {
    v[0] = v[0].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(13);
    v[1] ^= v[0];
    v[0] = v[0].rotate_left(32);
    v[2] = v[2].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(16);
    v[3] ^= v[2];
    v[0] = v[0].wrapping_add(v[3]);
    v[3] = v[3].rotate_left(21);
    v[3] ^= v[0];
    v[2] = v[2].wrapping_add(v[1]);
    v[1] = v[1].rotate_left(17);
    v[1] ^= v[2];
    v[2] = v[2].rotate_left(32);
}

/// Fixed key: "relax-stable-v1\0" (changing it changes every stable value: a format version).
const K0: u64 = 0x6174_732d_6469_7061;
const K1: u64 = 0x0031_762d_656c_6261;

impl Default for Sip128 {
    fn default() -> Self {
        Sip128::new()
    }
}

impl Sip128 {
    pub fn new() -> Sip128 {
        Sip128::new_with_rounds(K0, K1, 1, 3)
    }

    pub fn new_with_rounds(k0: u64, k1: u64, c: u8, d: u8) -> Sip128 {
        let v = [k0 ^ 0x736f_6d65_7073_6575, k1 ^ 0x646f_7261_6e64_6f6d ^ 0xee, k0 ^ 0x6c79_6765_6e65_7261, k1 ^ 0x7465_6462_7974_6573];
        Sip128 { v, tail: 0, ntail: 0, len: 0, c, d }
    }

    #[inline(always)]
    fn word(&mut self, m: u64) {
        self.v[3] ^= m;
        for _ in 0..self.c {
            round(&mut self.v);
        }
        self.v[0] ^= m;
    }

    pub fn write(&mut self, mut b: &[u8]) {
        self.len = self.len.wrapping_add(b.len() as u64);
        if self.ntail > 0 {
            while self.ntail < 8 && !b.is_empty() {
                self.tail |= (b[0] as u64) << (8 * self.ntail);
                self.ntail += 1;
                b = &b[1..];
            }
            if self.ntail < 8 {
                return;
            }
            let m = self.tail;
            self.word(m);
            self.tail = 0;
            self.ntail = 0;
        }
        let mut chunks = b.chunks_exact(8);
        for ch in &mut chunks {
            self.word(u64::from_le_bytes([ch[0], ch[1], ch[2], ch[3], ch[4], ch[5], ch[6], ch[7]]));
        }
        for (i, x) in chunks.remainder().iter().enumerate() {
            self.tail |= (*x as u64) << (8 * i);
        }
        self.ntail = chunks.remainder().len();
    }

    pub fn u8(&mut self, x: u8) {
        self.write(&[x]);
    }
    pub fn u32(&mut self, x: u32) {
        self.write(&x.to_le_bytes());
    }
    pub fn u64(&mut self, x: u64) {
        self.write(&x.to_le_bytes());
    }
    pub fn u128(&mut self, x: u128) {
        self.write(&x.to_le_bytes());
    }
    /// Length-prefixed, so consecutive strings never run together.
    pub fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.write(s.as_bytes());
    }

    pub fn finish128(&self) -> u128 {
        let mut v = self.v;
        let b = (self.len << 56) | self.tail;
        v[3] ^= b;
        for _ in 0..self.c {
            round(&mut v);
        }
        v[0] ^= b;
        v[2] ^= 0xee;
        for _ in 0..self.d {
            round(&mut v);
        }
        let lo = v[0] ^ v[1] ^ v[2] ^ v[3];
        v[1] ^= 0xdd;
        for _ in 0..self.d {
            round(&mut v);
        }
        let hi = v[0] ^ v[1] ^ v[2] ^ v[3];
        (hi as u128) << 64 | lo as u128
    }
}

pub fn hash_bytes(b: &[u8]) -> u128 {
    let mut h = Sip128::new();
    h.write(b);
    h.finish128()
}

// ------------------------------------------------------------ late inputs

/// Macro inputs that are not source text: files read by include!/include_str!/include_bytes!
/// and env!/option_env! values. Bodies expand lazily, so these are read during compiles, after
/// the fingerprints. Which inputs a crate reads is fixed by its source; their CONTENTS are not,
/// so the cache keeps, per crate world, the union of inputs seen with their content hashes and
/// re-checks them before trusting the world (C3).
pub const LATE_FILE: u8 = 1;
pub const LATE_ENV: u8 = 2;

static LATE: crate::sync::Mutex<Option<std::collections::HashMap<(String, u8, String), u128>>> = crate::sync::Mutex::new(None);

pub(crate) struct ZoneTaken(Option<std::collections::HashMap<(String, u8, String), u128>>);

pub(crate) fn zone_take() -> ZoneTaken {
    let mut late = LATE.lock().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken(std::mem::take(&mut *late));
    #[cfg(test)]
    assert!(late.is_none());
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut late = LATE.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *late, taken.0));
}

/// The content hash a late input is recorded with.
pub fn late_hash(content: Option<&[u8]>) -> u128 {
    let mut h = Sip128::new();
    h.str("late");
    match content {
        Some(b) => {
            h.u8(1);
            h.write(b);
        }
        None => h.u8(0),
    }
    h.finish128()
}

pub fn note_late_input(krate: &str, kind: u8, name: &str, content: Option<&[u8]>) {
    let v = late_hash(content);
    let mut g = LATE.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(Default::default).insert((krate.to_string(), kind, name.to_string()), v);
}

/// The late inputs seen so far, sorted: (crate, kind, name, content hash).
pub fn late_inputs() -> Vec<(String, u8, String, u128)> {
    let g = LATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<_> = g.as_ref().map_or(Vec::new(), |m| m.iter().map(|((c, k, n), h)| (c.clone(), *k, n.clone(), *h)).collect());
    v.sort();
    v
}
