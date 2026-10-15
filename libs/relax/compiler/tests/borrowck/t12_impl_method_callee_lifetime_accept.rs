// T12 (conform): pre-existing borrowck false positive, rustc accepts; found by N01 Gandalf audit
// (round 52 OB, arch-n01-oB.REPORT.md). Known failure: not listed in run_live_regressions.sh;
// `makepad check` with MAKEPAD_RUN_BORROWCK=full (that script's settings) reports the errors named below,
// as does the pre-N01 tip (which does not check derived impl bodies, so it misses those). Do not
// list it until conform fixes the cause.
// Cause: `Clone::clone(&self.image)` with `self.image: &'a Image<T>` resolves to the impl
// method `<&'a Image<T> as Clone>::clone`; its callee signature confuses the impl's lifetime with
// the autoref region, giving "'1 must outlive 'a" (hand-written below, and derived Clone on a
// generic struct with a lifetime: makepad draw text/image.rs Subimage).
pub struct Image<T>(Vec<T>);
pub struct Sub<'a, T> { image: &'a Image<T>, bounds: usize }
impl<'a, T: Clone> Clone for Sub<'a, T> {
    fn clone(&self) -> Self { Sub { image: Clone::clone(&self.image), bounds: Clone::clone(&self.bounds) } }
}
#[derive(Clone, Copy)]
pub struct Subimage<'a, T> { image: &'a Image<T>, bounds: usize }
fn run() {
    let image = Image(vec![1u8]);
    let a = Sub { image: &image, bounds: 2 }.clone();
    let b = Subimage { image: &image, bounds: 3 }.clone();
    assert_eq!(a.bounds + b.bounds + a.image.0.len() + b.image.0.len(), 7);
}
#[test]
#[ignore = "T12 conform: impl-method callee lifetimes vs autoref"]
fn impl_method_callee_lifetime() { run() }
fn main() { run() }
