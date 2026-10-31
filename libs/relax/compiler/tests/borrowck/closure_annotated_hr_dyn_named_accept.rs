// Annotated closure inputs whose expected region is free (the caller's): the supplied and
// expected signatures are one, so the annotation names the region the closure's input has.
// The accepting twin of closure_annotated_hr_dyn_named_reject, plus the annotated
// higher-ranked-object input of makepad draw's queue_style_roots and rustc ui
// expect-region-supply-region.rs expect_free_supply_bound.
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
fn named<'x>(roots: &'x [RootApply], root: &RootApply) -> bool {
    any_of(roots, |p: &'x Rc<dyn Fn(&mut Cx) -> u32>| Rc::ptr_eq(p, root))
}
fn queue(roots: &Vec<RootApply>, root: &RootApply) -> bool {
    roots.iter().any(|pending: &Rc<dyn Fn(&mut Cx) -> u32>| Rc::ptr_eq(pending, root))
}
fn closure_expecting_free<'a, F>(_: &'a u32, _: F) where F: FnMut(&'a u32) {}
fn expect_free_supply_bound() -> u32 {
    let x = 22;
    let mut f: Option<&u32> = None;
    closure_expecting_free(&x, |y: &u32| {
        f = Some(y);
    });
    f.map_or(0, |v| *v)
}
fn main() {
    let a: RootApply = Rc::new(|_cx: &mut Cx| 1);
    let b: RootApply = Rc::new(|_cx: &mut Cx| 2);
    let roots = vec![a.clone()];
    assert!(named(&roots, &a));
    assert!(!named(&roots, &b));
    assert!(queue(&roots, &a));
    assert!(!queue(&roots, &b));
    assert_eq!(expect_free_supply_bound(), 0);
}
