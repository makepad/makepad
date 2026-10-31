// The iterator borrow explicitly supplies the required closure input region.
pub fn s(v: &'static Vec<u32>, keep: &mut Vec<&'static u32>) {
    v.iter().for_each(|p| keep.push(p));
}

fn main() { let roots = Box::leak(Box::new(vec![7])); let mut keep = Vec::new(); s(roots, &mut keep); assert_eq!(*keep[0], 7); }
