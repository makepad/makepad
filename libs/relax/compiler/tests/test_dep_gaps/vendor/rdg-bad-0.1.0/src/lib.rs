// A registry crate with a body the checker rejects (a stand-in for a body cargo builds and Relax
// cannot: a std item Relax lacks), and one it accepts.
pub fn uses_missing(n: i64) -> i64 {
    n.no_such_method_here()
}

pub fn fine() -> i64 {
    1
}
