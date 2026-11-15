// Uses one macro crate's macro; another macro crate it depends on does not load in the host.
fn main() {
    println!("answer {}", pfi_good::answer!());
    #[cfg(feature = "bad")]
    println!("seven {}", pfi_bad::seven!());
}
