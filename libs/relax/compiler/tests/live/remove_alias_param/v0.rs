// A type alias `T` goes while a generic fn has its own parameter `T`: patched (the parameter is no
// use of the alias).
pub type T = u32;

fn id<T: Copy>(x: T) -> T {
    x
}

#[test]
fn run() {
    let v: T = 5;
    println!("out {}", id(v));
}
