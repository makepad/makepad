// The iterator borrow explicitly supplies the required closure input region.
use std::rc::Rc;
pub struct Cx;
pub type RootApply = Rc<dyn Fn(&mut Cx) -> u32>;
pub fn named<'x>(roots: &'x Vec<RootApply>, root: &'x RootApply) -> bool {
    roots.iter().any(|p: &'x RootApply| Rc::ptr_eq(p, root))
}

fn main() { let root: RootApply = Rc::new(|_: &mut Cx| 7); let roots = vec![root.clone()]; assert!(named(&roots, &root)); }
