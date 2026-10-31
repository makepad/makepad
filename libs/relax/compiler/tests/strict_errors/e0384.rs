//~ E0384
//~ help: consider making this binding mutable: `mut x`
fn main() { let x = 1; x = 2; let y; y = 3; println!("{} {}", x, y); }
