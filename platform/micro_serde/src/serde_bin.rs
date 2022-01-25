use std::{
    collections::HashMap,
    hash::Hash,
    convert::TryInto,
    ffi::{OsStr, OsString},
    path::{PathBuf, Path},
    str,
};

pub trait SerBin {
    fn serialize_bin(&self)->Vec<u8>{
        let mut s = Vec::new();
        self.ser_bin(&mut s);
        s
    }
    
    fn ser_bin(&self, s: &mut Vec<u8>);
}

pub trait DeBin:Sized {
    fn deserialize_bin(d:&[u8])->Result<Self, DeBinErr>{
        DeBin::de_bin(&mut 0, d)
    }

    fn de_bin(o:&mut usize, d:&[u8]) -> Result<Self, DeBinErr>;
}


pub struct DeBinErr{
    pub msg: String,
    pub o: usize,
    pub l: usize,
    pub s: usize
}

impl std::fmt::Display for DeBinErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error deserializing {} ", self.msg)?;
        if self.l != 0 {
            write!(f, "while trying to read {} bytes ", self.l)?
        }
        write!(f, " at offset {} in buffer of size {}", self.o, self.s)
    }
}

impl std::fmt::Debug for DeBinErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

macro_rules! impl_ser_de_bin_for {
    ($ty:ident) => {
        impl SerBin for $ty {
            fn ser_bin(&self, s: &mut Vec<u8>) {
                s.extend_from_slice(&self.to_le_bytes());
            }
        }
        
        impl DeBin for $ty {
            fn de_bin(o:&mut usize, d:&[u8]) -> Result<$ty, DeBinErr> {
                let l = std::mem::size_of::<$ty>();
                if *o + l > d.len(){
                    return Err(DeBinErr{o:*o, l:l, s:d.len(), msg:format!("{}", stringify!($ty))})
                }
                let ret = $ty::from_le_bytes(d[*o..*o+l].try_into().unwrap());
                *o += l;
                Ok(ret)
            }
        }
    };
}

impl_ser_de_bin_for!(f64);
impl_ser_de_bin_for!(f32);
impl_ser_de_bin_for!(u64);
impl_ser_de_bin_for!(i64);
impl_ser_de_bin_for!(u32);
impl_ser_de_bin_for!(i32);
impl_ser_de_bin_for!(u16);
impl_ser_de_bin_for!(i16);

impl SerBin for usize {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        s.extend_from_slice(&(*self as u64).to_le_bytes());
    }
}

impl DeBin for usize {
    fn de_bin(o:&mut usize, d:&[u8]) -> Result<usize, DeBinErr> {
        let l = std::mem::size_of::<u64>();
        if *o + l > d.len(){
            return Err(DeBinErr{o:*o, l:l, s:d.len(), msg:format!("usize")})
        }
        let ret = u64::from_le_bytes(d[*o..*o+l].try_into().unwrap()) as usize;
        *o += l;
        Ok(ret)
    }
}

impl DeBin for u8 {
    fn de_bin(o:&mut usize, d:&[u8]) -> Result<u8,DeBinErr> {
        if *o + 1 > d.len(){
            return Err(DeBinErr{o:*o, l:1, s:d.len(), msg:format!("u8")})
        } 
        let m = d[*o];
        *o += 1;
        Ok(m)
    }
}

impl SerBin for u8 {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        s.push(*self);
    }
}

impl SerBin for bool {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        s.push(if *self {1} else {0});
    }
}

impl DeBin for bool {
    fn de_bin(o:&mut usize, d:&[u8]) -> Result<bool, DeBinErr> {
        if *o + 1 > d.len(){
            return Err(DeBinErr{o:*o, l:1, s:d.len(), msg:format!("bool")})
        } 
        let m = d[*o];
        *o += 1;
        if m == 0{Ok(false)} else {Ok(true)}
    }
}

impl SerBin for String {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let len = self.len();
        len.ser_bin(s);
        s.extend_from_slice(self.as_bytes());
    }
}

