// read_volatile / write_volatile: values through every scalar width, floats, pointers and an
// aggregate (= rustc --test). The never-removed guarantee is checked on RIR in
// relax/codegen/tests/volatile.rs; an unused volatile read of null faulting is
// tools/hrcheck/repros/volatile_unused.rs.
use std::ptr::{read_volatile, write_volatile};

#[derive(Clone, Copy, PartialEq, Debug)]
struct P {
    a: u16,
    b: f64,
}

#[test]
fn volatile_values() {
    let mut x8 = 0u8;
    let mut x16 = 0i16;
    let mut x32 = 0u32;
    let mut x64 = 0i64;
    let mut f32_ = 0f32;
    let mut f64_ = 0f64;
    let mut pp: *const u32 = std::ptr::null();
    let mut agg = P { a: 0, b: 0.0 };
    unsafe {
        write_volatile(&mut x8, 0xfe);
        write_volatile(&mut x16, -12345);
        write_volatile(&mut x32, 0xdead_beef);
        write_volatile(&mut x64, -0x1234_5678_9abc);
        write_volatile(&mut f32_, 1.5);
        write_volatile(&mut f64_, -2.25);
        write_volatile(&mut pp, &x32 as *const u32);
        write_volatile(&mut agg, P { a: 7, b: 3.5 });
        println!("{} {} {:x} {} {} {} {:x} {:?}", read_volatile(&x8), read_volatile(&x16), read_volatile(&x32), read_volatile(&x64), read_volatile(&f32_), read_volatile(&f64_), *read_volatile(&pp), read_volatile(&agg));
        // repeated reads of one location in a loop: each is a load
        let mut s = 0u64;
        for _ in 0..10 {
            s += read_volatile(&x32) as u64;
        }
        println!("{}", s);
        assert_eq!(s, 0xdead_beef * 10);
    }
}
