#![allow(dead_code)]

use {
    std::{
        ops::{Index, IndexMut, Deref, DerefMut},
        collections::{HashMap, HashSet},
        collections::hash_map::Entry,
        sync::Once,
        fmt,
        cmp::Ordering,
    }
};


impl LiveIdInterner {
    pub fn add(&mut self, val: &str) {
        self.id_to_string.insert(LiveId::from_str_unchecked(val), val.to_string());
    }
    
    pub fn contains(&mut self, val: &str) -> bool {
        self.id_to_string.contains_key(&LiveId::from_str_unchecked(val))
    }
    
    pub fn with<F, R>(f: F) -> R
    where
    F: FnOnce(&mut Self) -> R,
    {
        static mut IDMAP: Option<LiveIdInterner> = None;
        static ONCE: Once = Once::new();
        ONCE.call_once( || unsafe {
            let mut map = LiveIdInterner {
                alloc: 0,
                id_to_string: HashMap::new()
            };
            // pre-seed list for debugging purposes
            let fill = [
                "default",
                "exp",
                "void",
                "true",
                "false",
                "use",
                "#",
                "$",
                "@",
                "^",
                "^=",
                "|",
                "||",
                "|=",
                "%",
                "%=",
                "!=",
                "!",
                "&&",
                "*=",
                "*",
                "+=",
                "+",
                ",",
                "-=",
                "->",
                "-",
                "..",
                "...",
                "..=",
                ".",
                "/=",
                "/",
                "::",
                ":",
                ";",
                "<=",
                "<",
                "<<",
                "<<=",
                "==",
                "=",
                ">=",
                "=>",
                ">",
                ">>",
                ">>=",
                "?",
                "tracks",
                "state",
                "state_id",
                "user",
                "play",
                "ended"
            ];
            for item in &fill {
                if map.contains(item) {
                    eprintln!("WE HAVE AN ID COLLISION!");
                }
                map.add(item);
            }
            IDMAP = Some(map)
        });
        f(unsafe {IDMAP.as_mut().unwrap()})
    }
}

#[derive(Clone, Default, Eq, Hash, Copy, PartialEq)]
pub struct LiveId(pub u64);

pub const LIVE_ID_SEED:u64 = 0xd6e8_feb8_6659_fd93;

impl LiveId {
    pub fn empty() -> Self {
        Self (0)
    }
    
    pub fn is_unique(&self) -> bool {
        (self.0 & 0x8000_0000_0000_0000) == 0 && self.0 != 0
    }
    
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    
    // from https://nullprogram.com/blog/2018/07/31/
    // i have no idea what im doing with start value and finalisation.
    pub const fn from_bytes(seed:u64, id_bytes: &[u8], start: usize, end: usize) -> Self {
        let mut x = seed;
        let mut i = start;
        while i < end {
            x = x.overflowing_add(id_bytes[i] as u64).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            i += 1;
        }
        // mark high bit as meaning that this is a hash id
        return Self ((x & 0x7fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000)
    }
    
    pub const fn from_str_unchecked(id_str: &str) -> Self {
        let bytes = id_str.as_bytes();
        Self::from_bytes(LIVE_ID_SEED, bytes, 0, bytes.len())
    }
    
    pub const fn from_str_num_unchecked(id_str: &str, num:u64) -> Self {
        let bytes = id_str.as_bytes();
        let id = Self::from_bytes(LIVE_ID_SEED, bytes, 0, bytes.len());
        Self::from_bytes(id.0, &num.to_be_bytes(), 0, 8)
    }
    
    pub const fn from_num_unchecked(seed:u64, num:u64) -> Self {
        Self::from_bytes(seed, &num.to_be_bytes(), 0, 8)
    }
    
    pub fn from_str(id_str: &str) -> Result<Self,
    String> {
        let id = Self::from_str_unchecked(id_str);
        LiveIdInterner::with( | idmap | {
            if let Some(stored) = idmap.id_to_string.get(&id) {
                if stored != id_str {
                    return Err(stored.clone())
                }
            }
            else {
                idmap.id_to_string.insert(id, id_str.to_string());
            }
            return Ok(id)
        })
    }
    
    pub fn from_str_num(id_str: &str, num:u64) -> Result<Self,
    String> {
        let id = Self::from_str_num_unchecked(id_str, num);
        LiveIdInterner::with( | idmap | {
            idmap.id_to_string.insert(id, format!("{}{}",id_str, num));
            return Ok(id)
        })
    }
    
    pub fn as_string<F, R>(&self, f: F) -> R
    where F: FnOnce(Option<&String>) -> R
    {
        LiveIdInterner::with( | idmap | {
            return f(idmap.id_to_string.get(self))
        })
    }

    pub fn unique() -> Self {
        LiveIdInterner::with( | idmap | {
            // cycle the hash
            idmap.alloc += 1;//idmap.gen_hash.add_id(idmap.gen_hash);
            LiveId(idmap.alloc)
        })
    }
}

impl Ord for LiveId {
    fn cmp(&self, other: &LiveId) -> Ordering {
        LiveIdInterner::with( | idmap | {
            if let Some(id1) = idmap.id_to_string.get(self) {
                if let Some(id2) = idmap.id_to_string.get(other) {
                    return id1.cmp(id2)
                }
            }
            return Ordering::Equal
        })
    }
}

impl PartialOrd for LiveId {
    fn partial_cmp(&self, other: &LiveId) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Debug for LiveId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for LiveId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if *self == LiveId::empty() {
            write!(f, "0")
        }
        else if self.is_unique(){
            write!(f, "UniqueId {}", self.0)
        }
        else{
            self.as_string( | string | {
                if let Some(id) = string {
                    write!(f, "{}", id)
                }
                else {
                    write!(f, "IdNotFound {:016x}", self.0)
                }
            })
        }
    }
}

