// The Iterator::Item region must satisfy the closure input requirement.
use std::rc::Rc;
pub struct Cx;
pub type RootApply = Rc<dyn Fn(&mut Cx) -> u32>;
pub fn named<'x>(roots: &Vec<RootApply>, root: &'x RootApply) -> bool {
    roots.iter().any(|p: &'x RootApply| Rc::ptr_eq(p, root))
}

fn main() {}
