// `Box<dyn Read>` through `impl<R: Read + ?Sized> Read for Box<R>`: a default method
// (`read_to_string`) on the trait object. A second, identical impl once made the instance
// ambiguous ("failed to resolve instance ... impls on non-nominal self types"); this keeps the
// single impl honest.
use std::io::{Cursor, Read};

#[test]
fn a01_default_method_through_a_boxed_trait_object() {
    let mut r: Box<dyn Read> = Box::new(Cursor::new(b"hello".to_vec()));
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    println!("| {s}");
    let mut buf = [0u8; 2];
    let mut r2: Box<dyn Read> = Box::new(Cursor::new(b"xy".to_vec()));
    println!("| {}", r2.read(&mut buf).unwrap());
}
