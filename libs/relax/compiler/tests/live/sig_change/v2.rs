// G3 / 3.6: signature edits. Callers must be re-checked and recompiled with the callee.
//  v1: f(i32) -> i32 becomes f(i64, bool) -> i64 and both callers are updated in the same save.
//  v2: f's return type changes to u8 and the callers are NOT updated: refused, errors at the callers,
//      the running code unchanged.
fn f(x: i64, neg: bool) -> u8 {
    let v = x * 3;
    if neg { 1 } else { v as u8 }
}

fn caller_a() -> i64 {
    let mut s = 0i64;
    for i in 0..10 {
        s += f(i as i64, i % 2 == 0);
    }
    s
}

fn caller_b() -> i64 {
    f(1000, false) - 7
}

#[test]
fn run() {
    println!("out a {} b {}", caller_a(), caller_b());
}
