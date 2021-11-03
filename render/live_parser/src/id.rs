#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Once;
use std::fmt;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialOrd, Hash, PartialEq)]
pub struct FileId(pub u16);

impl FileId{
    pub fn index(index:usize)->FileId{FileId(index as u16)}
    pub fn to_index(&self) -> usize{self.0 as usize}
}

#[derive(Clone, Eq, Hash, Copy, PartialEq)]
pub struct MultiPack(pub u64);

//TODO FIX THIS THING TO BE N LEVELS OF MODULES
#[derive(Clone, Eq, Hash, Debug, Copy, PartialEq)]
pub struct ModulePath(pub Id, pub Id);

impl ModulePath{
    pub const fn from_str_unchecked(module_path: &str)->Self{
        // ok lets split off the first 2 things from module_path
        let bytes = module_path.as_bytes();
        let len = bytes.len();
        // we have to find the first :
        let mut crate_id = Id(0);
        let mut i = 0;
        while i < len {
            if bytes[i] == ':' as u8{
                crate_id = Id::from_bytes(bytes, 0, i);
                i+=2;
                break
            }
            i+=1;
        }
        if i == len{ // module_path is only one thing
            return ModulePath(Id(0), Id::from_bytes(bytes, 0, len));
        }
        let module_start = i;
        while i < len {
            if bytes[i] == ':' as u8{
                break
            }
            i+=1;
        }
        return ModulePath(crate_id, Id::from_bytes(bytes, module_start, i));
    } 
    
    pub fn from_str(module_path: &str)->Result<Self, String>{
        // ok lets split off the first 2 things from module_path
        let bytes = module_path.as_bytes();
        let len = bytes.len();
        // we have to find the first :
        let mut crate_id = Id(0);
        let mut i = 0;
        while i < len {
            if bytes[i] == ':' as u8{
                crate_id = Id::from_str(std::str::from_utf8(&bytes[0..i]).unwrap())?;
                i+=2;
                break
            }
            i+=1;
        }
        if i == len{ // module_path is only one thing
            return Ok(ModulePath(Id(0), Id::from_str(std::str::from_utf8(&bytes[0..len]).unwrap())?));
        }
        let module_start = i;
        while i < len {
            if bytes[i] == ':' as u8{
                break
            }
            i+=1;
        }
        return Ok(ModulePath(crate_id, Id::from_str(std::str::from_utf8(&bytes[module_start..i]).unwrap())?));
    }
}

impl fmt::Display for ModulePath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}::{}", self.0, self.1)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialOrd, Copy, PartialEq)]
pub struct LocalPtr {
    pub level: usize,
    pub index: usize
}

#[derive(Clone, Debug, Eq, Hash, Copy, Ord, PartialOrd, PartialEq)]
pub struct LivePtr {
    pub file_id: FileId,
    pub local_ptr: LocalPtr,
}


impl fmt::Display for LivePtr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}_{}_{}", self.file_id.0, self.local_ptr.level, self.local_ptr.index)
    }
}


#[derive(Debug)]
pub enum MultiUnpack {
    Empty,
    SingleId(Id),
    MultiId{index: usize, count: usize},
    LivePtr (LivePtr),
    ZeroClass,
}

#[derive(Clone, Default, Eq, Hash, Copy, PartialEq)]
pub struct Id(pub u64);

impl Id{
    pub fn empty()->Self{
        Self(0)
    }
    
    pub fn is_empty(&self)->bool{
        self.0 == 0
    }
    
    // from https://nullprogram.com/blog/2018/07/31/
    // i have no idea what im doing with start value and finalisation.
    pub const fn from_str_unchecked(id_str: &str) -> Self {
        let bytes = id_str.as_bytes();
        Self::from_bytes(bytes, 0, bytes.len())
    }
    
