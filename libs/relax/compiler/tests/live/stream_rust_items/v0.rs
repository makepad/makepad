fn value() -> i32 { 1 }
fn label() -> &'static str { "old" }
#[test]
fn run() { println!("out {} {}", value(), label()); }
