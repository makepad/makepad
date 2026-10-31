// The app sees both crates: its own `[u8]: AsRef<?T>` would be ambiguous, but each crate's code
// is solved against the impls of its own dependency graph.
fn main() {
    let iv: [u8; 24] = core::array::from_fn(|i| i as u8);
    let h = ivx_cipher::head(&iv);
    let b: &ivx_bytes::Bytes = iv[..].as_ref();
    let s: u32 = h.iter().map(|&x| x as u32).sum();
    println!("impl_view_xcrate {} {}", s, b.0);
}
