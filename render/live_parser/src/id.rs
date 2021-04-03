#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Once;
use std::fmt;

#[derive(Clone, Eq, Hash, Copy, PartialEq)]
pub struct Id(pub u64);

impl Id {
    /*
    // ok fine maybe this one was too simple.
    pub const fn from_str(idstr: &str) -> Id {
        let id = idstr.as_bytes();
        let id_len = id.len();
        let mut ret = 0u64;
        let mut o = 0;
        let mut i = 0;
        while i < id_len {
            ret ^= (id[i] as u64) << ((o & 7) << 3);
            o += 1;
            i += 1;
        }
        return Id(ret & 0x7fff_ffff_ffff_ffff)
    }*/

    // from https://nullprogram.com/blog/2018/07/31/ 
    // i have no idea what im doing with start value and finalisation.
    pub const fn from_str(idstr: &str) -> Id {
        let id = idstr.as_bytes();
        let id_len = id.len();
        let mut x = 0xd6e8_feb8_6659_fd9u64;
        let mut i = 0;
        while i < id_len {
            x = x.overflowing_add(id[i] as u64).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            i += 1;
        }
        return Id(x & 0x7fff_ffff_ffff_ffff) // leave the first bit 
    }
    
    pub fn multi(index: u32, len: u32) -> Id {
        Id((((index as u64) << 32) | len as u64) & 0x7fff_ffff_ffff_ffff | 0x8000_0000_0000_0000)
    }
    
    pub fn single(val: u64) -> Id {
        Id(val & 0x7fff_ffff_ffff_ffff)
    }
    
    pub fn empty() -> Id {
        Id(0x8000_0000_0000_0000)
    }
    
    pub fn is_empty(&self) -> bool {
        (self.0 & 0x8000_0000_0000_0000) != 0 && (self.0 & 0x7fff_ffff_ffff_ffff) == 0
    }
    
    pub fn is_multi(&self) -> bool {
        (self.0 & 0x8000_0000_0000_0000) != 0 && (self.0 & 0x7fff_ffff_ffff_ffff) != 0
    }
    
    pub fn is_single(&self) -> bool {
        (self.0 & 0x8000_0000_0000_0000) == 0
    }
    
    pub fn get_multi(&self)->(u32,u32){
        (
            ((self.0 & 0x7fff_ffff_ffff_ffff)>>32) as u32,
            (self.0 & 0xffff_ffff) as u32
        )
    }
    
    pub fn check_collision(&self, val:&str)->Option<String>{
        IdMap::with( | idmap | {
            if self.is_single() {
                if let Some(stored) = idmap.id_to_string.get(self){
                    if stored != val{
                        return Some(stored.clone())
                    }
                }
                else{
                    idmap.id_to_string.insert(self.clone(), val.to_string());
                }
            }
            return None;
        })
    }
    
    fn as_string<F, R>(&self, f: F) -> R
    where F: FnOnce(Option<&String>) -> R
    {
        IdMap::with( | idmap | {
            if self.is_single() {
                return f(idmap.id_to_string.get(self))
            }
            return f(None);
        })
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_string( | string | {
            if let Some(id) = string{
                write!(f, "{}", id)
            }
            else{
                write!(f, "{}",self.0)
            }
        })
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_string( | string | {
            if let Some(id) = string{
                write!(f, "{}", id)
            }
            else{
                write!(f, "{:x}",self.0)
            }
        })
    }
}


impl fmt::LowerHex for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.as_string( | string | {
            if let Some(id) = string{
                write!(f, "{}", id)
            }
            else{
                write!(f, "{:X}",self.0)
            }
        })
    }
}


pub struct IdMap {
    id_to_string: HashMap<Id, String>,
}

impl IdMap {
    pub fn add(&mut self, val:&str){
        self.id_to_string.insert(Id::from_str(val), val.to_string());
    }
    
    pub fn contains(&mut self, val:&str)->bool{
        self.id_to_string.contains_key(&Id::from_str(val))
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
 
