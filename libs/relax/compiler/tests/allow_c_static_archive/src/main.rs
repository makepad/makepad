extern "C" {
    fn fixture_add(a: i32, b: i32) -> i32;
}

fn main() {
    println!("fixture_add(2, 3) = {}", unsafe { fixture_add(2, 3) });
}
