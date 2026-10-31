macro_rules! scale { ($x:literal) => { $x * 3 }; }
fn edited() -> u32 { scale!(10) + 3 }
fn unchanged() -> u32 { scale!(20) }
#[test]
fn run() { println!("out {} {}", edited(), unchanged()); }
