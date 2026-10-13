// The Iterator::Item region must satisfy the closure input requirement.
pub fn statics(roots: &Vec<u32>) -> bool {
    roots.iter().any(|_p: &'static u32| true)
}

fn main() {}
