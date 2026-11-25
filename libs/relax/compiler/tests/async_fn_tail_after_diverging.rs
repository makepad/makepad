//! An async fn whose body has a tail after a diverging statement types that tail as its output, as a
//! plain fn and an async block do (rustc coerces the unreachable tail): `panic!(..); 7u8` defines
//! `impl Debug` as u8. One whose body only diverges falls back like any variable a `!` reached.
//! Relax reported E0282 (cannot infer the opaque's hidden type) for both async fn shapes.
#![allow(unreachable_code)]
use std::future::Future;

fn hold<F: Future>(f: F) -> Option<F::Output> {
    drop(f);
    None
}

async fn opaque_tail() -> impl std::fmt::Debug {
    panic!("never polled");
    7u8
}

async fn declared_tail() -> Result<u8, String> {
    panic!("never polled");
    Ok(7)
}

async fn only_diverges() -> impl std::fmt::Debug {
    unimplemented!()
}

fn takes<F, Fut>(f: F) -> bool
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<u16, String>>,
{
    drop(f());
    true
}

struct Holder {
    t: Option<Option<Result<(), String>>>,
}

#[test]
fn tails_after_a_diverging_statement() {
    drop(opaque_tail());
    drop(declared_tail());
    drop(only_diverges());
    // async blocks typed from where they are used
    let h = Holder { t: Some(hold(async { panic!("never polled"); Ok(()) })) };
    assert!(h.t.unwrap().is_none());
    assert!(takes(|| async { panic!("never polled"); Ok(1) }));
}
