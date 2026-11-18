// The inline bin: prints the square of its argument, through the package's lib.
fn main() {
    let n: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(0);
    println!("inline-tool {}", inline_target_tables::square(n));
}
