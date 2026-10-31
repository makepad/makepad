// The iterator borrow explicitly supplies the required closure input region.
pub fn named<'x>(roots: &'x Vec<u32>, root: &'x u32) -> bool {
    roots.iter().any(|p: &'x u32| p == root)
}

fn main() { let roots = vec![7]; assert!(named(&roots, &7)); }
