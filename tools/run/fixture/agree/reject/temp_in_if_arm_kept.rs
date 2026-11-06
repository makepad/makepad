// rejects: temporary value dropped while borrowed
// A temporary made in an `if` branch that is a call argument is not extended by the `let`
// (rustc extends only through the initializer's own `if`, match and block tails): it is dropped
// at the branch's end while `r` still borrows it.
fn whole(k: usize) -> Vec<u32> {
    vec![k as u32; k + 1]
}

fn pick(v: &Vec<u32>) -> &Vec<u32> {
    v
}

pub fn if_arm_kept(c: bool, x: &Vec<u32>) -> usize {
    let r = pick(if c { &whole(2) } else { x });
    r.len()
}
