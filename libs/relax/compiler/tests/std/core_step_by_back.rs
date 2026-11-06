// std-core differential test (core_diff.sh): `StepBy` is double-ended over an exact-size,
// double-ended iterator (core step_by.rs), so `(0..n).step_by(k).rev()` builds. Relax's std had only
// the forward half: a slicer test's `for i in (0..10).step_by(3).rev()` was refused with E0277
// "`StepBy<Range<{integer}>>: DoubleEndedIterator` is not satisfied".

#[test]
fn a01_rev_in_for() {
    // the element type is the literal's until the body fixes it
    let mut v: Vec<usize> = Vec::new();
    for i in (0..10).step_by(3).rev() {
        v.push(i);
    }
    println!("| rev pushed {:?}", v);
    let mut s = String::new();
    for i in (0..10).step_by(4).rev() {
        s += &format!("{} ", i);
    }
    println!("| rev default {}", s);
    let a = vec![1.5f64; 7];
    let mut t = 0.0;
    for i in (1..a.len()).step_by(3).rev() {
        t += a[i] * i as f64;
    }
    println!("| rev usize {}", t);
}

#[test]
fn a02_rev_collect_and_ends() {
    let v: Vec<u32> = (1..9).step_by(2).rev().collect();
    println!("| collect {:?}", v);
    // every length against every step: the last element is the one `next_back` gives
    for n in 0..8u8 {
        for k in 1..5 {
            let f: Vec<u8> = (0..n).step_by(k).collect();
            let b: Vec<u8> = (0..n).step_by(k).rev().collect();
            println!("| n {} k {} fwd {:?} back {:?}", n, k, f, b);
        }
    }
    let e: Vec<i32> = (5..5).step_by(2).rev().collect();
    println!("| empty {:?}", e);
}

#[test]
fn a03_both_ends_and_nth_back() {
    // after the first forward take the remainder counts from the next element
    let mut it = (0..20).step_by(3);
    let a = it.next();
    let b = it.next_back();
    let c = it.next();
    let d = it.next_back();
    println!("| mixed {:?} {:?} {:?} {:?} len {}", a, b, c, d, it.len());
    let rest: Vec<i32> = it.rev().collect();
    println!("| rest {:?}", rest);
    let mut it = (0..30u16).step_by(4);
    println!("| nth_back {:?} {:?} {:?}", it.nth_back(0), it.nth_back(2), it.nth_back(100));
    let mut it = (0..30u16).step_by(4);
    println!("| nth_back after {:?} {:?} {:?}", it.next(), it.nth_back(usize::MAX), it.next());
    let r = (0..17).step_by(5).rfold(String::new(), |s, x| format!("{}{},", s, x));
    println!("| rfold {}", r);
    let f = (0..17).step_by(5).rfind(|x| x % 2 == 0);
    println!("| rfind {:?}", f);
}

#[test]
fn a04_other_double_ended_sources() {
    let v = [10, 11, 12, 13, 14, 15, 16];
    let s: Vec<&i32> = v.iter().step_by(3).rev().collect();
    println!("| slice iter {:?}", s);
    let c: Vec<char> = "abcdefgh".chars().collect::<Vec<_>>().into_iter().step_by(3).rev().collect();
    println!("| vec into_iter {:?}", c);
    let z: Vec<(usize, i32)> = v.iter().copied().enumerate().step_by(2).rev().collect();
    println!("| enumerate {:?}", z);
    let w: Vec<u8> = (0..=9u8).step_by(4).rev().collect();
    println!("| inclusive {:?}", w);
}
