// Disjoint indices (relaxed Rust D3): still rejected in relaxed crates. The same element twice,
// a borrow that does not dominate the other, the whole container, a sub-slice, a move, and a
// borrow kept from an earlier iteration stay errors.
//@ relaxed

fn same(v: &mut Vec<u32>, i: usize) {
    let a = &mut v[i];
    let b = &mut v[i]; //~ ERROR E0499
    *a += *b;
}

fn same_const(a: &mut [u32; 3]) {
    let x = &mut a[1];
    let y = &mut a[1]; //~ ERROR E0499
    *x += *y;
}

fn conditional(v: &mut Vec<u32>, c: bool, i: usize, j: usize) {
    let mut zero = 0;
    let mut r = &mut zero;
    if c {
        r = &mut v[i];
    }
    let b = &mut v[j]; //~ ERROR E0499
    *r += *b;
}

fn whole(v: &mut Vec<u32>, i: usize) {
    let a = &mut v[i];
    v.push(1); //~ ERROR E0499
    *a += 1;
}

fn sub_slice(s: &mut [u32], i: usize) {
    let a = &mut s[i];
    let t = &mut s[1..]; //~ ERROR E0499
    t[0] = 1;
    *a = 2;
}

fn moved(v: Vec<String>) {
    let a = &v[0];
    drop(v); //~ ERROR E0505
    drop(a);
}

fn kept_across_iterations(v: &mut Vec<u32>) {
    let mut refs = Vec::new();
    for i in 0..2 {
        refs.push(&mut v[i]); //~ ERROR E0499
    }
    drop(refs);
}

fn main() {
    let mut v = vec![1, 2];
    same(&mut v, 0); same_const(&mut [1, 2, 3]); conditional(&mut v, true, 0, 1); whole(&mut v, 0);
    sub_slice(&mut [1, 2], 0); moved(vec![]); kept_across_iterations(&mut v);
}
