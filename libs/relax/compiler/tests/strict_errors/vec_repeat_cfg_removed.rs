//~ E0061
fn main() { let v: Vec<u8> = vec![#[cfg(any())] 1; 2]; let _ = v; }
