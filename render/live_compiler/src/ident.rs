use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::sync::Once;
use crate::livetypes::LiveId;

#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct Ident(usize);
impl Ident {
    pub fn new<'a, S>(string: S) -> Ident
    where
    S: Into<Cow<'a, str >>,
    {
        let string = string.into();
        Interner::with( | interner | {
            Ident(
                if let Some(index) = interner.indices.get(string.as_ref()).cloned() {
                    index
                } else {
                    let string = string.into_owned();
                    let string_index = interner.strings.len();
                    interner.strings.push(string.clone());
                    interner.indices.insert(string.clone(), string_index);
                    string_index
                },
            )
        })
    }
    
    pub fn with<F, R>(self, f: F) -> R
    where
    F: FnOnce(&str) -> R,
    {
        Interner::with( | interner | f(&interner.strings[self.0]))
    }
}

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.with( | string | write!(f, "{}", string))
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.with( | string | write!(f, "{}", string))
    }
}

impl Ord for Ident {
    fn cmp(&self, other: &Ident) -> Ordering {
        Interner::with( | interner | interner.strings[self.0].cmp(&interner.strings[other.0]))
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Ident) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
struct Interner {
    strings: Vec<String>,
    indices: HashMap<String, usize>,
}

impl Interner {
    fn with<F, R>(f: F) -> R
    where
    F: FnOnce(&mut Interner) -> R,
    {
        static mut INTERNER: Option<Interner> = None;
        static ONCE: Once = Once::new();
        ONCE.call_once( || unsafe {
            INTERNER = Some(Interner {
                strings: {let mut v = Vec::new(); v.push("".to_string()); v},
                indices: {let mut h = HashMap::new(); h.insert("".to_string(), 0); h}
            })
        });
        f(unsafe {INTERNER.as_mut().unwrap()})
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct IdentPath {
    segs: [Ident; 4],
    len: usize
}

impl IdentPath {
    pub fn new() -> Self {
        IdentPath {
            segs: [Ident::default(), Ident::default(), Ident::default(), Ident::default()],
            len: 0
        }
    }
    
    pub fn is_self_id(&self) -> bool {
        self.len > 1 && self.segs[0] == Ident::new("self")
    }
    
    pub fn len(&self) -> usize {
        self.len
    }
    
    pub fn push(&mut self, ident: Ident) -> bool {
        if self.len >= 4 {
            return false
        }
        self.segs[self.len] = ident;
        self.len += 1;
        return true
    }
    
    pub fn from_two(one: Ident, two: Ident) -> Self {
        IdentPath {
            segs: [one, two, Ident(0), Ident(0)],
            len: 2
        }
    }
    pub fn get_single(&self) -> Option<Ident> {
        if self.len != 1 {
            return None
        }
        return Some(self.segs[0])
    }
    
    
    pub fn to_live_id(&self, modpath: &str) -> LiveId {
        // ok lets hash an IdentPath into a proper liveid;
        let modpath = modpath.as_bytes();
        let modpath_len = modpath.len();
        
        let mut value = 0u64;
        let mut o = 0;
        let start = if self.segs[0] == Ident::new("self") {
            let mut i = 0;
            while i < modpath_len {
                value ^= (modpath[i] as u64) << ((o & 7) << 3);
                o += 1;
                i += 1;
            }
            1
        }
        else if self.segs[0] == Ident::new("crate") {
            let mut i = 0;
            while i < modpath_len {
                if modpath[i] == ':' as u8 {
                    break
                }
                value ^= (modpath[i] as u64) << ((o & 7) << 3);
                o += 1;
                i += 1;
            }
            1
        }
        else {
            0
        };
        // lets add the other segs
        for i in start..self.len {
            if i != 0 {
                value ^= (':' as u64) << ((o & 7) << 3);
                o += 1;
                value ^= (':' as u64) << ((o & 7) << 3);
                o += 1;
            }
            self.segs[i].with( | id_str | {
                let id = id_str.as_bytes();
                for i in 0..id.len() {
                    value ^= (id[i] as u64) << ((o & 7) << 3);
                    o += 1;
                }
            })
        }
        LiveId(value)
    }
}

impl fmt::Debug for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                write!(f, "::").unwrap();
            }
            self.segs[i].with( | string | write!(f, "{}", string)).unwrap()
        }
        Ok(())
    }
}

impl fmt::Display for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                write!(f, "::").unwrap();
            }
            self.segs[i].with( | string | write!(f, "{}", string)).unwrap()
        }
        Ok(())
    }
}
