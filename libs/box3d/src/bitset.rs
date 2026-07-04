// Port of box3d/src/bitset.h + bitset.c
// Bit set provides fast operations on large arrays of bits.
// blockCapacity in C corresponds to bits.len(); blockCount is the active count.
// b3CountSetBits lives in table.c in the C source (ctz.h include quirk) but
// belongs here.

use crate::b3_assert;
use crate::ctz::pop_count64;

#[derive(Clone, Debug, Default)]
pub struct BitSet {
    /// The allocated blocks (len == blockCapacity in C).
    pub bits: Vec<u64>,
    /// The number of active blocks.
    pub block_count: u32,
}

impl BitSet {
    #[inline]
    pub fn block_capacity(&self) -> u32 {
        self.bits.len() as u32
    }
}

pub fn create_bit_set(bit_capacity: u32) -> BitSet {
    let block_capacity = (bit_capacity as usize + 64 - 1) / 64;
    BitSet {
        bits: vec![0; block_capacity],
        block_count: 0,
    }
}

pub fn destroy_bit_set(bit_set: &mut BitSet) {
    bit_set.bits = Vec::new();
    bit_set.block_count = 0;
}

pub fn set_bit_count_and_clear(bit_set: &mut BitSet, bit_count: u32) {
    let block_count = (bit_count + 64 - 1) / 64;
    if bit_set.block_capacity() < block_count {
        let new_bit_capacity = bit_count + (bit_count >> 1);
        *bit_set = create_bit_set(new_bit_capacity);
    }

    bit_set.block_count = block_count;
    for i in 0..block_count as usize {
        bit_set.bits[i] = 0;
    }
}

pub fn grow_bit_set(bit_set: &mut BitSet, block_count: u32) {
    b3_assert!(block_count > bit_set.block_count);
    if block_count > bit_set.block_capacity() {
        let new_capacity = block_count + block_count / 2;
        // Preserves existing blocks and zero-fills the rest.
        bit_set.bits.resize(new_capacity as usize, 0);
    }

    bit_set.block_count = block_count;
}

pub fn in_place_union(set_a: &mut BitSet, set_b: &BitSet) {
    b3_assert!(set_a.block_count == set_b.block_count);
    let block_count = set_a.block_count as usize;
    for i in 0..block_count {
        set_a.bits[i] |= set_b.bits[i];
    }
}

#[inline]
pub fn set_bit(bit_set: &mut BitSet, bit_index: u32) {
    let block_index = (bit_index / 64) as usize;
    b3_assert!(block_index < bit_set.block_count as usize);
    bit_set.bits[block_index] |= 1u64 << (bit_index % 64);
}

#[inline]
pub fn set_bit_grow(bit_set: &mut BitSet, bit_index: u32) {
    let block_index = bit_index / 64;
    if block_index >= bit_set.block_count {
        grow_bit_set(bit_set, block_index + 1);
    }
    bit_set.bits[block_index as usize] |= 1u64 << (bit_index % 64);
}

#[inline]
pub fn clear_bit(bit_set: &mut BitSet, bit_index: u32) {
    let block_index = bit_index / 64;
    if block_index >= bit_set.block_count {
        return;
    }
    bit_set.bits[block_index as usize] &= !(1u64 << (bit_index % 64));
}

#[inline]
pub fn get_bit(bit_set: &BitSet, bit_index: u32) -> bool {
    let block_index = bit_index / 64;
    if block_index >= bit_set.block_count {
        return false;
    }
    (bit_set.bits[block_index as usize] & (1u64 << (bit_index % 64))) != 0
}

#[inline]
pub fn get_bit_set_bytes(bit_set: &BitSet) -> i32 {
    (bit_set.block_capacity() as usize * std::mem::size_of::<u64>()) as i32
}

// This function is in table.c in the C source. See header note.
pub fn count_set_bits(bit_set: &BitSet) -> i32 {
    let mut pop_count = 0;
    let block_count = bit_set.block_count as usize;
    for i in 0..block_count {
        pop_count += pop_count64(bit_set.bits[i]);
    }

    pop_count
}
