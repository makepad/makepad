// the program's hf-shared has no `rate`: its Meter implements nothing
fn main() {
    let _ = hf_shared::Meter;
    println!("measured {} base {}", hf_macro::measured!(), hf_shared::base());
}
