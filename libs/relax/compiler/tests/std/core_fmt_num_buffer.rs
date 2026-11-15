// std-core differential test (core_diff.sh, agree.sh): fmt::NumBuffer and the integers'
// format_into (int_format_into, stable since 1.98). Every integer type at zero, small values, one
// below a power of ten, MIN and MAX, each compared with its Display; one buffer reused for several
// values; the buffer's size for each type (its longest decimal, with the sign); a buffer from a
// const; NumBuffer's Debug; std::fmt and alloc::fmt name the same type. "| " lines must equal rustc's.
use std::fmt::NumBuffer;
use std::mem::size_of;

macro_rules! each {
    ($($t:ident)*) => {$(
        {
            let mut buf = NumBuffer::<$t>::new();
            let mut shown = Vec::new();
            let mut same = true;
            for v in [0 as $t, 1, 7, 32, 99, 100, $t::MAX / 10 * 10 - 1, $t::MAX, $t::MIN, $t::MIN / 3] {
                let s = v.format_into(&mut buf);
                same &= s == v.to_string();
                shown.push(s.to_string());
            }
            println!("| {}: {} (as Display {}) size {}", stringify!($t), shown.join(" "), same, size_of::<NumBuffer<$t>>());
        }
    )*};
}

macro_rules! negatives {
    ($($t:ident)*) => {$(
        {
            let mut buf = NumBuffer::<$t>::new();
            let v: Vec<String> = [-1 as $t, -9, -10, -100, $t::MIN + 1].iter().map(|x| x.format_into(&mut buf).to_string()).collect();
            println!("| {} negatives {}", stringify!($t), v.join(" "));
        }
    )*};
}

const EMPTY: NumBuffer<u16> = NumBuffer::new();

#[test]
fn n01_every_integer() {
    each!(u8 i8 u16 i16 u32 i32 u64 i64 u128 i128 usize isize);
    negatives!(i8 i16 i32 i64 i128 isize);
}

#[test]
fn n02_reuse_const_and_debug() {
    let mut buf = EMPTY;
    let a = 65535u16.format_into(&mut buf).len();
    let b = 5u16.format_into(&mut buf);
    println!("| reused: first {} chars, then {:?}", a, b);
    let mut wide = NumBuffer::new();
    let s: &str = 18_446_744_073_709_551_616u128.format_into(&mut wide);
    println!("| just past u64::MAX {}", s);
    println!("| debug {:?}", NumBuffer::<i64>::new());
    let mut via_alloc: alloc::fmt::NumBuffer<u8> = std::fmt::NumBuffer::new();
    println!("| alloc::fmt {}", 255u8.format_into(&mut via_alloc));
}

extern crate alloc;
