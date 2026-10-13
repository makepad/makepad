// Gaps from the frontend's boundary audit (std Debug impls, conversions, int and ptr helpers):
// Debug for every range form, fn pointers, iterator adapters and sources; From<[T; N]> for
// Arc/Rc/Box/Vec (+ From<&mut [T; N]> for Vec); is_multiple_of; ptr::fn_addr_eq.
use std::rc::Rc;
use std::sync::Arc;

fn double(x: u32) -> u32 {
    x * 2
}
fn triple(x: u32) -> u32 {
    x * 3
}

#[derive(Debug)]
struct Holder {
    span: std::ops::Range<usize>,
    spans: Vec<std::ops::RangeInclusive<u8>>,
    f: fn(u32) -> u32,
}

#[test]
fn a01_range_debug() {
    println!("| {:?} {:?} {:?} {:?} {:?} {:?}", 1..4, 2.., ..5, ..=6, .., 1..=3);
    let mut r = 1..=2u8;
    r.next();
    r.next();
    println!("| exhausted {:?} {:?}", r, r.is_empty());
    println!("| formatted {:5?} {:#x?} {:.1?}", 1..2, 10..20, 0.5..1.25);
    let h = Holder { span: 3..9, spans: vec![1..=2, 5..=5], f: double };
    let s = format!("{:?}", h);
    println!("| holder {} fn ptr hex {}", &s[..s.find("f: ").unwrap() + 3], s.contains("f: 0x"));
}

#[test]
fn a02_fn_ptr_debug_and_eq() {
    let f: fn(u32) -> u32 = double;
    let g: fn(u32) -> u32 = triple;
    let s = format!("{:?}", f);
    println!("| debug hex {} pointer same {}", s.starts_with("0x"), format!("{:p}", f) == s);
    println!("| eq {} {} {}", std::ptr::fn_addr_eq(f, f), std::ptr::fn_addr_eq(f, g), std::ptr::fn_addr_eq(f, double as fn(u32) -> u32));
    let n: fn() = || {};
    let e: extern "C" fn(i32) -> i32 = { extern "C" fn h(x: i32) -> i32 { x } h };
    println!("| shapes {} {}", format!("{:?}", n).starts_with("0x"), format!("{:?}", e).starts_with("0x"));
}

#[test]
fn a03_adapter_debug() {
    let v = [1u32, 2, 3];
    println!("| map {:?}", v.iter().map(|x| x + 1));
    println!("| filter {:?}", v.iter().filter(|x| **x > 1));
    println!("| filter_map {:?}", v.iter().filter_map(|x| Some(x)));
    println!("| skip_while {:?}", v.iter().skip_while(|x| **x < 2));
    println!("| take_while {:?}", v.iter().take_while(|x| **x < 2));
    println!("| map_while {:?}", v.iter().map_while(|x| Some(x)));
    println!("| scan {:?}", v.iter().scan(10, |s, x| { *s += x; Some(*s) }));
    println!("| inspect {:?}", v.iter().inspect(|_| {}));
    println!("| flat_map {:?}", v.iter().flat_map(|x| vec![*x; 2]));
    println!("| flatten {:?}", vec![vec![1u8], vec![2]].into_iter().flatten());
    let mut fm = v.iter().flat_map(|x| vec![*x; 2]);
    fm.next();
    println!("| flat_map started {:?}", fm);
    println!("| sources {:?} {:?} {:?} {:?} {:?}", std::iter::empty::<u8>(), std::iter::once_with(|| 1), std::iter::repeat_with(|| 1), std::iter::from_fn(|| Some(1)), std::iter::successors(Some(1u8), |x| x.checked_mul(2)));
    let mut ow = std::iter::once_with(|| 5);
    ow.next();
    println!("| once_with used {:?}", ow);
}

#[test]
fn a04_from_arrays() {
    let a: Arc<[u16]> = Arc::from([1u16, 2, 3]);
    let r: Rc<[String]> = Rc::from([String::from("a"), String::from("b")]);
    let b: Box<[u8]> = Box::from([9u8; 4]);
    let v: Vec<i32> = Vec::from([4, 5]);
    let mut arr = [7u8, 8];
    let vm: Vec<u8> = Vec::from(&mut arr);
    let e: Arc<[u8]> = Arc::from([0u8; 0]);
    println!("| {:?} {:?} {:?} {:?} {:?} {:?}", a, r, b, v, vm, e);
}

#[test]
fn a05_is_multiple_of() {
    println!("| u32 {} {} {} {}", 6u32.is_multiple_of(3), 7u32.is_multiple_of(3), 0u32.is_multiple_of(0), 5u32.is_multiple_of(0));
    println!("| usize {} {} u8 {} u64 {} u128 {}", 4096usize.is_multiple_of(64), 4100usize.is_multiple_of(64), 255u8.is_multiple_of(5), u64::MAX.is_multiple_of(3), 10u128.is_multiple_of(4));
}
