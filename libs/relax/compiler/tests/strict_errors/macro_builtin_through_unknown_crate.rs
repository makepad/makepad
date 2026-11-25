//~ E0433
//~ help: cannot find module or crate `not_a_crate` in this scope
// A built-in's name reached through no crate is no call of the built-in (rustc's resolve error,
// not `vec!`).
fn main() {
    let v: Vec<u32> = not_a_crate::vec![1, 2];
    let _ = v;
}
