use std::fmt;
use makepad_live_parser::Span;
use makepad_live_parser::Id;
use makepad_live_parser::id;
use makepad_live_parser::FullNodePtr;
use std::fmt::Write;

#[derive(Clone, Copy, Ord, PartialOrd, Default, Eq, Hash, PartialEq)]
pub struct Ident(pub Id);
impl Ident {
    pub fn to_ident_path(self)->IdentPath{
        IdentPath::from_ident(self)
    }
}

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Default, Copy, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct IdentPath {
    pub segs: [Ident; 6],
    pub len: usize
}

#[derive(Clone, Default, Copy, Eq, PartialEq, Hash, Debug)]
pub struct QualifiedIdentPath{
    pub full_ptr: Option<FullNodePtr>
}

#[derive(Clone, Copy, Default, Eq, PartialEq, Debug)]
pub struct IdentPathWithSpan{
    pub span:Span,
    pub ident_path:IdentPath,
}


impl QualifiedIdentPath{
/*
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
    }*/
    
}

impl IdentPath {
    
    pub fn from_ident(ident:Ident)->Self{
        let mut p = IdentPath::default();
        p.segs[0] = ident;
        p.len = 1;
        p
    }
    
    pub fn from_two(ident1:Ident,ident2:Ident)->Self{
        let mut p = IdentPath::default();
        p.segs[0] = ident1;
        p.segs[1] = ident2;
        p.len = 2;
        p
    }

    pub fn from_three(ident1:Ident,ident2:Ident, ident3:Ident)->Self{
        let mut p = IdentPath::default();
        p.segs[0] = ident1;
        p.segs[1] = ident2;
        p.segs[1] = ident3;
        p.len = 3;
        p
    }

    pub fn from_array(idents:&[Ident])->Self{
        let mut p = IdentPath::default();
        for i in 0..idents.len(){
            p.segs[i] = idents[i];
        }
        p.len = idents.len();
        p
    }
    
    pub fn to_struct_fn_ident(&self)->Ident{
        let mut s = String::new();
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(s, "_").unwrap();
            }
            let _ = write!(s,"{}", self.segs[i]);
        }
        Ident(Id::from_str(&s).panic_collision(&s))
    }

    pub fn from_str(value:&str)->Self{
        let mut p = IdentPath::default();
        p.segs[0] = Ident(Id::from_str(value));
        p.len = 1;
        p
    }

    pub fn is_self_scope(&self) -> bool {
        self.len > 1 && self.segs[0] == Ident(id!(self))
    }
    
    pub fn len(&self) -> usize {
        self.len
    }
    
    pub fn push(&mut self, ident: Ident) -> bool {
        if self.len >= self.segs.len() {
            return false
        }
        self.segs[self.len] = ident;
        self.len += 1;
        return true
    }
    
    
    pub fn get_single(&self) -> Option<Ident> {
        if self.len != 1 {
            return None
        }
        return Some(self.segs[0])
    }
    
    pub fn qualify(&self, _modpath: &str) -> QualifiedIdentPath{
        let _out = IdentPath::default();
        /*
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
        };*/
        QualifiedIdentPath::default()
    }
    
}

impl fmt::Debug for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(f, "::").unwrap();
            }
            let _ = write!(f, "{}", self.segs[i]);
        }
        Ok(())
    }
}

impl fmt::Display for IdentPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in 0..self.len {
            if i != 0 {
                let _ = write!(f, "::").unwrap();
            }
            let _ = write!(f, "{}", self.segs[i]);
        }
        Ok(())
    }
}

impl fmt::Display for QualifiedIdentPath {
    fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result {
        //self.fmt(f)
        Ok(())
    }
}
