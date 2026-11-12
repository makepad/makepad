//! Two matcher positions reach the end of the input together: rustc's "multiple successful
//! parses" ambiguity (relax issue 21).
// rejects: ambiguity: multiple successful parses
macro_rules! twice {
    ($(a)* $(a)*) => { 0 };
}

pub fn ambiguous() {
    assert_eq!(twice!(a), 0);
}
