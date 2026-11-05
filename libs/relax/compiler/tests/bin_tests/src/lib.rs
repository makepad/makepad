/// Adds. A doc test (`cargo test` runs it; makepad test counts it as not run):
///
/// ```
/// assert_eq!(bt_app::add(1, 1), 2);
/// ```
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

/// Touched once by each of tests/fresh_a.rs and tests/fresh_b.rs: each test target is a process
/// of its own, so each finds it untouched.
pub static TOUCHED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
mod tests {
    #[test]
    fn lib_add() {
        assert_eq!(super::add(2, 3), 5);
    }
}
