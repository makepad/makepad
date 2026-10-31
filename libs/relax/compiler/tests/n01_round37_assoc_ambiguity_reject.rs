// rustc_hir_typeck/src/method/probe.rs:1772-1843: associated paths also pick uniquely.
trait A { fn make() -> u32 { 1 } }
trait B { fn make() -> u32 { 2 } }
fn ambiguous<T: A + B>() -> u32 { T::make() }
