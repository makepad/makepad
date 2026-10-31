/// Adds. A doc test (`cargo test` runs it; makepad test counts it as not run):
///
/// ```
/// assert_eq!(bt_app::add(1, 1), 2);
/// ```
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

#[cfg(test)]
mod tests {
    #[test]
    fn lib_add() {
        assert_eq!(super::add(2, 3), 5);
    }
}
