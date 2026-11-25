//~ E0433
//~ help: cannot find `vec` in `plain`
// A built-in's name through a module that has no such macro: rustc's resolve error at the macro's
// own segment.
mod plain {}
fn main() {
    let v: Vec<u32> = plain::vec![1, 2];
    let _ = v;
}
