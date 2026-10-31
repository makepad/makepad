// chacha20 0.10 xchacha.rs:62 shape: `[u8]: AsRef<?T>` has one candidate in this crate's
// dependency graph, so `?T = [u8]` (rustc: trait_impls_of only visits the impls of the crates
// this crate's session loaded).
pub fn head(iv: &[u8; 24]) -> &[u8; 16] {
    iv[..16].as_ref().try_into().unwrap()
}
