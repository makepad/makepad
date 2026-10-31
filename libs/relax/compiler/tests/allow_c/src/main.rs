extern "C" fn inc(x: i32) -> i32 { x + 1 }
fn main() {
    println!("add {}", cnative::add(40, 2));
    println!("hash {:016x}", cnative::hash(b"makepad"));
    println!("apply {}", cnative::apply(inc, 20));
    println!("cfg {}", cnative::built());
}
