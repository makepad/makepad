//~ E0308
// A `?` in a `try` block makes the block's type from its residual (`residual_into_try_type`), not
// from the context: a `ParseIntError` residual gives `Result<u8, ParseIntError>`, which is no
// `Result<u8, Box<dyn Error>>` (relax issue 34).
#![feature(try_blocks)]
fn main() {
    let r: Result<u8, Box<dyn std::error::Error>> = try { "300".parse::<u8>()? };
    let _ = r;
}
