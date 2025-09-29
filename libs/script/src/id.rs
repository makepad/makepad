#![allow(dead_code)]

use {
    std::{
        sync::atomic::{AtomicU64, Ordering},
        ops::{Index, IndexMut, Deref, DerefMut},
        collections::{HashMap},
        sync::Once,
        sync::Mutex,
        fmt,
    }
};
use crate::value::*;

impl IdToString {
    pub fn add(&mut self, val: &str) {
        self.id_to_string.insert(Id::from_str(val), val.to_string());
    }
        
    pub fn contains(&mut self, val: &str) -> bool {
        self.id_to_string.contains_key(&Id::from_str(val))
    }
        
    pub fn with<F, R>(f: F) -> R
    where
    F: FnOnce(&mut Self) -> R,
    {
        static IDMAP: Mutex<Option<IdToString>> = Mutex::new(None);
        static ONCE: Once = Once::new();
        ONCE.call_once( ||{
            let map = IdToString {
                id_to_string: HashMap::new()
            };
            *IDMAP.lock().unwrap() = Some(map)
        });
        let mut idmap = IDMAP.lock().unwrap();
        f(idmap.as_mut().unwrap())
    }
}

#[derive(Clone, Default, Eq, Hash, Copy, Ord, PartialOrd, PartialEq)]
pub struct Id(pub u64);

pub const ID_SEED:u64 = 0xd6e8_feb8_6659_fd93;

impl Id {
    pub fn empty() -> Self {
        Self (0)
    }
    
    pub fn from_lo_hi(lo:u32, hi:u32)->Self{
        Self( (lo as u64) | ((hi as u64)<<32) )
    }
    
    pub fn lo(&self)->u32{
        (self.0&0xffff_ffff) as u32
    }
    
    pub fn hi(&self)->u32{
        (self.0>>32) as u32
    }
    
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
        
    pub const fn to_value(self)->Value{
        Value::from_id(self)
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
        // truncate to 47 bits fitting in a NaN box
        Self (x & 0x0000_7fff_ffff_ffff)
    }
        
    pub const fn from_str(id_str: &str) -> Self {
        let bytes = id_str.as_bytes();
        Self::from_bytes(ID_SEED, bytes, 0, bytes.len())
    }
        
    pub const fn from_bytes_lc(seed:u64, id_bytes: &[u8], start: usize, end: usize) -> Self {
        let mut x = seed;
        let mut i = start;
        while i < end {
            let byte = id_bytes[i];
            let byte = if byte >= 65 && byte <=90{
                byte + 32
            }
            else{
                byte
            };
            x = x.overflowing_add(byte as u64).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            i += 1;
        }
        Self (x & 0x0000_7fff_ffff_ffff)
    }
        
    pub const fn from_num(seed:u64, num:u64) -> Self {
        Self::from_bytes(seed, &num.to_be_bytes(), 0, 8)
    }
    
    pub const fn from_str_lc(id_str: &str) -> Self {
        let bytes = id_str.as_bytes();
        Self::from_bytes_lc(ID_SEED, bytes, 0, bytes.len())
    }
        
    pub const fn str_append(self, id_str: &str) -> Self {
        let bytes = id_str.as_bytes();
        Self::from_bytes(self.0, bytes, 0, bytes.len())
    }
    
    pub const fn bytes_append(self, bytes: &[u8]) -> Self {
        Self::from_bytes(self.0, bytes, 0, bytes.len())
    }
        
    pub const fn id_append(self, id: Id) -> Self {
        let bytes = id.0.to_be_bytes();
        Self::from_bytes(self.0, &bytes, 0, bytes.len())
    }
        
    pub const fn num_append(self, num:u64) -> Self {
        let bytes = num.to_be_bytes();
        Self::from_bytes(self.0, &bytes, 0, bytes.len())
    }
    
    pub fn from_str_with_lut(id_str: &str) -> Result<Self,
    String> {
        let id = Self::from_str(id_str);
        IdToString::with( | idmap | {
            if let Some(stored) = idmap.id_to_string.get(&id) {
                if stored != id_str {
                    return Err(stored.clone())
                }
            }
            else {
                idmap.id_to_string.insert(id, id_str.to_string());
            }
            Ok(id)
        })
    }
    
    pub fn as_string<F, R>(&self, f: F) -> R
    where F: FnOnce(Option<&str>) -> R
    {
        IdToString::with( | idmap | {
            match idmap.id_to_string.get(self){
                Some(v)=>f(Some(v)),
                None=>f(None)
            }
        })
    }
    
    pub fn counted() -> Self {
        Id(COUNTED_ID.fetch_add(1, Ordering::SeqCst))
    }
}
 
pub (crate) static COUNTED_ID: AtomicU64 = AtomicU64::new(1);

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if *self == Id::empty() {
            return write!(f, "0");
        }
        self.as_string( | string | {
            if let Some(id) = string {
                write!(f, "{}", id)
            }
            else {
                write!(f, "{:016x}", self.0)
            }
        })
    }
}

impl fmt::LowerHex for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}


pub struct IdToString {
    //alloc: u64,
    id_to_string: HashMap<Id, String>,
}

// ----------------------------------------------------------------------------

// Idea taken from the `nohash_hasher` crate.
#[derive(Default)]
pub struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
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
pub struct IdHasherBuilder {}

impl std::hash::BuildHasher for IdHasherBuilder {
    type Hasher = IdHasher;
        
    #[inline(always)]
    fn build_hasher(&self) -> IdHasher {
        IdHasher::default()
    }
}

#[derive(Clone, Debug)]
pub struct IdMap<K, V> {
    map: HashMap<K, V, IdHasherBuilder>,
    //alloc_set: HashSet<K, LiveIdHasherBuilder>
}

impl<K, V> Default for IdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<Id> + std::fmt::Debug {
    fn default() -> Self {
        Self {
            map: HashMap::with_hasher(IdHasherBuilder {}),
            //alloc_set: HashSet::with_hasher(LiveIdHasherBuilder {})
        }
    }
}

impl<K, V> Deref for IdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<Id>
{
    type Target = HashMap<K, V, IdHasherBuilder>;
    fn deref(&self) -> &Self::Target {&self.map}
}

impl<K, V> DerefMut for IdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<Id>
{
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.map}
}

impl<K, V> Index<K> for IdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<Id>
{
    type Output = V;
    fn index(&self, index: K) -> &Self::Output {
        self.map.get(&index).unwrap()
    }
}

impl<K, V> IndexMut<K> for IdMap<K, V>
where K: std::cmp::Eq + std::hash::Hash + Copy + From<Id>
{
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.map.get_mut(&index).unwrap()
    }
}
