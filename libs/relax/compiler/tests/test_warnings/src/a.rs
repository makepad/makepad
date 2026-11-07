// Imports the code uses (a body, a signature, a trait's method through a macro): no warning.
use std::collections::HashMap;
use std::fmt::Write;

pub fn sizes(m: &HashMap<u8, u8>) -> usize {
    let mut s = String::new();
    let _ = write!(s, "{}", m.len());
    let n: HashMap<u8, u8> = HashMap::new();
    s.len() + n.len()
}
