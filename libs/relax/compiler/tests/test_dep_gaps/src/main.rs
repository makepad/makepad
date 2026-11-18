// A headless program (`makepad run`): it runs while the background type check loads the skipped
// file of rdg-good (extra.rs, which nothing it runs names) and stores that crate's verdict.
fn main() {
    println!("{}", test_dep_gaps::good());
    std::thread::sleep(std::time::Duration::from_millis(1000));
}
