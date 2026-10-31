// The iterator borrow explicitly supplies the required closure input region.
pub fn named<'x>(roots: &'x Vec<u32>, keep: &mut Vec<&'x u32>) {
    roots.iter().for_each(|p| keep.push(p));
}

fn main() { let roots = vec![7]; let mut keep = Vec::new(); named(&roots, &mut keep); assert_eq!(*keep[0], 7); }
