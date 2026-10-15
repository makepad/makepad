// moves out of *Box locals: the allocation is freed without dropping the contents again
use std::cell::Cell;
thread_local! { static DROPS: Cell<u32> = Cell::new(0); }
struct D(u32);
impl Drop for D { fn drop(&mut self) { DROPS.with(|d| d.set(d.get() + 1)); } }
fn drops() -> u32 { DROPS.with(|d| d.get()) }
fn reset() { DROPS.with(|d| d.set(0)); }
fn f1(b: Box<D>) -> D { *b }
fn f2(b: Box<D>) -> D { let d = *b; d }
fn f3(b: Option<Box<D>>) -> D { if let Some(b) = b { *b } else { D(0) } }
fn f4(b: Option<Box<D>>) -> D { if let Some(b) = b { return *b; } D(0) }
fn f5(b: Box<D>) -> D { return *b; }
#[test] fn t1() { reset(); drop(f1(Box::new(D(1)))); assert_eq!(drops(), 1); }
#[test] fn t2() { reset(); drop(f2(Box::new(D(1)))); assert_eq!(drops(), 1); }
#[test] fn t3() { reset(); drop(f3(Some(Box::new(D(1))))); assert_eq!(drops(), 1); }
#[test] fn t4() { reset(); drop(f4(Some(Box::new(D(1))))); assert_eq!(drops(), 1); }
#[test] fn t5() { reset(); drop(f5(Box::new(D(1)))); assert_eq!(drops(), 1); }
// float -> 128-bit casts saturate, NaN -> 0
#[test]
fn float_to_128() {
    assert_eq!(1.5e3f64 as u128, 1500);
    assert_eq!((-1.0f64) as u128, 0);
    assert_eq!(f64::NAN as i128, 0);
    assert_eq!(1e40f64 as u128, u128::MAX);
    assert_eq!((-1e40f64) as i128, i128::MIN);
    assert_eq!(1e40f64 as i128, i128::MAX);
    assert_eq!((-2.5f64) as i128, -2);
    assert_eq!(3.0e20f64 as u128, 300000000000000000000);
    assert_eq!((-3.0e20f64) as i128, -300000000000000000000);
    assert_eq!(7.9f32 as u128, 7);
}
