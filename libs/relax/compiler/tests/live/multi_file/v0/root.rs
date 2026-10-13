// G10 / 6.1: edits to files other than the crate root. v1 edits a body in helper.rs; v2 edits
// sub/deep.rs and the root in one save (one transaction over two files).
mod helper;
mod sub {
    pub mod deep;
}

#[test]
fn run() {
    println!("out {} {}", helper::scale(7), sub::deep::name());
}
