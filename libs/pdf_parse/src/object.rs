use std::collections::HashMap;
use std::fmt;

/// A PDF object reference (object number, generation number).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjRef {
    pub num: u32,
    pub gen: u16,
}

impl fmt::Display for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {} R", self.num, self.gen)
    }
}

/// Core PDF object types.
#[derive(Clone, Debug, PartialEq)]
pub enum PdfObj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<PdfObj>),
    Dict(PdfDict),
    Stream(PdfStream),
    Ref(ObjRef),
}

/// A PDF dictionary (key = Name without leading `/`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdfDict {
    pub map: HashMap<String, PdfObj>,
}

/// A PDF stream: dictionary + raw (possibly compressed) bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfStream {
    pub dict: PdfDict,
    pub data: Vec<u8>,
}

impl PdfDict {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&PdfObj> {
        self.map.get(key)
    }

    pub fn get_name(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            PdfObj::Name(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            PdfObj::Int(n) => Some(*n),
            PdfObj::Real(n) => Some(*n as i64),
            _ => None,
        }
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            PdfObj::Real(n) => Some(*n),
            PdfObj::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn get_dict(&self, key: &str) -> Option<&PdfDict> {
        match self.get(key)? {
            PdfObj::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn get_array(&self, key: &str) -> Option<&[PdfObj]> {
        match self.get(key)? {
            PdfObj::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn get_ref(&self, key: &str) -> Option<ObjRef> {
        match self.get(key)? {
            PdfObj::Ref(r) => Some(*r),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&[u8]> {
        match self.get(key)? {
            PdfObj::Str(s) => Some(s.as_slice()),
            _ => None,
        }
    }
}

impl PdfObj {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            PdfObj::Int(n) => Some(*n),
            PdfObj::Real(n) => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PdfObj::Real(n) => Some(*n),
            PdfObj::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&str> {
        match self {
            PdfObj::Name(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&PdfDict> {
        match self {
            PdfObj::Dict(d) => Some(d),
            PdfObj::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[PdfObj]> {
        match self {
            PdfObj::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn as_ref(&self) -> Option<ObjRef> {
        match self {
            PdfObj::Ref(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_stream(&self) -> Option<&PdfStream> {
        match self {
            PdfObj::Stream(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_str_bytes(&self) -> Option<&[u8]> {
        match self {
            PdfObj::Str(s) => Some(s.as_slice()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PdfObj::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to interpret this object as a number array (e.g. for rectangles, matrices).
    pub fn as_number_array(&self) -> Option<Vec<f64>> {
        let arr = self.as_array()?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            out.push(item.as_f64()?);
        }
        Some(out)
    }
}
