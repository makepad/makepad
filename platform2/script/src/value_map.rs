

use crate::makepad_live_id::LiveId;

use {
    std::{
        ops::{Index, IndexMut, Deref, DerefMut},
        collections::{HashMap},
    }
};

// Idea taken from the `nohash_hasher` crate.
#[derive(Default)]
pub struct ValueHasher(u64);

impl std::hash::Hasher for ValueHasher {
    fn write(&mut self, _: &[u8]) {unreachable!();}
    fn write_u8(&mut self, _n: u8) {unreachable!();}
    fn write_u16(&mut self, _n: u16) {unreachable!();}
    fn write_u32(&mut self, _n: u32) {unreachable!();}
    #[inline(always)]
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
    fn write_usize(&mut self, _n: usize) {unreachable!();}
    fn write_i8(&mut self, _n: i8) {unreachable!();}
    fn write_i16(&mut self, _n: i16){unreachable!();}
    fn write_i32(&mut self, _n: i32){unreachable!();}
    fn write_i64(&mut self, _n: i64){unreachable!();}
    fn write_isize(&mut self, _n: isize){unreachable!();}
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Copy, Clone, Default)]
pub struct ValueHasherBuilder {}

impl std::hash::BuildHasher for ValueHasherBuilder {
    type Hasher = ValueHasher;
                
    #[inline(always)]
    fn build_hasher(&self) -> ValueHasher {
        ValueHasher::default()
    }
}

#[derive(Clone, Debug)]
pub struct ValueMap<K, V> {
    map: HashMap<K, V, ValueHasherBuilder>,
    //alloc_set: HashSet<K, LiveIdHasherBuilder>
}

impl<K, V> Default for ValueMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId> + std::fmt::Debug {
    fn default() -> Self {
        Self {
            map: HashMap::with_hasher(ValueHasherBuilder {}),
            //alloc_set: HashSet::with_hasher(LiveIdHasherBuilder {})
        }
    }
}

impl<K, V> Deref for ValueMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    type Target = HashMap<K, V, ValueHasherBuilder>;
    fn deref(&self) -> &Self::Target {&self.map}
}

impl<K, V> DerefMut for ValueMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.map}
}

impl<K, V> Index<K> for ValueMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    type Output = V;
    fn index(&self, index: K) -> &Self::Output {
        self.map.get(&index).unwrap()
    }
}

impl<K, V> IndexMut<K> for ValueMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.map.get_mut(&index).unwrap()
    }
}