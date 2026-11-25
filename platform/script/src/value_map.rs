use relax_api::relax_api;
use crate::makepad_live_id::LiveId;

use std::{
    hash::BuildHasher,
    ops::{Index, IndexMut},
    sync::OnceLock,
};

// Idea taken from the `nohash_hasher` crate.
#[relax_api(plain, run)]
#[derive(Default)]
pub struct ValueHasher(u64);

impl std::hash::Hasher for ValueHasher {
    #[inline]
    fn write(&mut self, _: &[u8]) {
        unreachable!();
    }
    #[inline]
    fn write_u8(&mut self, _n: u8) {
        unreachable!();
    }
    #[inline]
    fn write_u16(&mut self, _n: u16) {
        unreachable!();
    }
    #[inline]
    fn write_u32(&mut self, _n: u32) {
        unreachable!();
    }
    #[inline(always)]
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
    #[inline]
    fn write_usize(&mut self, _n: usize) {
        unreachable!();
    }
    #[inline]
    fn write_i8(&mut self, _n: i8) {
        unreachable!();
    }
    #[inline]
    fn write_i16(&mut self, _n: i16) {
        unreachable!();
    }
    #[inline]
    fn write_i32(&mut self, _n: i32) {
        unreachable!();
    }
    #[inline]
    fn write_i64(&mut self, _n: i64) {
        unreachable!();
    }
    #[inline]
    fn write_isize(&mut self, _n: isize) {
        unreachable!();
    }
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[relax_api(plain, run)]
#[derive(Copy, Clone, Default)]
pub struct ValueHasherBuilder {}

impl std::hash::BuildHasher for ValueHasherBuilder {
    type Hasher = ValueHasher;

    #[inline(always)]
    fn build_hasher(&self) -> ValueHasher {
        ValueHasher::default()
    }
}

/// Fields occupy consecutive slots in insertion order. A spilled map indexes live slots
/// by key; deleting a field leaves its slot available for the next compaction.
#[derive(Clone, Debug)]
pub struct ValueMap<K, V> {
    entries: Vec<Slot<K, V>>,
    spill: Option<Box<SpillIndex>>,
}

#[derive(Clone, Debug)]
enum Slot<K, V> {
    Live((K, V)),
    // The first hole of a run knows its end, the last knows its start.
    // This lets a walk cross even a largely deleted map in O(live entries).
    Hole { start: usize, end: usize },
}

const SPILL_AT: usize = 16;
const EMPTY: usize = usize::MAX;

#[derive(Default, Debug)]
struct SpillIndex {
    buckets: Vec<usize>,
    holes: usize,
    generation: u32,
    // A caller requesting live ranks (obj_iter_at) can walk them in linear time too.
    ranks: OnceLock<Vec<usize>>,
    cursors: Vec<Option<MapCursor>>,
}

impl Clone for SpillIndex {
    fn clone(&self) -> Self {
        Self {
            buckets: self.buckets.clone(),
            holes: self.holes,
            generation: self.generation,
            // Cursors belong to the original map.
            ..Self::default()
        }
    }
}

// JIT code reaches `Option<MapCursor>` in the spill index.
relax_api::option_layouts! { MapCursor }

/// The physical boundary after the last yield and the live prefix it represented.
/// A delete leaves the boundary in place. On compaction the map translates it to
/// the count of *surviving* visited entries: deleting the current field can make
/// that count smaller than the number of yields. New fields append beyond it.
#[derive(Debug)]
struct MapCursor {
    thread: usize,
    frame: usize,
    position: usize,
    visited: usize,
    generation: u32,
}

impl MapCursor {
    fn next_position(&self, generation: u32) -> usize {
        if self.generation == generation { self.position } else { self.visited }
    }
}

impl SpillIndex {
    fn bucket_count(len: usize) -> usize {
        // Keep a quarter of the buckets empty for linear probing.
        (len + len.div_ceil(3)).next_power_of_two()
    }

    fn fits(&self, len: usize) -> bool {
        len <= self.buckets.len() - self.buckets.len() / 4
    }

    #[inline]
    fn start<K: std::hash::Hash>(&self, key: &K) -> usize {
        // ValueHasher returns the raw ScriptValue bits. Numeric keys in
        // particular can differ only in their high bits.
        let bits = ValueHasherBuilder {}.hash_one(key);
        let mixed = (bits ^ (bits >> 32)).wrapping_mul(0x9e3779b97f4a7c15);
        (mixed >> (64 - self.buckets.len().trailing_zeros())) as usize
    }

