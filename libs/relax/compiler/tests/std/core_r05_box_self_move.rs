// std-core repro F-STD17 (CRASH, core_diff.sh): calling a `self: Box<Self>` method moves the box; Relax also drops it at scope end -> double free, abort.
#[test]
fn a01() {
    let b: Box<str> = Box::from("hello");
    let t = b.into_string();
    println!("| {}", t);
}
#[test]
fn a02() {
    let b: Box<str> = Box::from("hello");
    let t = str::into_string(b);
    println!("| {}", t);
}
