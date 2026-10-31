// T12 (conform): pre-existing borrowck false positive, rustc accepts; found by N01 Gandalf audit
// (round 52 OB, arch-n01-oB.REPORT.md). Known failure: not listed in run_live_regressions.sh;
// `makepad check` with MAKEPAD_RUN_BORROWCK=full (that script's settings) reports the errors named below,
// as does the pre-N01 tip (which does not check derived impl bodies, so it misses those). Do not
// list it until conform fixes the cause.
// Cause: in an overriding `Iterator::position` of `It<'a, T>` (Item = &'a T),
// `while let Some(x) = self.next()` reports "'1 must outlive 'a" (Relax std
// core/slice/iter.rs:84).
pub struct It<'a, T> { s: &'a [T] }
impl<'a, T> Iterator for It<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> { let (first, rest) = self.s.split_first()?; self.s = rest; Some(first) }
    fn position<P: FnMut(&'a T) -> bool>(&mut self, predicate: P) -> Option<usize> {
        let mut predicate = predicate;
        let mut i = 0;
        while let Some(x) = self.next() {
            if predicate(x) { return Some(i); }
            i += 1;
        }
        None
    }
}
fn run() { assert_eq!(It { s: &[1, 2, 3] }.position(|x| *x == 3), Some(2)); }
#[test]
#[ignore = "T12 conform: Iterator::position override"]
fn iterator_position_override() { run() }
fn main() { run() }
