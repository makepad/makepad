// The accepted neighbours of reject/temp_*: a `let` initializer's own `if` and match extend a
// borrowed temporary in their branches to the `let`'s scope (rustc 1.79), and a call argument's
// temporary is fine for as long as its statement runs.
fn whole(k: usize) -> Vec<u32> {
    vec![k as u32; k + 1]
}

fn len_of(v: &Vec<u32>) -> usize {
    v.len()
}

fn first(v: &[u32]) -> &u32 {
    &v[0]
}

pub fn let_if_extended(c: bool, x: &Vec<u32>) -> usize {
    let r = if c { &whole(3) } else { x };
    r.len()
}

pub fn let_match_extended(k: usize, x: &Vec<u32>) -> usize {
    let r = match k {
        0 => &whole(1),
        _ => x,
    };
    r.len()
}

pub fn call_arg_in_statement() -> u32 {
    let n = *first(&whole(2)) + 1;
    n + len_of(&whole(4)) as u32
}

pub fn branch_temp_used_in_branch(c: bool, x: &Vec<u32>) -> usize {
    if c { len_of(&whole(3)) } else { len_of(x) }
}

#[test]
fn extended() {
    let x = whole(5);
    assert_eq!(let_if_extended(true, &x) + let_match_extended(1, &x) + branch_temp_used_in_branch(true, &x), 4 + 6 + 4);
    assert_eq!(call_arg_in_statement(), 3 + 5);
}