impl DeBin for String {
    fn de_bin(o:&mut usize, d:&[u8])->Result<String, DeBinErr> {
        let len:u64 = DeBin::de_bin(o,d)?;
        if *o + (len as usize) > d.len(){
            return Err(DeBinErr{o:*o, l:1, s:d.len(), msg:format!("String")})
        } 
        let r = std::str::from_utf8(&d[*o..(*o+(len as usize))]).unwrap().to_string();
        *o += len as usize;
        Ok(r)
    }
}

impl<T> SerBin for Vec<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let len = self.len() as u64;
        len.ser_bin(s);
        for item in self {
            item.ser_bin(s);
        }
    }
}

impl<T> DeBin for Vec<T> where T:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Result<Vec<T>, DeBinErr> {
        let len:u64 = DeBin::de_bin(o,d)?;
        let mut out = Vec::new();
        for _ in 0..len{
            out.push(DeBin::de_bin(o,d)?)
        }
        Ok(out)
    }
}

impl<T> SerBin for Option<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        match self {
            None => s.push(0),
            Some(v) => {
                s.push(1);
                v.ser_bin(s);
            }
        }
    }
}

impl<T> DeBin for Option<T> where T:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Result<Option<T>, DeBinErr> {
        if *o + 1 > d.len(){
            return Err(DeBinErr{o:*o, l:1, s:d.len(), msg:format!("Option<T>")})
        } 
        let m = d[*o];
        *o += 1;
        Ok(match m {
            0 => None,
            1 => Some(DeBin::de_bin(o,d)?),
            _ => return Err(DeBinErr{o:*o, l:0, s:d.len(), msg:format!("Option<T>")}),
        })
    }
}

impl<T, E> SerBin for Result<T, E> where T: SerBin, E: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        match self {
            Ok(v) => {
                s.push(0);
                v.ser_bin(s);
            }
            Err(e) => {
                s.push(1);
                e.ser_bin(s);
            }
        }
    }
}

impl<T, E> DeBin for Result<T, E> where T: DeBin, E: DeBin {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        if *o + 1 > d.len() {
            return Err(DeBinErr{o:*o, l:1, s:d.len(), msg:format!("Result<T, E>")});
        }
        let m = d[*o];
        *o += 1;
        Ok(match m {
            0 => Ok(T::de_bin(o, d)?),
            1 => Err(E::de_bin(o, d)?),
            _ => return Err(DeBinErr{o:*o, l:0, s:d.len(), msg:format!("Result<T, E>")}),
        })
    }
}

impl<T> SerBin for [T] where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        for item in self {
            item.ser_bin(s);
        }
    }
}


unsafe fn de_bin_array_impl_inner<T>(top: *mut T, count: usize, o:&mut usize, d:&[u8]) -> Result<(), DeBinErr> where T:DeBin{
    for c in 0..count {
        top.add(c).write(DeBin::de_bin(o, d) ?);
    }
    Ok(())
}

macro_rules!de_bin_array_impl {
    ( $($count:expr),*) => {
        $(
        impl<T> DeBin for [T; $count] where T: DeBin {
            fn de_bin(o:&mut usize, d:&[u8]) -> Result<Self,
            DeBinErr> {
                unsafe{
                    let mut to = std::mem::MaybeUninit::<[T; $count]>::uninit();
                    let top: *mut T = std::mem::transmute(&mut to);
                    de_bin_array_impl_inner(top, $count, o, d)?;
                    Ok(to.assume_init())
                }
            }
        }
        )*
    }
}

de_bin_array_impl!(2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32);

impl<A,B> SerBin for (A,B) where A: SerBin, B:SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
    }
}

impl<A,B> DeBin for (A,B) where A:DeBin, B:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Result<(A,B), DeBinErr> {Ok((DeBin::de_bin(o,d)?,DeBin::de_bin(o,d)?))}
}

