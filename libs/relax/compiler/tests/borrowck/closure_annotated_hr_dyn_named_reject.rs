// An annotated closure input over a higher-ranked trait object: the annotated 'x is equated with
// the callee's free input region 'i (the borrow of `roots`), which the parent does not relate to
// 'x -- the closure's requirement must reach the parent (rustc E0621).
use std::rc::Rc;
struct Cx;
type RootApply = Rc<dyn Fn(&mut Cx) -> u32>;
fn any_of<'i, F: FnMut(&'i RootApply) -> bool>(roots: &'i [RootApply], mut f: F) -> bool {
    for r in roots {
        if f(r) {
            return true;
        }
    }
    false
}
fn named<'x>(roots: &[RootApply], root: &'x RootApply) -> bool {
    any_of(roots, |p: &'x Rc<dyn Fn(&mut Cx) -> u32>| Rc::ptr_eq(p, root))
}
fn main() {
    let root: RootApply = Rc::new(|_cx: &mut Cx| 1);
    assert!(named(&[root.clone()], &root));
}
