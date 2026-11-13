// The overflow rerun is one evaluation at twice the recursion limit, no more: a 70-deep `Send`
// goal under `recursion_limit = "32"` is E0275 (rustc 1.99.1, with or without -Znext-solver;
// 60 deep holds with the next solver: solver_overflow_twice_the_limit.rs).
// rejects: overflow evaluating the requirement
#![recursion_limit = "32"]

pub struct Wrap<T>(pub T);

fn sendable<T: Send>() -> bool {
    true
}

pub fn deep() -> bool {
    sendable::<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<u8>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>()
}