impl<A,B,C> SerBin for (A,B,C) where A: SerBin, B:SerBin, C:SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
        self.2.ser_bin(s);
    } 
}

impl<A,B,C> DeBin for (A,B,C) where A:DeBin, B:DeBin, C:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Result<(A,B,C), DeBinErr> {Ok((DeBin::de_bin(o,d)?,DeBin::de_bin(o,d)?,DeBin::de_bin(o,d)?))}
}

impl<A,B,C,D> SerBin for (A,B,C,D) where A: SerBin, B:SerBin, C:SerBin, D:SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
        self.2.ser_bin(s);
        self.3.ser_bin(s);
    }
}

impl<A,B,C,D> DeBin for (A,B,C,D) where A:DeBin, B:DeBin, C:DeBin, D:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Result<(A,B,C,D), DeBinErr> {Ok((DeBin::de_bin(o,d)?,DeBin::de_bin(o,d)?,DeBin::de_bin(o,d)?,DeBin::de_bin(o,d)?))}
}

impl<K, V> SerBin for HashMap<K, V> where K: SerBin,
V: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let len = self.len() as u64;
        len.ser_bin(s);
        for (k, v) in self {
            k.ser_bin(s);
            v.ser_bin(s);
        }
    }
}

impl<K, V> DeBin for HashMap<K, V> where K: DeBin + Eq + Hash,
V: DeBin {
    fn de_bin(o:&mut usize, d:&[u8])->Result<Self, DeBinErr>{
        let len:u64 = DeBin::de_bin(o,d)?;
        let mut h = HashMap::new();
        for _ in 0..len{
            let k = DeBin::de_bin(o,d)?;
            let v = DeBin::de_bin(o,d)?;
            h.insert(k, v);
        }
        Ok(h)
    }
}


impl<T> SerBin for Box<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        (**self).ser_bin(s)
    }
}

impl<T> DeBin for Box<T> where T: DeBin {
    fn de_bin(o:&mut usize, d:&[u8])->Result<Box<T>, DeBinErr> {
        Ok(Box::new(DeBin::de_bin(o,d)?))
    }
}

impl SerBin for PathBuf {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.as_os_str().ser_bin(s)
    }
}

impl SerBin for Path {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.as_os_str().ser_bin(s)
    }
}

impl SerBin for OsString {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.as_os_str().ser_bin(s)
    }
}

#[cfg(unix)]
impl SerBin for OsStr {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        use std::os::unix::ffi::OsStrExt;

        self.as_bytes().ser_bin(s)
    }
}

impl SerBin for char {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let mut bytes = [0; 4];
        self.encode_utf8(&mut bytes).as_bytes().ser_bin(s);
    }
}

#[cfg(unix)]
impl DeBin for PathBuf {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        Ok(PathBuf::from(OsString::de_bin(o, d)?))
    }
}

#[cfg(unix)]
impl DeBin for OsString {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        use std::os::unix::ffi::OsStringExt;

        Ok(OsString::from_vec(Vec::de_bin(o, d)?))
    }
}

impl DeBin for char {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        let mut bytes = [0; 4];
        bytes[0] = u8::de_bin(o, d)?;
        let width = utf8_char_width(bytes[0]);
        for index in 1..width {
            bytes[index] = u8::de_bin(o, d)?;
        }
        Ok(str::from_utf8(&bytes[..width])
            .unwrap()
            .chars()
            .next()
            .unwrap())
    }
}

// Given a first byte, determines how many bytes are in this UTF-8 character.
#[inline]
pub fn utf8_char_width(b: u8) -> usize {
    static UTF8_CHAR_WIDTH: [u8; 256] = [
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, // 0x1F
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, // 0x3F
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, // 0x5F
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, // 0x7F
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, // 0x9F
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, // 0xBF
        0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
        2, // 0xDF
        3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, // 0xEF
        4, 4, 4, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0xFF
    ];

    UTF8_CHAR_WIDTH[b as usize] as usize
}
