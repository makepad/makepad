// Moving a body in the source preserves its parent-owned anonymous constant
// identity, including the array length in `sized`.
const TABLE: [u8; 3] = [1, 2, 3];

fn first(x: u64) -> u64 {
    x + 1
}

fn sized() -> usize {
    let buf = [0u8; TABLE.len() + 2];
    buf.len()
}

#[test]
fn run() {
    println!("out {} {}", first(1), sized());
}
