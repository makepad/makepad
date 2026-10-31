// The Iterator::Item region must satisfy the closure input requirement.
pub fn named<'x>(roots: &Vec<u32>, root: &'x u32) -> bool {
    roots.iter().any(|p: &'x u32| p == root)
}

fn main() {}
