// Trait associated consts through paths and patterns, and `self: Box<Self>` methods on a
// box of an integer literal (rustc tests/ui associated-const-const-eval,
// issue-88074-pat-range-type-inference, autoderef-method): `<T as Tr>::C` in a pattern picks
// the impl's const, `Tr::C` infers Self from its use, `Box::new(3).double()` fixes the
// literal to the one integer type whose impl's receiver matches. rustc prints the same lines.
trait Foo {
    const NUM: usize;
}
impl Foo for i32 {
    const NUM: usize = 1;
}
const FOO: usize = <i32 as Foo>::NUM;

trait Zero {
    const ZERO: Self;
}
impl Zero for i32 {
    const ZERO: Self = 0;
}
impl Zero for u8 {
    const ZERO: Self = 7;
}

trait Double {
    fn double(self: Box<Self>) -> usize;
}
impl Double for usize {
    fn double(self: Box<usize>) -> usize {
        *self * 2
    }
}

fn classify(v: i32) -> &'static str {
    match v {
        Zero::ZERO..=1 => "low",
        <i32 as Zero>::ZERO => "zero?",
        _ => "high",
    }
}

#[test]
fn assoc_const_paths() {
    assert_eq!(FOO, 1);
    let m = match 1 {
        <i32 as Foo>::NUM => "num",
        _ => "other",
    };
    let z: u8 = Zero::ZERO;
    let x: Box<_> = Box::new(3);
    let line = format!("{} {} {} {} {} {}", m, classify(0), classify(1), classify(5), z, x.double());
    println!("{}", line);
    assert_eq!(line, "num low low high 7 6");
}
