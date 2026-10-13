fn value() -> i32 { 2 }
fn dependent() -> i32 { helper() }
fn helper() -> i32 { 9 }
#[test]
fn run() { println!("out {} {}", value(), dependent()); }
