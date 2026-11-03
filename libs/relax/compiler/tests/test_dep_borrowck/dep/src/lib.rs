pub fn add(a: u32, b: u32) -> u32 { a + b }

pub fn first_after_push(v: &mut Vec<u32>) -> u32 {
    let first = &v[0];
    v.push(1);
    *first
}
