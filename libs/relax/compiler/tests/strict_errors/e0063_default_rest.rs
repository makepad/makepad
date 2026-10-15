//~ E0063
//~ help: missing field `b` in initializer of `S`
struct S { a: u32 = 1, b: u32 }
fn main() { let s = S { .. }; let _ = (s.a, s.b); }
