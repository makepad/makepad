// rejects: temporary value dropped while borrowed
// A match arm is a terminating scope (rustc region.rs resolve_arm): a temporary made in an arm is
// dropped at the arm's end, before the call that takes the borrow.
fn whole(k: usize) -> Vec<u32> {
    vec![k as u32; k + 1]
}

fn len_of(v: &Vec<u32>) -> usize {
    v.len()
}

pub fn match_arm_in_call(k: usize, x: &Vec<u32>) -> usize {
    len_of(match k {
        0 => &whole(1),
        _ => x,
    })
}
