use pivt_ext::{Apply, W};
fn main() {
    let mut w = W(0);
    w.apply(3);
    let a = w.0;
    W::apply(&mut w, 4);
    let b = w.0;
    println!("private-inherent {} {} {} {}", a, b, <W as Apply>::K + W::K, W::make());
}
