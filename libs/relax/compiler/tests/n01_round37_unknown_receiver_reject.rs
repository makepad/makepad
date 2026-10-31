// rustc_hir_typeck/src/method/probe.rs:473-563: reject an unknown final
// receiver before selecting a trait that might otherwise constrain its type.
trait Only { fn method(&self); }
impl Only for u8 { fn method(&self) {} }
fn main() { let value = Default::default(); value.method(); }
