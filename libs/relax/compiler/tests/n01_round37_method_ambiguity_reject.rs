// rustc_hir_typeck/src/method/probe.rs:1772-1843: two distinct traits are E0034.
trait A { fn same(&self) {} }
trait B { fn same(&self) {} }
fn ambiguous<T: A + B>(x: T) { x.same(); }
fn reference<T: A + B>(x: &T) { x.same(); }
trait Item { type Value: A + B; fn value(&self) -> Self::Value; }
fn projection<T: Item>(x: T) { x.value().same(); }
