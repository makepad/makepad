//! FxHasher taken from rustc_hash, except that it does not depend on the pointer size.

const K: u32 = 0x9e3779b9;

/// This is the same as FxHasher, but with the guarantee that the internal hash is
/// an u32 instead of something that depends on the platform.
pub struct FxHasher32 {
    hash: u32,
}

impl Default for FxHasher32 {
    #[inline]
    fn default() -> FxHasher32 {
        FxHasher32 { hash: 0 }
    }
}

impl FxHasher32 {
    #[inline]
    fn add_to_hash(&mut self, i: u32) {
        use core::ops::BitXor;
        self.hash = self.hash.rotate_left(5).bitxor(i).wrapping_mul(K);
    }
}

impl core::hash::Hasher for FxHasher32 {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        let read_u32 = |bytes: &[u8]| u32::from_ne_bytes(bytes[..4].try_into().unwrap());
        let mut hash = FxHasher32 { hash: self.hash };
        assert!(size_of::<u32>() <= 8);
        while bytes.len() >= size_of::<u32>() {
            hash.add_to_hash(read_u32(bytes));
            bytes = &bytes[size_of::<u32>()..];
        }
        if (size_of::<u32>() > 4) && (bytes.len() >= 4) {
            hash.add_to_hash(u32::from_ne_bytes(bytes[..4].try_into().unwrap()));
            bytes = &bytes[4..];
        }
        if (size_of::<u32>() > 2) && bytes.len() >= 2 {
            hash.add_to_hash(u16::from_ne_bytes(bytes[..2].try_into().unwrap()) as u32);
            bytes = &bytes[2..];
        }
        if (size_of::<u32>() > 1) && !bytes.is_empty() {
            hash.add_to_hash(bytes[0] as u32);
        }
        self.hash = hash.hash;
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(i as u32);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(i as u32);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(i);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add_to_hash(i as u32);
        self.add_to_hash((i >> 32) as u32);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i as u32);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash as u64
    }
}