    pub const fn from_bytes(id_bytes: &[u8], start:usize, end:usize) -> Self {
        //let id_len = id_bytes.len();
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
        return Self(x & 0x7fff_ffff_ffff_ffff) // leave the first bit
    }
    
    pub fn from_str(id_str: &str)->Result<Id, String>{
        let id = Self::from_str_unchecked(id_str);
        IdMap::with( | idmap | {
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
    /*
    pub fn panic_collision(self, val: &str) -> Id {
        if let Some(s) = self.check_collision(val) {
            panic!("Collision {} {}", val, s)
        }
        self
    }
    
    pub fn check_collision(&self, val: &str) -> Option<String> {
        IdMap::with( | idmap | {
            if let Some(stored) = idmap.id_to_string.get(self) {
                if stored != val {
                    return Some(stored.clone())
                }
            }
            else {
                idmap.id_to_string.insert(self.clone(), val.to_string());
            }
            return None
        })
    }*/
    
    pub fn as_string<F, R>(&self, f: F) -> R
    where F: FnOnce(Option<&String>) -> R
    {
        IdMap::with( | idmap | {
            return f(idmap.id_to_string.get(self))
        })
    }
}

impl Ord for Id {
    fn cmp(&self, other: &Id) -> Ordering {
        IdMap::with( | idmap | {
            if let Some(id1) = idmap.id_to_string.get(self){
                if let Some(id2) = idmap.id_to_string.get(other){
                    return id1.cmp(id2)
                }
            }
            return Ordering::Equal
        })
    }
}

impl PartialOrd for Id {
    fn partial_cmp(&self, other: &Id) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// MultiPack uses the high 3 bits to signal type
// 0?? = Single Id    0x8 == 0
// 0 = Empty
// 101 = NodePtr   0xA
// 110 = Multi     0xC
// 111 = Number    0xE

impl MultiPack {

    pub fn unpack(&self) -> MultiUnpack {
        if self.0 & 0x8000_0000_0000_0000 != 0 {
            match self.0 & 0xE000_0000_0000_0000 {
                0xA000_0000_0000_0000 => MultiUnpack::LivePtr(LivePtr{
                    file_id: FileId(((self.0 >> 32) & 0xffff) as u16),
                    local_ptr: LocalPtr {
                        level: ((self.0 >> 48) & 0x1fff) as usize,
                        index: (self.0 & 0xffff_ffff)as usize
                    }
                }),
                0xC000_0000_0000_0000 => MultiUnpack::MultiId {
                    index: (self.0 & 0xffff_ffff) as usize,
                    count: ((self.0 & 0x1fff_ffff_ffff_ffff) >> 32) as usize,
                },
                0xE000_0000_0000_0000 => {
                    match self.0{
                        0xE000_0000_0000_0000=>MultiUnpack::ZeroClass,
                        _=>MultiUnpack::Empty
                    }
                }
                _ => MultiUnpack::Empty
            }
        }
        else {
            if self.0 == 0{
                MultiUnpack::Empty
            }
            else{
                MultiUnpack::SingleId(Id(self.0 & 0x7fff_ffff_ffff_ffff))
            }
        }
    }
    
    pub fn multi_id(index: usize, len: usize) -> Self {
        Self(((((len as u64) << 32) | index as u64) & 0x1fff_ffff_ffff_ffff) | 0xC000_0000_0000_0000)
    }
    
    pub fn single_id(id: Id) -> Self {
        Self(id.0)
    }
    
    pub fn empty() -> Self {
        Self(0x0)
    }
    
    pub fn live_ptr(file_id: FileId, ptr: LocalPtr)->Self{
        Self(
            0xA000_0000_0000_0000 |
            (ptr.index as u64) |
            ((file_id.0 as u64) << 32) |
            ((ptr.level as u64) << 48) 
        )
    }

    pub fn zero_class()->Self{
        Self(0xE000_0000_0000_0000)
    }
    
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    
    pub fn is_live_ptr(&self) -> bool {
        self.0 & 0xE000_0000_0000_0000 == 0xA000_0000_0000_0000
    }
        
    pub fn is_multi_id(&self) -> bool {
        self.0 & 0xE000_0000_0000_0000 == 0xC000_0000_0000_0000
    }
    
    pub fn is_zero_class(&self) -> bool {
        self.0 == 0xE000_0000_0000_0000
    }
    
    pub fn is_single_id(&self) -> bool {
        (self.0 & 0x8000_0000_0000_0000) == 0
    }
    
    pub fn unwrap_multi_id(&self) -> (usize, usize) {
        if !self.is_multi_id() {
            panic!()
        }
        (
            (self.0 & 0xffff_ffff) as usize,
            ((self.0 & 0x1fff_ffff_ffff_ffff) >> 32) as usize,
        )
    }
    
    pub fn unwrap_single_id(&self) -> Id {
        if !self.is_single_id() {
            panic!()
        }
        Id(self.0)
    }
    
    pub fn as_single_id(&self) -> Id {
        if !self.is_single_id() {
            Id::empty()
        }
        else{
            Id(self.0)
        }
    }

}


impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_string( | string | {
            if let Some(id) = string {
                write!(f, "{}", id)
            }
            else {
                write!(f, "IdNotFound {:x}", self.0)
            }
        })
    }
}

impl fmt::Debug for MultiPack {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for MultiPack {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.unpack() {
            MultiUnpack::MultiId {index, count} => {
                write!(f, "<MultiId {} {}>", index, count)
            },
            MultiUnpack::SingleId(single) => {
                single.as_string( | string | {
                    if let Some(id) = string {
                        write!(f, "{}", id)
                    }
                    else {
                        write!(f, "<NotFound {:x}>", self.0)
                    }
                })
            },
            MultiUnpack::Empty => {
                write!(f, "<Empty>")
            }
            MultiUnpack::LivePtr(full_ptr)=>{
                write!(f, "<LivePtr{{file:{}, level:{}, index:{}}}>", full_ptr.file_id.0, full_ptr.local_ptr.level, full_ptr.local_ptr.index)
            }
            MultiUnpack::ZeroClass=>{
                write!(f, "<ZeroClass>")
            }
        }
        
    }
}


impl fmt::LowerHex for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}


pub struct IdMap {
    id_to_string: HashMap<Id, String>,
}

impl IdMap {
    pub fn add(&mut self, val: &str) {
        self.id_to_string.insert(Id::from_str_unchecked(val), val.to_string());
    }
    
