// `<[T]>::array_windows::<N>()` and its ArrayWindows iterator (core slice/iter.rs, stable 1.94):
// overlapping `&[T; N]` windows from both ends, exact lengths, and what a step past either end
// leaves (its Debug shows it). A path walked in pairs this way (epaint 0.37's dashes_from_line)
// was E0599 here.
#[test]
fn a01_forward_and_back() {
    let v = [1, 2, 3, 4, 5];
    let w: Vec<[i32; 2]> = v.array_windows::<2>().copied().collect();
    println!("| {:?} {:?}", w, v.array_windows::<2>());
    let mut it = v.array_windows::<3>();
    println!("| len {} back {:?} nth1 {:?} next {:?} {:?}", it.len(), it.next_back(), it.nth(1), it.next(), it);
    let rev: Vec<_> = v.array_windows::<4>().rev().collect();
    println!("| {:?}", rev);
    let s: i32 = v.array_windows().map(|[a, b]| b - a).sum();
    println!("| sum {}", s);
}

#[test]
fn a02_past_the_ends() {
    let v = [1, 2, 3, 4, 5];
    let mut a = v.array_windows::<3>();
    println!("| nth3 {:?} {:?}", a.nth(3), a);
    let mut a = v.array_windows::<3>();
    println!("| nth9 {:?} {:?}", a.nth(9), a);
    let mut a = v.array_windows::<3>();
    println!("| nth_back3 {:?} {:?}", a.nth_back(3), a);
    let mut a = v.array_windows::<3>();
    println!("| nth_back1 {:?} {:?}", a.nth_back(1), a);
    let mut a = v.array_windows::<6>();
    println!("| n6 {:?} {:?} {:?} {:?}", a.next(), a.next_back(), a.size_hint(), a);
    let e: [u8; 0] = [];
    println!("| empty {:?} {}", e.array_windows::<1>().size_hint(), e.array_windows::<2>().count());
}

#[test]
fn a03_counts_and_ends() {
    let v: Vec<u16> = (0..7).collect();
    let a = v.array_windows::<3>();
    println!("| count {} last {:?} clone {:?}", a.clone().count(), a.clone().last(), a.clone().nth(4));
    let strs = ["a", "b", "c"];
    let joined: Vec<String> = strs.array_windows::<2>().map(|[x, y]| format!("{x}{y}")).collect();
    println!("| {:?}", joined);
    println!("| {:?}", std::slice::ArrayWindows::<u8, 1>::clone(&b"xy".array_windows::<1>()));
}
