// 7.1 / 7.3: item edits that do not type-check are refused whole, and leave nothing behind.
//  v1: an added fn has a type error: refused (the running program is unchanged).
//  v2: `a` is removed while `b` still calls it: refused, the error at the call.
//  v3: `c` is added, `b` calls it, `a` is removed: applies.
fn a(x: u64) -> u64 {
    x + 1
}

fn b(x: u64) -> u64 {
    a(x) * 2
}

#[test]
fn run() {
    println!("out {}", b(5));
}
