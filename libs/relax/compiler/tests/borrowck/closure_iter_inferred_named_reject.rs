// The Iterator::Item region must satisfy the closure input requirement.
pub fn named<'x>(roots: &Vec<u32>, keep: &mut Vec<&'x u32>) {
    roots.iter().for_each(|p| keep.push(p));
}

fn main() {}
