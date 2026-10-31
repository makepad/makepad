// The Iterator::Item region must satisfy the closure input requirement.
pub fn s(v: &Vec<u32>, keep: &mut Vec<&'static u32>) {
    v.iter().for_each(|p| keep.push(p));
}

fn main() {}
