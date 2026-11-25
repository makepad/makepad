//! A `vis` fragment matches nothing only before a token it may be followed by: at the end of
//! the input no position waits for it, so the arm does not match (rustc's matcher; relax issue 21).
// rejects: unexpected end of macro invocation
macro_rules! vis_only {
    ($v:vis) => { 0 };
}

pub fn no_match() {
    assert_eq!(vis_only!(), 0);
}
