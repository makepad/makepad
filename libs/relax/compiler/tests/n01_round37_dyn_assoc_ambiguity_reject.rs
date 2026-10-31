// rustc_hir_typeck/src/method/probe.rs: object candidates include all supertraits.
trait A { fn method(&self) {} }
trait B { fn method(&self) {} }
trait C: A + B {}
fn ambiguous(value: &dyn C) { <dyn C>::method(value); }
