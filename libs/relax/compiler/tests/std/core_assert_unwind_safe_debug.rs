// Debug (stable since 1.16) and Default (1.62) for panic::AssertUnwindSafe<T>. futures-util 0.3's
// `Remote<Fut>` (src/future/future/remote_handle.rs) prints its `CatchUnwind<AssertUnwindSafe<Fut>>`
// field with `f.debug_tuple("Remote").field(&self.future).finish()`: CatchUnwind derives Debug,
// so the field is Debug only when the wrapper is, and the std here had no impl ("expected `&dyn
// core::fmt::Debug`, found `&CatchUnwind<AssertUnwindSafe<T0>>`").
use std::fmt;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};

// futures-util's shape
#[derive(Debug)]
struct CatchUnwind<Fut> {
    future: Fut,
}
struct Remote<Fut: Future> {
    future: CatchUnwind<AssertUnwindSafe<Fut>>,
}
impl<Fut: Future + fmt::Debug> fmt::Debug for Remote<Fut> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Remote").field(&self.future).finish()
    }
}

#[derive(Debug, Default)]
struct Guarded {
    count: AssertUnwindSafe<u32>,
    names: AssertUnwindSafe<Vec<String>>,
}

#[test]
fn assert_unwind_safe_prints_as_a_tuple_struct() {
    assert_eq!(format!("{:?}", AssertUnwindSafe(5)), "AssertUnwindSafe(5)");
    assert_eq!(format!("{:?}", AssertUnwindSafe("text")), "AssertUnwindSafe(\"text\")");
    assert_eq!(format!("{:#?}", AssertUnwindSafe(vec![1u8])), "AssertUnwindSafe(\n    [\n        1,\n    ],\n)");
    assert_eq!(format!("{:x?}", AssertUnwindSafe(255)), "AssertUnwindSafe(ff)");
    // through a reference to a wrapped mutable borrow, the usual catch_unwind argument
    let mut total = 3;
    let mut wrapped = AssertUnwindSafe(&mut total);
    assert_eq!(format!("{:?}", &wrapped), "AssertUnwindSafe(3)");
    let r = catch_unwind(move || {
        **wrapped += 1;
    });
    assert!(r.is_ok());
    assert_eq!(total, 4);

    let remote = Remote { future: CatchUnwind { future: AssertUnwindSafe(std::future::ready(7)) } };
    assert_eq!(format!("{:?}", remote), "Remote(CatchUnwind { future: AssertUnwindSafe(Ready(Some(7))) })");
}

#[test]
fn assert_unwind_safe_default() {
    let g = Guarded::default();
    assert_eq!((*g.count, g.names.len()), (0, 0));
    assert_eq!(format!("{:?}", g), "Guarded { count: AssertUnwindSafe(0), names: AssertUnwindSafe([]) }");
    let s: AssertUnwindSafe<String> = Default::default();
    assert_eq!(s.0, "");
}
