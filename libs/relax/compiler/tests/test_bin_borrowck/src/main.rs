// The package's bin: rustc rejects `bad` (E0502), so cargo builds no bin and runs no test of the
// package's integration tests, which never start it.
struct Cx {
    n: u32,
}
impl Cx {
    fn pool(&self) -> u32 {
        self.n
    }
}
struct Outer<'a> {
    cx: &'a mut Cx,
}
struct Draw<'a> {
    cx: &'a mut Cx,
}
impl<'a> Draw<'a> {
    fn new(cx: &'a mut Cx) -> Self {
        Draw { cx }
    }
    fn draw(&mut self) {
        self.cx.n += 1;
    }
}
fn bad(cx: &mut Outer) -> u32 {
    let d = &mut Draw::new(cx.cx);
    let n = cx.cx.pool();
    d.draw();
    n
}
fn main() {
    if std::env::args().count() > 1 {
        let mut c = Cx { n: 1 };
        println!("{}", bad(&mut Outer { cx: &mut c }));
    }
    println!("{}", test_bin_borrowck::ok());
}
