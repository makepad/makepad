// Relaxed Rust `disjoint-index`: two consecutive `let`s borrowing elements of one container. The
// write-back is `get_disjoint_mut`, which rustc accepts.

fn add_next(v: &mut Vec<u32>, i: usize) {
    let a = &mut v[i];
    let b = &mut v[i + 1];
    *a += *b;
}

fn swap_any(s: &mut [u32], i: usize, j: usize) {
    let x = &mut s[i];
    let mut y = &mut s[j];
    std::mem::swap(x, y);
    y = &mut s[0];
    *y += 0;
}

fn main() {
    let mut v = vec![1, 2, 3];
    add_next(&mut v, 1);
    assert_eq!(v, [1, 5, 3]);
    swap_any(&mut v, 0, 2);
    assert_eq!(v, [3, 5, 1]);
    println!("disjoint_pair ok");
}
