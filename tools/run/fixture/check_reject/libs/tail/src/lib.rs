// rustc rejects this (E0597, a tail-expression temporary, edition 2021). It sits under libs/ on
// purpose: the crate is outside the application scope a run's background check keeps to, and
// `makepad check` and `makepad test` must report it all the same. check_reject.sh.
pub fn first_len(name: &str) -> Option<usize> {
    let name = name.to_lowercase();
    name.split(' ').flat_map(|w| [w, w.trim_end()]).find_map(|w| Some(w.len()))
}

#[test]
fn runs() {
    assert_eq!(first_len("a b"), Some(1));
}
