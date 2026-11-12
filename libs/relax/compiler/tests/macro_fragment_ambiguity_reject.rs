//! Two positions wait for a fragment at the same token: a local ambiguity (relax issue 21).
// rejects: local ambiguity when calling macro `last`: multiple parsing options: built-in NTs
macro_rules! last {
    ($($i:ident)* $j:ident) => { stringify!($j) };
}

pub fn ambiguous() {
    assert_eq!(last!(a b), "b");
}
