// rejects: temporary value dropped while borrowed
// A temporary in a call argument lives to the end of its statement (rustc region.rs): `r` borrows
// it past the `let`.
fn whole(k: usize) -> Vec<u32> {
    vec![k as u32; k + 1]
}

fn first(v: &[u32]) -> &u32 {
    &v[0]
}

pub fn call_arg_kept() -> u32 {
    let r = first(&whole(2));
    *r + 1
}
