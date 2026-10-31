//~ E0596
struct S { v: Vec<u8> }
impl S { fn add(&mut self) { self.v.push(1); } fn bad(&self) { self.add(); } }
fn main() { let mut s = S { v: vec![] }; s.add(); s.bad(); }
