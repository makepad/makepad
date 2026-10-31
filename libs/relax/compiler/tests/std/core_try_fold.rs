// Iterator::try_fold / try_for_each over Option, Result and ControlFlow, and Hash for [T; N]
// (an array hashes as its slice). "| " lines must equal real std's (core_diff.sh).
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::ops::ControlFlow;

#[test]
fn a01_try_fold() {
    let v = [3usize, 4, 5];
    let total = v.iter().try_fold(0usize, |acc, x| {
        (*x < 10).then_some(())?;
        Some(acc + x)
    });
    println!("| option ok {:?}", total);
    let stop = v.iter().try_fold(0usize, |acc, x| if *x == 4 { None } else { Some(acc + x) });
    println!("| option stop {:?}", stop);
    let r: Result<i32, String> = [1, 2, 3].iter().try_fold(0, |a, x| if *x > 2 { Err(format!("big {}", x)) } else { Ok(a + x) });
    println!("| result {:?}", r);
    let mut it = [1, 2, 3, 4].iter();
    let c = it.try_for_each(|x| if *x == 3 { ControlFlow::Break(*x) } else { ControlFlow::Continue(()) });
    println!("| control {:?} rest {:?}", c.break_value(), it.next());
    let e: Result<(), std::num::ParseIntError> = ["1", "2"].iter().try_for_each(|s| s.parse::<i32>().map(|_| ()));
    println!("| for_each ok {}", e.is_ok());
}

#[test]
fn a02_array_hash() {
    let mut a = DefaultHasher::new();
    [1u8, 2, 3].hash(&mut a);
    let mut b = DefaultHasher::new();
    (&[1u8, 2, 3][..]).hash(&mut b);
    println!("| array = slice {}", a.finish() == b.finish());
    let mut s: HashSet<[u8; 32]> = HashSet::new();
    s.extend([[1u8; 32], [2u8; 32], [1u8; 32]]);
    println!("| set {}", s.len());
    let hash = |t: &(Option<[u8; 16]>, String)| {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    };
    let (x, y, z) = ((Some([7u8; 16]), String::from("x")), (Some([7u8; 16]), String::from("x")), (Some([8u8; 16]), String::from("x")));
    println!("| tuple equal {} differ {}", hash(&x) == hash(&y), hash(&x) != hash(&z));
}
