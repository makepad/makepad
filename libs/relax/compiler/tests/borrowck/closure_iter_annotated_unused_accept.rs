// The iterator borrow explicitly supplies the required closure input region.
pub fn named<'x>(roots: &'x Vec<u32>, _root: &'x u32) -> bool {
    roots.iter().any(|_p: &'x u32| true)
}

fn main() { let roots = vec![7]; assert!(named(&roots, &7)); }
