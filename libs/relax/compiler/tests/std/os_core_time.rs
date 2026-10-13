// std-os lane: core::time::Duration (differential: os_diff.sh)
use core::time::Duration;

#[test]
fn a01_construct() {
    let ds = [
        Duration::new(5, 30),
        Duration::new(1, 2_000_000_001),
        Duration::from_secs(7),
        Duration::from_millis(1234),
        Duration::from_micros(1_234_567),
        Duration::from_nanos(9_876_543_210),
        Duration::ZERO,
        Duration::MAX,
    ];
    for d in ds.iter() {
        println!("| {} {} {} {} {} {} {} {}", d.as_secs(), d.subsec_nanos(), d.subsec_millis(), d.subsec_micros(), d.as_millis(), d.as_micros(), d.as_nanos(), d.is_zero());
    }
}

#[test]
fn a02_arith() {
    let a = Duration::new(3, 700_000_000);
    let b = Duration::new(1, 900_000_000);
    println!("| {:?} {:?} {:?} {:?}", a + b, a - b, a * 3, a / 7);
    println!("| {:?} {:?}", a.checked_sub(b), b.checked_sub(a));
    println!("| {:?} {:?}", Duration::MAX.checked_add(a), a.checked_div(0));
    println!("| {:?} {:?}", Duration::MAX.saturating_add(a), b.saturating_sub(a));
    println!("| {:?} {:?}", a.checked_mul(u32::MAX), a.saturating_mul(u32::MAX));
    println!("| {:?} {:?}", a.abs_diff(b), b.abs_diff(a));
    let mut c = a;
    c += b;
    c -= Duration::from_millis(1);
    c *= 2;
    c /= 3;
    println!("| {:?} {:?}", c, 4u32 * b);
    let v = [a, b, c];
    let s: Duration = v.iter().sum();
    let s2: Duration = v.iter().copied().sum();
    println!("| {:?} {:?} {}", s, s2, a > b);
}

#[test]
fn a03_float() {
    let a = Duration::new(2, 500_000_001);
    println!("| {} {}", a.as_secs_f64(), a.as_secs_f32());
    let xs = [0.0f64, 1e-10, 4.2e-7, 0.5, 2.7, 1.999999999, 0.9999999995, 3e10, 123456.789012345, 1e-9, 1.5e-9, 2.5e-9];
    for x in xs.iter() {
        println!("| f64 {} -> {:?}", x, Duration::from_secs_f64(*x));
    }
    let ys = [0.0f32, 4.2e-7, 2.7, 3e10, 0.1, 1e-9, 7.25];
    for y in ys.iter() {
        println!("| f32 {} -> {:?}", y, Duration::from_secs_f32(*y));
    }
    println!("| {:?}", Duration::try_from_secs_f64(-1.0));
    println!("| {:?}", Duration::try_from_secs_f64(f64::NAN));
    println!("| {:?}", Duration::try_from_secs_f64(2e19));
    match Duration::try_from_secs_f32(-5.0) {
        Ok(_) => println!("| ok?"),
        Err(e) => println!("| {}", e),
    }
    match Duration::try_from_secs_f64(1e30) {
        Ok(_) => println!("| ok?"),
        Err(e) => println!("| {}", e),
    }
    println!("| {:?} {:?} {:?} {:?}", a.mul_f64(1.5), a.mul_f32(0.25), a.div_f64(3.0), a.div_f32(2.0));
    println!("| {} {}", a.div_duration_f64(Duration::from_millis(300)), a.div_duration_f32(Duration::from_millis(300)));
}

#[test]
fn a04_debug() {
    let ds = [
        Duration::new(1, 500_000_000),
        Duration::new(1, 0),
        Duration::new(0, 1_500_000),
        Duration::new(0, 1_000),
        Duration::new(0, 1_500),
        Duration::new(0, 7),
        Duration::new(0, 0),
        Duration::new(100, 1),
        Duration::new(0, 999_999_999),
        Duration::MAX,
        Duration::new(0, 123_456_789),
    ];
    for d in ds.iter() {
        println!("| {:?} [{:.0?}] [{:.2?}] [{:.3?}] [{:.12?}]", d, d, d, d, d);
        println!("| [{:10?}] [{:<12?}] [{:>12?}] [{:^12?}] [{:*^14.1?}] [{:+?}] [{:+12.2?}]", d, d, d, d, d, d, d);
    }
}
