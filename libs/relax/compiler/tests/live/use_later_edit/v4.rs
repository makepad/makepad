mod one { pub fn pick() -> u32 { 1 } }
mod two { pub fn pick() -> u32 { 2 } }
#[test]
fn run() { println!("out c {}", pick()); }
