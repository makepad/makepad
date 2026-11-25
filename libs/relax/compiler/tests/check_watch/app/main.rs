fn value() -> u32 { 1 }
fn caller() -> u32 { value() }
fn untouched() {}
trait Shape { fn area(&self, scale: u32) -> u32; }
struct Square(u32);
impl Shape for Square { fn area(&self, scale: u32) -> u32 { let _ = scale; self.0 * self.0 } }
fn main() { let _ = caller(); let _ = Square(2).area(1); }
