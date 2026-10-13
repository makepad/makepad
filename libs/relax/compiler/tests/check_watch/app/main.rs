fn value() -> u32 { 1 }
fn caller() -> u32 { value() }
fn untouched() {}
fn main() { let _ = caller(); }
