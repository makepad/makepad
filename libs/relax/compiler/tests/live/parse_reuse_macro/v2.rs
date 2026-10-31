macro_rules! scale { ($x:literal) => { $x * 2 }; }
fn edited() -> u32 { scale!(10) + 2 }
fn unchanged() -> u32 { scale!(20) }
#[test]
fn run() { println!("out {} {}", edited(), unchanged()); }
