//~ E0594
struct P { x: u32 }
fn main() { let p = P { x: 0 }; p.x = 3; println!("{}", p.x); }