impl fmt::LowerHex for LiveId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}


pub struct LiveIdInterner {
    alloc: u64,
    id_to_string: HashMap<LiveId, String>,
}

// ----------------------------------------------------------------------------

// Idea taken from the `nohash_hasher` crate.
#[derive(Default)]
pub struct LiveIdHasher(u64);

impl std::hash::Hasher for LiveIdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("Invalid use of IdHasher");
    }
    
    fn write_u8(&mut self, _n: u8) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_u16(&mut self, _n: u16) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_u32(&mut self, _n: u32) {
        unreachable!("Invalid use of IdHasher");
    }
    
    #[inline(always)]
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
    
    fn write_usize(&mut self, _n: usize) {
        unreachable!("Invalid use of IdHasher");
    }
    
    fn write_i8(&mut self, _n: i8) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_i16(&mut self, _n: i16) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_i32(&mut self, _n: i32) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_i64(&mut self, _n: i64) {
        unreachable!("Invalid use of IdHasher");
    }
    fn write_isize(&mut self, _n: isize) {
        unreachable!("Invalid use of IdHasher");
    }
    
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Copy, Clone, Default)]
pub struct LiveIdHasherBuilder {}

impl std::hash::BuildHasher for LiveIdHasherBuilder {
    type Hasher = LiveIdHasher;
    
    #[inline(always)]
    fn build_hasher(&self) -> LiveIdHasher {
        LiveIdHasher::default()
    }
}

#[derive(Clone, Debug)]
pub struct LiveIdMap<K, V> {
    map: HashMap<K, V, LiveIdHasherBuilder>,
    alloc_set: HashSet<K, LiveIdHasherBuilder>
}

impl<K, V> Default for LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId> + std::fmt::Debug {
    fn default() -> Self {
        Self {
            map: HashMap::with_hasher(LiveIdHasherBuilder {}),
            alloc_set: HashSet::with_hasher(LiveIdHasherBuilder {})
        }
    }
}

impl<K, V> LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId> + std::fmt::Debug
{
    pub fn new() -> Self {Self::default()}
    
    pub fn alloc_key(&mut self) -> K {
        loop {
            let new_id = LiveId::unique().into();
            if self.map.get(&new_id).is_none() && !self.alloc_set.contains(&new_id) {
                self.alloc_set.insert(new_id);
                return new_id.into()
            }
        }
    }
    
    pub fn insert_unique(&mut self, value: V) -> K {
        loop {
            let new_id = LiveId::unique().into();
            if self.alloc_set.contains(&new_id) {
                continue
            }
            match self.map.entry(new_id) {
                Entry::Occupied(_) => continue,
                Entry::Vacant(v) => {
                    
                    v.insert(value);
                    return new_id
                }
            }
        }
    }
    
    pub fn insert(&mut self, k: impl Into<K>, value: V) {
        let k = k.into();
        self.alloc_set.remove(&k);
        match self.map.entry(k) {
            Entry::Occupied(_) => panic!("Item {:?} already inserted",k),
            Entry::Vacant(v) => v.insert(value)
        };
    }
}

impl<K, V> Deref for LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    type Target = HashMap<K, V, LiveIdHasherBuilder>;
    fn deref(&self) -> &Self::Target {&self.map}
}

impl<K, V> DerefMut for LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.map}
}

impl<K, V> Index<K> for LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    type Output = V;
    fn index(&self, index: K) -> &Self::Output {
        self.map.get(&index).unwrap()
    }
}

impl<K, V> IndexMut<K> for LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.map.get_mut(&index).unwrap()
    }
}
