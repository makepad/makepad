//~ E0034
// N01 method-probe inventory: both where-clause candidates must be considered.
trait A { fn method(&self) {} }
trait B { fn method(&self) {} }
fn call<T: A + B>(value: T) { value.method(); }
fn main() {}
