// The iterator borrow explicitly supplies the required closure input region.
pub fn statics(roots: &'static Vec<u32>) -> bool {
    roots.iter().any(|_p: &'static u32| true)
}

fn main() { let roots = Box::leak(Box::new(vec![7])); assert!(statics(roots)); }
