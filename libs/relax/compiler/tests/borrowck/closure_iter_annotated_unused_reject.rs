// The Iterator::Item region must satisfy the closure input requirement.
pub fn named<'x>(roots: &Vec<u32>, _root: &'x u32) -> bool {
    roots.iter().any(|_p: &'x u32| true)
}

fn main() {}
