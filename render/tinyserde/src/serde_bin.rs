use std::collections::{HashMap};
use std::hash::Hash;

pub trait SerBin {
    fn serialize_bin(&self)->Vec<u8>{
        let mut s = Vec::new();
        self.ser_bin(&mut s);
        s
    }
    
    fn ser_bin(&self, s: &mut Vec<u8>);
}

pub trait DeBin:Sized {
    fn deserialize_bin(&self, d:&[u8])->Result<Self, DeBinErr>{
        DeBin::de_bin(&mut 0, d)
    }

    fn de_bin(o:&mut usize, d:&[u8]) -> Result<Self, DeBinErr>;
}


pub struct DeBinErr{
    pub o:usize,
    pub l: usize,
    pub s: usize
}

impl std::fmt::Debug for DeBinErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bin deserialize error at:{} wanted:{} bytes but max size is {}", self.o, self.l, self.s)
    }
}

macro_rules! impl_ser_de_bin_for {
    ($ty:ident) => {
        impl SerBin for $ty {
            fn ser_bin(&self, s: &mut Vec<u8>) {
                let du8 = unsafe {std::mem::transmute::<&$ty, &[u8; std::mem::size_of::<$ty>()]>(&self)};
                s.extend_from_slice(du8);
            }
        }
        
        impl DeBin for $ty {
            fn de_bin(o:&mut usize, d:&[u8]) -> Result<$ty, DeBinErr> {
                let l = std::mem::size_of::<$ty>();
                if *o + l > d.len(){
                    return Err(DeBinErr{o:*o, l:l, s:d.len()})
                } 
                let mut m = [0 as $ty];
                unsafe {std::ptr::copy_nonoverlapping(d.as_ptr().offset(*o as isize) as *const $ty, m.as_mut_ptr() as *mut $ty, 1)}
                *o += l;
                Ok(m[0])
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
impl_ser_de_bin_for!(usize);

impl DeBin for u8 {
    fn de_bin(o:&mut usize, d:&[u8]) -> Result<u8,DeBinErr> {
        if *o + 1 > d.len(){
            return Err(DeBinErr{o:*o, l:1, s:d.len()})
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
            return Err(DeBinErr{o:*o, l:1, s:d.len()})
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
        let len:usize = DeBin::de_bin(o,d)?;
        if *o + len > d.len(){
            return Err(DeBinErr{o:*o, l:1, s:d.len()})
        } 
        let r = std::str::from_utf8(&d[*o..(*o+len)]).unwrap().to_string();
        *o += len;
        Ok(r)
    }
}

impl<T> SerBin for Vec<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        let len = self.len();
        len.ser_bin(s);
        for item in self {
            item.ser_bin(s);
        }
    }
}

impl<T> DeBin for Vec<T> where T:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Result<Vec<T>, DeBinErr> {
        let len:usize = DeBin::de_bin(o,d)?;
        let mut out = Vec::new();
        for _ in 0..len{
            out.push(DeBin::de_bin(o,d)?)
        }
        Ok(out)
    }
}

impl<T> SerBin for Option<T> where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        if let Some(v) = self{
            s.push(1);
            v.ser_bin(s);
        }
        else{
            s.push(0);
        }
    }
}

impl<T> DeBin for Option<T> where T:DeBin{
    fn de_bin(o:&mut usize, d:&[u8])->Result<Option<T>, DeBinErr> {
        if *o + 1 > d.len(){
            return Err(DeBinErr{o:*o, l:1, s:d.len()})
        } 
        let m = d[*o];
        *o += 1;
        if m == 1{
            Ok(Some(DeBin::de_bin(o,d)?))
        }
        else{
            Ok(None)
        }
    }
}

impl<T> SerBin for [T] where T: SerBin {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        for item in self {
            item.ser_bin(s);
        }
    }
}


/*
unsafe fn de_bin_array_impl_inner<T>(top: *mut T, count: usize, s: &mut DeRonState, i: &mut Chars) -> Result<(), String> where T:DeRon{
    for c in 0..count {
        top.add(c).write(DeRon::de_bin(s, i) ?);
    }
    Ok(())
}

macro_rules!de_bin_array_impl {
    ( $($count:expr),*) => {
        $(
        impl<T> DeRon for [T; $count] where T: DeRon {
            fn de_ron(s: &mut DeRonState, i: &mut Chars) -> Result<Self,
            String> {
                unsafe{
                    let mut to = std::mem::MaybeUninit::<[T; $count]>::uninit();
                    let top: *mut T = std::mem::transmute(&mut to);
                    de_ron_array_impl_inner(top, $count, s, i)?;
                    Ok(to.assume_init())
                }
            }
        }
        )*
    }
}

de_bin_array_impl!(2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32);
*/

/*
macro_rules! expand_de_bin {
    ($o:expr, $($d:expr),*) => ([$(DeBin::de_bin($o, $d)),*]);
}*/

// kinda nasty i have to do this this way, is there a better one?
/*
impl<T> DeBin for [T;2] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d)}}
impl<T> DeBin for [T;3] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d)}}
impl<T> DeBin for [T;4] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d)}}
impl<T> DeBin for [T;5] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d)}}
impl<T> DeBin for [T;6] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d)}}
impl<T> DeBin for [T;7] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;8] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;9] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;10] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;11] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;12] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;13] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;14] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;15] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;16] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;17] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;18] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;19] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;20] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;21] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;22] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;23] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;24] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;25] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;26] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;27] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;28] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;29] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;30] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;31] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
impl<T> DeBin for [T;32] where T:DeBin{fn de_bin(o:&mut usize, d:&[u8])->Self {expand_de_bin!(o,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d,d)}}
*/
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
        let len = self.len();
        len.ser_bin(s);
        for (k, v) in self {
            k.ser_bin(s);
            v.ser_bin(s);
        }
    }
}

impl<K, V> DeBin for HashMap<K, V> where K: DeBin + Eq + Hash,
V: DeBin + Eq {
    fn de_bin(o:&mut usize, d:&[u8])->Result<Self, DeBinErr>{
        let len:usize = DeBin::de_bin(o,d)?;
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