// The next solver evaluates a root goal that overflows once more at twice the recursion limit
// (rustc_next_trait_solver eval_ctxt; rustc 1.99.1 with -Znext-solver: a 60-deep `Send` goal
// holds under `recursion_limit = "32"`, a 70-deep one is E0275). Stable rustc's old solver proves
// `Deep: Send` here only through its cached `Shallow: Send`; Relax's solver has no such cache and
// gave E0275 without the rerun. Coherence, overlap and vtable checks keep the plain limit.
#![recursion_limit = "32"]

pub struct Wrap<T>(pub T);

// 24 levels
pub type Shallow = Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<u8>>>>>>>>>>>>>>>>>>>>>>>>;
// 18 more
pub type Deep = Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Wrap<Shallow>>>>>>>>>>>>>>>>>>;

fn sendable<T: Send>() -> bool {
    true
}

fn shareable<T: Sync>() -> bool {
    true
}

#[test]
fn a_goal_within_twice_the_limit_holds() {
    assert!(sendable::<Shallow>(), "shallow send");
    assert!(sendable::<Deep>(), "deep send");
    assert!(shareable::<Shallow>() && shareable::<Deep>(), "sync");
    assert!(sendable::<(Deep, Shallow)>(), "tuple");
}
