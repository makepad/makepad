// Borrowck regression found by the N01 Gandalf audit (round 52); run_live_regressions.sh checks it.
// Regression (rustc accepts; Relax once rejected it): `Clone::clone(&self.image)` with `self.image: &'a Image<T>` resolves to the impl
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
fn impl_method_callee_lifetime() { run() }
fn main() { run() }
