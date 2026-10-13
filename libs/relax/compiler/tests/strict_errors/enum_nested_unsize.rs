//~ msg: mismatched types
// CoerceUnsized cannot convert an enum's contents, even when those contents
// could coerce on their own. rustc reports E0308 at the assignment.
fn main() {
    let concrete = Some(Box::new(7u32));
    let _object: Option<Box<dyn std::any::Any>> = concrete;
}
