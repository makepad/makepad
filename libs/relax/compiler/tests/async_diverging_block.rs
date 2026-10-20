// An async block whose body only diverges has a return type variable that falls back
// like any variable a `!` flowed into (it used to report E0282).
use std::future::Future;

fn block_on_never<F: Future>(_f: F) -> F::Output { loop {} }

#[allow(dead_code, unreachable_code)]
fn never_bodies() {
    let _a = async move { unimplemented!() };
    let _b = || async move { unimplemented!() };
    let _c = block_on_never(async move { (move || async move { unimplemented!() })() });
}

#[test]
fn compiles() {}
