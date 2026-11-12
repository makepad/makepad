//! A fragment that does not parse ends the call: the later arm that would match is not tried
//! (rustc's matcher reports the fragment parser's error; relax issue 21).
// rejects: expected expression
macro_rules! first {
    ($e:expr) => { 1 };
    ($($t:tt)*) => { 2 };
}

pub fn not_tried() {
    assert_eq!(first!(1 +), 2);
}
