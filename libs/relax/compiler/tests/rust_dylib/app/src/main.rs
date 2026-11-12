fn main() {
    assert_eq!(rust_dylib_dep::answer(), 42);
    println!("rust dylib: 42");
}

#[test]
fn calls_dylib_dependency() {
    assert_eq!(rust_dylib_dep::answer(), 42);
}
