// 2b: a live save of a file the code includes (include_str!/include_bytes!) re-evaluates the
// macro like a source edit. v1 and v2 save prog.txt only: the const and the fn that include it
// take the new text (v2: from v1's, not v0's); other() includes an unchanged file.
const PROG: &str = include_str!("prog.txt");

fn program() -> &'static str {
    include_str!("prog.txt")
}

fn other() -> usize {
    include_bytes!("other.txt").len()
}

#[test]
fn run() {
    println!("out const {}", PROG.trim());
    println!("out fn {}", program().trim());
    println!("out other {}", other());
}
