// rejects: temporary value dropped while borrowed
// An `if`'s branches are terminating scopes (rustc region.rs resolve_expr): a temporary made in
// a branch is dropped at the branch's end, before the call that takes the borrow. Fab's
// Printability had `placed(stack, if first.is_empty() { &whole(peak) } else { &first }, 0)`.
fn whole(k: usize) -> Vec<u32> {
    vec![k as u32; k + 1]
}

fn len_of(v: &Vec<u32>) -> usize {
    v.len()
}

pub fn if_arm_in_call(c: bool, x: &Vec<u32>) -> usize {
    len_of(if c { &whole(3) } else { x })
}
