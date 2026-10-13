fn value() -> i32 { 2 }
fn label() -> &'static str { "new { /* text */ }" }
fn helper() -> i32 { 7 }
#[test]
fn run() { println!("out {} {}", value() + helper(), label()); }
