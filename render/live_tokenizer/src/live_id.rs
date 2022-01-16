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
                gen_hash: LiveId(0xd6e8_feb8_6659_fd93),
                id_to_string: HashMap::new()
            };
            // pre-seed list for debugging purposes
            let fill = [
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

impl LiveId {
    pub fn empty() -> Self {
        Self (0)
    }
    
    // doing this cuts the hashsize but yolo.
    pub fn with_num(&self, num: u32) -> Self {
        Self (self.0 & 0xffff_ffff_0000_0000 | (num as u64))
    }
    
    pub fn mask_num(&self) -> Self {
        Self (self.0 & 0xffff_ffff_0000_0000)
    }
    
    pub fn get_num(&self) -> u32 {
        (self.0 & 0xffff_ffff) as u32
    }
    
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    
    pub fn is_capitalised(&self) -> bool {
        self.0 & 0x8000_0000_0000_0000 != 0
    }
    
    
    // from https://nullprogram.com/blog/2018/07/31/
    // i have no idea what im doing with start value and finalisation.
    pub const fn from_bytes(id_bytes: &[u8], start: usize, end: usize) -> Self {
        let mut x = 0xd6e8_feb8_6659_fd93u64;
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
        // use high bit to mark id as capitalised
        if id_bytes[0] >= 'A' as u8 && id_bytes[0] <= 'Z' as u8 {
            return Self (x | 0x8000_0000_0000_0000)
        }
        else {
            return Self (x & 0x7fff_ffff_ffff_ffff)
        }
    }
    
    // merges 2 ids in a nonsymmetric fashion
    
    pub const fn add_id(&self, id: LiveId) -> Self {
        //let id_len = id_bytes.len();
        let mut x = id.0;
        x = x.overflowing_add(self.0).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        return Self (x) // leave the first bit
    }
    
    pub const fn from_str_unchecked(id_str: &str) -> Self {
        let bytes = id_str.as_bytes();
        Self::from_bytes(bytes, 0, bytes.len())
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
    
    pub fn as_string<F, R>(&self, f: F) -> R
    where F: FnOnce(Option<&String>) -> R
    {
        LiveIdInterner::with( | idmap | {
            return f(idmap.id_to_string.get(self))
        })
    }
    
    pub fn gen() -> Self {
        LiveIdInterner::with( | idmap | {
            // cycle the hash
            idmap.gen_hash = idmap.gen_hash.add_id(idmap.gen_hash);
            idmap.gen_hash
        })
    }
    
    pub fn gen_with_input(&self) -> Self {
        LiveIdInterner::with( | idmap | {
            // cycle the hash
            idmap.gen_hash = idmap.gen_hash.add_id(*self);
            idmap.gen_hash
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
        else {
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
    gen_hash: LiveId,
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

#[derive(Copy, Clone, Debug, Default)]
pub struct LiveIdHasherBuilder {}

impl std::hash::BuildHasher for LiveIdHasherBuilder {
    type Hasher = LiveIdHasher;
    
    #[inline(always)]
    fn build_hasher(&self) -> LiveIdHasher {
        LiveIdHasher::default()
    }
}

#[derive(Clone, Debug)]
pub struct LiveIdMap<K, V>{
    map:HashMap<K, V, LiveIdHasherBuilder>,
    alloc_set: HashSet<K, LiveIdHasherBuilder>
}

impl<K, V> Default for LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId> {
    fn default() -> Self {
        Self {
            map: HashMap::with_hasher(LiveIdHasherBuilder {}),
            alloc_set: HashSet::with_hasher(LiveIdHasherBuilder {})
        }
    }
}

impl<K, V> LiveIdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>
{
    pub fn new() -> Self {Self::default()}
    
    pub fn alloc_key(&mut self) -> K {
        loop {
            let new_id = LiveId::gen().into();
            if self.map.get(&new_id).is_none() && !self.alloc_set.contains(&new_id){
                self.alloc_set.insert(new_id);
                return new_id.into()
            }
        }
    }
    
    pub fn insert_unique(&mut self, value: V) -> K {
        loop {
            let new_id = LiveId::gen().into();
            if self.alloc_set.contains(&new_id){
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
            Entry::Occupied(_) => panic!(),
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
