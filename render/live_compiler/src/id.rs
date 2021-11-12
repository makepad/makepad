#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Once;
use std::fmt;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialOrd, Hash, PartialEq)]
pub struct FileId(pub u16);

impl FileId {
    pub fn index(index: usize) -> FileId {FileId(index as u16)}
    pub fn to_index(&self) -> usize {self.0 as usize}
}

//TODO FIX THIS THING TO BE N LEVELS OF MODULES
#[derive(Clone, Eq, Hash, Debug, Copy, PartialEq)]
pub struct ModulePath(pub Id, pub Id);

impl ModulePath {
    pub const fn from_str_unchecked(module_path: &str) -> Self {
        // ok lets split off the first 2 things from module_path
        let bytes = module_path.as_bytes();
        let len = bytes.len();
        // we have to find the first :
        let mut crate_id = Id(0);
        let mut i = 0;
        while i < len {
            if bytes[i] == ':' as u8 {
                crate_id = Id::from_bytes(bytes, 0, i);
                i += 2;
                break
            }
            i += 1;
        }
        if i == len { // module_path is only one thing
            return ModulePath(Id(0), Id::from_bytes(bytes, 0, len));
        }
        let module_start = i;
        while i < len {
            if bytes[i] == ':' as u8 {
                break
            }
            i += 1;
        }
        return ModulePath(crate_id, Id::from_bytes(bytes, module_start, i));
    }
    
    pub fn from_str(module_path: &str) -> Result<Self,
    String> {
        // ok lets split off the first 2 things from module_path
        let bytes = module_path.as_bytes();
        let len = bytes.len();
        // we have to find the first :
        let mut crate_id = Id(0);
        let mut i = 0;
        while i < len {
            if bytes[i] == ':' as u8 {
                crate_id = Id::from_str(std::str::from_utf8(&bytes[0..i]).unwrap()) ?;
                i += 2;
                break
            }
            i += 1;
        }
        if i == len { // module_path is only one thing
            return Ok(ModulePath(Id(0), Id::from_str(std::str::from_utf8(&bytes[0..len]).unwrap()) ?));
        }
        let module_start = i;
        while i < len {
            if bytes[i] == ':' as u8 {
                break
            }
            i += 1;
        }
        return Ok(ModulePath(crate_id, Id::from_str(std::str::from_utf8(&bytes[module_start..i]).unwrap()) ?));
    }
}

impl fmt::Display for ModulePath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}::{}", self.0, self.1)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialOrd, Copy, PartialEq)]
pub struct LocalPtr(pub usize);

#[derive(Clone, Debug, Eq, Hash, Copy, Ord, PartialOrd, PartialEq)]
pub struct LivePtr {
    pub file_id: FileId,
    pub local_ptr: LocalPtr,
}

impl fmt::Display for LivePtr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}_{}", self.file_id.0, self.local_ptr.0)
    }
}

#[derive(Clone, Default, Eq, Hash, Copy, PartialEq)]
pub struct Id(pub u64);

impl Id {
    pub fn empty() -> Self {
        Self (0)
    }
    
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    
    // from https://nullprogram.com/blog/2018/07/31/
    // i have no idea what im doing with start value and finalisation.
    pub const fn from_bytes(id_bytes: &[u8], start: usize, end: usize) -> Self {
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
        return Self (x) // leave the first bit
    }
    
    // merges 2 ids in a nonsymmetric fashion
    pub const fn add_id(&self, id: Id) -> Self {
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
    
    pub fn from_str(id_str: &str) -> Result<Id, String> {
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
            if let Some(id1) = idmap.id_to_string.get(self) {
                if let Some(id2) = idmap.id_to_string.get(other) {
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


impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if *self == Id::empty() {
            write!(f, "<IdEmpty>")
        }
        else {
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


pub fn hex_bytes_to_u32(bytes: &[u8]) -> Result<u32, ()> {
    fn hex_to_int(c: u8) -> Result<u32, ()> {
        if c >= 48 && c <= 57 {
            return Ok((c - 48) as u32);
        }
        if c >= 65 && c <= 70 {
            return Ok((c - 65 + 10) as u32);
        }
        if c >= 97 && c <= 102 {
            return Ok((c - 97 + 10) as u32);
        }
        return Err(());
    }
    
    match bytes.len() {
        1 => {
            // #w
            let val = hex_to_int(bytes[0]) ?;
            return Ok((val << 28) | (val << 24) | (val << 20) | (val << 16) | (val << 12) | (val << 8) | 0xff);
        }
        2 => { //#ww
            let val = (hex_to_int(bytes[0]) ? << 4) + hex_to_int(bytes[1]) ?;
            return Ok((val << 24) | (val << 16) | (val << 8) | 0xff)
        },
        3 => {
            // #rgb
            let r = hex_to_int(bytes[0]) ?;
            let g = hex_to_int(bytes[1]) ?;
            let b = hex_to_int(bytes[2]) ?;
            return Ok((r << 28) | (r << 24) | (g << 20) | (g << 16) | (b << 12) | (b << 8) | 0xff);
        }
        4 => {
            // #rgba
            let r = hex_to_int(bytes[0]) ?;
            let g = hex_to_int(bytes[1]) ?;
            let b = hex_to_int(bytes[2]) ?;
            let a = hex_to_int(bytes[3]) ?;
            return Ok((r << 28) | (r << 24) | (g << 20) | (g << 16) | (b << 12) | (b << 8) | (a << 4) | a);
        }
        6 => {
            // #rrggbb
            let r = (hex_to_int(bytes[0]) ? << 4) + hex_to_int(bytes[1]) ?;
            let g = (hex_to_int(bytes[2]) ? << 4) + hex_to_int(bytes[3]) ?;
            let b = (hex_to_int(bytes[4]) ? << 4) + hex_to_int(bytes[5]) ?;
            return Ok((r << 24) | (g << 16) | (b << 8) | 0xff)
        }
        8 => {
            // #rrggbbaa
            let r = (hex_to_int(bytes[0]) ? << 4) + hex_to_int(bytes[1]) ?;
            let g = (hex_to_int(bytes[2]) ? << 4) + hex_to_int(bytes[3]) ?;
            let b = (hex_to_int(bytes[4]) ? << 4) + hex_to_int(bytes[5]) ?;
            let a = (hex_to_int(bytes[6]) ? << 4) + hex_to_int(bytes[7]) ?;
            return Ok((r << 24) | (g << 16) | (b << 8) | a)
        }
        _ => (),
    }
    return Err(());
}
