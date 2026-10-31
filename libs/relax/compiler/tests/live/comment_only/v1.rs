// Comment-only and whitespace-only edits are no change: nothing is rebuilt, and positions in
// later errors refer to the new text.
/// Triples.
fn kernel(x: u64) -> u64 {
    // the formula
    x  *  3 + 1
}

#[test]
fn run() {
    println!("out {}", kernel(5));
}
