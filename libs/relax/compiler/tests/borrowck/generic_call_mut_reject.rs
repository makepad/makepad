fn bad<F: FnMut()>(f: F) {
    let mut o = Some(f);
    (|| if let Some(f) = o.take() { f() })(); // E0596 even for an Fn instance
}
fn main() {
    bad(|| {});
    let mut n = 0;
    bad(|| n += 1);
}
