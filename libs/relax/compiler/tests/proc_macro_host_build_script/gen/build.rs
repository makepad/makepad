// The generated code follows the package's features, as a parser generator's does: with `wide`
// it names the variant only `wide` has.
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let wide = std::env::var_os("CARGO_FEATURE_WIDE").is_some();
    let sum = if wide { "Kind::One as u32 + Kind::Wide as u32" } else { "Kind::One as u32" };
    std::fs::write(format!("{out}/total.rs"), format!("pub fn total() -> u32 {{ {sum} }}\n")).unwrap();
}
