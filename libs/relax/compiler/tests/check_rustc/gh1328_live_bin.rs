// GitHub #1328 use patterns (a binary): constants used only in thread_local!/LazyLock initialisers,
// array::from_fn closures, assert!/vec!/format!/matches!, for loops; methods used only as fn
// values, in iterator closures, through dyn, from generic/async fns. rustc: no diagnostics.
use std::cell::RefCell;
use std::sync::LazyLock;
const SH_QUAD: usize = 0;
const SH_SIMPLE: usize = 1;
const SH_A: usize = 2;
const SH_B: usize = 3;
const SH_C: usize = 4;
const SH_D: usize = 5;
const SH_E: usize = 6;
const SH_F: usize = 7;
const SH_G: usize = 8;
const SH_H: usize = 9;
const SHADER_NAMES: [&str; 2] = ["q", "s"];
const SHADER_FLOATS: [usize; 2] = [8, 6];
thread_local! { static TL: RefCell<usize> = RefCell::new(SH_QUAD); }
static LZ: LazyLock<usize> = LazyLock::new(|| SH_SIMPLE);
struct R { a: u32, b: u32, c: u32, d: u32, e: u32 }
impl R {
    fn intersect_rect(&self) -> u32 { self.a }
    fn add(&mut self) { self.b += 1; }
    fn flush(&mut self) -> u32 { self.c }
    fn submit(r: &R) -> u32 { r.d }
    fn via_dyn(&self) -> u32 { self.e }
}
trait T { fn t(&self) -> u32; }
impl T for R { fn t(&self) -> u32 { self.via_dyn() } }
fn generic<X: Default>() -> X { let _ = SH_G; X::default() }
async fn asy() -> usize { SH_H }
fn imp() -> impl Fn() -> usize { || SH_F }
fn main() {
    let names: [usize; 2] = std::array::from_fn(|i| SHADER_FLOATS[i]);
    assert!(SH_A < 10);
    let v = vec![SH_B];
    let s = format!("{}", SHADER_NAMES[0]);
    let m = matches!(3usize, SH_C);
    let mut sum = 0;
    for sh in [SH_D, SH_E] { sum += sh; }
    let mut r = R { a: 1, b: 2, c: 3, d: 4, e: 5 };
    let o = Some(&r).map(R::intersect_rect);
    [&mut r].into_iter().for_each(R::add);
    let fl = (|x: &mut R| x.flush())(&mut r);
    let sb = [&r].iter().map(|x| R::submit(x)).sum::<u32>();
    let d: &dyn T = &r;
    let _fut = asy();
    let g: u8 = generic();
    println!("{:?} {} {} {} {} {:?} {} {} {} {} {} {}", names, v[0], s, m, sum, o, fl, sb, d.t(), g, TL.with(|x| *x.borrow()), *LZ + imp()());
}
