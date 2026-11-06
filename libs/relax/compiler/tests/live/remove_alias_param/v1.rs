// A type alias `T` goes while a generic fn has its own parameter `T`: patched (the parameter is no
// use of the alias).
fn id<T: Copy>(x: T) -> T {
    x
}

#[test]
fn run() {
    let v: u32 = 6;
    println!("out {}", id(v));
}
