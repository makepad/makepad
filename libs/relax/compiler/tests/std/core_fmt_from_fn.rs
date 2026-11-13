// fmt::from_fn and fmt::FromFn (relax #104): the closure gets the formatter as the format spec set
// it (width, precision, flags) for Display and Debug, through core::fmt and alloc::fmt, nested,
// with write!, to_string and pad. Differential file: `| ` lines equal under rustc --test and
// makepad test.
extern crate alloc;
use std::fmt::{self, Formatter};

fn spec(f: &mut Formatter<'_>) -> fmt::Result {
    write!(f, "[{:?} {:?} {} {} {} {} {:?}]", f.width(), f.precision(), f.alternate(), f.sign_plus(),
        f.sign_minus(), f.sign_aware_zero_pad(), f.fill())
}

#[test]
fn a01_from_fn() {
    let s = fmt::from_fn(spec);
    println!("| {s}");
    println!("| {s:?}");
    println!("| {s:>7.2}");
    println!("| {s:#?}");
    println!("| {s:+}");
    println!("| {s:-}");
    println!("| {s:*^9}");
    println!("| {s:08}");
    let pair = fmt::from_fn(|f| write!(f, "{}-{}", 1, 2));
    println!("| {pair} {} {:?}", pair.to_string(), format!("{pair:?}"));
    let padded = fmt::from_fn(|f| f.pad("ab"));
    println!("| <{padded:>5}> <{padded:<5}> <{padded:.1}>");
    let n = 3;
    let list = fmt::from_fn(|f| f.debug_list().entries((0..n).map(|i| fmt::from_fn(move |f| write!(f, "#{i}")))).finish());
    println!("| {list:?}");
    println!("| {}", format!("{list:#?}").replace('\n', "/"));
    let typed: alloc::fmt::FromFn<fn(&mut Formatter<'_>) -> fmt::Result> = alloc::fmt::from_fn(spec);
    println!("| {typed:5}");
    let by_ref = &typed;
    println!("| {by_ref:<3}");
    let err = fmt::from_fn(|_| Err(fmt::Error));
    let mut out = String::new();
    println!("| {:?} {:?}", fmt::write(&mut out, format_args!("x{err}")), out);
}