    #[inline]
    fn find<K: Eq + std::hash::Hash, V>(&self, entries: &[Slot<K, V>], key: &K) -> (usize, usize) {
        let mask = self.buckets.len() - 1;
        let mut bucket = self.start(key);
        loop {
            let pos = self.buckets[bucket];
            if pos == EMPTY || matches!(&entries[pos], Slot::Live((k, _)) if k == key) {
                return (bucket, pos);
            }
            bucket = (bucket + 1) & mask;
        }
    }

    fn insert_absent<K: std::hash::Hash>(&mut self, key: &K, pos: usize) {
        let mut bucket = self.start(key);
        while self.buckets[bucket] != EMPTY {
            bucket = (bucket + 1) & (self.buckets.len() - 1);
        }
        self.buckets[bucket] = pos;
    }
}

impl<K, V> Default for ValueMap<K, V> {
    fn default() -> Self {
        Self { entries: Vec::new(), spill: None }
    }
}

pub struct ValueMapIter<'a, K, V>(std::iter::Enumerate<std::slice::Iter<'a, Slot<K, V>>>, usize);
pub struct ValueMapIterMut<'a, K, V>(std::iter::Enumerate<std::slice::IterMut<'a, Slot<K, V>>>, usize);

impl<'a, K, V> Iterator for ValueMapIter<'a, K, V> {
    type Item = (&'a K, &'a V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((pos, slot)) = self.0.next() {
            match slot {
                Slot::Live((k, v)) => {
                    self.1 -= 1;
                    return Some((k, v));
                }
                Slot::Hole { end, .. } if *end > pos + 1 => { self.0.nth(*end - pos - 2); }
                Slot::Hole { .. } => {}
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.1, Some(self.1))
    }
}

impl<'a, K, V> Iterator for ValueMapIterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((pos, slot)) = self.0.next() {
            match slot {
                Slot::Live((k, v)) => {
                    self.1 -= 1;
                    return Some((&*k, v));
                }
                Slot::Hole { end, .. } if *end > pos + 1 => { self.0.nth(*end - pos - 2); }
                Slot::Hole { .. } => {}
            }
        }
        None
    }
}

impl<K: Eq + std::hash::Hash + Copy, V> ValueMap<K, V> {
    #[inline]
    fn position(&self, key: &K) -> Option<usize> {
        if let Some(index) = self.spill.as_ref().filter(|index| !index.buckets.is_empty()) {
            let (_, pos) = index.find(&self.entries, key);
            return (pos != EMPTY).then_some(pos);
        }
        self.entries.iter().position(|slot| matches!(slot, Slot::Live((k, _)) if k == key))
    }

    #[inline]
    pub fn get(&self, key: &K) -> Option<&V> {
        match &self.entries[self.position(key)?] {
            Slot::Live((_, v)) => Some(v),
            Slot::Hole { .. } => unreachable!(),
        }
    }

