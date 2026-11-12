// std surface the async / crypto dependency stack reaches for (tokio, ring): PhantomPinned's
// derives, Pin::as_deref_mut, Cursor over a boxed slice and over an array, and Wrapping's
// by-reference operators. Each `| ` line is the same under rustc and makepad test.
use std::io::{Cursor, Write};
use std::marker::PhantomPinned;
use std::num::Wrapping;
use std::pin::Pin;

#[test]
fn a01_phantom_pinned_derives() {
    let p = PhantomPinned;
    println!("| {:?} eq {} ord {:?} partial {:?} default {:?}", p, p == PhantomPinned, p.cmp(&PhantomPinned), p.partial_cmp(&PhantomPinned), PhantomPinned::default());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&p, &mut h);
    std::hash::Hash::hash(&1u8, &mut h);
    let mut g = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&1u8, &mut g);
    println!("| hash writes nothing {}", std::hash::Hasher::finish(&h) == std::hash::Hasher::finish(&g));
}

#[test]
fn a02_pin_as_deref_mut() {
    let mut boxed: Pin<Box<[u8; 3]>> = Box::pin([1, 2, 3]);
    {
        let outer: Pin<&mut Pin<Box<[u8; 3]>>> = Pin::new(&mut boxed);
        let mut inner: Pin<&mut [u8; 3]> = outer.as_deref_mut();
        inner.as_mut().get_mut()[0] = 9;
        println!("| inner {:?}", &*inner);
    }
    println!("| boxed {:?}", &*boxed);
}

#[test]
fn a03_cursor_boxed_slice_and_array() {
    let mut c = Cursor::new(vec![0u8; 6].into_boxed_slice());
    let n = c.write(b"abcdefgh").unwrap();
    println!("| boxed wrote {} pos {} {:?}", n, c.position(), c.get_ref());
    let mut a = Cursor::new([0u8; 4]);
    a.set_position(2);
    let n = a.write(b"xyz").unwrap();
    let flushed = a.flush().is_ok();
    a.set_position(9);
    let past = a.write(b"q").unwrap();
    println!("| array wrote {} {:?} flush {} past the end {}", n, a.get_ref(), flushed, past);
}

#[test]
fn a04_wrapping_by_reference() {
    let a = Wrapping(0xffff_fff0u32);
    let b = Wrapping(0x20u32);
    let w = [b, Wrapping(3)];
    let mut acc = a;
    for x in w.iter() {
        acc = acc + x;
    }
    println!("| {:?} {:?} {:?} {:?}", acc, &a - &b, &a * b, a ^ &b);
    println!("| {:?} {:?} {:?} {:?}", &a | &b, a & &b, &a % b, &a / &b);
    let mut m = Wrapping(7i8);
    m += &Wrapping(125);
    m -= &Wrapping(1);
    m *= &Wrapping(3);
    m /= &Wrapping(2);
    m %= &Wrapping(50);
    m ^= &Wrapping(1);
    m |= &Wrapping(64);
    m &= &Wrapping(0x7c);
    println!("| assign {:?} neg {:?} not {:?}", m, -&m, !&m);
    let mut d = Wrapping(100u16);
    d /= Wrapping(7);
    d %= Wrapping(5);
    println!("| div rem assign {:?}", d);
}

// ring names every C symbol through prefixed_extern!: `#[link_name = concat!(prefix, stringify!(name))]`,
// directly or passed through a rules macro as an `expr` fragment. The prefix here is libc's `l`.
macro_rules! symbol_prefix {
    () => {
        "l"
    };
}
macro_rules! prefixed_item {
    { $attr:ident $name:ident { $($item:tt)+ } } => {
        prefixed_item! { $attr { concat!(symbol_prefix!(), stringify!($name)) } { $($item)+ } }
    };
    { $attr:ident { $prefixed_name:expr } { $($item:tt)+ } } => {
        #[$attr = $prefixed_name]
        $($item)+
    };
}
extern "C" {
    #[link_name = concat!(symbol_prefix!(), stringify!(abs))]
    fn direct_labs(x: std::ffi::c_long) -> std::ffi::c_long;
    prefixed_item! { link_name abs { fn passed_labs(x: std::ffi::c_long) -> std::ffi::c_long; } }
}

#[test]
fn a05_link_name_from_a_macro() {
    let (d, p) = unsafe { (direct_labs(-41), passed_labs(-42)) };
    println!("| direct {} passed {}", d, p);
}
