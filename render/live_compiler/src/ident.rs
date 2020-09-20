use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::sync::Once;
use crate::span::Span;
use crate::livetypes::LiveId;
use crate::livestyles::LiveStyles;
use std::fmt::Write;

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
    
    pub fn to_ident_path(self)->IdentPath{
        IdentPath::from_ident(self)
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

#[derive(Clone, Default, Copy, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct IdentPath {
    pub segs: [Ident; 4],
    pub len: usize
}

#[derive(Clone, Default, Copy, Eq, PartialEq, Hash, PartialOrd, Ord, Debug)]
pub struct QualifiedIdentPath(pub IdentPath);

#[derive(Clone, Copy, Default, Eq, PartialEq, Debug)]
pub struct IdentPathWithSpan{
    pub span:Span,
    pub ident_path:IdentPath,
}

impl IdentPathWithSpan{
    pub fn to_live_id(&self, live_styles:&LiveStyles)->LiveId{
        self.ident_path.qualify(&live_styles.live_bodies[self.span.live_body_id.0].module_path).to_live_id()
    }
}

impl QualifiedIdentPath{

    pub fn to_live_id(&self) -> LiveId {
        // lets add the other segs
        let mut value = 0u64;
        let mut o = 0;
        for i in 0..self.0.len {
            if i != 0 {
                value ^= (':' as u64) << ((o & 7) << 3);
                o += 1;
                value ^= (':' as u64) << ((o & 7) << 3);
                o += 1;
            }
            self.0.segs[i].with( | id_str | {
                let id = id_str.as_bytes();
                for i in 0..id.len() {
                    value ^= (id[i] as u64) << ((o & 7) << 3);
                    o += 1;
                }
            })
        }
        LiveId(value)
    }    


    pub fn write_underscored_ident(&self, string:&mut String){
        for i in 0..self.0.len{
            if i != 0{
                write!(string, "_").unwrap();
            }
            write!(string, "{}", self.0.segs[i]).unwrap();
        }
    }

    pub fn with_final_ident(&self, ident:Ident)->Self{
        let mut new = self.clone();
        new.0.push(ident);
        new
    }
    
}

impl IdentPath {
    
    pub fn from_ident(ident:Ident)->Self{
        IdentPath{
            segs: [ident, Ident::default(), Ident::default(), Ident::default()],
            len :1
        }
    }

    pub fn from_two_idents(ident1:Ident, ident2:Ident)->Self{
        IdentPath{
            segs: [ident1, ident2, Ident::default(), Ident::default()],
            len :2
        }
    }
    
    pub fn to_struct_fn_ident(&self)->Ident{
        let mut s = String::new();
        for i in 0..self.len {
            if i != 0 {
                write!(s, "_").unwrap();
            }
            self.segs[i].with( | string | write!(s, "{}", string)).unwrap()
        }
        Ident::new(&s)
    }

    pub fn from_str(value:&str)->Self{
        IdentPath{
            segs: [Ident::new(value), Ident::default(), Ident::default(), Ident::default()],
            len :1
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
    
    pub fn qualify(&self, modpath: &str) -> QualifiedIdentPath{
        let mut out = IdentPath::default();
        if self.segs[0] == Ident::new("self") {
            let mut last = 0;
            for (index,c) in modpath.chars().enumerate(){
                if c == ':'{
                    // do the range last->us and make an ident
                    if index-last > 0{
                        out.push(Ident::new(&modpath[last..index]));
                    }
                    last = index + 1;
                }
            }
            out.push(Ident::new(&modpath[last..]));
            for i in 1..self.len{
                out.push(self.segs[i]);
            }
        }
        else if self.segs[0] == Ident::new("crate") {
            for (index,c) in modpath.chars().enumerate(){
                if c == ':' as char {
                    out.push(Ident::new(&modpath[0..index]));
                    break
                }
            }
            for i in 1..self.len{
                out.push(self.segs[i]);
            }
        }
        else {
            for i in 0..self.len{
                out.push(self.segs[i]);
            }
        };
        QualifiedIdentPath(out)
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

impl fmt::Display for QualifiedIdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}
