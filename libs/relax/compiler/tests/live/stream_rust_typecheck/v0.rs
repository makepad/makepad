fn value() -> i32 { 1 }
fn dependent() -> i32 { 3 }
#[test]
fn run() { println!("out {} {}", value(), dependent()); }
