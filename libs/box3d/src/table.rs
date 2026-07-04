// Port of box3d/src/table.h + table.c
// The C source uses a hand-rolled open-addressing hash set (not verstable —
// that is only used elsewhere via hull_map.h). This is an exact port of that
// implementation over a Vec, which keeps it fully deterministic. No part of
// the engine iterates the set (only world_snapshot.c serializes raw items,
// and snapshots are not ported), so probe-order is the only order that exists
// and it is identical to C.
//
// The debug-only global probe counter (b3_probeCount) is not ported.

use crate::b3_assert;
use crate::constants::{CHILD_POWER, MAX_CHILD_SHAPES, MAX_SHAPES, SHAPE_POWER};
use crate::ctz::round_up_power_of_2;

#[derive(Clone, Copy, Debug, Default)]
pub struct SetItem {
    pub key: u64,
    pub hash: u32,
}

/// Open-addressing hash set of non-zero u64 keys (b3HashSet).
#[derive(Clone, Debug, Default)]
pub struct HashSet {
    /// The slots (len == capacity, always a power of 2).
    pub items: Vec<SetItem>,
    pub count: u32,
}

impl HashSet {
    #[inline]
    pub fn capacity(&self) -> u32 {
        self.items.len() as u32
    }
}

pub const SHAPE_MASK: i32 = MAX_SHAPES - 1;
pub const CHILD_MASK: i32 = MAX_CHILD_SHAPES - 1;

const _: () = assert!(2 * SHAPE_POWER + CHILD_POWER == 64, "compound power");
const _: () = assert!(CHILD_POWER > 8, "compound child power");

#[inline]
pub fn shape_pair_key(s1: i32, s2: i32, c: i32) -> u64 {
    if s1 < s2 {
        return (((SHAPE_MASK & s1) as u64) << (64 - SHAPE_POWER))
            | (((SHAPE_MASK & s2) as u64) << (64 - 2 * SHAPE_POWER))
            | ((CHILD_MASK & c) as u64);
    }

    (((SHAPE_MASK & s2) as u64) << (64 - SHAPE_POWER))
        | (((SHAPE_MASK & s1) as u64) << (64 - 2 * SHAPE_POWER))
        | ((CHILD_MASK & c) as u64)
}

pub fn create_set(capacity: i32) -> HashSet {
    // Capacity must be a power of 2
    let capacity = if capacity > 16 { round_up_power_of_2(capacity) } else { 16 };

    HashSet {
        items: vec![SetItem::default(); capacity as usize],
        count: 0,
    }
}

pub fn destroy_set(set: &mut HashSet) {
    set.items = Vec::new();
    set.count = 0;
}

pub fn clear_set(set: &mut HashSet) {
    set.count = 0;
    for item in set.items.iter_mut() {
        *item = SetItem::default();
    }
}

// I need a good hash because the keys are built from pairs of increasing integers.
// A simple hash like hash = (integer1 XOR integer2) has many collisions.
// https://lemire.me/blog/2018/08/15/fast-strongly-universal-64-bit-hashing-everywhere/
#[inline]
fn key_hash(key: u64) -> u32 {
    let mut h = key;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;

    h as u32
}

fn find_slot(set: &HashSet, key: u64, hash: u32) -> i32 {
    let capacity = set.capacity();
    let mut index = hash & (capacity - 1);
    let items = &set.items;
    while items[index as usize].hash != 0 && items[index as usize].key != key {
        index = (index + 1) & (capacity - 1);
    }

    index as i32
}

fn add_key_have_capacity(set: &mut HashSet, key: u64, hash: u32) {
    let index = find_slot(set, key, hash) as usize;
    b3_assert!(set.items[index].hash == 0);

    set.items[index].key = key;
    set.items[index].hash = hash;
    set.count += 1;
}

fn grow_table(set: &mut HashSet) {
    let old_count = set.count;

    let old_capacity = set.capacity();
    let old_items = std::mem::take(&mut set.items);

    set.count = 0;
    // Capacity must be a power of 2
    set.items = vec![SetItem::default(); 2 * old_capacity as usize];

    // Transfer items into new array
    for item in &old_items {
        if item.hash == 0 {
            // this item was empty
            continue;
        }

        add_key_have_capacity(set, item.key, item.hash);
    }

    b3_assert!(set.count == old_count);
    let _ = old_count;
}

pub fn contains_key(set: &HashSet, key: u64) -> bool {
    // key of zero is a sentinel
    b3_assert!(key != 0);
    let hash = key_hash(key);
    let index = find_slot(set, key, hash);
    set.items[index as usize].key == key
}

pub fn get_hash_set_bytes(set: &HashSet) -> i32 {
    (set.capacity() as usize * std::mem::size_of::<SetItem>()) as i32
}

/// Returns true if the key was already in the set.
pub fn add_key(set: &mut HashSet, key: u64) -> bool {
    // key of zero is a sentinel
    b3_assert!(key != 0);

    let hash = key_hash(key);
    b3_assert!(hash != 0);

    let index = find_slot(set, key, hash) as usize;
    if set.items[index].hash != 0 {
        // Already in set
        b3_assert!(set.items[index].hash == hash && set.items[index].key == key);
        return true;
    }

    if 2 * set.count >= set.capacity() {
        grow_table(set);
    }

    add_key_have_capacity(set, key, hash);
    false
}

/// Returns true if the key was found.
// See https://en.wikipedia.org/wiki/Open_addressing
pub fn remove_key(set: &mut HashSet, key: u64) -> bool {
    let hash = key_hash(key);
    let mut i = find_slot(set, key, hash) as u32;
    let items = &mut set.items;
    if items[i as usize].hash == 0 {
        // Not in set
        return false;
    }

    // Mark item i as unoccupied
    items[i as usize].key = 0;
    items[i as usize].hash = 0;

    b3_assert!(set.count > 0);
    set.count -= 1;

    // Attempt to fill item i
    let mut j = i;
    let capacity = items.len() as u32;
    loop {
        j = (j + 1) & (capacity - 1);
        if items[j as usize].hash == 0 {
            break;
        }

        // k is the first item for the hash of j
        let k = items[j as usize].hash & (capacity - 1);

        // determine if k lies cyclically in (i,j]
        // i <= j: | i..k..j |
        // i > j: |.k..j  i....| or |....j     i..k.|
        if i <= j {
            if i < k && k <= j {
                continue;
            }
        } else if i < k || k <= j {
            continue;
        }

        // Move j into i
        items[i as usize] = items[j as usize];

        // Mark item j as unoccupied
        items[j as usize].key = 0;
        items[j as usize].hash = 0;

        i = j;
    }

    true
}
