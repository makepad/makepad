// Polonius (relaxed Rust A2): still rejected in relaxed crates. Polonius accepts only what is
// sound: real aliasing, use after move, dangling borrows and a borrow used on every path stay errors.
//@ relaxed
use std::collections::HashMap;

fn alias() {
    let mut v = vec![1];
    let a = &mut v;
    let b = &mut v; //~ ERROR E0499
    a.push(1);
    b.push(2);
}

fn use_after_move() {
    let s = String::from("x");
    let t = s;
    drop((s, t)); //~ ERROR E0382
}

fn dangle() -> &'static String {
    let s = String::new();
    &s //~ ERROR E0515
}

fn scoped() {
    let r;
    {
        let x = 5;
        r = &x; //~ ERROR E0597
    }
    drop(r);
}

// the lookup's borrow is used after the insert on the same path
fn used_after(map: &mut HashMap<u32, String>) -> &mut String {
    let v = map.get_mut(&0);
    map.insert(1, String::new()); //~ ERROR E0499
    v.unwrap()
}

// the borrow is returned on both paths, the mutation is between
fn both_paths(v: &mut Vec<u32>, c: bool) -> &u32 {
    let first = &v[0];
    if c {
        v.push(1); //~ ERROR E0502
    }
    first
}

// assigning to a borrowed place while the borrow is live on that path
fn assign_borrowed(mut n: u32) -> u32 {
    let r = &n;
    n = 3; //~ ERROR E0506
    *r + n
}

// a loop that keeps the borrow of the previous iteration alive
fn keep_across_iterations(v: &mut Vec<u32>) {
    let mut last: Option<&mut u32> = None;
    for _ in 0..2 {
        v.push(0); //~ ERROR E0499
        if let Some(l) = last { *l += 1; }
        last = v.last_mut();
    }
}

fn main() {
    alias(); use_after_move(); let _ = dangle(); scoped();
    let mut m = HashMap::new(); used_after(&mut m);
    let mut v = vec![1]; both_paths(&mut v, true); assign_borrowed(1); keep_across_iterations(&mut v);
}
