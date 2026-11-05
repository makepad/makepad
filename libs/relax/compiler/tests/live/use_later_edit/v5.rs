mod one { pub fn pick() -> u32 { 1 } }
mod two { pub fn pick() -> u32 { 2 } }
use two::pick;
#[test]
fn run() { println!("out d {}", pick()); }
