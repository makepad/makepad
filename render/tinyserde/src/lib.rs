pub use makepad_tinyserde_derive::*;
use std::collections::{HashMap};

pub trait SerBin {
    fn ser_bin(&self, s: &mut SerBinData);
}

pub trait DeBin {
    fn de_bin(d:&mut DeBinData) -> Self;
}

pub struct DeBinData {
    pub dat: Vec<u8>,
    pub off: usize
}

pub struct SerBinData {
    pub dat: Vec<u8>
}

macro_rules! impl_ser_de_bin_for {
    ($ty:ident) => {
        impl SerBin for $ty {
            fn ser_bin(&self, s: &mut SerBinData) {
                let du8 = unsafe {std::mem::transmute::<&$ty, &[u8; std::mem::size_of::<$ty>()]>(&self)};
                s.dat.extend_from_slice(du8);
            }
        }
        
        impl DeBin for $ty {
            fn de_bin(d:&mut DeBinData) -> $ty {
                let mut m = [0 as $ty];
                unsafe {std::ptr::copy_nonoverlapping(d.dat.as_ptr().offset(d.off as isize) as *const $ty, m.as_mut_ptr() as *mut $ty, 1)}
                d.off += std::mem::size_of::<$ty>();
                m[0]
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
    fn de_bin(d:&mut DeBinData) -> u8 {
        let m = d.dat[d.off];
        d.off += 1;
        m
    }
}

impl SerBin for u8 {
    fn ser_bin(&self, s: &mut SerBinData) {
        s.dat.push(*self);
    }
}

impl SerBin for bool {
    fn ser_bin(&self, s: &mut SerBinData) {
        s.dat.push(if *self {1} else {0});
    }
}

impl DeBin for bool {
    fn de_bin(d:&mut DeBinData) -> bool {
        let m = d.dat[d.off];
        d.off += 1;
        if m == 0{false} else {true}
    }
}

impl SerBin for String {
    fn ser_bin(&self, s: &mut SerBinData) {
        let len = self.len();
        len.ser_bin(s);
        s.dat.extend_from_slice(self.as_bytes());
    }
}

impl DeBin for String {
    fn de_bin(d:&mut DeBinData)->String {
        let len:usize = DeBin::de_bin(d);
        let r = std::str::from_utf8(&d.dat[d.off..(d.off+len)]).unwrap().to_string();
        d.off += len;
        r
    }
}

impl<T> SerBin for Vec<T> where T: SerBin {
    fn ser_bin(&self, s: &mut SerBinData) {
        let len = self.len();
        len.ser_bin(s);
        for item in self {
            item.ser_bin(s);
        }
    }
}

impl<T> DeBin for Vec<T> where T:DeBin{
    fn de_bin(d:&mut DeBinData)->Vec<T> {
        let len:usize = DeBin::de_bin(d);
        let mut out = Vec::new();
        for _ in 0..len{
            out.push(DeBin::de_bin(d))
        }
        out
    }
}

impl<T> SerBin for Option<T> where T: SerBin {
    fn ser_bin(&self, s: &mut SerBinData) {
        if let Some(v) = self{
            s.dat.push(1);
            v.ser_bin(s);
        }
        else{
            s.dat.push(0);
        }
    }
}

impl<T> DeBin for Option<T> where T:DeBin{
    fn de_bin(d:&mut DeBinData)->Option<T> {
        let m = d.dat[d.off];
        d.off += 1;
        if m == 1{
            Some(DeBin::de_bin(d))
        }
        else{
            None
        }
    }
}

impl<T> SerBin for [T] where T: SerBin {
    fn ser_bin(&self, s: &mut SerBinData) {
        for item in self {
            item.ser_bin(s);
        }
    }
}

impl<T> DeBin for [T;2] where T:DeBin{
    fn de_bin(d:&mut DeBinData)->[T;2] {[DeBin::de_bin(d),DeBin::de_bin(d)]}
}

impl<T> DeBin for [T;3] where T:DeBin{
    fn de_bin(d:&mut DeBinData)->[T;3] {[DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d)]}
}

impl<T> DeBin for [T;4] where T:DeBin{
    fn de_bin(d:&mut DeBinData)->[T;4] {[DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d)]}
}

impl<T> DeBin for [T;5] where T:DeBin{
    fn de_bin(d:&mut DeBinData)->[T;5] {[DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d)]}
}

impl<T> DeBin for [T;6] where T:DeBin{
    fn de_bin(d:&mut DeBinData)->[T;6] {[DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d)]}
}

impl<T> DeBin for [T;7] where T:DeBin{
    fn de_bin(d:&mut DeBinData)->[T;7] {[DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d)]}
}

impl<T> DeBin for [T;8] where T:DeBin{
    fn de_bin(d:&mut DeBinData)->[T;8] {[DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d)]}
}

impl<A,B> SerBin for (A,B) where A: SerBin, B:SerBin {
    fn ser_bin(&self, s: &mut SerBinData) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
    }
}

impl<A,B> DeBin for (A,B) where A:DeBin, B:DeBin{
    fn de_bin(d:&mut DeBinData)->(A,B) {(DeBin::de_bin(d),DeBin::de_bin(d))}
}

impl<A,B,C> SerBin for (A,B,C) where A: SerBin, B:SerBin, C:SerBin {
    fn ser_bin(&self, s: &mut SerBinData) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
        self.2.ser_bin(s);
    }
}

impl<A,B,C> DeBin for (A,B,C) where A:DeBin, B:DeBin, C:DeBin{
    fn de_bin(d:&mut DeBinData)->(A,B,C) {(DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d))}
}

impl<A,B,C,D> SerBin for (A,B,C,D) where A: SerBin, B:SerBin, C:SerBin, D:SerBin {
    fn ser_bin(&self, s: &mut SerBinData) {
        self.0.ser_bin(s);
        self.1.ser_bin(s);
        self.2.ser_bin(s);
        self.3.ser_bin(s);
    }
}

impl<A,B,C,D> DeBin for (A,B,C,D) where A:DeBin, B:DeBin, C:DeBin, D:DeBin{
    fn de_bin(d:&mut DeBinData)->(A,B,C,D) {(DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d),DeBin::de_bin(d))}
}

impl<K, V> SerBin for HashMap<K, V> where K: SerBin,
V: SerBin {
    fn ser_bin(&self, s: &mut SerBinData) {
        let len = self.len();
        len.ser_bin(s);
        for (k, v) in self {
            k.ser_bin(s);
            v.ser_bin(s);
        }
    }
}

impl<T> DeBin for Box<T> where T: DeBin {
    fn de_bin(d: &mut DeBinData)->Box<T> {
        Box::new(DeBin::de_bin(d))
    }
}