    #[inline]
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let pos = self.position(key)?;
        match &mut self.entries[pos] {
            Slot::Live((_, v)) => Some(v),
            Slot::Hole { .. } => unreachable!(),
        }
    }

    #[inline]
    fn live_at_or_after(&self, mut pos: usize) -> Option<(usize, &(K, V))> {
        loop {
            match self.entries.get(pos)? {
                Slot::Live(pair) => return Some((pos, pair)),
                Slot::Hole { end, .. } => pos = *end,
            }
        }
    }

    pub fn get_index(&self, rank: usize) -> Option<&(K, V)> {
        let pos = if let Some(index) = self.spill.as_ref().filter(|index| index.holes != 0) {
            *index.ranks.get_or_init(|| {
                let mut ranks = Vec::with_capacity(self.len());
                let mut from = 0;
                while let Some((pos, _)) = self.live_at_or_after(from) {
                    ranks.push(pos);
                    from = pos + 1;
                }
                ranks
            }).get(rank)?
        } else {
            rank
        };
        match self.entries.get(pos)? {
            Slot::Live(pair) => Some(pair),
            Slot::Hole { .. } => unreachable!(),
        }
    }

    /// The frame holds the slot number. Reuse slots of loops unwound by a return or
    /// exception as well as those closed by the normal end/break opcode.
    pub(crate) fn open_cursor(&mut self, thread: usize, frame: usize, active: impl Fn(usize, usize, usize) -> bool) -> usize {
        let index = self.spill.get_or_insert_with(Default::default);
        let cursor = MapCursor { thread, frame, position: 0, visited: 0, generation: index.generation };
        if let Some((id, slot)) = index.cursors.iter_mut().enumerate().find(|(id, slot)| {
            slot.as_ref().is_none_or(|old| !active(old.thread, old.frame, *id))
        }) {
            *slot = Some(cursor);
            id
        } else {
            index.cursors.push(Some(cursor));
            index.cursors.len() - 1
        }
    }

    pub(crate) fn close_cursor(&mut self, id: usize, thread: usize, frame: usize) {
        let Some(index) = &mut self.spill else { return };
        let Some(slot) = index.cursors.get_mut(id) else { return };
        if slot.as_ref().is_some_and(|cursor| cursor.thread == thread && cursor.frame == frame) {
            *slot = None;
            // A read-only loop over a small object only needs this allocation while it runs.
            if index.buckets.is_empty() && index.holes == 0 && index.cursors.iter().all(Option::is_none) {
                self.spill = None;
            }
        }
    }

    #[inline]
    pub(crate) fn cursor_next(&mut self, id: usize) -> Option<&(K, V)> {
        let index = self.spill.as_mut()?;
        let cursor = index.cursors.get_mut(id)?.as_mut()?;
        let mut pos = cursor.next_position(index.generation);
        cursor.position = pos;
        cursor.generation = index.generation;
        loop {
            match self.entries.get(pos)? {
                Slot::Live(pair) => {
                    cursor.position = pos + 1;
                    cursor.visited += 1;
                    return Some(pair);
                }
                Slot::Hole { end, .. } => pos = *end,
            }
        }
    }

    #[inline]
    pub fn contains_key(&self, key: &K) -> bool {
        self.position(key).is_some()
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(index) = self.spill.as_mut().filter(|index| !index.buckets.is_empty()) {
            let (bucket, pos) = index.find(&self.entries, &key);
            if pos != EMPTY {
                let Slot::Live((_, old)) = &mut self.entries[pos] else { unreachable!() };
                return Some(std::mem::replace(old, value));
            }
            let live = self.entries.len() - index.holes;
            if index.holes <= live && index.fits(live + 1) {
                let pos = self.entries.len();
                index.buckets[bucket] = pos;
                if let Some(ranks) = index.ranks.get_mut() { ranks.push(pos); }
                self.entries.push(Slot::Live((key, value)));
                return None;
            }
        } else if let Some(old) = self.get_mut(&key) {
            return Some(std::mem::replace(old, value));
        }
        self.insert_absent(key, value);
        None
    }

    // Rebuild with the insertion's required capacity. Counting the surviving prefix at
    // every old boundary also remaps any number of active loops in O(slots + cursors).
    fn compact_for_insert(&mut self, live: usize) {
        let index = self.spill.as_mut().unwrap();
        let mut prefix = if index.cursors.iter().all(Option::is_none) {
            None
        } else {
            Some(Vec::with_capacity(self.entries.len() + 1))
        };
        let want = live + 1;
        let buckets = if want <= SPILL_AT { 0 } else { SpillIndex::bucket_count(want) };
        if buckets == index.buckets.len() {
            index.buckets.fill(EMPTY);
        } else {
            // A previously large table must not be cleared at its old size on every
            // subsequent compaction of a now-small map.
            index.buckets = vec![EMPTY; buckets];
        }
        let mut pos = 0;
        self.entries.retain(|slot| {
            if let Some(prefix) = &mut prefix { prefix.push(pos); }
            if let Slot::Live((key, _)) = slot {
                if !index.buckets.is_empty() { index.insert_absent(key, pos); }
                pos += 1;
                true
            } else {
                false
            }
        });
        if let Some(mut prefix) = prefix {
            prefix.push(pos);
            for cursor in index.cursors.iter_mut().flatten() {
                cursor.visited = prefix[cursor.next_position(index.generation)];
                cursor.generation = index.generation;
            }
        }
        index.generation = index.generation.wrapping_add(1);
        index.holes = 0;
        index.ranks.take();
    }

    pub fn reserve(&mut self, additional: usize) {
        // If the next append will compact, its capacity can include the reclaimed slots.
        let holes = self.spill.as_ref().map_or(0, |index| index.holes);
        self.entries.reserve(if holes > self.len() { additional.saturating_sub(holes) } else { additional });
        let want = self.len() + additional;
        if want > SPILL_AT {
            let index = self.spill.get_or_insert_with(Default::default);
            if !index.fits(want) {
                index.buckets = vec![EMPTY; SpillIndex::bucket_count(want)];
                for (pos, slot) in self.entries.iter().enumerate() {
                    if let Slot::Live((key, _)) = slot {
                        index.insert_absent(key, pos);
                    }
                }
            }
        }
    }

    /// Insert a key the caller just checked is absent.
    #[inline]
    pub fn insert_absent(&mut self, key: K, value: V) {
        let live = self.len();
        if self.entries.len() - live > live {
            // Appending would exceed the logical slot budget, even if the allocation has room.
            self.compact_for_insert(live);
        }
        if live >= SPILL_AT && !self.spill.as_ref().is_some_and(|index| index.fits(live + 1)) {
            self.reserve(1);
        }
        if let Some(index) = &mut self.spill {
            if !index.buckets.is_empty() { index.insert_absent(&key, self.entries.len()); }
            if let Some(ranks) = index.ranks.get_mut() { ranks.push(self.entries.len()); }
        }
        self.entries.push(Slot::Live((key, value)));
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let indexed = self.spill.as_ref().filter(|s| !s.buckets.is_empty())
            .map(|index| index.find(&self.entries, key));
        let pos = match indexed {
            Some((_, EMPTY)) => return None,
            Some((_, pos)) => pos,
            None => self.position(key)?,
        };
        let Some(index) = &mut self.spill else {
            let Slot::Live((_, value)) = self.entries.remove(pos) else { unreachable!() };
            return Some(value);
        };
        if let Some((mut hole, _)) = indexed {
            // Close the probe chain, moving only the neighbors that probed past the hole.
            let mask = index.buckets.len() - 1;
            let mut next = (hole + 1) & mask;
            while index.buckets[next] != EMPTY {
                let entry = index.buckets[next];
                let Slot::Live((key, _)) = &self.entries[entry] else { unreachable!() };
                let home = index.start(key);
                if (hole.wrapping_sub(home) & mask) < (next.wrapping_sub(home) & mask) {
                    index.buckets[hole] = entry;
                    hole = next;
                }
                next = (next + 1) & mask;
            }
            index.buckets[hole] = EMPTY;
        }

        // Join adjacent runs of holes. A walk jumps over the whole run, even after
        // many deletes with no intervening insert to trigger a compaction.
        let mut start = pos;
        let mut end = pos + 1;
        if let Some(Slot::Hole { start: left, .. }) = pos.checked_sub(1).and_then(|i| self.entries.get(i)) {
            start = *left;
        }
        if let Some(Slot::Hole { end: right, .. }) = self.entries.get(end) {
            end = *right;
        }
        let old = std::mem::replace(&mut self.entries[pos], Slot::Hole { start, end });
        self.entries[start] = Slot::Hole { start, end };
        self.entries[end - 1] = Slot::Hole { start, end };
        index.holes += 1;
        index.ranks.take();
        let Slot::Live((_, value)) = old else { unreachable!() };
        Some(value)
    }

    pub fn retained_bytes(&self) -> usize {
        let entries = self.entries.capacity() * std::mem::size_of::<Slot<K, V>>();
        entries.saturating_add(self.spill.as_ref().map_or(0, |index| {
            let words = index.buckets.capacity() + index.ranks.get().map_or(0, Vec::capacity);
            std::mem::size_of::<SpillIndex>() + words * std::mem::size_of::<usize>()
                + index.cursors.capacity() * std::mem::size_of::<Option<MapCursor>>()
        }))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len() - self.spill.as_ref().map_or(0, |s| s.holes)
    }

    #[cfg(test)]
    pub(crate) fn hole_count(&self) -> usize {
        self.spill.as_ref().map_or(0, |index| index.holes)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn clear(&mut self) {
        if let Some(index) = &mut self.spill {
            self.entries = Vec::new();
            if index.cursors.iter().all(Option::is_none) {
                self.spill = None;
            } else {
                for cursor in index.cursors.iter_mut().flatten() {
                    cursor.visited = 0;
                    cursor.generation = index.generation;
                }
                index.generation = index.generation.wrapping_add(1);
                index.buckets = Vec::new();
                index.holes = 0;
                index.ranks.take();
            }
        } else {
            self.entries.clear();
        }
    }

    #[inline]
    pub fn iter(&self) -> ValueMapIter<'_, K, V> {
        ValueMapIter(self.entries.iter().enumerate(), self.len())
    }

    #[inline]
    pub fn iter_mut(&mut self) -> ValueMapIterMut<'_, K, V> {
        let live = self.len();
        ValueMapIterMut(self.entries.iter_mut().enumerate(), live)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }
}

impl<'a, K, V> IntoIterator for &'a ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy,
{
    type Item = (&'a K, &'a V);
    type IntoIter = ValueMapIter<'a, K, V>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K, V> Index<K> for ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>,
{
    type Output = V;
    #[inline]
    fn index(&self, index: K) -> &Self::Output {
        self.get(&index).unwrap()
    }
}

impl<K, V> IndexMut<K> for ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>,
{
    #[inline]
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.get_mut(&index).unwrap()
    }
}