    pub fn contains(&mut self, val: &str) -> bool {
        self.id_to_string.contains_key(&Id::from_str_unchecked(val))
    }
    
    pub fn with<F, R>(f: F) -> R
    where
    F: FnOnce(&mut IdMap) -> R,
    {
        static mut IDMAP: Option<IdMap> = None;
        static ONCE: Once = Once::new();
        ONCE.call_once( || unsafe {
            IDMAP = Some(IdMap {
                id_to_string: HashMap::new()
            })
        });
        f(unsafe {IDMAP.as_mut().unwrap()})
    }
}


pub struct MultiFmt<'a> {
    multi_ids: &'a [Id],
    multi_pack: MultiPack
}

impl <'a> MultiFmt<'a> {
    pub fn new(multi_ids: &'a [Id], multi_pack: MultiPack) -> Self {
        Self {multi_ids, multi_pack}
    }
}

impl <'a> fmt::Display for MultiFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.multi_pack.unpack() {
            MultiUnpack::MultiId {index, count} => {
                for i in 0..count {
                    let _ = write!(f, "{}", self.multi_ids[(i + index) as usize]);
                    if i < count - 1 {
                        let _ = write!(f, "::");
                    }
                }
                fmt::Result::Ok(())
            },
            _ => {
                write!(f, "{}", self.multi_pack)
            },
        }
    }
}

