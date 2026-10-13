// G9 / 7.1: adding items, the commonest edit.
//  v1: a new helper fn, called from kernel.
//  v2: a new struct with a Display impl and a derive, used from kernel.

fn helper(x: u64) -> u64 {
    x * 100
}

fn kernel(x: u64) -> String {
    format!("{}", helper(x) + 1)
}

#[test]
fn run() {
    println!("out {}", kernel(41));
}
