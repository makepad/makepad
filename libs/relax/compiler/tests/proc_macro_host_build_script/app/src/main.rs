// the program's hb-gen has `wide`: its build script's run is the program side's
fn main() {
    println!("macro side {}, program side {}", hb_macro::total!(), hb_gen::total());
}
