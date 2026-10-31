//! `makepad run relax-provided-fixture --bin relax-stderr-flood -- <lines> [panic]`: writes <lines>
//! lines to stderr with the program's own eprintln!, then "flood done" to stdout, or panics with
//! "flood panic" (stderr_full.sh).
fn main() {
    let mut args = std::env::args().skip(1);
    let lines: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(100_000);
    let panic = args.next().as_deref() == Some("panic");
    for i in 0..lines {
        eprintln!("flood line {i:08} ................................................................");
    }
    if panic {
        panic!("flood panic after {lines} lines");
    }
    println!("flood done {lines}");
}
