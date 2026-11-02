// Borrowck regression found by the N01 Gandalf audit (round 52); run_live_regressions.sh checks it.
// Regression (rustc accepts; Relax once rejected it): in an overriding `Iterator::position` of `It<'a, T>` (Item = &'a T),
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
fn iterator_position_override() { run() }
fn main() { run() }
