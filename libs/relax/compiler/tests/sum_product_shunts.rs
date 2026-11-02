// core's blanket Sum/Product impls (iter/traits/accum.rs): Option and Result short-circuit at the
// first None / Err, Wrapping and Saturating fold with their own operators.
use std::num::{Saturating, Wrapping};

fn rate(rows: &[(f64, f64)]) -> Option<f64> {
    rows.iter().map(|r| -> Option<f64> { if r.1 == 0.0 { None } else { Some(r.0 / r.1) } }).sum::<Option<f64>>()
}

#[test]
fn option_sum_product() {
    assert_eq!(rate(&[(1.0, 2.0), (3.0, 4.0)]), Some(1.25));
    assert_eq!(rate(&[(1.0, 2.0), (3.0, 0.0)]), None);
    let p: Option<u32> = [Some(2u32), Some(3), Some(4)].into_iter().product();
    assert_eq!(p, Some(24));
    let words = ["have", "a", "good", "day"];
    let total: Option<usize> = words.iter().map(|w| w.find('a')).sum();
    println!("D option_none {:?}", total);
}

#[test]
fn result_sum_product() {
    let ok: Result<i64, String> = ["5", "10", "1", "2"].iter().map(|w| w.parse::<i64>().map_err(|e| e.to_string())).product();
    assert_eq!(ok, Ok(100));
    let bad: Result<i64, &str> = [Ok(1i64), Err("stop"), Ok(3)].into_iter().product();
    assert_eq!(bad, Err("stop"));
    let s: Result<i32, &str> = [Ok(1), Ok(2)].into_iter().sum();
    println!("D result_sum {:?}", s);
}

#[test]
fn wrapping_saturating() {
    let w: Wrapping<u8> = [Wrapping(200u8), Wrapping(100)].iter().sum();
    assert_eq!(w, Wrapping(44));
    let m: Wrapping<i8> = [Wrapping(16i8), Wrapping(16)].into_iter().product();
    assert_eq!(m, Wrapping(0));
    let s: Saturating<u8> = [Saturating(200u8), Saturating(100)].into_iter().sum();
    assert_eq!(s, Saturating(255));
    let sp: Saturating<u16> = [Saturating(300u16), Saturating(300)].iter().product();
    println!("D saturating_product {}", sp.0);
}

#[test]
fn tuple_collections() {
    let (a, b, c): (Vec<u8>, String, Vec<i32>) = [(1u8, 'x', -1i32), (2, 'y', -2)].into_iter().collect();
    assert_eq!((a, b.as_str(), c), (vec![1, 2], "xy", vec![-1, -2]));
    let (one,): (Vec<u8>,) = [(7u8,), (8,)].into_iter().collect();
    let mut both = (Vec::new(), Vec::new(), Vec::new());
    both.extend([(1, 2, 3), (4, 5, 6)]);
    let units: () = [(), ()].into_iter().collect();
    println!("D tuples {:?} {:?} {:?}", one, both, units);
}
