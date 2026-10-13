mod shared;

fn main() {
    let library = cache_shared_file::shared::run();
    let binary = shared::run();
    assert_eq!(library.0, 42);
    assert_eq!(binary.0, 42);
    println!("cache_shared_file result {} {} lib-size {} bin-size {}", library.0, binary.0, library.1, binary.1);
}
