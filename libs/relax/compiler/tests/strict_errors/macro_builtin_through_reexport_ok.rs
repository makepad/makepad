//~ none
// A built-in reached through std's path or a module's re-export of it is the built-in.
mod again {
    pub use std::vec;
}
fn main() {
    let a: Vec<u32> = again::vec![1, 2];
    let b: Vec<u32> = std::vec![3];
    let c = ::std::format!("{}", a.len() + b.len());
    let _ = c;
}
