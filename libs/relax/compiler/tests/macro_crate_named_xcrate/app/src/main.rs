//! A dependency's `macro_rules!` macro named like the crate and like a private module of it,
//! imported in a use group beside a nested path and called in consts, with the crate's paths in
//! its arguments (relax issue 40: "macro `zerovec!` is not supported"). `cargo run -p xvec-app`
//! prints `bmp 2 Some(65536) Some(0) all Some(1114112) empty 0`.
use xvec::{ule::AsULE, xvec, XVec};

const BMP: XVec<u32> = xvec!(u32; xvec::ule::Potential::to_unaligned; [0x0, 0x1_0000]);
// a cast in the macro's arguments
const ALL: XVec<u32> = xvec!(u32; xvec::ule::Potential::to_unaligned; [0x0, (char::MAX as u32) + 1]);

fn first<T: AsULE>(v: &XVec<T>) -> Option<T> {
    v.get(0)
}

fn main() {
    // the empty arm
    let empty: XVec<u32> = xvec!();
    println!("bmp {} {:?} {:?} all {:?} empty {}", BMP.len(), BMP.get(1), first(&BMP), ALL.get(1), empty.len());
}
