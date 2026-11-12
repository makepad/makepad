//! A `tt` repetition followed by a token both can take: rustc's matcher stops with a local
//! ambiguity instead of choosing a way (relax issue 21).
// rejects: local ambiguity when calling macro `split`: multiple parsing options: built-in NTs tt ('t') or 1 other option.
macro_rules! split {
    ($($t:tt)* ; $e:expr) => { $e };
}

pub fn ambiguous() {
    assert_eq!(split!(a b ; 1), 1);
}
