mod one { pub fn pick() -> u32 { 1 } }
mod two { pub fn pick() -> u32 { 2 } }
use one::pick;
#[test]
fn run() { println!("out b {}", pick()); }
