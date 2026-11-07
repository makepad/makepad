// `makepad test` prints the warnings of the code under test, as `cargo test` does: the test fn in
// t.rs has an unused `let`, an unused import and an unused `Result` (cargo test: 3 warnings); a.rs
// uses its imports (none).
pub fn half(x: u64) -> u64 {
    x / 2
}

pub fn parse(s: &str) -> Result<u64, std::num::ParseIntError> {
    s.parse()
}

pub mod a;

#[cfg(test)]
mod t;
